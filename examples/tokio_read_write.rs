//! Async read/write example using `fastio::tokio::File`.
//!
//! Run with:
//!
//! ```bash
//! cargo run --example tokio_read_write --features tokio
//! ```

#[cfg(feature = "tokio")]
#[tokio::main]
async fn main() -> std::io::Result<()> {
    let path = std::env::temp_dir().join("fastio_tokio_example.bin");

    let file = fastio::tokio::File::create(&path).await?;
    file.write_all_at(0, b"async data").await?;

    let file = fastio::tokio::File::open(&path).await?;
    let bytes = file.read_at(6, 4).await?;
    let slice: &[u8] = bytes.as_ref();
    println!("read_at(6, 4) = {:?}", std::str::from_utf8(slice).unwrap());

    let all = file.read_all().await?;
    let all_slice: &[u8] = all.as_ref();
    println!("read_all = {:?}", std::str::from_utf8(all_slice).unwrap());

    std::fs::remove_file(&path).ok();
    Ok(())
}

#[cfg(not(feature = "tokio"))]
fn main() {
    println!("This example requires the tokio feature.");
}
