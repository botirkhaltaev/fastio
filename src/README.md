# Source Layout

`src` contains the core API vocabulary and backend modules.

- `range.rs`: byte ranges, file range requests, and ordered range read results.
- `write.rs`: positioned write slices and non-overlap validation.
- `traits.rs`: `BlockingIo`, `AsyncIo`, and `MmapIo`.
- `buffer.rs`: owned byte buffers and optional pooled, aligned, or mapped storage.
- `sync/`: blocking backend implementations.
- `tokio/`: async Tokio backend.
- `mmap.rs`: read-only memory mapping backend.
- `io_uring.rs`: Linux `io_uring` backend.

The crate root re-exports the public API.
