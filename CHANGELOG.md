# Release Notes

## [0.3.0] — 2025-12-08

### Improvements

* Added optional context support via `Job<N, R, C>` with `new_with_ctx` and `run_with_ctx`, plus `new`/`run` conveniences for `C = ()`.
* Documented the context API in README and crate docs, including examples and a unit test.

## [0.2.0] — 2025-12-07

### ⚠️  Breaking Changes

* Changed the `Job` by making the buffer size `N` explicit (removing the default) and generalizing `Job` over a return type `R` (e.g., `Job<64, String>`). Closures stored in a Job can now return values instead of always producing `()`, and callers must explicitly specify the buffer size (e.g., `Job<64>`). This expands flexibility for schedulers, worker threads, and job pipelines. Existing code may need to update type annotations such as specifying Job<64> where defaults previously applied.

### Improvements

* Refined documentation to clearly describe the behavior and constraints of `Job<N, R = ()>`, including how run consumes the closure and handles drop semantics without double-dropping.

* Added tests for value-returning jobs (e.g., numeric and String results).
