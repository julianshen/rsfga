# Changelog

All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

### Added
- **ReverseExpand algorithm for ListObjects API** - New graph traversal algorithm that starts from the user and expands outward to find all objects, rather than iterating through all objects and checking permissions individually.
- **Fuzz testing infrastructure** - Added cargo-fuzz targets for security-critical graph resolver code (Invariant I4):
  - `fuzz_list_objects`: Tests ListObjects request construction and validation
  - `fuzz_reverse_expand`: Tests model/userset construction and traversal

### Changed
- **ListObjects performance improvement** - Replaced per-object Check() approach with ReverseExpand algorithm:
  - p95 latency: 14.2s → 5.9ms (2400x improvement)
  - Throughput: 176 req/s sustained
  - Validated with k6 load tests against PostgreSQL backend

### Fixed
- **Truncation signaling in intersection/exclusion** - Fixed correctness bug where truncation wasn't properly signaled when hitting the result limit during iteration over This/Intersection/Exclusion usersets.
- **Error propagation in condition evaluation** - Condition evaluation errors are now properly propagated instead of silently returning false.
- **Parent relation lookup error handling** - Only skip processing when relation or type is genuinely not found, rather than on any lookup error.

### Documentation
- Updated ADR-018 with validated performance results
- Documented TupleReader default trait implementation behavior
- Documented union branch cycle detection semantics (backtracking approach)
