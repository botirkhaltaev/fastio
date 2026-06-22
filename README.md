# fastio

Fast file I/O backends for Rust libraries and applications.

`fastio` provides a small set of traits and backend implementations for blocking I/O, Tokio I/O, memory maps, and Linux `io_uring`. Backends are cheap configuration values. Constructors validate local options; platform, kernel, filesystem, and permission failures are returned by the operation that encounters them.

## Features

- `sync`: synchronous positioned I/O with Rayon-backed batch operations.
- `tokio`: async I/O using Tokio, without a Rayon dependency.
- `mmap`: read-only memory maps using `memmap2`.
- `io-uring`: Linux-only `io_uring` backend.
- `pool`: pooled read buffers using `zeropool`.

Default features enable all supported backends for the current platform.

## Example

```rust
use fastio::{BlockingIo, SyncIo};

let bytes = SyncIo::new().read_file("model.bin".as_ref())?;
# Ok::<(), std::io::Error>(())
```

## Development

```bash
cargo test --all-features
cargo clippy --all-targets --all-features -- -D warnings
```

License: Apache-2.0.
