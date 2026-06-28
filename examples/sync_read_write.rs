//! Synchronous read/write example using `fastio::sync::File`.
//!
//! Run with:
//!
//! ```bash
//! cargo run --example sync_read_write --features sync
//! ```

#[cfg(feature = "sync")]
fn main() -> std::io::Result<()> {
    use fastio::sync::File;
    let path = std::env::temp_dir().join("fastio_sync_example.bin");

    // Write some data at offset 0.
    let file = File::create(&path)?;
    file.write_all_at(0, b"hello world")?;

    // Positioned read at offset 6.
    let file = File::open(&path)?;
    let bytes = file.read_at(6, 5)?;
    println!(
        "read_at(6, 5) = {:?}",
        std::str::from_utf8(bytes.as_ref()).unwrap()
    );

    // Read the whole file.
    let all = file.read_all()?;
    println!(
        "read_all = {:?}",
        std::str::from_utf8(all.as_ref()).unwrap()
    );

    // Batch write with non-overlapping slices.
    let file = File::options().read(true).write(true).open(&path)?;
    let slices = [
        fastio::WriteSlice::new(0, b"HELLO"),
        fastio::WriteSlice::new(6, b"WORLD"),
    ];
    file.write_slices_at(fastio::WriteSlices::new(&slices)?)?;

    let result = file.read_all()?;
    println!(
        "after batch write = {:?}",
        std::str::from_utf8(result.as_ref()).unwrap()
    );

    std::fs::remove_file(&path).ok();
    Ok(())
}

#[cfg(not(feature = "sync"))]
fn main() {
    println!("This example requires the sync feature.");
}
