# fastio

A set of I/O primitives as a `std::fs` replacement for Rust. Experiment with
different I/O backends to find what fits your application best. It should also
be faster than `std::fs`.

[![License: Apache-2.0](https://img.shields.io/badge/license-Apache--2.0-blue.svg)](LICENSE)
[![CI](https://github.com/botirk38/fastio/actions/workflows/ci.yml/badge.svg)](https://github.com/botirk38/fastio/actions)

## About

`fastio` gives you explicit, backend owned file handles with no hidden defaults.
Instead of a single `File` type, you pick the backend that matches your workload:
`sync::File`, `tokio::File`, `mmap::File`, or `uring::File`. Each backend is
tuned for its I/O strategy, and reads allocate through a configurable `Allocator`
(pooled by default) so bulk workloads skip repeated allocation overhead.

There is no default backend and no module level convenience API. You choose.

## Backends

* **`sync`**: Synchronous file I/O with positioned read/write methods. Uses
  platform native calls (`pread`/`pwrite` on Unix, `SetFilePointerEx` on Windows).

* **`tokio`**: Async I/O via Tokio. Positioned operations run in `spawn_blocking`
  to keep the runtime free. No Rayon dependency.

* **`mmap`**: Read only memory maps backed by `memmap2`. Returns a lazy mapped
  region in ~5 µs regardless of file size.

* **`uring`** (Linux only): Ring backed positioned I/O with `read_at_batch` for
  submitting multiple reads in a single syscall.

* **`pool`**: Pooled read buffers via `zeropool`. Recycles allocations to avoid
  repeated `mmap`/`munmap` syscalls. Up to 19,000x faster than system allocation
  at large buffer sizes.

## Performance

All benchmarks use [Criterion.rs](https://bheisler.github.io/criterion.rs/book/)
on Linux (x86_64). Files range from 4 KiB to 64 MiB. Run them yourself:

```bash
cargo bench --all-features
```

### Full File Read (`read_all`)

| File Size | `std::fs::read` | fastio sync (pool) | Speedup | fastio mmap |
|:----------|:-----------------|:-------------------|:--------|:------------|
| 4 KiB     | 6.37 µs          | 2.46 µs            | **2.6x** | 5.25 µs    |
| 64 KiB    | 9.97 µs          | 5.59 µs            | **1.8x** | 5.24 µs    |
| 1 MiB     | 76.6 µs          | 72.6 µs            | **1.1x** | 5.24 µs    |
| 16 MiB    | 1.53 ms          | 1.49 ms            | **1.02x** | 5.54 µs   |
| 64 MiB    | 52.3 ms          | 29.2 ms            | **1.8x** | 6.28 µs    |

> mmap times reflect the mapping syscall only. Actual data transfer happens on
> first access via page faults.

### Positioned Write (`write_all_at`)

fastio sync matches raw `pwrite` with zero abstraction overhead:

| File Size | `std pwrite` | fastio sync |
|:----------|:-------------|:------------|
| 64 KiB    | 3.73 µs      | 3.67 µs     |
| 1 MiB     | 3.76 µs      | 3.74 µs     |
| 64 MiB    | 3.78 µs      | 3.70 µs     |

### Buffer Allocator

The pool allocator recycles buffers instead of going through `mmap`/`munmap` on
every allocation:

| Buffer Size | System alloc | Pool alloc | Speedup          |
|:------------|:-------------|:-----------|:-----------------|
| 4 KiB       | 155 ns       | 17.6 ns    | **8.8x**         |
| 64 KiB      | 932 ns       | 32.3 ns    | **29x**          |
| 1 MiB       | 19.4 µs      | 64.0 ns    | **303x**         |
| 16 MiB      | 588 µs       | 30.9 ns    | **19,000x**      |

### io_uring Batch Reads (`read_at_batch`)

Batched submission amortizes the `io_uring_enter` syscall across multiple reads:

| Batch Size | Sequential `pread` | uring batch | Speedup vs pread |
|:-----------|:-------------------|:------------|:-----------------|
| n=16       | 13.6 µs            | 12.3 µs     | **1.1x**         |
| n=64       | 68.8 µs            | 68.5 µs     | ~1x              |
| n=256      | 225 µs             | 189 µs      | **1.2x**         |

At 256 concurrent reads, uring batch beats sequential `pread` by 19% and
individual uring calls by 2x.

### Cursor Traits (`Read` / `Write`)

fastio sync implements `std::io::Read` and `std::io::Write` at the same speed
as `std::fs::File`:

| Operation | File Size | `std::fs` | fastio sync |
|:----------|:----------|:----------|:------------|
| Read      | 1 MiB     | 79.7 µs   | 79.0 µs     |
| Read      | 64 MiB    | 5.98 ms   | 5.95 ms     |
| Write     | 1 MiB     | 119 µs    | 118 µs      |
| Write     | 16 MiB    | 1.84 ms   | 1.83 ms     |

### Async Read (Tokio)

| File Size | `tokio::fs::read` | fastio tokio (pool) |
|:----------|:-------------------|:--------------------|
| 1 MiB     | 135 µs             | 279 µs              |
| 16 MiB    | 1.55 ms            | 2.08 ms             |
| 64 MiB    | 53.1 ms            | **51.0 ms**         |

fastio tokio wins at large file sizes where the pooled allocator pays off. At
smaller sizes, `spawn_blocking` dispatch overhead dominates.

## Installation

Add to your `Cargo.toml`:

```toml
[dependencies]
fastio = "0.3"
```

All backends are enabled by default. To select specific backends:

```toml
[dependencies]
fastio = { version = "0.3", default-features = false, features = ["sync", "pool"] }
```

### Requirements

* Rust 1.92+ (edition 2024)
* Linux kernel 5.6+ for the `io_uring` backend

## Usage

### Synchronous Read

```rust
use fastio::sync::File;

let file = File::open("data.bin")?;
let bytes = file.read_at(0, 4096)?;
# Ok::<(), std::io::Error>(())
```

### System Allocator

```rust
use fastio::{System, sync::File};

let file = File::options()
    .read(true)
    .allocator(System)
    .open("data.bin")?;
let bytes = file.read_at(0, 4096)?;
# Ok::<(), std::io::Error>(())
```

### Memory Mapped Read

```rust
use fastio::mmap::File;

let file = File::open("data.bin")?;
let region = file.map()?;
let contents: &[u8] = &region;
# Ok::<(), std::io::Error>(())
```

### Async Read (Tokio)

```rust
use fastio::tokio::File;

let file = File::open("data.bin").await?;
let bytes = file.read_all().await?;
# Ok::<(), std::io::Error>(())
```

### io_uring Batch Read (Linux)

```rust
use fastio::uring::File;

let file = File::open("data.bin")?;
let regions = vec![(0, 4096), (4096, 4096), (8192, 4096)];
let results = file.read_at_batch(&regions)?;
# Ok::<(), std::io::Error>(())
```

## Available Features

| Feature      | Description                                         |
|:-------------|:----------------------------------------------------|
| `sync`       | Synchronous file I/O with positioned read/write     |
| `tokio`      | Async I/O via Tokio (no Rayon dependency)            |
| `mmap`       | Read only memory maps via `memmap2`                  |
| `io-uring`   | Linux `io_uring` backend with batch support          |
| `pool`       | Pooled read buffers via `zeropool`                   |

With `pool` enabled, read methods allocate from the process wide pool by default
and return `OwnedBytes::Pooled`. Pass `System` to an `OpenOptions` to force heap
backed `OwnedBytes::Vec` reads. Zero length reads return an empty
`OwnedBytes::Vec` without touching the allocator.

## Development

```bash
cargo fmt --all -- --check
cargo check --all-targets
cargo check --no-default-features --all-targets
cargo check --no-default-features --features tokio --all-targets
cargo test --all-features
cargo clippy --all-targets --all-features -- -D warnings
```

### Running Benchmarks

```bash
cargo bench --all-features
```

Benchmark results are generated in `target/criterion/` with HTML reports per
group.

## Contributing

Contributions are welcome. Please run the full validation suite before submitting
a pull request.

## License

fastio is licensed under the Apache 2.0 license. See the [`LICENSE`](LICENSE)
file for details.
