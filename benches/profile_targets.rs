// Standalone profiling binary — run under `perf record` to get clean call stacks
// for the specific fastio operations that are slower than baselines.
//
// Usage:
//   cargo build --release --all-features --bench profile_targets
//   perf record -g --call-graph dwarf -- target/release/deps/profile_targets-* <target>
//
// Targets: read_at_fastio, read_at_std, async_read_fastio,
//          async_read_raw, uring_read_at, uring_read_all

use std::hint::black_box;
use std::io::Write;

const FILE_SIZE: usize = 64 * 1024; // 64 KiB
const READ_AT_LEN: usize = 32 * 1024; // 32 KiB
const READ_AT_OFFSET: u64 = 0;
const ITERATIONS: usize = 100_000;

fn create_test_file(dir: &std::path::Path, name: &str, size: usize) -> std::path::PathBuf {
    let path = dir.join(name);
    let data: Vec<u8> = (0..size).map(|i| (i % 251) as u8).collect();
    std::fs::write(&path, &data).unwrap();
    path
}

fn profile_read_at_fastio(path: &std::path::Path) {
    let file = fastio::sync::File::open(path).unwrap();
    for _ in 0..ITERATIONS {
        let buf = file.read_at(READ_AT_OFFSET, READ_AT_LEN).unwrap();
        black_box(buf.len());
    }
}

#[cfg(unix)]
fn profile_read_at_std(path: &std::path::Path) {
    use std::os::unix::fs::FileExt;
    let file = std::fs::File::open(path).unwrap();
    let mut buf = vec![0u8; READ_AT_LEN];
    for _ in 0..ITERATIONS {
        file.read_exact_at(&mut buf, READ_AT_OFFSET).unwrap();
        black_box(buf.len());
    }
}

fn profile_async_read_fastio(path: &std::path::Path) {
    let rt = tokio::runtime::Runtime::new().unwrap();
    let path = path.to_path_buf();
    rt.block_on(async {
        let file = fastio::tokio::File::open(&path).await.unwrap();
        for _ in 0..ITERATIONS {
            let buf = file.read_at(READ_AT_OFFSET, READ_AT_LEN).await.unwrap();
            black_box(buf.len());
        }
    });
}

fn profile_async_read_raw(path: &std::path::Path) {
    let rt = tokio::runtime::Runtime::new().unwrap();
    let path = path.to_path_buf();
    rt.block_on(async {
        let data = tokio::fs::read(&path).await.unwrap();
        for _ in 0..ITERATIONS {
            black_box(data.len());
            // Re-read to match overhead
            let d = tokio::fs::read(&path).await.unwrap();
            black_box(d.len());
        }
    });
}

#[cfg(all(target_os = "linux", feature = "io-uring"))]
fn profile_uring_read_at(path: &std::path::Path) {
    let file = fastio::uring::File::open(path).unwrap();
    for _ in 0..ITERATIONS {
        let buf = file.read_at(READ_AT_OFFSET, READ_AT_LEN).unwrap();
        black_box(buf.len());
    }
}

#[cfg(all(target_os = "linux", feature = "io-uring"))]
fn profile_uring_read_all(path: &std::path::Path) {
    let file = fastio::uring::File::open(path).unwrap();
    for _ in 0..ITERATIONS {
        let buf = file.read_all().unwrap();
        black_box(buf.len());
    }
}

fn main() {
    let target = match std::env::args().nth(1) {
        Some(t) => t,
        None => {
            // No argument: exit gracefully so `cargo test --bench` succeeds.
            eprintln!(
                "Usage: profile_targets <target>\n\
                 Targets: read_at_fastio, read_at_std, async_read_fastio,\n\
                 async_read_raw, uring_read_at, uring_read_all"
            );
            return;
        }
    };

    let dir = tempfile::TempDir::new().unwrap();
    let path = create_test_file(dir.path(), "test.bin", FILE_SIZE);

    // Warm up page cache
    let _ = std::fs::read(&path).unwrap();

    eprintln!("Profiling {target} with {ITERATIONS} iterations...");

    match target.as_str() {
        "read_at_fastio" => profile_read_at_fastio(&path),
        #[cfg(unix)]
        "read_at_std" => profile_read_at_std(&path),
        "async_read_fastio" => profile_async_read_fastio(&path),
        "async_read_raw" => profile_async_read_raw(&path),
        #[cfg(all(target_os = "linux", feature = "io-uring"))]
        "uring_read_at" => profile_uring_read_at(&path),
        #[cfg(all(target_os = "linux", feature = "io-uring"))]
        "uring_read_all" => profile_uring_read_all(&path),
        other => {
            eprintln!("Unknown target: {other}");
            std::process::exit(1);
        }
    }

    // Force flush
    std::io::stderr().flush().unwrap();
    eprintln!("Done.");
}
