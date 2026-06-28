//! Criterion benchmark suite for fastio.
//!
//! Compares every fastio backend against `std::fs` and raw `memmap2` baselines
//! across a range of file sizes.  Async benchmarks compare `fastio::tokio`
//! against direct `tokio::fs` usage.
//!
//! Run with: `cargo bench --all-features`

use criterion::{BenchmarkId, Criterion, Throughput, black_box, criterion_group, criterion_main};
use std::io::{Read, Write};
#[cfg(unix)]
use std::os::unix::fs::FileExt;
use std::path::PathBuf;

// ---------------------------------------------------------------------------
// Constants
// ---------------------------------------------------------------------------

const SIZES: &[(u64, &str)] = &[
    (4 * 1024, "4KiB"),
    (64 * 1024, "64KiB"),
    (1024 * 1024, "1MiB"),
    (16 * 1024 * 1024, "16MiB"),
    (64 * 1024 * 1024, "64MiB"),
];

const READ_AT_OFFSET: u64 = 4096;
const READ_AT_LEN: usize = 32 * 1024;

const WRITE_PAYLOAD: usize = 32 * 1024;
const WRITE_SLICES_COUNT: usize = 16;
const WRITE_SLICE_LEN: usize = 4096;

// ---------------------------------------------------------------------------
// Test fixture: creates temp files of each size once per benchmark run
// ---------------------------------------------------------------------------

struct Fixture {
    dir: tempfile::TempDir,
}

impl Fixture {
    fn new() -> Self {
        Self {
            dir: tempfile::TempDir::new().expect("failed to create temp dir"),
        }
    }

    fn create_file(&self, name: &str, size: u64) -> PathBuf {
        let path = self.dir.path().join(name);
        let data: Vec<u8> = (0u8..=255).cycle().take(size as usize).collect();
        std::fs::write(&path, &data).expect("failed to write fixture file");
        path
    }

    fn create_write_target(&self, name: &str, size: u64) -> PathBuf {
        let path = self.dir.path().join(name);
        std::fs::write(&path, vec![0u8; size as usize]).expect("failed to write target");
        path
    }
}

// ---------------------------------------------------------------------------
// io_uring availability guard (matches the crate's test pattern)
// ---------------------------------------------------------------------------

#[cfg(all(target_os = "linux", feature = "io-uring"))]
fn uring_available() -> bool {
    let dir = tempfile::TempDir::new().unwrap();
    let path = dir.path().join("probe.bin");
    std::fs::write(&path, b"x").unwrap();
    fastio::uring::File::open(&path)
        .and_then(|f| f.read_at(0, 1))
        .is_ok()
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

fn tokio_rt() -> tokio::runtime::Runtime {
    tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
        .expect("failed to build tokio runtime")
}

fn make_write_payload() -> Vec<u8> {
    vec![0xABu8; WRITE_PAYLOAD]
}

fn make_write_slices_data() -> Vec<Vec<u8>> {
    (0..WRITE_SLICES_COUNT)
        .map(|_| vec![0xCDu8; WRITE_SLICE_LEN])
        .collect()
}

// ---------------------------------------------------------------------------
// 1. read_all — full file read
// ---------------------------------------------------------------------------

fn bench_read_all(c: &mut Criterion) {
    let fixture = Fixture::new();

    for &(size, label) in SIZES {
        let path = fixture.create_file(&format!("read_all_{label}.bin"), size);
        let mut group = c.benchmark_group(format!("read_all/{label}"));
        group.throughput(Throughput::Bytes(size));

        // --- std::fs::read ---
        group.bench_function(BenchmarkId::new("std_fs_read", label), |b| {
            b.iter(|| {
                let bytes = std::fs::read(&path).unwrap();
                black_box(bytes.len());
            });
        });

        // --- std::fs::File + Read::read_to_end ---
        group.bench_function(BenchmarkId::new("std_fs_read_to_end", label), |b| {
            b.iter(|| {
                let mut f = std::fs::File::open(&path).unwrap();
                let mut buf = Vec::with_capacity(size as usize);
                f.read_to_end(&mut buf).unwrap();
                black_box(buf.len());
            });
        });

        // --- fastio::sync::File::read_all ---
        group.bench_function(BenchmarkId::new("fastio_sync", label), |b| {
            let file = fastio::sync::File::open(&path).unwrap();
            b.iter(|| {
                let bytes = file.read_all().unwrap();
                black_box(bytes.len());
            });
        });

        // --- fastio::mmap::File::map ---
        group.bench_function(BenchmarkId::new("fastio_mmap", label), |b| {
            let file = fastio::mmap::File::open(&path).unwrap();
            b.iter(|| {
                let region = file.map().unwrap();
                black_box(region.len());
            });
        });

        // --- raw memmap2 (no fastio) ---
        group.bench_function(BenchmarkId::new("raw_memmap2", label), |b| {
            let file = std::fs::File::open(&path).unwrap();
            b.iter(|| {
                // SAFETY: file is open read-only and lives for the duration.
                let mmap = unsafe { memmap2::MmapOptions::new().map(&file).unwrap() };
                black_box(mmap.len());
            });
        });

        // --- fastio::uring::File::read_all ---
        #[cfg(all(target_os = "linux", feature = "io-uring"))]
        if uring_available() {
            group.bench_function(BenchmarkId::new("fastio_uring", label), |b| {
                let file = fastio::uring::File::open(&path).unwrap();
                b.iter(|| {
                    let bytes = file.read_all().unwrap();
                    black_box(bytes.len());
                });
            });
        }

        // --- fastio::tokio::File::read_all ---
        group.bench_function(BenchmarkId::new("fastio_tokio", label), |b| {
            let rt = tokio_rt();
            b.iter(|| {
                rt.block_on(async {
                    let file = fastio::tokio::File::open(&path).await.unwrap();
                    let bytes = file.read_all().await.unwrap();
                    black_box(bytes.len());
                });
            });
        });

        // --- raw tokio::fs::read ---
        group.bench_function(BenchmarkId::new("raw_tokio_fs_read", label), |b| {
            let rt = tokio_rt();
            b.iter(|| {
                rt.block_on(async {
                    let bytes = tokio::fs::read(&path).await.unwrap();
                    black_box(bytes.len());
                });
            });
        });

        group.finish();
    }
}

// ---------------------------------------------------------------------------
// 2. read_at — positioned reads
// ---------------------------------------------------------------------------

fn bench_read_at(c: &mut Criterion) {
    let fixture = Fixture::new();

    // Only use sizes large enough for the positioned read
    let min_size = READ_AT_OFFSET + READ_AT_LEN as u64;
    let applicable: Vec<_> = SIZES.iter().filter(|(s, _)| *s >= min_size).collect();

    for &&(size, label) in &applicable {
        let path = fixture.create_file(&format!("read_at_{label}.bin"), size);
        let mut group = c.benchmark_group(format!("read_at/{label}"));
        group.throughput(Throughput::Bytes(READ_AT_LEN as u64));

        // --- std::os::unix::fs::FileExt::read_exact_at ---
        #[cfg(unix)]
        group.bench_function(BenchmarkId::new("std_pread", label), |b| {
            let file = std::fs::File::open(&path).unwrap();
            let mut buf = vec![0u8; READ_AT_LEN];
            b.iter(|| {
                file.read_exact_at(&mut buf, READ_AT_OFFSET).unwrap();
                black_box(buf.len());
            });
        });

        // --- fastio::sync::File::read_at ---
        group.bench_function(BenchmarkId::new("fastio_sync", label), |b| {
            let file = fastio::sync::File::open(&path).unwrap();
            b.iter(|| {
                let bytes = file.read_at(READ_AT_OFFSET, READ_AT_LEN).unwrap();
                black_box(bytes.len());
            });
        });

        // --- fastio::mmap::File::map_range ---
        group.bench_function(BenchmarkId::new("fastio_mmap_range", label), |b| {
            let file = fastio::mmap::File::open(&path).unwrap();
            b.iter(|| {
                let region = file.map_range(READ_AT_OFFSET, READ_AT_LEN).unwrap();
                black_box(region.len());
            });
        });

        // --- raw memmap2 subslice ---
        group.bench_function(BenchmarkId::new("raw_memmap2_slice", label), |b| {
            let file = std::fs::File::open(&path).unwrap();
            // SAFETY: file is open read-only and lives for the duration.
            let mmap = unsafe { memmap2::MmapOptions::new().map(&file).unwrap() };
            b.iter(|| {
                let start = READ_AT_OFFSET as usize;
                let end = start + READ_AT_LEN;
                let slice = &mmap[start..end];
                black_box(slice.len());
            });
        });

        // --- fastio::uring::File::read_at ---
        #[cfg(all(target_os = "linux", feature = "io-uring"))]
        if uring_available() {
            group.bench_function(BenchmarkId::new("fastio_uring", label), |b| {
                let file = fastio::uring::File::open(&path).unwrap();
                b.iter(|| {
                    let bytes = file.read_at(READ_AT_OFFSET, READ_AT_LEN).unwrap();
                    black_box(bytes.len());
                });
            });
        }

        // --- fastio::tokio::File::read_at ---
        group.bench_function(BenchmarkId::new("fastio_tokio", label), |b| {
            let rt = tokio_rt();
            b.iter(|| {
                rt.block_on(async {
                    let file = fastio::tokio::File::open(&path).await.unwrap();
                    let bytes = file.read_at(READ_AT_OFFSET, READ_AT_LEN).await.unwrap();
                    black_box(bytes.len());
                });
            });
        });

        group.finish();
    }
}

// ---------------------------------------------------------------------------
// 3. write_all_at — positioned writes
// ---------------------------------------------------------------------------

fn bench_write_all_at(c: &mut Criterion) {
    let fixture = Fixture::new();
    let payload = make_write_payload();

    for &(size, label) in SIZES {
        if (size as usize) < WRITE_PAYLOAD {
            continue;
        }
        let path = fixture.create_write_target(&format!("write_{label}.bin"), size);
        let mut group = c.benchmark_group(format!("write_all_at/{label}"));
        group.throughput(Throughput::Bytes(WRITE_PAYLOAD as u64));

        // --- std::os::unix::fs::FileExt::write_all_at ---
        #[cfg(unix)]
        group.bench_function(BenchmarkId::new("std_pwrite", label), |b| {
            let file = std::fs::OpenOptions::new().write(true).open(&path).unwrap();
            b.iter(|| {
                file.write_all_at(&payload, 0).unwrap();
                black_box(());
            });
        });

        // --- fastio::sync::File::write_all_at ---
        group.bench_function(BenchmarkId::new("fastio_sync", label), |b| {
            let file = fastio::sync::File::options()
                .write(true)
                .open(&path)
                .unwrap();
            b.iter(|| {
                file.write_all_at(0, &payload).unwrap();
                black_box(());
            });
        });

        // --- fastio::uring::File::write_all_at ---
        #[cfg(all(target_os = "linux", feature = "io-uring"))]
        if uring_available() {
            group.bench_function(BenchmarkId::new("fastio_uring", label), |b| {
                let file = fastio::uring::File::options()
                    .write(true)
                    .open(&path)
                    .unwrap();
                b.iter(|| {
                    file.write_all_at(0, &payload).unwrap();
                    black_box(());
                });
            });
        }

        // --- fastio::tokio::File::write_all_at ---
        group.bench_function(BenchmarkId::new("fastio_tokio", label), |b| {
            let rt = tokio_rt();
            b.iter(|| {
                rt.block_on(async {
                    let file = fastio::tokio::File::options()
                        .write(true)
                        .open(&path)
                        .await
                        .unwrap();
                    file.write_all_at(0, &payload).await.unwrap();
                    black_box(());
                });
            });
        });

        group.finish();
    }
}

// ---------------------------------------------------------------------------
// 4. write_slices_at — batch positioned writes
// ---------------------------------------------------------------------------

fn bench_write_slices(c: &mut Criterion) {
    let fixture = Fixture::new();
    let slices_data = make_write_slices_data();
    let total_bytes: u64 = (WRITE_SLICES_COUNT * WRITE_SLICE_LEN) as u64;

    // Need at least enough space for non-overlapping slices
    let min_file_size = total_bytes * 2;

    for &(size, label) in SIZES {
        if size < min_file_size {
            continue;
        }
        let path = fixture.create_write_target(&format!("write_slices_{label}.bin"), size);
        let mut group = c.benchmark_group(format!("write_slices/{label}"));
        group.throughput(Throughput::Bytes(total_bytes));

        let spacing = (size as usize) / WRITE_SLICES_COUNT;

        // --- std manual pwrite loop ---
        #[cfg(unix)]
        group.bench_function(BenchmarkId::new("std_pwrite_loop", label), |b| {
            let file = std::fs::OpenOptions::new().write(true).open(&path).unwrap();
            b.iter(|| {
                for (i, data) in slices_data.iter().enumerate() {
                    let offset = (i * spacing) as u64;
                    file.write_all_at(data, offset).unwrap();
                }
                black_box(());
            });
        });

        // --- fastio::sync::File::write_slices_at ---
        group.bench_function(BenchmarkId::new("fastio_sync", label), |b| {
            let file = fastio::sync::File::options()
                .write(true)
                .open(&path)
                .unwrap();
            b.iter(|| {
                let slices: Vec<fastio::write::WriteSlice<'_>> = slices_data
                    .iter()
                    .enumerate()
                    .map(|(i, data)| fastio::write::WriteSlice::new((i * spacing) as u64, data))
                    .collect();
                let writes = fastio::WriteSlices::new(&slices).unwrap();
                file.write_slices_at(writes).unwrap();
                black_box(());
            });
        });

        // --- fastio::uring::File::write_slices_at ---
        #[cfg(all(target_os = "linux", feature = "io-uring"))]
        if uring_available() {
            group.bench_function(BenchmarkId::new("fastio_uring", label), |b| {
                let file = fastio::uring::File::options()
                    .write(true)
                    .open(&path)
                    .unwrap();
                b.iter(|| {
                    let slices: Vec<fastio::write::WriteSlice<'_>> = slices_data
                        .iter()
                        .enumerate()
                        .map(|(i, data)| fastio::write::WriteSlice::new((i * spacing) as u64, data))
                        .collect();
                    let writes = fastio::WriteSlices::new(&slices).unwrap();
                    file.write_slices_at(writes).unwrap();
                    black_box(());
                });
            });
        }

        // --- fastio::tokio::File::write_slices_at ---
        group.bench_function(BenchmarkId::new("fastio_tokio", label), |b| {
            let rt = tokio_rt();
            b.iter(|| {
                rt.block_on(async {
                    let file = fastio::tokio::File::options()
                        .write(true)
                        .open(&path)
                        .await
                        .unwrap();
                    let slices: Vec<fastio::write::WriteSlice<'_>> = slices_data
                        .iter()
                        .enumerate()
                        .map(|(i, data)| fastio::write::WriteSlice::new((i * spacing) as u64, data))
                        .collect();
                    let writes = fastio::WriteSlices::new(&slices).unwrap();
                    file.write_slices_at(writes).await.unwrap();
                    black_box(());
                });
            });
        });

        group.finish();
    }
}

// ---------------------------------------------------------------------------
// 5. mmap — map vs map_range vs raw memmap2
// ---------------------------------------------------------------------------

fn bench_mmap(c: &mut Criterion) {
    let fixture = Fixture::new();

    for &(size, label) in SIZES {
        let path = fixture.create_file(&format!("mmap_{label}.bin"), size);
        let mut group = c.benchmark_group(format!("mmap/{label}"));
        group.throughput(Throughput::Bytes(size));

        // --- fastio::mmap::File::map (full file) ---
        group.bench_function(BenchmarkId::new("fastio_map_full", label), |b| {
            let file = fastio::mmap::File::open(&path).unwrap();
            b.iter(|| {
                let region = file.map().unwrap();
                black_box(region.len());
            });
        });

        // --- raw memmap2 (full file) ---
        group.bench_function(BenchmarkId::new("raw_memmap2_full", label), |b| {
            let file = std::fs::File::open(&path).unwrap();
            b.iter(|| {
                // SAFETY: file is open read-only and lives for the duration.
                let mmap = unsafe { memmap2::MmapOptions::new().map(&file).unwrap() };
                black_box(mmap.len());
            });
        });

        // --- fastio::mmap::File::map_range (half of file) ---
        if size >= 8192 {
            let half = (size / 2) as usize;
            group.bench_function(BenchmarkId::new("fastio_map_range_half", label), |b| {
                let file = fastio::mmap::File::open(&path).unwrap();
                b.iter(|| {
                    let region = file.map_range(0, half).unwrap();
                    black_box(region.len());
                });
            });

            // --- raw memmap2 range (half of file) ---
            group.bench_function(BenchmarkId::new("raw_memmap2_range_half", label), |b| {
                let file = std::fs::File::open(&path).unwrap();
                b.iter(|| {
                    // SAFETY: file is open read-only, range is within file size.
                    let mmap = unsafe {
                        memmap2::MmapOptions::new()
                            .offset(0)
                            .len(half)
                            .map(&file)
                            .unwrap()
                    };
                    black_box(mmap.len());
                });
            });
        }

        group.finish();
    }
}

// ---------------------------------------------------------------------------
// 7. async_read — fastio::tokio vs raw tokio::fs
// ---------------------------------------------------------------------------

fn bench_async_read(c: &mut Criterion) {
    let fixture = Fixture::new();

    for &(size, label) in SIZES {
        let path = fixture.create_file(&format!("async_read_{label}.bin"), size);
        let mut group = c.benchmark_group(format!("async_read/{label}"));
        group.throughput(Throughput::Bytes(size));

        // --- raw tokio::fs::read ---
        group.bench_function(BenchmarkId::new("raw_tokio_fs_read", label), |b| {
            let rt = tokio_rt();
            let p = path.clone();
            b.iter(|| {
                rt.block_on(async {
                    let bytes = tokio::fs::read(&p).await.unwrap();
                    black_box(bytes.len());
                });
            });
        });

        // --- raw tokio::fs::File + read_to_end ---
        group.bench_function(BenchmarkId::new("raw_tokio_read_to_end", label), |b| {
            let rt = tokio_rt();
            let p = path.clone();
            b.iter(|| {
                rt.block_on(async {
                    use tokio::io::AsyncReadExt;
                    let mut file = tokio::fs::File::open(&p).await.unwrap();
                    let mut buf = Vec::with_capacity(size as usize);
                    file.read_to_end(&mut buf).await.unwrap();
                    black_box(buf.len());
                });
            });
        });

        // --- fastio::tokio::File::read_all ---
        group.bench_function(BenchmarkId::new("fastio_tokio", label), |b| {
            let rt = tokio_rt();
            let p = path.clone();
            b.iter(|| {
                rt.block_on(async {
                    let file = fastio::tokio::File::open(&p).await.unwrap();
                    let bytes = file.read_all().await.unwrap();
                    black_box(bytes.len());
                });
            });
        });

        group.finish();
    }
}

// ---------------------------------------------------------------------------
// 8. async_write — fastio::tokio vs raw tokio::fs
// ---------------------------------------------------------------------------

fn bench_async_write(c: &mut Criterion) {
    let fixture = Fixture::new();
    let payload = make_write_payload();

    for &(size, label) in SIZES {
        if (size as usize) < WRITE_PAYLOAD {
            continue;
        }
        let path = fixture.create_write_target(&format!("async_write_{label}.bin"), size);
        let mut group = c.benchmark_group(format!("async_write/{label}"));
        group.throughput(Throughput::Bytes(WRITE_PAYLOAD as u64));

        // --- raw tokio::fs::File + AsyncWriteExt ---
        group.bench_function(BenchmarkId::new("raw_tokio_write_at", label), |b| {
            let rt = tokio_rt();
            let p = path.clone();
            let data = payload.clone();
            b.iter(|| {
                rt.block_on(async {
                    use tokio::io::{AsyncSeekExt, AsyncWriteExt};
                    let mut file = tokio::fs::OpenOptions::new()
                        .write(true)
                        .open(&p)
                        .await
                        .unwrap();
                    file.seek(std::io::SeekFrom::Start(0)).await.unwrap();
                    file.write_all(&data).await.unwrap();
                    black_box(());
                });
            });
        });

        // --- fastio::tokio::File::write_all_at ---
        group.bench_function(BenchmarkId::new("fastio_tokio", label), |b| {
            let rt = tokio_rt();
            let p = path.clone();
            let data = payload.clone();
            b.iter(|| {
                rt.block_on(async {
                    let file = fastio::tokio::File::options()
                        .write(true)
                        .open(&p)
                        .await
                        .unwrap();
                    file.write_all_at(0, &data).await.unwrap();
                    black_box(());
                });
            });
        });

        group.finish();
    }
}

// ---------------------------------------------------------------------------
// 9. cursor_read — std::io::Read trait throughput comparison
// ---------------------------------------------------------------------------

fn bench_cursor_read(c: &mut Criterion) {
    let fixture = Fixture::new();

    for &(size, label) in SIZES {
        let path = fixture.create_file(&format!("cursor_read_{label}.bin"), size);
        let mut group = c.benchmark_group(format!("cursor_read/{label}"));
        group.throughput(Throughput::Bytes(size));

        let chunk = 64 * 1024;

        // --- std::fs::File via Read trait ---
        group.bench_function(BenchmarkId::new("std_fs_read_trait", label), |b| {
            b.iter(|| {
                let mut f = std::fs::File::open(&path).unwrap();
                let mut buf = vec![0u8; chunk];
                let mut total = 0usize;
                loop {
                    let n = f.read(&mut buf).unwrap();
                    if n == 0 {
                        break;
                    }
                    total += n;
                }
                black_box(total);
            });
        });

        // --- fastio::sync::File via Read trait ---
        group.bench_function(BenchmarkId::new("fastio_sync_read_trait", label), |b| {
            b.iter(|| {
                let mut f = fastio::sync::File::open(&path).unwrap();
                let mut buf = vec![0u8; chunk];
                let mut total = 0usize;
                loop {
                    let n = f.read(&mut buf).unwrap();
                    if n == 0 {
                        break;
                    }
                    total += n;
                }
                black_box(total);
            });
        });

        // --- fastio::uring::File via Read trait ---
        #[cfg(all(target_os = "linux", feature = "io-uring"))]
        if uring_available() {
            group.bench_function(BenchmarkId::new("fastio_uring_read_trait", label), |b| {
                b.iter(|| {
                    let mut f = fastio::uring::File::open(&path).unwrap();
                    let mut buf = vec![0u8; chunk];
                    let mut total = 0usize;
                    loop {
                        let n = f.read(&mut buf).unwrap();
                        if n == 0 {
                            break;
                        }
                        total += n;
                    }
                    black_box(total);
                });
            });
        }

        group.finish();
    }
}

// ---------------------------------------------------------------------------
// 10. cursor_write — std::io::Write trait throughput comparison
// ---------------------------------------------------------------------------

fn bench_cursor_write(c: &mut Criterion) {
    let fixture = Fixture::new();
    let chunk = vec![0xFFu8; 64 * 1024];

    for &(size, label) in SIZES {
        let path = fixture.create_write_target(&format!("cursor_write_{label}.bin"), size);
        let mut group = c.benchmark_group(format!("cursor_write/{label}"));
        group.throughput(Throughput::Bytes(size));

        // --- std::fs::File via Write trait ---
        group.bench_function(BenchmarkId::new("std_fs_write_trait", label), |b| {
            b.iter(|| {
                let mut f = std::fs::OpenOptions::new().write(true).open(&path).unwrap();
                let mut remaining = size as usize;
                while remaining > 0 {
                    let n = remaining.min(chunk.len());
                    f.write_all(&chunk[..n]).unwrap();
                    remaining -= n;
                }
                black_box(());
            });
        });

        // --- fastio::sync::File via Write trait ---
        group.bench_function(BenchmarkId::new("fastio_sync_write_trait", label), |b| {
            b.iter(|| {
                let mut f = fastio::sync::File::options()
                    .write(true)
                    .open(&path)
                    .unwrap();
                let mut remaining = size as usize;
                while remaining > 0 {
                    let n = remaining.min(chunk.len());
                    f.write_all(&chunk[..n]).unwrap();
                    remaining -= n;
                }
                black_box(());
            });
        });

        // --- fastio::uring::File via Write trait ---
        #[cfg(all(target_os = "linux", feature = "io-uring"))]
        if uring_available() {
            group.bench_function(BenchmarkId::new("fastio_uring_write_trait", label), |b| {
                b.iter(|| {
                    let mut f = fastio::uring::File::options()
                        .write(true)
                        .open(&path)
                        .unwrap();
                    let mut remaining = size as usize;
                    while remaining > 0 {
                        let n = remaining.min(chunk.len());
                        f.write_all(&chunk[..n]).unwrap();
                        remaining -= n;
                    }
                    black_box(());
                });
            });
        }

        group.finish();
    }
}

// ---------------------------------------------------------------------------
// 11. read_at_batch — batched positioned reads (io_uring, unix only)
// ---------------------------------------------------------------------------

#[cfg(unix)]
const BATCH_SIZES: &[usize] = &[4, 16, 64, 256];
#[cfg(unix)]
const BATCH_READ_LEN: usize = 4096;

#[cfg(unix)]
fn bench_read_at_batch(c: &mut Criterion) {
    let fixture = Fixture::new();
    // Use a 16 MiB file so all batch reads fit
    let file_size = 16 * 1024 * 1024u64;
    let path = fixture.create_file("batch_read.bin", file_size);

    for &batch_size in BATCH_SIZES {
        let total_bytes = batch_size * BATCH_READ_LEN;
        let regions: Vec<(u64, usize)> = (0..batch_size)
            .map(|i| (i as u64 * BATCH_READ_LEN as u64, BATCH_READ_LEN))
            .collect();
        let mut group = c.benchmark_group(format!("read_at_batch/n={batch_size}"));
        group.throughput(Throughput::Bytes(total_bytes as u64));

        // --- baseline: N sequential pread calls ---
        #[cfg(unix)]
        group.bench_function(BenchmarkId::new("sequential_pread", batch_size), |b| {
            let file = std::fs::File::open(&path).unwrap();
            let mut buf = vec![0u8; BATCH_READ_LEN];
            b.iter(|| {
                for &(offset, _len) in &regions {
                    file.read_exact_at(&mut buf, offset).unwrap();
                }
                black_box(buf.len());
            });
        });

        // --- fastio::uring::File::read_at_batch ---
        #[cfg(all(target_os = "linux", feature = "io-uring"))]
        if uring_available() {
            group.bench_function(BenchmarkId::new("fastio_uring_batch", batch_size), |b| {
                let file = fastio::uring::File::open(&path).unwrap();
                b.iter(|| {
                    let results = file.read_at_batch(&regions).unwrap();
                    black_box(results.len());
                });
            });
        }

        // --- fastio::uring single reads for comparison ---
        #[cfg(all(target_os = "linux", feature = "io-uring"))]
        if uring_available() {
            group.bench_function(
                BenchmarkId::new("fastio_uring_sequential", batch_size),
                |b| {
                    let file = fastio::uring::File::open(&path).unwrap();
                    b.iter(|| {
                        for &(offset, len) in &regions {
                            let bytes = file.read_at(offset, len).unwrap();
                            black_box(bytes.len());
                        }
                    });
                },
            );
        }

        group.finish();
    }
}

// ---------------------------------------------------------------------------
// Criterion harness
// ---------------------------------------------------------------------------

#[cfg(unix)]
criterion_group!(
    benches,
    bench_read_all,
    bench_read_at,
    bench_write_all_at,
    bench_write_slices,
    bench_mmap,
    bench_async_read,
    bench_async_write,
    bench_cursor_read,
    bench_cursor_write,
    bench_read_at_batch,
);

#[cfg(not(unix))]
criterion_group!(
    benches,
    bench_read_all,
    bench_read_at,
    bench_write_all_at,
    bench_write_slices,
    bench_mmap,
    bench_async_read,
    bench_async_write,
    bench_cursor_read,
    bench_cursor_write,
);

criterion_main!(benches);
