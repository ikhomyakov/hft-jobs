use std::{mem, ptr, sync::mpsc, thread};

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

#[repr(C)]
struct Job<const N: usize = 64> {
    data: Align16<[u8; N]>, // N must be >= sizeof(biggest closure)
    fn_call: unsafe fn(*mut u8),
    fn_clone: unsafe fn(*const u8, *mut u8),
    fn_drop: unsafe fn(*mut u8),
}

// SAFETY: Job only ever contains F: FnOnce() + Send + 'static, enforced in Job::new.
unsafe impl<const N: usize> Send for Job<N> {}

impl<const N: usize> Job<N> {
    fn new<F>(f: F) -> Self
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

    fn run(mut self) {
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

#[derive(Debug, Clone)]
struct X;

impl Drop for X {
    fn drop(&mut self) {
        println!("DROP!");
    }
}

fn main() {
    dbg!(mem::size_of::<Job>());
    dbg!(mem::align_of::<Job>());
    let (tx, rx) = mpsc::channel::<Job>();

    // Background thread that *owns* rx and executes jobs
    let worker = thread::spawn(move || {
        while let Ok(job) = rx.recv() {
            job.run();
        }
    });

    // "Hot" thread: create a job and move it through the channel
    log!(tx, "Hello, world!");

    let price = 123_i64;
    let size = 10_u128;
    let symbol = "SWPPX";
    let x = X;
    log!(
        tx,
        "trade: symbol={}, price={} size={}, x={:?}",
        symbol,
        price,
        size,
        x
    );

    let a = String::from("Hello");
    let b = String::from("world");
    let c: u128 = 42;
    log!(tx, "{}, {}! {}", a, b, c);

    let mut vs = vec![1, 2, 3];
    let mut closure1 = move || {
        vs.push(vs.last().unwrap().clone() + 1);
        println!("Hello, vec {:?}!", vs);
    };

    dbg!(mem::size_of_val(&closure1));

    closure1();
    closure1();

    let job3: Job = Job::new(closure1.clone());
    dbg!(mem::size_of_val(&job3));

    job3.clone().run();
    job3.run();

    let job4 = Job::new(move || {
        closure1.clone()();
        closure1();
    });

    tx.send(job4).unwrap();

    drop(tx);
    worker.join().unwrap();
}

#[cfg(test)]
mod tests {
    use super::*;
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
