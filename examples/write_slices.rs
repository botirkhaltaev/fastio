//! Non-overlapping batch write example using `WriteSlice` and `WriteSlices`.
//!
//! Works with any write-capable backend. This example uses the sync backend.
//!
//! Run with:
//!
//! ```bash
//! cargo run --example write_slices --features sync
//! ```

#[cfg(feature = "sync")]
fn main() -> std::io::Result<()> {
    use fastio::sync::File;

    let path = std::env::temp_dir().join("fastio_write_slices_example.bin");
    std::fs::write(&path, b"----------")?;

    let file = File::options().read(true).write(true).open(&path)?;

    let slices = [
        fastio::WriteSlice::new(0, b"AB"),
        fastio::WriteSlice::new(8, b"YZ"),
    ];
    file.write_slices_at(fastio::WriteSlices::new(&slices)?)?;

    let result = file.read_all()?;
    println!(
        "after write_slices: {:?}",
        std::str::from_utf8(result.as_ref()).unwrap()
    );

    std::fs::remove_file(&path).ok();
    Ok(())
}

#[cfg(not(feature = "sync"))]
fn main() {
    println!("This example requires the sync feature.");
}
