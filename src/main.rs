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
    fn_drop: unsafe fn(*mut u8),
}

// SAFETY: Job only ever contains F: FnOnce() + Send + 'static, enforced in Job::new.
unsafe impl Send for Job {}

impl<const N: usize> Job<N> {
    fn new<F>(f: F) -> Self
    where
        F: FnOnce() + Send + 'static,
    {
        dbg!(mem::size_of::<F>());
        dbg!(mem::align_of::<F>());
        dbg!(mem::align_of::<Align16<[u8; N]>>());

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

        unsafe fn fn_drop<F: FnOnce()>(data: *mut u8) {
            unsafe {
                ptr::drop_in_place(data as *mut F);
            }
        }

        let mut job = Job {
            fn_call: fn_call::<F>,
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

impl<const N: usize> Drop for Job<N> {
    fn drop(&mut self) {
        unsafe {
            (self.fn_drop)(self.data.0.as_mut_ptr());
        }
    }
}

#[derive(Debug)]
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

    let vs = vec![1, 2, 3];
    let closure1 = move || {
        println!("Hello, vec {:?}!", dbg!(vs));
    };
    dbg!(mem::size_of_val(&closure1));

    let job4 = Job::new(move || {
        closure1.clone()();
        closure1();
    });

    tx.send(job4).unwrap();

    drop(tx);
    worker.join().unwrap();
}
