use std::{mem, sync::mpsc, thread};
use hft_logger::{Job, log};

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
