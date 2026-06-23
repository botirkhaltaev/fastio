//! Tokio async file I/O.
//!
//! This module mirrors `tokio::fs` where behavior matches, while adding async
//! positioned I/O methods for large-file workloads.

use std::fs::{Metadata, Permissions};
use std::io;
use std::path::Path;

use crate::{IoResult, WriteSlices, buffer::OwnedBytes};

#[cfg(not(any(target_os = "linux", target_os = "macos", target_os = "windows")))]
compile_error!("fastio tokio supports Linux, macOS, and Windows only");

/// A Tokio-backed file handle.
#[derive(Debug)]
pub struct File {
    inner: std::fs::File,
}

impl File {
    /// Opens a file in read-only mode.
    pub async fn open<P: AsRef<Path>>(path: P) -> io::Result<Self> {
        OpenOptions::new().read(true).open(path).await
    }

    /// Opens a file in write-only mode, truncating it if it exists.
    pub async fn create<P: AsRef<Path>>(path: P) -> io::Result<Self> {
        OpenOptions::new()
            .write(true)
            .create(true)
            .truncate(true)
            .open(path)
            .await
    }

    /// Opens a file in write-only mode, failing if it already exists.
    pub async fn create_new<P: AsRef<Path>>(path: P) -> io::Result<Self> {
        OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(path)
            .await
    }

    /// Returns a new options object for opening a file.
    #[must_use]
    pub fn options() -> OpenOptions {
        OpenOptions::new()
    }

    /// Creates a new `File` instance sharing the same underlying handle.
    pub async fn try_clone(&self) -> io::Result<Self> {
        let file = self.inner.try_clone()?;
        Ok(Self { inner: file })
    }

    /// Queries metadata about the underlying file.
    pub async fn metadata(&self) -> io::Result<Metadata> {
        self.inner.metadata()
    }

    /// Truncates or extends the underlying file.
    pub async fn set_len(&self, size: u64) -> io::Result<()> {
        let file = self.inner.try_clone()?;
        ::tokio::task::spawn_blocking(move || file.set_len(size))
            .await
            .map_err(io::Error::other)?
    }

    /// Attempts to sync all OS-internal file content and metadata to disk.
    pub async fn sync_all(&self) -> io::Result<()> {
        let file = self.inner.try_clone()?;
        ::tokio::task::spawn_blocking(move || file.sync_all())
            .await
            .map_err(io::Error::other)?
    }

    /// Attempts to sync file content to disk.
    pub async fn sync_data(&self) -> io::Result<()> {
        let file = self.inner.try_clone()?;
        ::tokio::task::spawn_blocking(move || file.sync_data())
            .await
            .map_err(io::Error::other)?
    }

    /// Changes permissions on the underlying file.
    pub async fn set_permissions(&self, perm: Permissions) -> io::Result<()> {
        let file = self.inner.try_clone()?;
        ::tokio::task::spawn_blocking(move || file.set_permissions(perm))
            .await
            .map_err(io::Error::other)?
    }

    /// Reads the whole file into memory from offset 0.
    pub async fn read_all(&self) -> io::Result<OwnedBytes> {
        let file = self.inner.try_clone()?;
        ::tokio::task::spawn_blocking(move || {
            let len = usize::try_from(file.metadata()?.len())
                .map_err(|_| io::Error::other("file too large"))?;
            if len == 0 {
                return Ok(OwnedBytes::Vec(Vec::new()));
            }
            let mut bytes = vec![0; len];
            read_at_positioned(&file, 0, &mut bytes)?;
            Ok(OwnedBytes::Vec(bytes))
        })
        .await
        .map_err(io::Error::other)?
    }

    /// Reads `len` bytes at `offset` into a new buffer.
    pub async fn read_at(&self, offset: u64, len: usize) -> io::Result<OwnedBytes> {
        if len == 0 {
            return Ok(OwnedBytes::Vec(Vec::new()));
        }
        let file = self.inner.try_clone()?;
        ::tokio::task::spawn_blocking(move || {
            let mut bytes = vec![0; len];
            read_at_positioned(&file, offset, &mut bytes)?;
            Ok(OwnedBytes::Vec(bytes))
        })
        .await
        .map_err(io::Error::other)?
    }

    /// Reads exactly enough bytes to fill `buf` at `offset`.
    pub async fn read_exact_at(&self, offset: u64, buf: &mut [u8]) -> io::Result<()> {
        let file = self.inner.try_clone()?;
        ::tokio::task::block_in_place(|| read_at_positioned(&file, offset, buf))
    }

    /// Writes all bytes from `buf` at `offset`.
    pub async fn write_all_at(&self, offset: u64, buf: &[u8]) -> io::Result<()> {
        let file = self.inner.try_clone()?;
        ::tokio::task::block_in_place(|| write_at_positioned(&file, offset, buf))
    }

    /// Writes non-overlapping slices at their offsets.
    pub async fn write_slices_at(&self, writes: WriteSlices<'_>) -> io::Result<()> {
        let file = self.inner.try_clone()?;
        ::tokio::task::block_in_place(|| {
            std::thread::scope(|scope| {
                let handles = writes
                    .as_slice()
                    .iter()
                    .map(|w| scope.spawn(|| write_at_positioned(&file, w.offset, w.data)))
                    .collect::<Vec<_>>();
                for handle in handles {
                    handle
                        .join()
                        .map_err(|_| io::Error::other("positioned write worker panicked"))??;
                }
                Ok(())
            })
        })
    }
}

impl AsRef<std::fs::File> for File {
    fn as_ref(&self) -> &std::fs::File {
        &self.inner
    }
}

/// Options and flags for opening a Tokio-backed file.
#[derive(Debug, Clone)]
pub struct OpenOptions {
    inner: std::fs::OpenOptions,
}

impl OpenOptions {
    /// Creates a blank set of options.
    #[must_use]
    pub fn new() -> Self {
        Self {
            inner: std::fs::OpenOptions::new(),
        }
    }

    /// Sets read access.
    pub fn read(&mut self, read: bool) -> &mut Self {
        self.inner.read(read);
        self
    }

    /// Sets write access.
    pub fn write(&mut self, write: bool) -> &mut Self {
        self.inner.write(write);
        self
    }

    /// Sets append mode.
    pub fn append(&mut self, append: bool) -> &mut Self {
        self.inner.append(append);
        self
    }

    /// Sets truncate-on-open behavior.
    pub fn truncate(&mut self, truncate: bool) -> &mut Self {
        self.inner.truncate(truncate);
        self
    }

    /// Sets create-if-missing behavior.
    pub fn create(&mut self, create: bool) -> &mut Self {
        self.inner.create(create);
        self
    }

    /// Sets create-new behavior.
    pub fn create_new(&mut self, create_new: bool) -> &mut Self {
        self.inner.create_new(create_new);
        self
    }

    /// Opens a file with the configured options.
    pub async fn open<P: AsRef<Path>>(&self, path: P) -> io::Result<File> {
        let path = path.as_ref().to_path_buf();
        let options = self.inner.clone();
        ::tokio::task::spawn_blocking(move || options.open(path).map(|inner| File { inner }))
            .await
            .map_err(io::Error::other)?
    }
}

impl Default for OpenOptions {
    fn default() -> Self {
        Self::new()
    }
}

/// Positioned read that doesn't require a seek (uses OS-level pread).
#[cfg(unix)]
fn read_at_positioned(file: &std::fs::File, offset: u64, buf: &mut [u8]) -> IoResult<()> {
    use std::os::unix::fs::FileExt;
    file.read_exact_at(buf, offset)
}

#[cfg(windows)]
fn read_at_positioned(file: &std::fs::File, offset: u64, buf: &mut [u8]) -> IoResult<()> {
    use std::os::windows::fs::FileExt;
    let mut read = 0;
    while read < buf.len() {
        let n = file.seek_read(&mut buf[read..], offset + read as u64)?;
        if n == 0 {
            return Err(std::io::Error::new(
                std::io::ErrorKind::UnexpectedEof,
                "unexpected EOF during positioned read",
            ));
        }
        read += n;
    }
    Ok(())
}

/// Positioned write that doesn't require a seek (uses OS-level pwrite).
#[cfg(unix)]
fn write_at_positioned(file: &std::fs::File, offset: u64, data: &[u8]) -> IoResult<()> {
    use std::os::unix::fs::FileExt;
    file.write_all_at(data, offset)
}

#[cfg(windows)]
fn write_at_positioned(file: &std::fs::File, offset: u64, data: &[u8]) -> IoResult<()> {
    use std::os::windows::fs::FileExt;
    let mut written = 0;
    while written < data.len() {
        let n = file.seek_write(&data[written..], offset + written as u64)?;
        written += n;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::WriteSlice;
    use crate::test_utils::run_async;
    use tempfile::TempDir;

    fn write_tmp(dir: &TempDir, name: &str, data: &[u8]) -> std::path::PathBuf {
        let path = dir.path().join(name);
        std::fs::write(&path, data).unwrap();
        path
    }

    fn read_all(path: &std::path::Path) -> crate::OwnedBytes {
        let file = run_async(File::open(path)).unwrap();
        run_async(file.read_all()).unwrap()
    }

    #[test]
    fn read_roundtrip() {
        let dir = TempDir::new().unwrap();
        let data: Vec<u8> = (0u8..=255).cycle().take(4096).collect();
        let path = write_tmp(&dir, "file.bin", &data);
        let result = read_all(&path);
        assert_eq!(result.as_ref(), &data[..]);
    }

    #[test]
    fn read_empty() {
        let dir = TempDir::new().unwrap();
        let path = write_tmp(&dir, "empty.bin", b"");
        let result = read_all(&path);
        assert!(result.is_empty());
    }

    #[test]
    fn file_read_at_returns_correct_slice() {
        let dir = TempDir::new().unwrap();
        let path = write_tmp(&dir, "data.bin", b"hello world");
        let file = run_async(File::open(&path)).unwrap();
        let result = run_async(file.read_at(6, 5)).unwrap();
        assert_eq!(result.as_ref(), b"world");
    }

    #[test]
    fn file_read_at_zero_len() {
        let dir = TempDir::new().unwrap();
        let path = write_tmp(&dir, "data.bin", b"hello");
        let file = run_async(File::open(&path)).unwrap();
        let result = run_async(file.read_at(0, 0)).unwrap();
        assert!(result.is_empty());
    }

    #[test]
    fn write_roundtrip() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("out.bin");
        let file = run_async(File::create(&path)).unwrap();
        run_async(file.write_all_at(0, b"test data")).unwrap();
        let result = read_all(&path);
        assert_eq!(result.as_ref(), b"test data");
    }

    #[test]
    fn write_truncates_existing() {
        let dir = TempDir::new().unwrap();
        let path = write_tmp(&dir, "existing.bin", b"old data here");
        let file = run_async(File::create(&path)).unwrap();
        run_async(file.write_all_at(0, b"new")).unwrap();
        let result = read_all(&path);
        assert_eq!(result.as_ref(), b"new");
    }

    #[test]
    fn write_at_preserves_surrounding_bytes() {
        let dir = TempDir::new().unwrap();
        let path = write_tmp(&dir, "data.bin", b"hello world");
        let file = run_async(OpenOptions::new().write(true).open(&path)).unwrap();
        run_async(file.write_all_at(6, b"rust!")).unwrap();
        let result = read_all(&path);
        assert_eq!(result.as_ref(), b"hello rust!");
    }

    #[test]
    fn write_slices_batches_into_existing_file() {
        let dir = TempDir::new().unwrap();
        let path = write_tmp(&dir, "data.bin", b"----------");
        let slices = vec![WriteSlice::new(0, b"AB"), WriteSlice::new(8, b"CD")];
        let file = run_async(OpenOptions::new().write(true).open(&path)).unwrap();
        run_async(file.write_slices_at(crate::WriteSlices::new(&slices).unwrap())).unwrap();
        let result = read_all(&path);
        assert_eq!(result.as_ref(), b"AB------CD");
    }

    #[test]
    fn write_slices_empty_batch_is_noop() {
        let dir = TempDir::new().unwrap();
        let path = write_tmp(&dir, "data.bin", b"unchanged");
        let file = run_async(OpenOptions::new().write(true).open(&path)).unwrap();
        run_async(file.write_slices_at(crate::WriteSlices::new(&[]).unwrap())).unwrap();
        let result = read_all(&path);
        assert_eq!(result.as_ref(), b"unchanged");
    }

    #[test]
    fn set_len_creates_exact_length() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("positioned.bin");
        let file = run_async(File::create(&path)).unwrap();
        run_async(file.set_len(16)).unwrap();
        let result = read_all(&path);
        assert_eq!(result.as_ref().len(), 16);
    }

    #[test]
    fn create_creates_file() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("new.bin");
        run_async(File::create(&path)).unwrap();
        assert!(path.exists());
    }
}
