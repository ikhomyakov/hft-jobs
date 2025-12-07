# Release Notes

## [0.2.0] — 2025-12-07

### ⚠️  Breaking Changes

* Generalized `Job` over a return type `R`, so closures stored in a `Job` can now produce values instead of always returning `()`.
  This change enables jobs to return values, improving flexibility for schedulers, worker threads, and job-based pipelines.
  Code written against earlier versions may need to explicitly specify `Job<(), N>`.

### Improvements

* Refined documentation to clearly describe the behavior and constraints of Job<R, N>, including how run consumes the closure and
  handles drop semantics without double-dropping.

* Added tests for value-returning jobs (e.g., numeric and String results).
