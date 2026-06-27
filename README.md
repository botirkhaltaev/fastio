# fastio

Fast file I/O backends for Rust libraries and applications.

`fastio` exposes explicit backend-owned file handles. There is no default backend and no module-level convenience API: choose `sync::File`, `tokio::File`, `mmap::File`, or Linux `uring::File` directly.

## Features

- `sync`: synchronous file I/O with positioned read/write methods.
- `tokio`: async I/O using Tokio, without a Rayon dependency.
- `mmap`: read-only memory maps using `memmap2`.
- `io-uring`: Linux-only `io_uring` backend. Cursor traits and positioned methods are ring-backed; append mode is intentionally unsupported.

Default features enable all supported backends for the current platform.
Read methods allocate through an internal process-wide pool and return `Bytes`.
Zero-length reads return an empty buffer without touching the pool.

## Example

```rust
use fastio::sync::File;

let file = File::open("model.bin")?;
let bytes = file.read_at(0, 4096)?;
# Ok::<(), std::io::Error>(())
```

## Development

```bash
cargo fmt --all -- --check
cargo check --all-targets
cargo check --no-default-features --all-targets
cargo check --no-default-features --features tokio --all-targets
cargo test --all-features
cargo clippy --all-targets --all-features -- -D warnings
```

License: Apache-2.0.
