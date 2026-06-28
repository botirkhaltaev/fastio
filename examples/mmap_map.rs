//! Memory-mapped read example using `fastio::mmap::File`.
//!
//! Run with:
//!
//! ```bash
//! cargo run --example mmap_map --features mmap
//! ```

#[cfg(feature = "mmap")]
fn main() -> std::io::Result<()> {
    use fastio::mmap::File;

    let path = std::env::temp_dir().join("fastio_mmap_example.bin");
    std::fs::write(&path, b"full file contents here")?;

    let file = File::open(&path)?;

    // Map the entire file.
    let full = file.map()?;
    println!(
        "map() = {:?}",
        std::str::from_utf8(full.as_slice()).unwrap()
    );

    // Map a sub-range.
    let range = file.map_range(5, 4)?;
    println!(
        "map_range(5, 4) = {:?}",
        std::str::from_utf8(range.as_slice()).unwrap()
    );

    std::fs::remove_file(&path).ok();
    Ok(())
}

#[cfg(not(feature = "mmap"))]
fn main() {
    println!("This example requires the mmap feature.");
}
