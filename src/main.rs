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

const JOB_DATA_SIZE: usize = 64; // tweak: must be >= sizeof(biggest closure)

#[repr(align(16))]
struct Align16<T>(pub T);

#[repr(C)]
struct Job {
    data: Align16<[u8; JOB_DATA_SIZE]>,
    func: unsafe fn(*mut u8),
}

// SAFETY: Job only ever contains F: FnOnce() + Send + 'static, enforced in Job::new.
unsafe impl Send for Job {}

impl Job {
    fn new<F>(f: F) -> Job
    where
        F: FnOnce() + Send + 'static,
    {
        dbg!(mem::size_of::<F>());
        dbg!(mem::align_of::<F>());
        dbg!(mem::align_of::<Align16<[u8; JOB_DATA_SIZE]>>());

        // Ensure the closure fits into our inline storage
        assert!(mem::size_of::<F>() <= JOB_DATA_SIZE);
        assert!(mem::align_of::<F>() <= mem::align_of::<Align16<[u8; JOB_DATA_SIZE]>>());

        unsafe fn call<F>(data: *mut u8)
        where
            F: FnOnce(),
        {
            // Recreate the closure from the buffer and run it.
            // `read` takes ownership, so `F: FnOnce()` is fine.
            unsafe {
                let f = ptr::read(data as *const F);
                f();
            }
        }

        let mut job = Job {
            func: call::<F>,
            data: Align16([0u8; JOB_DATA_SIZE]),
        };

        unsafe {
            // Place the closure into `data` without heap allocation
            let dst = job.data.0.as_mut_ptr() as *mut F;
            ptr::write(dst, f);
        }

        job
    }

    fn run(self) {
        unsafe {
            (self.func)(self.data.0.as_ptr() as *mut u8);
        }
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
    log!(tx, "trade: symbol={}, price={} size={}", symbol, price, size);

    let a = String::from("Hello");
    let b = String::from("world");
    let c: u128 = 42;
    log!(tx, "{}, {}! {}", a, b, c);

    let vs = vec![1, 2, 3];
    let closure1 = move || {
        println!("Hello, vec {:?}!", vs);
    };
    dbg!(mem::size_of_val(&closure1));

    let job4 = Job::new(move || {
        closure1();
        closure1();
    });

    tx.send(job4).unwrap();

    drop(tx);
    worker.join().unwrap();
}
