# tokio Backend

The `tokio` module provides a `tokio::fs`-like `File` and `OpenOptions` with positioned I/O methods.

File operations are async. Positioned writes use blocking positioned I/O inside Tokio blocking sections so callers can write independent file ranges in parallel.

`File` and `OpenOptions` are allocator-generic. Default reads use pooled buffers when `pool` is enabled; call `OpenOptions::allocator(System)` to force heap-backed reads.

This feature intentionally does not depend on Rayon.
