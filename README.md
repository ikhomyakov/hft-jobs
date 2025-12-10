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
* Explicit inline capacity via const generic (`Job<N, R, C>`), so you size storage for your largest capture
* Optional mutable context (`C`, default `()`), passed to jobs at execution time via `run_with_ctx`
* Return type is generic (`R`, defaulting to `()`), so jobs can either be fire-and-forget or yield a value

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
hft-jobs = "0.3.1"
```

## Quick Example (Thread + MPSC)

```rust
use std::sync::mpsc;
use std::thread;
use hft_jobs::Job;

// Create a simple channel for dispatching jobs to a worker.
// `Job<N, R, C>` now requires an explicit inline capacity; 64 bytes works well
// for small captures. `C` defaults to `()`.
let (tx, rx) = mpsc::channel::<Job<64, String>>();

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
```

This creates a minimal, allocation-free job execution pipeline.

### Choosing an inline capacity

`Job<N, R, C>` requires an explicit inline buffer size `N`. Pick a value large enough
for your biggest closure capture:

- `64` bytes works for most lightweight logging/telemetry closures.
- Larger captures (e.g., big structs or arrays) may need `128` or `256`.
- Oversizing is cheap (storage is inline) and fixed at compile time.

### Context-aware jobs

Jobs can receive a mutable context at execution time via the third generic parameter `C` and the `run_with_ctx` API. The zero-context case (`C = ()`) still works with the familiar `new`/`run` helpers.

```rust
use hft_jobs::Job;

#[derive(Default)]
struct Context {
    total: u32,
}

let mut ctx = Context::default();

let job = Job::<64, u32, Context>::new_with_ctx(|c| {
    c.total += 1;
    c.total
});

assert_eq!(job.run_with_ctx(&mut ctx), 1);
assert_eq!(ctx.total, 1);
```

You can still use `Job::<N, R>::new` and `run` when no context is needed.

### Generic parameters

- `N`: inline buffer size in bytes (must be provided). Choose a capacity that fits your largest closure capture.
- `R`: return type of the stored closure. Defaults to `()`, so you only specify it when you need to return a value (e.g., `Job<64, String>`).
- `C`: mutable context type passed to the closure when running. Defaults to `()`. Use `run_with_ctx` when `C` is non-`()`; otherwise `run` is available for the zero-context case.

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
