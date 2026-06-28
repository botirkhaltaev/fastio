//! Linux io_uring batch read example using `fastio::uring::File`.
//!
//! Run with:
//!
//! ```bash
//! cargo run --example uring_batch_read --features io-uring
//! ```

#[cfg(all(target_os = "linux", feature = "io-uring"))]
fn main() -> std::io::Result<()> {
    use fastio::uring::File;

    let path = std::env::temp_dir().join("fastio_uring_example.bin");
    std::fs::write(&path, b"0123456789abcdef")?;

    let file = File::open(&path)?;

    let regions = [(0, 4), (8, 4), (12, 4)];
    let results = file.read_at_batch(&regions)?;

    for (i, buf) in results.iter().enumerate() {
        let slice: &[u8] = buf.as_ref();
        println!("region {}: {:?}", i, std::str::from_utf8(slice).unwrap());
    }

    std::fs::remove_file(&path).ok();
    Ok(())
}

#[cfg(not(all(target_os = "linux", feature = "io-uring")))]
fn main() {
    println!("This example requires Linux and the io-uring feature.");
}
