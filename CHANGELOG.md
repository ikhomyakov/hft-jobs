# Release Notes

## [0.3.0] — 2026-01-15

### Improvements

* Added fallible constructors `Job::try_new` and `JobInitError` so oversized or misaligned closures return structured errors instead of panicking.
* Documented the non-panicking constructor in the README and added tests covering the new error cases.

## [0.2.0] — 2025-12-07

### ⚠️  Breaking Changes

* Changed the `Job` by making the buffer size `N` explicit (removing the default) and generalizing `Job` over a return type `R` (e.g., `Job<64, String>`). Closures stored in a Job can now return values instead of always producing `()`, and callers must explicitly specify the buffer size (e.g., `Job<64>`). This expands flexibility for schedulers, worker threads, and job pipelines. Existing code may need to update type annotations such as specifying Job<64> where defaults previously applied.

### Improvements

* Refined documentation to clearly describe the behavior and constraints of `Job<N, R = ()>`, including how run consumes the closure and handles drop semantics without double-dropping.

* Added tests for value-returning jobs (e.g., numeric and String results).

* Updated documentation, examples, and tests to show the explicit inline-capacity requirement.
