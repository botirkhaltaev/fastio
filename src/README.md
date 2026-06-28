# Source Layout

`src` contains shared value types and explicit backend modules.

- `write.rs`: positioned write slices and validated write batches.
- `buffer.rs`: shared owned byte buffers (`Bytes`) and the internal buffer pool.
- `sync/`: `std::fs`-like synchronous backend.
- `tokio/`: async backend using `tokio::fs` for regular operations and `spawn_blocking` for positioned I/O.
- `mmap.rs`: read-only memory mapping file backend.
- `uring.rs`: Linux `io_uring` file backend and ring implementation. Cursor traits and positioned methods use the ring; append mode is unsupported.

The crate root exports backend modules and shared value types, but no default file backend or backend free functions.

Read-capable file backends allocate through the internal `Bytes::allocate` path, which reuses buffers from a process-wide pool. Zero-length reads return an empty buffer without touching the pool. `mmap` is not pool-backed.
