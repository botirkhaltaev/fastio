# Source Layout

`src` contains shared value types and explicit backend modules.

- `range.rs`: byte ranges, file range requests, and ordered range read results.
- `write.rs`: positioned write slices and non-overlap validation.
- `buffer.rs`: owned byte buffers and optional pooled, aligned, or mapped storage.
- `sync/`: `std::fs`-like synchronous backend.
- `tokio/`: `tokio::fs`-like async backend.
- `mmap.rs`: read-only memory mapping file backend.
- `io_uring.rs`: internal Linux `io_uring` primitives.
- `uring.rs`: public Linux `io_uring` file backend.

The crate root exports backend modules and shared value types, but no default file backend.
