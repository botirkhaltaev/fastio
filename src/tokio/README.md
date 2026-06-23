# tokio Backend

The `tokio` module provides a `tokio::fs`-like `File` and `OpenOptions` with positioned I/O methods.

Regular filesystem operations use `tokio::fs` directly. Positioned reads and writes move owned buffers into blocking tasks because Tokio does not expose positioned file I/O; batch writes use bounded worker waves inside one blocking task.

`File` and `OpenOptions` are allocator-generic. Default reads use pooled buffers when `pool` is enabled; call `OpenOptions::allocator(System)` to force heap-backed reads.

This feature intentionally does not depend on Rayon.
