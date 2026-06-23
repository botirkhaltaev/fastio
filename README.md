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
With `pool` enabled, read methods allocate from the process-wide pool by
default and return `OwnedBytes::Pooled`. Use `System` on an `OpenOptions` value
to force normal heap-backed `OwnedBytes::Vec` reads.

## Example

```rust
use fastio::sync::File;

let file = File::open("model.bin")?;
let bytes = file.read_at(0, 4096)?;
# Ok::<(), std::io::Error>(())
```

```rust
use fastio::{System, sync::File};

let file = File::options()
    .read(true)
    .allocator(System)
    .open("model.bin")?;
let bytes = file.read_at(0, 4096)?;
# Ok::<(), std::io::Error>(())
```

## Development

```bash
cargo test --all-features
cargo clippy --all-targets --all-features -- -D warnings
```

License: Apache-2.0.
