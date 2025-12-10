//! A lightweight, allocation-free job system for executing type-erased closures.
//!
//! This crate provides the [`Job`] type, which stores closures inline inside a
//! fixed-size buffer and executes them without heap allocation. Each job embeds
//! its own `call`, `clone`, and `drop` functions via type-erased function
//! pointers, enabling predictable, low-latency execution suitable for
//! high-frequency or real-time workloads.
//!
//! `Job` is generic over the inline capacity `N`, the closure's return type
//! `R`, and an optional context type `C`. The inline storage size `N` has no
//! default; callers must choose a capacity explicitly. `R` defaults to `()`,
//! and `C` defaults to `()`.
//!
//! # Example: Dispatching Jobs to a Worker Thread
//!
//! ```rust
//! use std::sync::mpsc;
//! use std::thread;
//! use hft_jobs::Job;
//!
//! // Create a channel for sending jobs.
//! let (tx, rx) = mpsc::channel::<Job<24, String>>();
//!
//! // A worker thread that receives and runs jobs.
//! thread::spawn(move || {
//!     while let Ok(job) = rx.recv() {
//!         let s = job.run();
//!         println!("{}", s);
//!     }
//! });
//!
//! // Send a simple job.
//! let job = Job::<24, _>::new(|| "Hello from a job!".to_string());
//! tx.send(job).unwrap();
//!
//! // A convenience macro that enqueues a logging job.
//! macro_rules! log {
//!     ($tx:expr, $fmt:literal $(, $arg:expr)* $(,)?) => {{
//!         let job = Job::<24, String>::new(move || {
//!             format!($fmt $(, $arg)*)
//!         });
//!         let _ = $tx.send(job);
//!     }};
//! }
//!
//! // Use the `log!` macro to enqueue a println job.
//! log!(tx, "Logging from thread: {}", 42);
//! ```
//!
//! This model provides a minimal, fast job runtime suitable for embedded
//! systems, schedulers, executors, or lightweight logging systems.

use std::{marker::PhantomData, mem, ptr};

#[inline(always)]
fn padding_for<F>(data: *const u8) -> usize {
    let align = mem::align_of::<F>();
    debug_assert!(align.is_power_of_two());
    let base = data as usize;
    let aligned = (base + (align - 1)) & !(align - 1);
    aligned - base
}

#[inline(always)]
unsafe fn data_ptr<F>(data: *mut u8) -> *mut F {
    let offset = padding_for::<F>(data as *const u8);
    unsafe { data.add(offset) as *mut F }
}

/// A type-erased, fixed-size job container for storing and invoking closures.
///
/// `Job` holds an opaque buffer and a trio of function pointers that know how
/// to call, clone, and drop the stored value. This allows arbitrary closures
/// (or other callable types) to be stored in a uniform, `repr(C)` layout,
/// making it suitable for job queues, thread pools, and FFI boundaries.
///
/// The stored callable is expected to live entirely inside the `data` buffer,
/// and the function pointers must treat that buffer as their backing storage.
///
/// # Type Parameters
///
/// * `N` – The size of the internal storage buffer, in bytes. This must be
///   large enough to hold the largest closure (or callable) you intend to
///   store. There is no default; choose the inline capacity that fits your
///   workload (e.g., `64` for small captures).
/// * `R` – The return type of the stored closure. This is the value produced
///   by [`Job::run`] or [`Job::run_with_ctx`]. Use `()` for fire-and-forget
///   jobs (the default).
/// * `C` – A mutable context passed to the closure when it is run. Defaults to
///   `()` for the common zero-context case.
///
/// # Layout
///
/// * `data` – An `Align16<[u8; N]>` buffer that stores the erased closure.
///   The alignment wrapper ensures the buffer has at least 16-byte alignment,
///   which is typically sufficient for most captured types.
/// * `fn_call` – An `unsafe fn(*mut u8) -> R` that takes a pointer into
///   `data`, invokes the stored callable, and returns its result.
/// * `fn_clone` – An `unsafe fn(*const u8, *mut u8)` that clones the stored
///   value from one buffer to another (source and destination pointers into
///   `data`-like storage).
/// * `fn_drop` – An `unsafe fn(*mut u8)` that drops the stored value in place.
///
/// The `repr(C)` attribute guarantees a stable field layout, which is useful if
/// instances of `Job` are passed across FFI or need a predictable memory
/// representation.
///
/// # Threading and Lifetime
///
/// A `Job` created via [`Job::new`]:
///
/// * is backed by a `FnOnce() -> R + Clone + Send + 'static` callable,
///   ensuring that the stored closure can be invoked exactly once, cloned for
///   replication, transferred across threads, and contains no non-`'static`
///   borrows;
/// * is `Clone`, allowing the same logical callable to be duplicated into
///   multiple `Job` instances (e.g., for SPMC job queues);
/// * is `Send`, so it may be moved freely to other threads for execution;
/// * is `'static`, meaning it holds no borrowed references tied to a shorter
///   lifetime and may safely outlive the scope in which it was created.
///
/// # Example
///
/// ```rust
/// # use hft_jobs::Job;
/// // Typically constructed via a helper like `Job::new`:
/// let job = Job::<64>::new(|| {
///     println!("Hello from a job!");
/// });
///
/// // Later, the job executor would call something like:
/// job.run();
/// ```
#[repr(C)]
pub struct Job<const N: usize, R = (), C = ()> {
    data: Align16<[u8; N]>, // N must be >= sizeof(biggest closure)
    fn_call: unsafe fn(*mut u8, &mut C) -> R,
    fn_clone: unsafe fn(*const u8, *mut u8),
    fn_drop: unsafe fn(*mut u8),
    _marker: PhantomData<R>,
}

#[repr(align(16))]
struct Align16<T>(pub T);

// SAFETY: Job only ever contains F: FnOnce(&mut C) -> R + Clone + Send + 'static,
// enforced in Job::new_with_ctx / Job::new, so it is safe to move between threads.
unsafe impl<const N: usize, R, C> Send for Job<N, R, C> {}

impl<const N: usize, R, C> Job<N, R, C> {
    /// Creates a new job from a closure, storing it inline without heap
    /// allocation.
    ///
    /// # Type Requirements
    ///
    /// The closure `F` must satisfy:
    /// - `FnOnce() -> R` — the closure is invoked exactly once to produce `R`;
    /// - `Clone` — needed so the job queue or scheduler can duplicate jobs if
    ///   desired;
    /// - `Send + 'static` — ensures the closure may be transferred across
    ///   threads and does not borrow non-`'static` data.
    ///
    /// # Storage Constraints
    ///
    /// The closure must fit inside the inline buffer:
    /// - `size_of::<F>() <= N`
    /// - `align_of::<F>() <= align_of::<Align16<[u8; N]>>`
    ///
    /// If either condition fails, the function **panics**.
    ///
    /// # How it Works
    ///
    /// This method:
    /// 1. Generates specialized `fn_call`, `fn_clone`, and `fn_drop` functions
    ///    for the captured closure type `F`.
    /// 2. Writes the closure directly into the inline buffer using
    ///    [`ptr::write`], avoiding heap allocations.
    /// 3. Returns a fully-constructed [`Job`] that owns the closure.
    ///
    /// # Safety
    ///
    /// Internally uses unsafe operations to:
    /// - Reconstruct `F` from raw bytes.
    /// - Clone `F` via raw pointers.
    /// - Drop `F` in place.
    ///
    /// These operations are memory-safe *only* because:
    /// - The size and alignment checks guarantee the buffer is valid for `F`.
    /// - The caller cannot violate the type invariants through the public API.
    ///
    /// # Example (with context)
    ///
    /// ```
    /// # use hft_jobs::Job;
    /// #[derive(Default)]
    /// struct Ctx {
    ///     sum: u32,
    /// }
    ///
    /// let job = Job::<64, u32, Ctx>::new_with_ctx(|ctx| {
    ///     ctx.sum += 1;
    ///     ctx.sum
    /// });
    ///
    /// let mut ctx = Ctx::default();
    /// assert_eq!(job.run_with_ctx(&mut ctx), 1);
    /// assert_eq!(ctx.sum, 1);
    /// ```
    ///
    /// F: FnOnce(&mut C) -> R + Clone + Send + 'static
    #[inline]
    pub fn new_with_ctx<F>(f: F) -> Self
    where
        F: FnOnce(&mut C) -> R + Clone + Send + 'static,
    {
        // Ensure (at compile time) that the closure fits into our inline storage
        const {
            let align = mem::align_of::<F>();
            let base_align = mem::align_of::<Align16<[u8; N]>>();
            let padding = (align.wrapping_sub(base_align % align)) % align;
            let required = padding + mem::size_of::<F>();
            assert!(required <= N);
        }

        unsafe fn fn_call<R, C, F>(data: *mut u8, ctx: &mut C) -> R
        where
            F: FnOnce(&mut C) -> R,
        {
            unsafe {
                let f = ptr::read(data_ptr::<F>(data));
                f(ctx)
            }
        }

        unsafe fn fn_clone<R, C, F>(src: *const u8, dst: *mut u8)
        where
            F: FnOnce(&mut C) -> R + Clone,
        {
            unsafe {
                let f_src = &*data_ptr::<F>(src as *mut u8);
                let f_clone = f_src.clone();
                ptr::write(data_ptr::<F>(dst), f_clone);
            }
        }

        unsafe fn fn_drop<R, C, F>(data: *mut u8)
        where
            F: FnOnce(&mut C) -> R,
        {
            unsafe {
                ptr::drop_in_place(data_ptr::<F>(data));
            }
        }

        let mut job = Job {
            fn_call: fn_call::<R, C, F>,
            fn_clone: fn_clone::<R, C, F>,
            fn_drop: fn_drop::<R, C, F>,
            data: Align16([0u8; N]),
            _marker: PhantomData,
        };

        unsafe {
            // Place the closure into `data` without heap allocation
            let base = job.data.0.as_mut_ptr();
            let padding = padding_for::<F>(base as *const u8);
            let required = padding + mem::size_of::<F>();
            debug_assert!(
                required <= N,
                "closure does not fit in inline storage: requires {required} bytes with alignment {}, but capacity is {N}",
                mem::align_of::<F>(),
            );
            let dst = base.add(padding) as *mut F;
            ptr::write(dst, f);
        }

        job
    }

    /// Runs the stored closure with a mutable context, consuming the job and
    /// returning its result `R`.
    ///
    /// This method invokes the closure that was previously stored in the inline
    /// buffer. Running the job consumes its captured environment and yields
    /// a value of type `R`.
    ///
    /// After invocation, the job's internal `fn_drop` function pointer is replaced
    /// with a no-op drop function, ensuring that the closure's environment is not
    /// dropped **twice** when the `Job` itself is later dropped.
    ///
    /// # How it Works
    ///
    /// - `fn_call` reconstructs the original closure from the buffer, taking
    ///   ownership of it.
    /// - The closure is executed with the provided context, and its return value
    ///   is propagated back to the caller as `R`.
    /// - Because the closure has now been consumed, `fn_drop` is overwritten with
    ///   a version that performs no action, preventing a double-drop.
    ///
    /// # Safety
    ///
    /// Although this method uses unsafe raw-pointer calls internally, it is safe
    /// to use because:
    ///
    /// - The closure is guaranteed (via `Job::new_with_ctx`) to fit in the inline
    ///   buffer with correct alignment.
    /// - The closure is executed exactly once.
    /// - The memory backing the closure is never accessed again after ownership
    ///   is taken by `fn_call`.
    ///
    /// # Example
    ///
    /// ```rust
    /// # use hft_jobs::Job;
    /// #[derive(Default)]
    /// struct Ctx {
    ///     total: u32,
    /// }
    ///
    /// let job = Job::<64, u32, Ctx>::new_with_ctx(|ctx| {
    ///     ctx.total += 2;
    ///     ctx.total
    /// });
    ///
    /// let mut ctx = Ctx::default();
    /// assert_eq!(job.run_with_ctx(&mut ctx), 2);
    /// assert_eq!(ctx.total, 2);
    /// ```
    #[inline]
    pub fn run_with_ctx(mut self, ctx: &mut C) -> R {
        #[inline(always)]
        unsafe fn drop_noop(_data: *mut u8) {}

        unsafe {
            // Replace drop with a no-op drop (ZST panic closure) to avoid double-drop.
            self.fn_drop = drop_noop;
            (self.fn_call)(self.data.0.as_ptr() as *mut u8, ctx)
        }
    }
}

impl<const N: usize, R, C> Default for Job<N, R, C> {
    /// Constructs a "default" job which panics if it is ever run.
    ///
    /// This is mainly useful as a placeholder. Calling [`Job::run_with_ctx`] on a
    /// default-constructed job will panic with the message
    /// `"attempt to execute an empty job"`.
    /// Used as a source of a no-op drop function after `run_with_ctx`.
    #[inline]
    fn default() -> Self {
        // ZST closure; drop is effectively a no-op.
        Self::new_with_ctx(|_ctx: &mut C| panic!("attempt to execute an empty job"))
    }
}

impl<const N: usize, R, C> Clone for Job<N, R, C> {
    /// Clones the underlying closure into a new `Job`.
    ///
    /// Both the original and cloned `Job` instances own independent copies of
    /// the captured environment and can be run separately.
    #[inline]
    fn clone(&self) -> Self {
        let mut new_job = Job {
            fn_call: self.fn_call,
            fn_clone: self.fn_clone,
            fn_drop: self.fn_drop,
            data: Align16([0u8; N]),
            _marker: PhantomData,
        };
        unsafe {
            (self.fn_clone)(self.data.0.as_ptr(), new_job.data.0.as_mut_ptr());
        }
        new_job
    }
}

impl<const N: usize, R, C> Drop for Job<N, R, C> {
    /// Drops the stored closure (if any) in place.
    ///
    /// If the job has not been run, this will drop the captured environment of
    /// the stored closure. If the job **has** been run, [`Job::run_with_ctx`]
    /// (or [`Job::run`] for the `C = ()` convenience impl) ensures that
    /// `fn_drop` has been replaced so that no double-drop occurs.
    #[inline]
    fn drop(&mut self) {
        unsafe {
            (self.fn_drop)(self.data.0.as_mut_ptr());
        }
    }
}

/// For the common “no context” case (C = ()):
impl<const N: usize, R> Job<N, R, ()> {
    /// Convenience constructor for the common zero-context case.
    ///
    /// Equivalent to [`Job::new_with_ctx`] with `C = ()`.
    /// Backwards-compatible constructor: closure takes no arguments.
    #[inline]
    pub fn new<F>(f: F) -> Self
    where
        F: FnOnce() -> R + Clone + Send + 'static,
    {
        // Wrap zero-arg closure into a context-taking closure
        Self::new_with_ctx(move |_ctx: &mut ()| f())
    }

    /// Convenience runner for `C = ()`.
    ///
    /// Equivalent to [`Job::run_with_ctx`] with an empty context.
    /// Backwards-compatible runner: no context required.
    #[inline]
    pub fn run(self) -> R {
        self.run_with_ctx(&mut ())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::{
        Arc,
        atomic::{AtomicUsize, Ordering},
        mpsc,
    };
    use std::thread;

    /// Helper: prove at compile time that Job is Send.
    fn assert_send<T: Send>() {}
    #[test]
    fn job_is_send() {
        assert_send::<Job<64, ()>>();
        assert_send::<Job<128, ()>>();
    }

    #[test]
    fn job_runs_closure_once() {
        let counter = Arc::new(AtomicUsize::new(0));
        let c = {
            let counter = counter.clone();
            move || {
                counter.fetch_add(1, Ordering::SeqCst);
            }
        };

        let job: Job<64, ()> = Job::new(c);
        job.run();

        assert_eq!(counter.load(Ordering::SeqCst), 1);
    }

    #[test]
    fn cloned_jobs_both_run() {
        let counter = Arc::new(AtomicUsize::new(0));
        let c = {
            let counter = counter.clone();
            move || {
                counter.fetch_add(1, Ordering::SeqCst);
            }
        };

        let job1: Job<64, ()> = Job::new(c);
        let job2 = job1.clone();

        job1.run();
        job2.run();

        assert_eq!(counter.load(Ordering::SeqCst), 2);
    }

    /// Type whose Drop increments a shared AtomicUsize.
    #[derive(Clone)]
    struct DropGuard {
        drops: Arc<AtomicUsize>,
    }

    impl Drop for DropGuard {
        fn drop(&mut self) {
            self.drops.fetch_add(1, Ordering::SeqCst);
        }
    }

    #[test]
    fn job_drop_calls_closure_drop_when_not_run() {
        let drops = Arc::new(AtomicUsize::new(0));
        {
            let guard = DropGuard {
                drops: drops.clone(),
            };
            let c = move || {
                // Do nothing; we only care about Drop.
                let _ = &guard;
            };
            let _job: Job<64, ()> = Job::new(c);
            // _job is dropped here without run()
        }

        // The closure's captured DropGuard should have been dropped exactly once.
        assert_eq!(drops.load(Ordering::SeqCst), 1);
    }

    #[test]
    fn job_run_does_not_double_drop_closure() {
        let drops = Arc::new(AtomicUsize::new(0));
        {
            let guard = DropGuard {
                drops: drops.clone(),
            };
            let c = move || {
                // When this closure is called, its captured guard will
                // be dropped at end of call.
                let _ = &guard;
            };
            let job: Job<64, ()> = Job::new(c);
            job.run();
            // After run(), Job's Drop should not try to drop the original F again.
        }

        // Exactly one drop of the captured guard.
        assert_eq!(drops.load(Ordering::SeqCst), 1);
    }

    #[test]
    #[should_panic(expected = "attempt to execute an empty job")]
    fn default_job_panics_on_run() {
        let job: Job<64, ()> = Job::default();
        job.run();
    }

    #[test]
    fn job_can_be_sent_to_worker_thread_and_run() {
        let counter = Arc::new(AtomicUsize::new(0));
        let (tx, rx) = mpsc::channel::<Job<1024, ()>>();

        // Worker thread that receives and runs jobs.
        let worker_counter = counter.clone();
        let worker = thread::spawn(move || {
            while let Ok(job) = rx.recv() {
                job.run();
                worker_counter.fetch_add(1, Ordering::SeqCst);
            }
        });

        // Send a few jobs.
        for _ in 0..3 {
            let c_counter = counter.clone();
            let job = Job::new(move || {
                c_counter.fetch_add(1, Ordering::SeqCst);
            });
            tx.send(job).unwrap();
        }

        drop(tx); // close channel
        worker.join().unwrap();

        // 3 jobs executed, and worker ran loop 3 times.
        assert_eq!(counter.load(Ordering::SeqCst), 6);
    }

    #[test]
    fn job_alignment_and_size_are_sufficient_for_small_closure() {
        // Small closure capturing a couple of integers.
        let a = 1u32;
        let b = 2u64;
        let c = move || {
            let _ = a + b as u32;
        };

        let size_f = mem::size_of_val(&c);
        let align_f = mem::align_of_val(&c);

        assert!(size_f <= 64);
        assert!(align_f <= mem::align_of::<Align16<[u8; 64]>>());

        // Should not panic:
        let job: Job<64, ()> = Job::new(c);
        job.run();
    }

    #[test]
    fn job_handles_high_alignment_closure() {
        #[repr(align(64))]
        #[derive(Clone)]
        struct Aligned(u64);

        let captured = Aligned(7);
        let job: Job<128, u64> = Job::new(move || captured.0 * 6);
        assert_eq!(job.run(), 42);
    }

    #[test]
    fn job_returns_value() {
        let job: Job<64, u32> = Job::new(|| 40 + 2);
        let result = job.run();
        assert_eq!(result, 42);
    }

    #[test]
    fn job_returns_owned_string() {
        let job: Job<64, _> = Job::new(|| "hello".to_owned() + " world");
        let result = job.run();
        assert_eq!(result, "hello world");
    }

    #[test]
    fn cloned_jobs_both_return_values() {
        let job1: Job<64, u64> = Job::new(|| 10u64 * 10);
        let job2 = job1.clone();

        let r1 = job1.run();
        let r2 = job2.run();

        assert_eq!(r1, 100);
        assert_eq!(r2, 100);
    }

    #[test]
    fn job_with_result_can_be_sent_to_worker_thread() {
        let (tx, rx) = mpsc::channel::<Job<128, u32>>();

        // Worker thread that receives jobs and collects their results.
        let worker = thread::spawn(move || {
            let mut results = Vec::new();
            while let Ok(job) = rx.recv() {
                results.push(job.run());
            }
            results
        });

        // Send a few value-returning jobs.
        for i in 0..3u32 {
            let job = Job::new(move || i * 2);
            tx.send(job).unwrap();
        }

        drop(tx); // close channel
        let results = worker.join().unwrap();

        assert_eq!(results.len(), 3);
        assert!(results.contains(&0));
        assert!(results.contains(&2));
        assert!(results.contains(&4));
    }

    #[test]
    fn job_runs_with_mutable_context() {
        #[derive(Default, Clone)]
        struct Ctx {
            ticks: u32,
        }

        let mut ctx = Ctx::default();

        let job = Job::<64, u32, Ctx>::new_with_ctx(|c| {
            c.ticks += 1;
            c.ticks
        });

        let job_clone = job.clone();

        let first = job_clone.run_with_ctx(&mut ctx);
        assert_eq!(first, 1);
        assert_eq!(ctx.ticks, 1);

        let second = job.run_with_ctx(&mut ctx);
        assert_eq!(second, 2);
        assert_eq!(ctx.ticks, 2);
    }

    #[test]
    fn example_from_readme() {
        // Create a simple channel for dispatching jobs to a worker
        let (tx, rx) = mpsc::channel::<Job<64, _>>();

        // Spawn the worker thread
        thread::spawn(move || {
            while let Ok(job) = rx.recv() {
                let s = job.run(); // executes the closure
                println!("{}", s);
            }
        });

        // Send a job
        let job = Job::<64, _>::new(|| "Hello from a job!".to_string());
        tx.send(job).unwrap();

        // A convenience macro that enqueues a logging job.
        macro_rules! log {
            ($tx:expr, $fmt:literal $(, $arg:expr)* $(,)?) => {{
                let job = Job::<64, String>::new(move || {
                    format!($fmt $(, $arg)*)
                });
                let _ = $tx.send(job);
            }};
        }

        // Use the `log!` macro to enqueue a logging job.
        log!(tx, "Logging from thread: {}", 42);
    }
}
