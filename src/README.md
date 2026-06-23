# Source Layout

`src` contains shared value types and explicit backend modules.

- `write.rs`: positioned write slices and validated write batches.
- `buffer.rs`: owned byte buffers plus the `Allocator` trait, `Pool`, and `System`.
- `sync/`: `std::fs`-like synchronous backend.
- `tokio/`: `tokio::fs`-like async backend.
- `mmap.rs`: read-only memory mapping file backend.
- `uring.rs`: Linux `io_uring` file backend and ring implementation. Cursor traits and positioned methods use the ring; append mode is unsupported.

The crate root exports backend modules and shared value types, but no default file backend or backend free functions.

Read-capable file backends are generic over an allocator. With `feature = "pool"`, `DefaultAllocator` is `Pool`; without it, `DefaultAllocator` is `System`. Zero-length reads return an empty `OwnedBytes::Vec` without using the allocator. `mmap` is not allocator-backed.
