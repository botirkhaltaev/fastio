//! Tokio async file I/O.
//!
//! This module mirrors `tokio::fs` where behavior matches, while adding async
//! positioned I/O methods for large-file workloads.

use std::fs::{Metadata, Permissions};
use std::io;
#[cfg(windows)]
use std::io::Seek;
use std::path::Path;

use crate::{Bytes, IoResult, WriteSlices};

#[cfg(not(any(target_os = "linux", target_os = "macos", target_os = "windows")))]
compile_error!("fastio tokio supports Linux, macOS, and Windows only");

/// A Tokio-backed file handle.
///
/// Regular filesystem operations delegate to `tokio::fs`. Positioned
/// reads and writes move the underlying `std::fs::File` into `spawn_blocking`
/// so they do not block Tokio runtime worker threads.
#[derive(Debug)]
pub struct File {
    inner: ::tokio::fs::File,
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
        Ok(Self {
            inner: self.inner.try_clone().await?,
        })
    }

    /// Queries metadata about the underlying file.
    pub async fn metadata(&self) -> io::Result<Metadata> {
        self.inner.metadata().await
    }

    /// Truncates or extends the underlying file.
    pub async fn set_len(&self, size: u64) -> io::Result<()> {
        self.inner.set_len(size).await
    }

    /// Attempts to sync all OS-internal file content and metadata to disk.
    pub async fn sync_all(&self) -> io::Result<()> {
        self.inner.sync_all().await
    }

    /// Attempts to sync file content to disk.
    pub async fn sync_data(&self) -> io::Result<()> {
        self.inner.sync_data().await
    }

    /// Changes permissions on the underlying file.
    pub async fn set_permissions(&self, perm: Permissions) -> io::Result<()> {
        self.inner.set_permissions(perm).await
    }

    /// Reads the whole file into memory from offset 0.
    pub async fn read_all(&self) -> io::Result<Bytes> {
        let file = self.inner.try_clone().await?.into_std().await;
        ::tokio::task::spawn_blocking(move || {
            let len = usize::try_from(file.metadata()?.len())
                .map_err(|_| io::Error::other("file too large"))?;
            Bytes::allocate(len, |buf| Self::read_at_positioned(&file, 0, buf))
        })
        .await
        .map_err(io::Error::other)?
    }

    /// Reads `len` bytes at `offset` into a new buffer.
    pub async fn read_at(&self, offset: u64, len: usize) -> io::Result<Bytes> {
        let file = self.inner.try_clone().await?.into_std().await;
        ::tokio::task::spawn_blocking(move || {
            Bytes::allocate(len, |buf| Self::read_at_positioned(&file, offset, buf))
        })
        .await
        .map_err(io::Error::other)?
    }

    /// Reads exactly enough bytes to fill `buf` at `offset`.
    pub async fn read_exact_at(&self, offset: u64, buf: &mut [u8]) -> io::Result<()> {
        if buf.is_empty() {
            return Ok(());
        }
        let file = self.inner.try_clone().await?.into_std().await;
        let len = buf.len();
        let bytes = ::tokio::task::spawn_blocking(move || {
            let mut bytes = vec![0u8; len];
            Self::read_at_positioned(&file, offset, &mut bytes)?;
            Ok::<_, io::Error>(bytes)
        })
        .await
        .map_err(io::Error::other)??;
        buf.copy_from_slice(&bytes);
        Ok(())
    }

    /// Writes all bytes from `buf` at `offset`.
    pub async fn write_all_at(&self, offset: u64, buf: &[u8]) -> io::Result<()> {
        if buf.is_empty() {
            return Ok(());
        }
        let file = self.inner.try_clone().await?.into_std().await;
        let bytes = buf.to_vec();
        ::tokio::task::spawn_blocking(move || Self::write_at_positioned(&file, offset, &bytes))
            .await
            .map_err(io::Error::other)?
    }

    /// Writes non-overlapping slices at their offsets.
    pub async fn write_slices_at(&self, writes: WriteSlices<'_, '_>) -> io::Result<()> {
        let file = self.inner.try_clone().await?.into_std().await;
        let writes = writes
            .as_slice()
            .iter()
            .map(|w| (w.offset, w.data.to_vec()))
            .collect::<Vec<_>>();
        ::tokio::task::spawn_blocking(move || {
            if writes.is_empty() {
                return Ok(());
            }
            let workers = std::thread::available_parallelism()
                .map(usize::from)
                .unwrap_or(1)
                .min(writes.len())
                .max(1);
            for batch in writes.chunks(workers) {
                std::thread::scope(|scope| {
                    let handles = batch
                        .iter()
                        .map(|(offset, data)| {
                            scope.spawn(|| Self::write_at_positioned(&file, *offset, data))
                        })
                        .collect::<Vec<_>>();
                    for handle in handles {
                        handle
                            .join()
                            .map_err(|_| io::Error::other("positioned write worker panicked"))??;
                    }
                    Ok::<_, io::Error>(())
                })?;
            }
            Ok(())
        })
        .await
        .map_err(io::Error::other)?
    }

    /// Positioned read that does not require a seek.
    #[cfg(unix)]
    fn read_at_positioned(file: &std::fs::File, offset: u64, buf: &mut [u8]) -> IoResult<()> {
        use std::os::unix::fs::FileExt;
        file.read_exact_at(buf, offset)
    }

    #[cfg(windows)]
    fn read_at_positioned(file: &std::fs::File, offset: u64, buf: &mut [u8]) -> IoResult<()> {
        use std::os::windows::fs::FileExt;
        let mut handle = file;
        let original_position = handle.stream_position()?;
        let mut read = 0;
        let mut result = Ok(());
        while read < buf.len() {
            let read_offset = offset.checked_add(read as u64).ok_or_else(|| {
                io::Error::new(io::ErrorKind::InvalidInput, "read offset overflow")
            })?;
            let n = file.seek_read(&mut buf[read..], read_offset)?;
            if n == 0 {
                result = Err(io::Error::new(
                    io::ErrorKind::UnexpectedEof,
                    "unexpected EOF during positioned read",
                ));
                break;
            }
            read += n;
        }
        let restore = handle.seek(std::io::SeekFrom::Start(original_position));
        result.and(restore.map(|_| ()))
    }

    /// Positioned write that does not require a seek.
    #[cfg(unix)]
    fn write_at_positioned(file: &std::fs::File, offset: u64, data: &[u8]) -> IoResult<()> {
        use std::os::unix::fs::FileExt;
        file.write_all_at(data, offset)
    }

    #[cfg(windows)]
    fn write_at_positioned(file: &std::fs::File, offset: u64, data: &[u8]) -> IoResult<()> {
        use std::os::windows::fs::FileExt;
        let mut handle = file;
        let original_position = handle.stream_position()?;
        let mut written = 0;
        let mut result = Ok(());
        while written < data.len() {
            let write_offset = offset.checked_add(written as u64).ok_or_else(|| {
                io::Error::new(io::ErrorKind::InvalidInput, "write offset overflow")
            })?;
            let n = file.seek_write(&data[written..], write_offset)?;
            if n == 0 {
                result = Err(io::Error::new(
                    io::ErrorKind::WriteZero,
                    "seek_write returned zero bytes",
                ));
                break;
            }
            written += n;
        }
        let restore = handle.seek(std::io::SeekFrom::Start(original_position));
        result.and(restore.map(|_| ()))
    }
}

impl AsRef<::tokio::fs::File> for File {
    fn as_ref(&self) -> &::tokio::fs::File {
        &self.inner
    }
}

/// Options and flags for opening a Tokio-backed file.
#[derive(Debug, Clone)]
pub struct OpenOptions {
    read: bool,
    write: bool,
    append: bool,
    truncate: bool,
    create: bool,
    create_new: bool,
}

impl OpenOptions {
    /// Creates a blank set of options.
    #[must_use]
    pub fn new() -> Self {
        Self {
            read: false,
            write: false,
            append: false,
            truncate: false,
            create: false,
            create_new: false,
        }
    }

    /// Sets read access.
    pub fn read(&mut self, read: bool) -> &mut Self {
        self.read = read;
        self
    }

    /// Sets write access.
    pub fn write(&mut self, write: bool) -> &mut Self {
        self.write = write;
        self
    }

    /// Sets append mode.
    pub fn append(&mut self, append: bool) -> &mut Self {
        self.append = append;
        self
    }

    /// Sets truncate-on-open behavior.
    pub fn truncate(&mut self, truncate: bool) -> &mut Self {
        self.truncate = truncate;
        self
    }

    /// Sets create-if-missing behavior.
    pub fn create(&mut self, create: bool) -> &mut Self {
        self.create = create;
        self
    }

    /// Sets create-new behavior.
    pub fn create_new(&mut self, create_new: bool) -> &mut Self {
        self.create_new = create_new;
        self
    }

    /// Opens a file with the configured options.
    pub async fn open<P: AsRef<Path>>(&self, path: P) -> io::Result<File> {
        let mut options = ::tokio::fs::OpenOptions::new();
        options
            .read(self.read)
            .write(self.write)
            .append(self.append)
            .truncate(self.truncate)
            .create(self.create)
            .create_new(self.create_new);
        options.open(path).await.map(|inner| File { inner })
    }
}

impl Default for OpenOptions {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
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

    #[tokio::test(flavor = "multi_thread")]
    async fn default_allocator_returns_pooled_read_buffer() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("data.bin");
        std::fs::write(&path, b"hello world").unwrap();
        let file = File::open(&path).await.unwrap();
        let result = file.read_all().await.unwrap();

        assert!(result.is_pooled());
        assert_eq!(result.as_ref(), b"hello world");
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

    #[tokio::test(flavor = "current_thread")]
    async fn positioned_io_works_on_current_thread_runtime() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("current-thread.bin");
        let file = File::create(&path).await.unwrap();
        file.write_all_at(0, b"hello").await.unwrap();
        let file = File::open(&path).await.unwrap();
        let buf = file.read_at(0, 5).await.unwrap();
        assert_eq!(buf.as_ref(), b"hello");
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn write_at_preserves_surrounding_bytes() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("data.bin");
        std::fs::write(&path, b"----------").unwrap();
        let file = OpenOptions::new().write(true).open(&path).await.unwrap();
        file.write_all_at(2, b"XX").await.unwrap();
        let file = File::open(&path).await.unwrap();
        let result = file.read_all().await.unwrap();
        assert_eq!(result.as_ref(), b"--XX------");
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn write_roundtrip() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("write.bin");
        let file = File::create(&path).await.unwrap();
        file.write_all_at(0, b"hello world").await.unwrap();
        let file = File::open(&path).await.unwrap();
        let result = file.read_all().await.unwrap();
        assert_eq!(result.as_ref(), b"hello world");
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn create_creates_file() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("create.bin");
        let file = File::create(&path).await.unwrap();
        file.write_all_at(0, b"x").await.unwrap();
        assert!(path.exists());
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn write_truncates_existing() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("truncate.bin");
        std::fs::write(&path, b"original content").unwrap();
        let file = File::create(&path).await.unwrap();
        file.write_all_at(0, b"new").await.unwrap();
        let file = File::open(&path).await.unwrap();
        let result = file.read_all().await.unwrap();
        assert_eq!(result.as_ref(), b"new");
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn set_len_creates_exact_length() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("len.bin");
        let file = File::create(&path).await.unwrap();
        file.set_len(1024).await.unwrap();
        let file = File::open(&path).await.unwrap();
        let result = file.read_all().await.unwrap();
        assert_eq!(result.len(), 1024);
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn metadata_and_try_clone_use_same_file_contents() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("data.bin");
        std::fs::write(&path, b"hello world").unwrap();
        let file = File::open(&path).await.unwrap();
        let cloned = file.try_clone().await.unwrap();
        assert_eq!(
            file.metadata().await.unwrap().len(),
            cloned.metadata().await.unwrap().len()
        );
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn write_slices_empty_batch_is_noop() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("noop.bin");
        std::fs::write(&path, b"----------").unwrap();
        let file = OpenOptions::new().write(true).open(&path).await.unwrap();
        let slices: Vec<crate::WriteSlice> = vec![];
        file.write_slices_at(crate::WriteSlices::new(&slices).unwrap())
            .await
            .unwrap();
        let file = File::open(&path).await.unwrap();
        let result = file.read_all().await.unwrap();
        assert_eq!(result.as_ref(), b"----------");
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn zero_length_write_is_noop() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("noop.bin");
        std::fs::write(&path, b"----------").unwrap();

        let file = OpenOptions::new().write(true).open(&path).await.unwrap();
        file.write_all_at(0, b"").await.unwrap();
        file.write_all_at(5, b"").await.unwrap();

        let file = File::open(&path).await.unwrap();
        let result = file.read_all().await.unwrap();
        assert_eq!(result.as_ref(), b"----------");
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn write_slices_batches_into_existing_file() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("data.bin");
        std::fs::write(&path, b"----------").unwrap();
        let slices = vec![
            crate::WriteSlice::new(0, b"AB"),
            crate::WriteSlice::new(8, b"CD"),
        ];
        let file = OpenOptions::new().write(true).open(&path).await.unwrap();
        file.write_slices_at(crate::WriteSlices::new(&slices).unwrap())
            .await
            .unwrap();
        let file = File::open(&path).await.unwrap();
        let result = file.read_all().await.unwrap();
        assert_eq!(result.as_ref(), b"AB------CD");
    }

    #[tokio::test(flavor = "current_thread")]
    async fn write_slices_at_works_on_current_thread_runtime() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("current-thread-batch.bin");
        std::fs::write(&path, b"--------").unwrap();
        let slices = [
            crate::WriteSlice::new(0, b"AB"),
            crate::WriteSlice::new(6, b"YZ"),
        ];
        let file = OpenOptions::new().write(true).open(&path).await.unwrap();

        file.write_slices_at(crate::WriteSlices::new(&slices).unwrap())
            .await
            .unwrap();
        let file = File::open(&path).await.unwrap();
        let result = file.read_all().await.unwrap();
        assert_eq!(result.as_ref(), b"AB----YZ");
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn write_slices_handles_batches_larger_than_worker_count() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("batch.bin");
        let payloads: Vec<Vec<u8>> = (0..16).map(|i| vec![b'a' + i as u8; 2]).collect();
        let slices = payloads
            .iter()
            .enumerate()
            .map(|(idx, data)| crate::WriteSlice::new((idx * 2) as u64, data.as_slice()))
            .collect::<Vec<_>>();
        let file = OpenOptions::new()
            .write(true)
            .create(true)
            .truncate(true)
            .open(&path)
            .await
            .unwrap();

        file.write_slices_at(crate::WriteSlices::new(&slices).unwrap())
            .await
            .unwrap();
        let file = File::open(&path).await.unwrap();
        let result = file.read_all().await.unwrap();
        let expected: Vec<u8> = (0..16).flat_map(|i| vec![b'a' + i as u8; 2]).collect();
        assert_eq!(result.as_ref(), &expected[..]);
    }
}
