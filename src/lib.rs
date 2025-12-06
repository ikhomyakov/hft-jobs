//! A lightweight, allocation-free job system for executing type-erased closures.
//!
//! This crate provides the [`Job`] type and helpers (such as the [`log!`] macro)
//! for storing closures inline in a fixed-size buffer and dispatching them
//! through worker threads or job queues. Each job carries its own `call`,
//! `clone`, and `drop` logic via function pointers, allowing predictable,
//! allocation-free execution.
//!
//! # Example: Running Jobs on a Worker Thread
//!
//! ```rust
//! use std::sync::mpsc;
//! use std::thread;
//! use hft_logger::{Job, log};
//!
//! // Create a channel for sending jobs
//! let (tx, rx) = mpsc::channel::<Job>();
//!
//! // Spawn a worker thread that receives and runs jobs
//! thread::spawn(move || {
//!     while let Ok(job) = rx.recv() {
//!         job.run();
//!     }
//! });
//!
//! // Send a simple job
//! let job = Job::<64>::new(|| println!("Hello from a job!"));
//! tx.send(job).unwrap();
//!
//! // Or use the `log!` macro to enqueue a println job
//! log!(tx, "Logging from thread: {}", 42);
//! ```
//!
//! This model provides a minimal, fast job runtime suitable for embedded
//! systems, schedulers, executors, or lightweight logging systems.
use std::{mem, ptr};

/// Asynchronously logs a formatted message by sending a `Job` containing a
/// `println!` invocation over the provided sender.
///
/// # Overview
/// The `log!` macro constructs a `Job` that, when executed, prints a formatted
/// message to stdout. Instead of performing I/O immediately, the macro sends
/// the job through the given channel, allowing logging to occur on a separate
/// worker thread.
///
/// # Parameters
/// - `tx`: A sender capable of sending `Job` instances (e.g. `std::sync::mpsc::Sender<Job>`).
/// - `fmt`: A string literal used as the format string for `println!`.
/// - `args`: Optional additional expressions used to fill formatting placeholders.
///
/// The macro accepts an optional trailing comma.
///
/// # Behavior
/// - A closure is created that runs `println!(fmt, args...)`.
/// - This closure is wrapped in a `Job` via `Job::new`.
/// - The resulting job is sent through `tx`.
/// - Any error returned by `send` is ignored.
///
/// # Example
/// ```
/// # use std::sync::mpsc::channel;
/// # use hft_logger::{log, Job};
/// let (tx, rx) = channel::<Job>();
///
/// // Spawn a worker thread to run jobs.
/// std::thread::spawn(move || {
///     while let Ok(job) = rx.recv() {
///         job.run();
///     }
/// });
///
/// log!(tx, "User {} logged in from {}", "alice", "127.0.0.1");
/// ```
#[macro_export]
macro_rules! log {
    ($tx:expr, $fmt:literal $(, $arg:expr)* $(,)?) => {{
        let job = Job::new(move || {
            println!($fmt $(, $arg)*);
        });
        let _ = $tx.send(job);
    }};
}

#[repr(align(16))]
struct Align16<T>(pub T);

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
///   store. The default is `64`.
///
/// # Layout
///
/// * `data` – An `Align16<[u8; N]>` buffer that stores the erased closure.
///   The alignment wrapper ensures the buffer has at least 16-byte alignment,
///   which is typically sufficient for most captured types.
/// * `fn_call` – An `unsafe fn(*mut u8)` that takes a pointer into `data` and
///   invokes the stored callable.
/// * `fn_clone` – An `unsafe fn(*const u8, *mut u8)` that clones the stored
///   value from one buffer to another (source and destination pointers into
///   `data`-like storage).
/// * `fn_drop` – An `unsafe fn(*mut u8)` that drops the stored value in place.
///
/// The `repr(C)` attribute guarantees a stable field layout, which is useful if
/// instances of `Job` are passed across FFI or need a predictable memory
/// representation.
///
/// # Safety
///
/// This type is fundamentally unsafe to construct manually:
///
/// * The function pointers **must** be consistent with the actual type stored
///   inside `data`.
/// * The `fn_call`, `fn_clone`, and `fn_drop` implementations must only read
///   and write within the `N`-byte buffer.
/// * `N` must be at least `size_of::<T>()` for the stored type `T`, and the
///   alignment provided by `Align16` must be sufficient for `T`.
///
/// Incorrect construction or misuse may lead to undefined behavior. Safe APIs
/// (such as `Job::new`, `Job::run`, etc.) should enforce these invariants.
///
/// # Example
///
/// ```rust
/// # use hft_logger::Job;
/// // Typically constructed via a helper like `Job::new`:
/// let job = Job::<64>::new(|| {
///     println!("Hello from a job!");
/// });
///
/// // Later, the job executor would call something like:
/// job.run();
/// ```
#[repr(C)]
pub struct Job<const N: usize = 64> {
    data: Align16<[u8; N]>, // N must be >= sizeof(biggest closure)
    fn_call: unsafe fn(*mut u8),
    fn_clone: unsafe fn(*const u8, *mut u8),
    fn_drop: unsafe fn(*mut u8),
}

// SAFETY: Job only ever contains F: FnOnce() + Send + 'static, enforced in Job::new.
unsafe impl<const N: usize> Send for Job<N> {}

impl<const N: usize> Job<N> {
    /// Creates a new job from a closure, storing it inline without heap
    /// allocation.
    ///
    /// # Type Requirements
    /// The closure `F` must satisfy:
    /// - `FnOnce()`
    /// - `Clone` — needed so the job queue or scheduler can duplicate jobs if
    ///   desired.
    /// - `Send` + 'static — ensures the closure may be transferred across threads.
    ///
    /// # Storage Constraints
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
    ///    `ptr::write`, avoiding heap allocations.
    /// 3. Returns a fully-constructed `Job` that owns the closure.
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
    /// ```
    /// # use hft_logger::Job;
    /// let job = Job::<64>::new(|| println!("Hello from a job!"));
    /// // The job is now a self-contained, type-erased closure.
    /// ```
    pub fn new<F>(f: F) -> Self
    where
        F: FnOnce() + Clone + Send + 'static,
    {
        // dbg!(mem::size_of::<F>());
        // dbg!(mem::align_of::<F>());
        // dbg!(mem::align_of::<Align16<[u8; N]>>());

        // Ensure the closure fits into our inline storage
        assert!(mem::size_of::<F>() <= N);
        assert!(mem::align_of::<F>() <= mem::align_of::<Align16<[u8; N]>>());

        unsafe fn fn_call<F: FnOnce()>(data: *mut u8) {
            // Recreate the closure from the buffer and run it.
            // `read` takes ownership, so `F: FnOnce()` is fine.
            unsafe {
                let f = ptr::read(data as *const F);
                f();
            }
        }

        unsafe fn fn_clone<F>(src: *const u8, dst: *mut u8)
        where
            F: FnOnce() + Clone,
        {
            unsafe {
                let f_src = &*(src as *const F);
                let f_clone = f_src.clone();
                ptr::write(dst as *mut F, f_clone);
            }
        }

        unsafe fn fn_drop<F: FnOnce()>(data: *mut u8) {
            unsafe {
                ptr::drop_in_place(data as *mut F);
            }
        }

        let mut job = Job {
            fn_call: fn_call::<F>,
            fn_clone: fn_clone::<F>,
            fn_drop: fn_drop::<F>,
            data: Align16([0u8; N]),
        };

        unsafe {
            // Place the closure into `data` without heap allocation
            let dst = job.data.0.as_mut_ptr() as *mut F;
            ptr::write(dst, f);
        }

        job
    }

    /// Runs the stored closure, consuming the job.
    ///
    /// This invokes the closure that was previously stored in the inline buffer.
    /// After invocation, the job's drop function pointer is reset to the default
    /// `fn_drop`, preventing the closure from being dropped **twice**.
    ///
    /// # How it Works
    /// - `fn_call` takes ownership of the closure by reading it out of the buffer.
    /// - The closure is executed.
    /// - The job updates `fn_drop` to a "do-nothing" drop function so that
    ///   dropping the `Job` struct afterward does not attempt to drop the closure
    ///   again.
    ///
    /// # Safety
    ///
    /// Internally uses unsafe raw-pointer calls, but the public API guarantees:
    /// - The closure is stored correctly.
    /// - Running a job consumes its stored closure exactly once.
    ///
    /// # Example
    /// ```rust
    /// # use hft_logger::Job;
    /// let job = Job::<64>::new(|| println!("Running a job"));
    /// job.run(); // prints "Running a job"
    /// ```
    pub fn run(mut self) {
        unsafe {
            (self.fn_call)(self.data.0.as_ptr() as *mut u8);
            self.fn_drop = Job::<N>::default().fn_drop;
        }
    }
}

impl<const N: usize> Default for Job<N> {
    fn default() -> Self {
        Self::new(|| panic!("attempt to execute an empty job"))
    }
}

impl<const N: usize> Clone for Job<N> {
    fn clone(&self) -> Self {
        let mut new_job = Job {
            fn_call: self.fn_call,
            fn_clone: self.fn_clone,
            fn_drop: self.fn_drop,
            data: Align16([0u8; N]),
        };
        unsafe {
            (self.fn_clone)(
                self.data.0.as_ptr() as *const u8,
                new_job.data.0.as_mut_ptr() as *mut u8,
            );
        }
        new_job
    }
}

impl<const N: usize> Drop for Job<N> {
    fn drop(&mut self) {
        unsafe {
            (self.fn_drop)(self.data.0.as_mut_ptr());
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::thread;
    use std::sync::{
        Arc,
        atomic::{AtomicUsize, Ordering},
        mpsc,
    };

    /// Helper: prove at compile time that Job is Send.
    fn assert_send<T: Send>() {}
    #[test]
    fn job_is_send() {
        assert_send::<Job>();
        assert_send::<Job<128>>();
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

        let job: Job = Job::new(c);
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

        let job1: Job = Job::new(c);
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
            let _job: Job = Job::new(c);
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
            let job: Job = Job::new(c);
            job.run();
            // After run(), Job's Drop should not try to drop the original F again.
        }

        // Exactly one drop of the captured guard.
        assert_eq!(drops.load(Ordering::SeqCst), 1);
    }

    #[test]
    #[should_panic(expected = "attempt to execute an empty job")]
    fn default_job_panics_on_run() {
        let job: Job = Job::default();
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
        let _job: Job<64> = Job::new(c);
        let _ = _job;
    }

    #[test]
    fn log_macro_sends_job_over_channel() {
        let (tx, rx) = mpsc::channel::<Job>();

        log!(tx, "hello from log macro");
        let job = rx.recv().expect("expected a Job from log! macro");

        // If this runs without panic, we’re good enough here.
        job.run();
    }

    #[test]
    fn job_can_be_sent_to_worker_thread_and_run() {
        let counter = Arc::new(AtomicUsize::new(0));
        let (tx, rx) = mpsc::channel::<Job<1024>>();

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
        let job: Job = Job::new(c);
        job.run();
    }
}
