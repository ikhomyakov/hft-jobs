# Release Notes

## [0.3.1] — 2025-12-09

### Improvements

* Inline storage automatically pads to the captured closure’s alignment (no fixed 16-byte cap) and fails at compile time if the padded size would exceed `N`.
* Faster run path: post-execution drop handler is now a static no-op function instead of constructing a default job.

## [0.3.0] — 2025-12-08

### Improvements

* Generalized `Job` over optional execution context `C`: `Job<N, R = (), C = ()>`. Introduced methods `new_with_ctx<F>(f: F) -> Self` and `run_with_ctx(self, ctx: &mut C) -> R`. The existing `new` and `run` methods remain available as conveniences for the common case `C = ()`. This enhancement allows jobs to receive external execution context at the call site.

* Expanded documentation for the new context API, including runnable examples and a dedicated unit test.

* Optimization: closure size and alignment are now validated at compile time rather than runtime, reducing overhead and catching configuration issues earlier.

* Optimization: Applied inline declarations to `Job` methods to enhance performance and reduce function call overhead.

## [0.2.0] — 2025-12-07

### ⚠️  Breaking Changes

* Changed the `Job` by making the buffer size `N` explicit (removing the default) and generalizing `Job` over a return type `R` (e.g., `Job<64, String>`). Closures stored in a Job can now return values instead of always producing `()`, and callers must explicitly specify the buffer size (e.g., `Job<64>`). This expands flexibility for schedulers, worker threads, and job pipelines. Existing code may need to update type annotations such as specifying Job<64> where defaults previously applied.

### Improvements

* Refined documentation to clearly describe the behavior and constraints of `Job<N, R = ()>`, including how run consumes the closure and handles drop semantics without double-dropping.

* Added tests for value-returning jobs (e.g., numeric and String results).
