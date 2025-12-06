# Allocation-free job system for HFT and real-time systems

A lightweight, ultra-low-latency **inline closure job system** designed for **high-frequency trading (HFT)**, real-time processing, and low-latency systems.

Jobs are stored **inline in fixed-size buffers**, using **type erasure** and function pointers for call/clone/drop operations.

Suitable for **thread-to-thread work queues**, **background logging**, **task dispatch**, and **embedded runtimes**.

[![Crates.io](https://img.shields.io/crates/v/hft-jobs.svg)](https://crates.io/crates/hft-jobs)
[![Documentation](https://docs.rs/hft-jobs/badge.svg)](https://docs.rs/hft-jobs)
[![License: LGPL-3.0-or-later](https://img.shields.io/badge/License-LGPL%203.0--or--later-blue.svg)](https://www.gnu.org/licenses/lgpl-3.0)
[![Rust](https://img.shields.io/badge/rust-stable-brightgreen.svg)](https://www.rust-lang.org)

## Features

* **Inline, allocation-free closure storage**
* **Predictable memory layout** using `repr(C)`
* **Type-erased call/clone/drop** via function pointers
* Designed for **real-time logging**, **job queues**, and **worker thread dispatch**
* Thread-safe (`Send` requirement for jobs)
* Compatible with **SPMC**, **MPSC**, and thread-pool patterns

## Latency Characteristics

Jobs store closures directly in a fixed-size inline buffer, avoiding the allocator entirely.

**Implications:**

* **Lowest possible dispatch latency** (no heap activity)
* Excellent for **hot loops**, **HFT components**, **embedded systems**
* Jobs require the closure to be **small enough** to fit in the inline buffer
* Predictable performance due to no dynamic typing or heap behavior

This design is ideal for **HFT**, **real-time telemetry**, **background I/O tasks**, **structured logging**, and other high-performance workloads that require minimal jitter.

## Installation

```toml
[dependencies]
hft-jobs = "0.1"
```

## Quick Example (Thread + MPSC)

```rust
use std::sync::mpsc;
use std::thread;
use hft_jobs::Job;

// Create a simple channel for dispatching jobs to a worker
let (tx, rx) = mpsc::channel::<Job>();

// Spawn the worker thread
thread::spawn(move || {
    while let Ok(job) = rx.recv() {
        job.run(); // executes the closure
    }
});

// Send a job
let job = Job::<64>::new(|| println!("Hello from a job!"));
tx.send(job).unwrap();

// A convenience macro that enqueues a logging job.
macro_rules! log {
    ($tx:expr, $fmt:literal $(, $arg:expr)* $(,)?) => {{
        let job = Job::new(move || {
            println!($fmt $(, $arg)*);
        });
        let _ = $tx.send(job);
    }};
}

// Use the `log!` macro to enqueue a println job.
log!(tx, "Logging from thread: {}", 42);
```

This creates a minimal, allocation-free job execution pipeline.

## Design Overview

A job is stored internally as:

```
[ inline buffer (N bytes) | fn_call | fn_clone | fn_drop ]
```

### Guarantees

* Closure fits entirely in a fixed-size inline buffer (`N`)
* Closure is reconstructed **without dynamic allocation**
* `fn_call`, `fn_clone`, and `fn_drop` are monomorphized for the closure type
* Safe outer API, with invariants upheld internally by careful unsafe code

### Pattern

1. `Job::new` writes a closure into the buffer using `ptr::write`
2. Worker thread calls `job.run()`
3. Job invokes its specialized `fn_call`
4. Drop logic is safely erased to avoid double-drops

This avoids all trait objects, dynamic allocation, or vtables.

## Testing

```bash
cargo test
```

## License

Copyright © 2005–2025 IKH Software, Inc.

Licensed under **LGPL-3.0-or-later**.
See [`LICENSE`](LICENSE) or [https://www.gnu.org/licenses/lgpl-3.0.html](https://www.gnu.org/licenses/lgpl-3.0.html).

## Contributing

Contributions are welcome! Please open issues or pull requests on GitHub.

By submitting a contribution, you agree that it will be licensed under the
project’s **LGPL-3.0-or-later** license.

