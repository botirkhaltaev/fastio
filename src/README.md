# Source Layout

`src` contains shared value types and explicit backend modules.

- `write.rs`: positioned write slices and validated write batches.
- `buffer.rs`: owned byte buffers and optional pooled, aligned, or mapped storage.
- `sync/`: `std::fs`-like synchronous backend.
- `tokio/`: `tokio::fs`-like async backend.
- `mmap.rs`: read-only memory mapping file backend.
- `uring.rs`: Linux `io_uring` file backend and ring implementation.

The crate root exports backend modules and shared value types, but no default file backend or backend free functions.
