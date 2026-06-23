# fastio

Fast file I/O backends for Rust libraries and applications.

`fastio` exposes explicit backend-owned file handles. There is no default backend and no module-level convenience API: choose `sync::File`, `tokio::File`, `mmap::File`, or Linux `uring::File` directly.

## Features

- `sync`: synchronous file I/O with positioned read/write methods.
- `tokio`: async I/O using Tokio, without a Rayon dependency.
- `mmap`: read-only memory maps using `memmap2`.
- `io-uring`: Linux-only `io_uring` backend.
- `pool`: pooled read buffers using `zeropool`.

Default features enable all supported backends for the current platform.

## Example

```rust
use fastio::sync::File;

let file = File::open("model.bin")?;
let bytes = file.read_at(0, 4096)?;
# Ok::<(), std::io::Error>(())
```

## Development

```bash
cargo test --all-features
cargo clippy --all-targets --all-features -- -D warnings
```

License: Apache-2.0.
