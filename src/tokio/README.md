# tokio Backend

The `tokio` module provides a `tokio::fs`-like `File` and `OpenOptions` with positioned I/O methods.

File operations are async where they can avoid blocking runtime worker threads. Positioned reads and writes move owned buffers into Tokio blocking tasks; batch writes use bounded worker waves inside one blocking task.

`File` and `OpenOptions` are allocator-generic. Default reads use pooled buffers when `pool` is enabled; call `OpenOptions::allocator(System)` to force heap-backed reads.

This feature intentionally does not depend on Rayon.
