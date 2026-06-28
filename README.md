# fastio

A set of I/O primitives as a `std::fs` replacement for Rust. Experiment with
different I/O backends to find what fits your application best.

[![License: Apache-2.0](https://img.shields.io/badge/license-Apache--2.0-blue.svg)](LICENSE)
[![CI](https://github.com/botirk38/fastio/actions/workflows/ci.yml/badge.svg)](https://github.com/botirk38/fastio/actions)

## About

`fastio` gives you explicit, backend owned file handles with no hidden defaults.
Instead of a single `File` type, you pick the backend that matches your workload:
`sync::File`, `tokio::File`, `mmap::File`, or `uring::File`. Each backend is
tuned for its I/O strategy. Read methods allocate through an internal process-wide
pool so bulk workloads skip repeated allocation overhead.

There is no default backend and no module level convenience API. You choose.

## Backends

* **`sync`**: Synchronous file I/O with positioned read/write methods. Uses
  platform native calls (`pread`/`pwrite` on Unix, `SetFilePointerEx` on Windows).

* **`tokio`**: Async I/O via Tokio. Positioned operations run in `spawn_blocking`
  with `try_clone`d handles to keep the runtime free. No Rayon dependency.

* **`mmap`**: Read only memory maps backed by `memmap2`. Returns a lazy mapped
  region in ~5 µs regardless of file size.

* **`uring`** (Linux only): Ring backed positioned I/O with `read_at_batch` for
  submitting multiple reads in a single syscall.

## Performance

All benchmarks use [Criterion.rs](https://bheisler.github.io/criterion.rs/book/)
on Linux (x86_64). Files range from 4 KiB to 64 MiB. Run them yourself:

```bash
cargo bench --all-features
```

### Full File Read (`read_all`)

![read_all benchmark chart](charts/read_all.svg)

| File Size | `std::fs::read` | fastio sync | Speedup | fastio mmap |
|:----------|:----------------|:------------|:--------|:------------|
| 4 KiB     | 6.42 µs         | 2.23 µs     | **2.9x** | 5.39 µs    |
| 64 KiB    | 9.73 µs         | 5.67 µs     | **1.7x** | 5.23 µs    |
| 1 MiB     | 72.6 µs         | 66.8 µs     | **1.1x** | 5.14 µs    |
| 16 MiB    | 1.66 ms         | 1.70 ms     | ~1x      | 5.73 µs    |
| 64 MiB    | 55.3 ms         | 25.3 ms     | **2.2x** | 6.39 µs    |

fastio sync is faster for most file sizes because the internal buffer pool
recycles allocations instead of going through `mmap`/`munmap` on every call. At
16 MiB the two are within measurement noise. The mmap backend returns a lazy
mapping in ~5 µs; actual data transfer happens on first access via page faults.

### Positioned Write (`write_all_at`)

fastio sync matches raw `pwrite` with zero abstraction overhead:

| File Size | `std pwrite` | fastio sync |
|:----------|:-------------|:------------|
| 64 KiB    | 3.72 µs      | 3.74 µs     |
| 1 MiB     | 3.76 µs      | 3.84 µs     |
| 16 MiB    | 3.74 µs      | 4.10 µs     |
| 64 MiB    | 3.82 µs      | 3.76 µs     |

### Buffer Allocation

Read buffers are allocated through an internal `zeropool` pool. The pool
recycles large buffers instead of going through `mmap`/`munmap` on every call,
which is the main reason fastio sync outperforms `std::fs::read` on large files.
The pool is not a public API; it is used automatically by the read-capable
backends.

### io_uring Batch Reads (`read_at_batch`)

![uring batch benchmark chart](charts/uring_batch.svg)

Batched submission amortizes the `io_uring_enter` syscall across multiple reads.
The benefit scales with batch size:

| Batch Size | Sequential `pread` | uring batch | Speedup  |
|:-----------|:-------------------|:------------|:---------|
| n=4        | 3.48 µs            | 3.41 µs     | ~1x      |
| n=16       | 13.8 µs            | 16.9 µs     | 0.82x    |
| n=64       | 73.5 µs            | 47.8 µs     | **1.5x** |
| n=256      | 324 µs             | 243 µs      | **1.3x** |

At small batch sizes (n<=16), io_uring submission overhead exceeds the savings.
The crossover point is around n=32 where batched submission starts to win. For
single positioned reads, `sync::File::read_at` is the fastest option.

### Cursor Traits (`Read` / `Write`)

fastio sync implements `std::io::Read` and `std::io::Write` at the same speed
as `std::fs::File`:

| Operation | File Size | `std::fs` | fastio sync |
|:----------|:----------|:----------|:------------|
| Read      | 1 MiB     | 81.0 µs   | 81.3 µs     |
| Read      | 16 MiB    | 1.17 ms   | 1.16 ms     |
| Read      | 64 MiB    | 5.99 ms   | 6.14 ms     |
| Write     | 1 MiB     | 123 µs    | 125 µs      |
| Write     | 16 MiB    | 1.91 ms   | 1.94 ms     |
| Write     | 64 MiB    | 9.07 ms   | 9.39 µs     |

### Async Read (Tokio)

![async read benchmark chart](charts/async_read.svg)

| File Size | `tokio::fs::read` | fastio tokio | Notes                   |
|:----------|:-------------------|:-------------|:------------------------|
| 4 KiB     | 35.4 µs            | 74.8 µs      | dispatch overhead       |
| 64 KiB    | 38.8 µs            | 83.4 µs      | dispatch overhead       |
| 1 MiB     | 125 µs             | 200 µs       | dispatch overhead       |
| 16 MiB    | 1.46 ms            | 2.22 ms      | dispatch overhead       |
| 64 MiB    | 53.6 ms            | **48.7 ms**  | pool pays off           |

fastio tokio uses `spawn_blocking` with a `try_clone`d file handle per call.
This adds dispatch overhead compared to `tokio::fs` which caches its internal
handle. At 64 MiB the internal pool saves enough allocation cost to offset the
dispatch overhead. For async workloads with mostly small reads,
`tokio::fs::read` will be faster.

### Methodology

All numbers are Criterion medians from 100 sample runs on the same machine.
The `change` lines in Criterion output compare against a previous run and
can be ignored for absolute comparisons. Measurements reflect hot page cache
conditions (files pre read before benchmarking). Real world performance with
cold caches or under memory pressure will differ.

## Installation

Add to your `Cargo.toml`:

```toml
[dependencies]
fastio = "0.3"
```

All backends are enabled by default. To select specific backends:

```toml
[dependencies]
fastio = { version = "0.3", default-features = false, features = ["sync", "tokio"] }
```

### Requirements

* Rust 1.93+ (edition 2024)
* Linux kernel 5.6+ for the `io_uring` backend

## Usage

### Synchronous Read

```rust
use fastio::sync::File;

let file = File::open("data.bin")?;
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

Read-capable backends use an internal `zeropool` pool by default. Mapped reads
return the mmap backend's region directly and are not pooled.

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
