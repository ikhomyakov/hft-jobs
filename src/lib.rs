//! A lightweight, allocation-free job system for executing type-erased closures.
//!
//! This crate provides the [`Job`] type, which stores closures inline inside a
//! fixed-size buffer and executes them without heap allocation. Each job embeds
//! its own `call`, `clone`, and `drop` functions via type-erased function
//! pointers, enabling predictable, low-latency execution suitable for
//! high-frequency or real-time workloads.
//!
//! `Job` is generic over the inline capacity `N` and the closure's return type
//! `R`, so jobs can either be fire-and-forget (`R = ()`) or produce a result.
//! The inline storage size `N` has no default; callers must choose a capacity
//! explicitly.
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

use std::{fmt, marker::PhantomData, mem, ptr};

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
///   by [`Job::run`]. Use `()` for fire-and-forget jobs (the default).
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
pub struct Job<const N: usize, R = ()> {
    data: Align16<[u8; N]>, // N must be >= sizeof(biggest closure)
    fn_call: unsafe fn(*mut u8) -> R,
    fn_clone: unsafe fn(*const u8, *mut u8),
    fn_drop: unsafe fn(*mut u8),
    _marker: PhantomData<R>,
}

#[repr(align(16))]
struct Align16<T>(pub T);

/// Errors that can occur when constructing a [`Job`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum JobInitError {
    /// The closure does not fit inside the inline buffer of size `N`.
    TooLarge {
        /// The size in bytes of the closure type.
        size: usize,
        /// The inline buffer capacity in bytes.
        capacity: usize,
    },
    /// The closure's alignment requirement exceeds the storage alignment.
    InsufficientAlignment {
        /// The alignment required by the closure type.
        align: usize,
        /// The maximum alignment provided by the job storage.
        max_align: usize,
    },
}

impl fmt::Display for JobInitError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            JobInitError::TooLarge { size, capacity } => {
                write!(
                    f,
                    "closure of size {} bytes exceeds inline capacity {} bytes",
                    size, capacity
                )
            }
            JobInitError::InsufficientAlignment { align, max_align } => {
                write!(
                    f,
                    "closure alignment {} exceeds inline alignment {}",
                    align, max_align
                )
            }
        }
    }
}

impl std::error::Error for JobInitError {}

// SAFETY: Job only ever contains F: FnOnce() -> R + Clone + Send + 'static,
// enforced in Job::new, so it is safe to move between threads.
unsafe impl<const N: usize, R> Send for Job<N, R> {}

impl<const N: usize, R> Job<N, R> {
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
    /// # Example
    ///
    /// ```
    /// # use hft_jobs::Job;
    /// let job = Job::<64>::new(|| println!("Hello from a job!"));
    /// // The job is now a self-contained, type-erased closure.
    /// ```
    pub fn new<F>(f: F) -> Self
    where
        F: FnOnce() -> R + Clone + Send + 'static,
    {
        Self::try_new(f).unwrap_or_else(|err| panic!("{}", err))
    }

    /// Like [`Job::new`], but returns an error instead of panicking when the
    /// closure does not fit in the inline buffer or exceeds alignment.
    ///
    /// # Errors
    ///
    /// Returns:
    /// - `JobInitError::TooLarge` if `size_of::<F>() > N`
    /// - `JobInitError::InsufficientAlignment` if
    ///   `align_of::<F>() > align_of::<Align16<[u8; N]>>()`
    pub fn try_new<F>(f: F) -> Result<Self, JobInitError>
    where
        F: FnOnce() -> R + Clone + Send + 'static,
    {
        let size_f = mem::size_of::<F>();
        let align_f = mem::align_of::<F>();
        let storage_align = mem::align_of::<Align16<[u8; N]>>();

        if size_f > N {
            return Err(JobInitError::TooLarge {
                size: size_f,
                capacity: N,
            });
        }

        if align_f > storage_align {
            return Err(JobInitError::InsufficientAlignment {
                align: align_f,
                max_align: storage_align,
            });
        }

        Ok(Self::build_job(f))
    }

    /// Internal helper that writes the closure into the inline buffer.
    fn build_job<F>(f: F) -> Self
    where
        F: FnOnce() -> R + Clone + Send + 'static,
    {
        unsafe fn fn_call<R, F>(data: *mut u8) -> R
        where
            F: FnOnce() -> R,
        {
            // Recreate the closure from the buffer and run it.
            // `read` takes ownership, so `F: FnOnce() -> R` is fine.
            unsafe {
                let f = ptr::read(data as *const F);
                f()
            }
        }

        unsafe fn fn_clone<R, F>(src: *const u8, dst: *mut u8)
        where
            F: FnOnce() -> R + Clone,
        {
            unsafe {
                let f_src = &*(src as *const F);
                let f_clone = f_src.clone();
                ptr::write(dst as *mut F, f_clone);
            }
        }

        unsafe fn fn_drop<R, F>(data: *mut u8)
        where
            F: FnOnce() -> R,
        {
            unsafe {
                ptr::drop_in_place(data as *mut F);
            }
        }

        let mut job = Job {
            fn_call: fn_call::<R, F>,
            fn_clone: fn_clone::<R, F>,
            fn_drop: fn_drop::<R, F>,
            data: Align16([0u8; N]),
            _marker: PhantomData,
        };
        unsafe {
            // Place the closure into `data` without heap allocation
            let dst = job.data.0.as_mut_ptr() as *mut F;
            ptr::write(dst, f);
        }
        job
    }

    /// Runs the stored closure, consuming the job and returning its result `R`.
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
    /// - The closure is executed, and its return value is propagated back to the
    ///   caller as `R`.
    /// - Because the closure has now been consumed, `fn_drop` is overwritten with
    ///   a version that performs no action, preventing a double-drop.
    ///
    /// # Safety
    ///
    /// Although this method uses unsafe raw-pointer calls internally, it is safe
    /// to use because:
    ///
    /// - The closure is guaranteed (via `Job::new`) to fit in the inline buffer
    ///   with correct alignment.
    /// - The closure is executed exactly once.
    /// - The memory backing the closure is never accessed again after ownership
    ///   is taken by `fn_call`.
    ///
    /// # Example
    ///
    /// ```rust
    /// # use hft_jobs::Job;
    /// let job = Job::<64, u32>::new(|| 21 * 2);
    /// assert_eq!(job.run(), 42);
    ///
    /// let void_job = Job::<0>::new(|| println!("Running a job"));
    /// void_job.run(); // prints "Running a job"
    /// ```
    pub fn run(mut self) -> R {
        unsafe {
            self.fn_drop = Job::<N, R>::default().fn_drop;
            (self.fn_call)(self.data.0.as_ptr() as *mut u8)
        }
    }
}

impl<const N: usize, R> Default for Job<N, R> {
    /// Constructs a "default" job which panics if it is ever run.
    ///
    /// This is mainly useful as a placeholder. Calling [`Job::run`] on a
    /// default-constructed job will panic with the message
    /// `"attempt to execute an empty job"`.
    fn default() -> Self {
        Self::new(|| panic!("attempt to execute an empty job"))
    }
}

impl<const N: usize, R> Clone for Job<N, R> {
    /// Clones the underlying closure into a new `Job`.
    ///
    /// Both the original and cloned `Job` instances own independent copies of
    /// the captured environment and can be run separately.
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

impl<const N: usize, R> Drop for Job<N, R> {
    /// Drops the stored closure (if any) in place.
    ///
    /// If the job has not been run, this will drop the captured environment of
    /// the stored closure. If the job **has** been run, [`Job::run`] ensures
    /// that `fn_drop` has been replaced so that no double-drop occurs.
    fn drop(&mut self) {
        unsafe {
            (self.fn_drop)(self.data.0.as_mut_ptr());
        }
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
    #[should_panic]
    fn job_new_panics_if_closure_too_big() {
        // Capture a big array by value so the closure is larger than N=64 bytes.
        let big = [0u8; 128];
        let c = move || {
            let _ = &big;
        };

        // This should panic on the size assertion inside Job::new::<F, 64>.
        let _job: Job<64, ()> = Job::new(c);
        let _ = _job;
    }

    #[test]
    fn try_new_returns_error_instead_of_panicking_for_large_closure() {
        let big = [0u8; 128];
        let c = move || {
            let _ = &big;
        };

        match Job::<64, ()>::try_new(c) {
            Err(JobInitError::TooLarge { size, capacity }) => {
                assert!(size > capacity);
            }
            Err(other) => panic!("unexpected error: {:?}", other),
            Ok(_) => panic!("expected try_new to fail for oversized closure"),
        }
    }

    #[derive(Clone, Copy)]
    #[repr(align(32))]
    struct Align32;

    #[test]
    fn try_new_reports_alignment_mismatch() {
        let aligned = Align32;
        let c = move || {
            let _ = &aligned;
        };

        match Job::<64, ()>::try_new(c) {
            Err(JobInitError::InsufficientAlignment { align, max_align }) => {
                assert!(align > max_align);
            }
            Err(other) => panic!("unexpected error: {:?}", other),
            Ok(_) => panic!("expected try_new to fail for alignment"),
        }
    }

    #[test]
    fn try_new_constructs_job_without_panicking() {
        let job: Job<64, u32> = Job::try_new(|| 7 + 35).unwrap();
        assert_eq!(job.run(), 42);
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
