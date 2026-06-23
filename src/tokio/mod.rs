//! Tokio async file I/O.
//!
//! This module mirrors `tokio::fs` where behavior matches, while adding async
//! positioned I/O methods for large-file workloads.

use std::fs::{Metadata, Permissions};
use std::io;
use std::path::Path;

use crate::{Allocator, DefaultAllocator, IoResult, OwnedBytes, WriteSlices};

#[cfg(not(any(target_os = "linux", target_os = "macos", target_os = "windows")))]
compile_error!("fastio tokio supports Linux, macOS, and Windows only");

/// A Tokio-backed file handle.
#[derive(Debug)]
pub struct File<A = DefaultAllocator> {
    inner: std::fs::File,
    allocator: A,
}

impl File<DefaultAllocator> {
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
}

impl<A: Allocator> File<A> {
    /// Creates a new `File` instance sharing the same underlying handle.
    pub async fn try_clone(&self) -> io::Result<Self> {
        let file = self.inner.try_clone()?;
        Ok(Self {
            inner: file,
            allocator: self.allocator.clone(),
        })
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
        let allocator = self.allocator.clone();
        ::tokio::task::spawn_blocking(move || {
            let len = usize::try_from(file.metadata()?.len())
                .map_err(|_| io::Error::other("file too large"))?;
            if len == 0 {
                return Ok(OwnedBytes::Vec(Vec::new()));
            }
            let mut bytes = allocator.allocate(len);
            let buf = bytes
                .as_mut_slice()
                .ok_or_else(|| io::Error::other("allocator returned immutable buffer"))?;
            if buf.len() != len {
                return Err(io::Error::other("allocator returned wrong-sized buffer"));
            }
            read_at_positioned(&file, 0, buf)?;
            Ok(bytes)
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
        let allocator = self.allocator.clone();
        ::tokio::task::spawn_blocking(move || {
            let mut bytes = allocator.allocate(len);
            let buf = bytes
                .as_mut_slice()
                .ok_or_else(|| io::Error::other("allocator returned immutable buffer"))?;
            if buf.len() != len {
                return Err(io::Error::other("allocator returned wrong-sized buffer"));
            }
            read_at_positioned(&file, offset, buf)?;
            Ok(bytes)
        })
        .await
        .map_err(io::Error::other)?
    }

    /// Reads exactly enough bytes to fill `buf` at `offset`.
    pub async fn read_exact_at(&self, offset: u64, buf: &mut [u8]) -> io::Result<()> {
        read_at_positioned(&self.inner, offset, buf)
    }

    /// Writes all bytes from `buf` at `offset`.
    pub async fn write_all_at(&self, offset: u64, buf: &[u8]) -> io::Result<()> {
        write_at_positioned(&self.inner, offset, buf)
    }

    /// Writes non-overlapping slices at their offsets.
    pub async fn write_slices_at(&self, writes: WriteSlices<'_, '_>) -> io::Result<()> {
        let file = &self.inner;
        std::thread::scope(|scope| {
            let handles = writes
                .as_slice()
                .iter()
                .map(|w| scope.spawn(|| write_at_positioned(file, w.offset, w.data)))
                .collect::<Vec<_>>();
            for handle in handles {
                handle
                    .join()
                    .map_err(|_| io::Error::other("positioned write worker panicked"))??;
            }
            Ok(())
        })
    }
}

impl<A> AsRef<std::fs::File> for File<A> {
    fn as_ref(&self) -> &std::fs::File {
        &self.inner
    }
}

/// Options and flags for opening a Tokio-backed file.
#[derive(Debug, Clone)]
pub struct OpenOptions<A = DefaultAllocator> {
    inner: std::fs::OpenOptions,
    allocator: A,
}

impl OpenOptions<DefaultAllocator> {
    /// Creates a blank set of options.
    #[must_use]
    pub fn new() -> Self {
        Self {
            inner: std::fs::OpenOptions::new(),
            allocator: DefaultAllocator::default(),
        }
    }
}

impl<A: Allocator> OpenOptions<A> {
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

    /// Sets the allocator used by reads on files opened with these options.
    pub fn allocator<B: Allocator>(&self, allocator: B) -> OpenOptions<B> {
        OpenOptions {
            inner: self.inner.clone(),
            allocator,
        }
    }

    /// Opens a file with the configured options.
    pub async fn open<P: AsRef<Path>>(&self, path: P) -> io::Result<File<A>> {
        let path = path.as_ref().to_path_buf();
        let options = self.inner.clone();
        let allocator = self.allocator.clone();
        ::tokio::task::spawn_blocking(move || {
            options.open(path).map(|inner| File { inner, allocator })
        })
        .await
        .map_err(io::Error::other)?
    }
}

impl Default for OpenOptions<DefaultAllocator> {
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
        let read_offset = offset
            .checked_add(read as u64)
            .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidInput, "read offset overflow"))?;
        let n = file.seek_read(&mut buf[read..], read_offset)?;
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
        let write_offset = offset
            .checked_add(written as u64)
            .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidInput, "write offset overflow"))?;
        let n = file.seek_write(&data[written..], write_offset)?;
        if n == 0 {
            return Err(io::Error::new(
                io::ErrorKind::WriteZero,
                "seek_write returned zero bytes",
            ));
        }
        written += n;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{OwnedBytes, System, WriteSlice};
    use tempfile::TempDir;

    #[tokio::test(flavor = "multi_thread")]
    async fn read_roundtrip() {
        let dir = TempDir::new().unwrap();
        let data: Vec<u8> = (0u8..=255).cycle().take(4096).collect();
        let path = dir.path().join("file.bin");
        std::fs::write(&path, &data).unwrap();
        let file = File::open(&path).await.unwrap();
        let result = file.read_all().await.unwrap();
        assert_eq!(result.as_ref(), &data[..]);
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn read_empty() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("empty.bin");
        std::fs::write(&path, b"").unwrap();
        let file = File::open(&path).await.unwrap();
        let result = file.read_all().await.unwrap();
        assert!(result.is_empty());
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn file_read_at_returns_correct_slice() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("data.bin");
        std::fs::write(&path, b"hello world").unwrap();
        let file = File::open(&path).await.unwrap();
        let result = file.read_at(6, 5).await.unwrap();
        assert_eq!(result.as_ref(), b"world");
    }

    #[cfg(feature = "pool")]
    #[tokio::test(flavor = "multi_thread")]
    async fn default_allocator_returns_pooled_read_buffer() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("data.bin");
        std::fs::write(&path, b"hello world").unwrap();

        let file = File::open(&path).await.unwrap();
        let result = file.read_all().await.unwrap();

        assert!(matches!(&result, OwnedBytes::Pooled(_)));
        assert_eq!(result.as_ref(), b"hello world");
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn system_allocator_returns_vec_read_buffer() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("data.bin");
        std::fs::write(&path, b"hello world").unwrap();

        let file = OpenOptions::new()
            .read(true)
            .allocator(System)
            .open(&path)
            .await
            .unwrap();
        let result = file.read_at(6, 5).await.unwrap();

        assert!(matches!(&result, OwnedBytes::Vec(_)));
        assert_eq!(result.as_ref(), b"world");
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn file_read_at_zero_len() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("data.bin");
        std::fs::write(&path, b"hello").unwrap();
        let file = File::open(&path).await.unwrap();
        let result = file.read_at(0, 0).await.unwrap();
        assert!(result.is_empty());
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn write_roundtrip() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("out.bin");
        let file = File::create(&path).await.unwrap();
        file.write_all_at(0, b"test data").await.unwrap();
        let file = File::open(&path).await.unwrap();
        let result = file.read_all().await.unwrap();
        assert_eq!(result.as_ref(), b"test data");
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn write_truncates_existing() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("existing.bin");
        std::fs::write(&path, b"old data here").unwrap();
        let file = File::create(&path).await.unwrap();
        file.write_all_at(0, b"new").await.unwrap();
        let file = File::open(&path).await.unwrap();
        let result = file.read_all().await.unwrap();
        assert_eq!(result.as_ref(), b"new");
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn write_at_preserves_surrounding_bytes() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("data.bin");
        std::fs::write(&path, b"hello world").unwrap();
        let file = OpenOptions::new().write(true).open(&path).await.unwrap();
        file.write_all_at(6, b"rust!").await.unwrap();
        let file = File::open(&path).await.unwrap();
        let result = file.read_all().await.unwrap();
        assert_eq!(result.as_ref(), b"hello rust!");
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn write_slices_batches_into_existing_file() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("data.bin");
        std::fs::write(&path, b"----------").unwrap();
        let slices = vec![WriteSlice::new(0, b"AB"), WriteSlice::new(8, b"CD")];
        let file = OpenOptions::new().write(true).open(&path).await.unwrap();
        file.write_slices_at(crate::WriteSlices::new(&slices).unwrap())
            .await
            .unwrap();
        let file = File::open(&path).await.unwrap();
        let result = file.read_all().await.unwrap();
        assert_eq!(result.as_ref(), b"AB------CD");
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn write_slices_empty_batch_is_noop() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("data.bin");
        std::fs::write(&path, b"unchanged").unwrap();
        let file = OpenOptions::new().write(true).open(&path).await.unwrap();
        file.write_slices_at(crate::WriteSlices::new(&[]).unwrap())
            .await
            .unwrap();
        let file = File::open(&path).await.unwrap();
        let result = file.read_all().await.unwrap();
        assert_eq!(result.as_ref(), b"unchanged");
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn set_len_creates_exact_length() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("positioned.bin");
        let file = File::create(&path).await.unwrap();
        file.set_len(16).await.unwrap();
        let file = File::open(&path).await.unwrap();
        let result = file.read_all().await.unwrap();
        assert_eq!(result.as_ref().len(), 16);
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn create_creates_file() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("new.bin");
        File::create(&path).await.unwrap();
        assert!(path.exists());
    }
}
