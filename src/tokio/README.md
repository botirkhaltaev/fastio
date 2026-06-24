# tokio Backend

The `tokio` module provides async `File` and `OpenOptions` with positioned I/O methods.

All operations run via `tokio::task::spawn_blocking` with a `try_clone`d OS handle (a cheap kernel `dup`). This avoids `tokio::fs` entirely. On Unix, positioned reads and writes use `pread`/`pwrite` which are cursor-independent; on Windows, platform helpers save/restore the file position. Batch writes use bounded worker waves inside one blocking task.

`File` and `OpenOptions` are allocator-generic. Default reads use pooled buffers when `pool` is enabled; call `OpenOptions::allocator(System)` to force heap-backed reads.

This feature intentionally does not depend on Rayon.
