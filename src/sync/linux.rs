use std::fs::{Metadata, Permissions};
use std::io::{self, Read, Seek, SeekFrom, Write};
use std::os::unix::fs::FileExt;
use std::path::Path;

use crate::{Bytes, WriteSlices};

/// A synchronous Linux file handle.
#[derive(Debug)]
pub struct File {
    inner: std::fs::File,
}

impl File {
    /// Returns a reference to the underlying `std::fs::File`.
    #[inline]
    #[must_use]
    pub const fn get_ref(&self) -> &std::fs::File {
        &self.inner
    }

    /// Consumes this handle, returning the underlying `std::fs::File`.
    #[inline]
    #[must_use]
    pub fn into_inner(self) -> std::fs::File {
        self.inner
    }

    /// Opens a file for reading.
    ///
    /// This is a convenience shortcut for `OpenOptions::new().read(true).open(path)`.
    pub fn open<P: AsRef<Path>>(path: P) -> io::Result<Self> {
        OpenOptions::new().read(true).open(path)
    }

    /// Creates a new file for writing, truncating it if it already exists.
    ///
    /// This is a convenience shortcut for `OpenOptions::new().write(true).create(true).truncate(true).open(path)`.
    pub fn create<P: AsRef<Path>>(path: P) -> io::Result<Self> {
        OpenOptions::new()
            .write(true)
            .create(true)
            .truncate(true)
            .open(path)
    }

    /// Creates a new file for writing, failing if it already exists.
    ///
    /// This is a convenience shortcut for `OpenOptions::new().write(true).create_new(true).open(path)`.
    pub fn create_new<P: AsRef<Path>>(path: P) -> io::Result<Self> {
        OpenOptions::new().write(true).create_new(true).open(path)
    }

    /// Returns a new [`OpenOptions`] instance to configure how the file is opened.
    #[must_use]
    pub fn options() -> OpenOptions {
        OpenOptions::new()
    }

    /// Duplicates this file handle.
    ///
    /// The returned handle shares the same underlying file description as the original.
    pub fn try_clone(&self) -> io::Result<Self> {
        Ok(Self {
            inner: self.inner.try_clone()?,
        })
    }

    /// Returns metadata for the file.
    #[inline]
    pub fn metadata(&self) -> io::Result<Metadata> {
        self.inner.metadata()
    }

    /// Truncates or extends the file to the given size.
    #[inline]
    pub fn set_len(&self, size: u64) -> io::Result<()> {
        self.inner.set_len(size)
    }

    /// Synchronizes all file data and metadata to disk.
    #[inline]
    pub fn sync_all(&self) -> io::Result<()> {
        self.inner.sync_all()
    }

    /// Synchronizes all file data to disk without flushing metadata.
    #[inline]
    pub fn sync_data(&self) -> io::Result<()> {
        self.inner.sync_data()
    }

    /// Changes the permissions of the file.
    #[inline]
    pub fn set_permissions(&self, perm: Permissions) -> io::Result<()> {
        self.inner.set_permissions(perm)
    }

    /// Reads the entire file into a [`Bytes`] buffer.
    ///
    /// Returns an error if the file length exceeds the platform's address space.
    pub fn read_all(&self) -> io::Result<Bytes> {
        let len = usize::try_from(self.inner.metadata()?.len())
            .map_err(|_| io::Error::other("file too large"))?;
        Bytes::allocate(len, |buf| self.read_exact_at(0, buf))
    }

    /// Reads exactly `len` bytes starting at `offset` into a [`Bytes`] buffer.
    ///
    /// # Platform-specific behavior
    ///
    /// On Linux, this uses `pread` and does not affect the current cursor position.
    pub fn read_at(&self, offset: u64, len: usize) -> io::Result<Bytes> {
        Bytes::allocate(len, |buf| self.read_exact_at(offset, buf))
    }

    /// Reads exactly enough bytes to fill `buf` starting at `offset`.
    ///
    /// # Platform-specific behavior
    ///
    /// On Linux, this uses `pread` and does not affect the current cursor position.
    ///
    /// # Errors
    ///
    /// Returns an error if the end of the file is reached before `buf` is filled.
    #[inline]
    pub fn read_exact_at(&self, offset: u64, buf: &mut [u8]) -> io::Result<()> {
        self.inner.read_exact_at(buf, offset)
    }

    /// Writes all bytes from `buf` starting at `offset`.
    ///
    /// # Platform-specific behavior
    ///
    /// On Linux, this uses `pwrite` and does not affect the current cursor position.
    #[inline]
    pub fn write_all_at(&self, offset: u64, buf: &[u8]) -> io::Result<()> {
        self.inner.write_all_at(buf, offset)
    }

    /// Writes all byte slices from `writes` at their specified offsets.
    ///
    /// Each entry in `writes` is written atomically at its offset using the platform's
    /// positioned write primitive.
    ///
    /// # Platform-specific behavior
    ///
    /// On Linux, this uses `pwrite` for each write and does not affect the current cursor
    /// position.
    pub fn write_slices_at(&self, writes: WriteSlices<'_, '_>) -> io::Result<()> {
        for write in writes.as_slice() {
            self.write_all_at(write.offset, write.data)?;
        }
        Ok(())
    }
}

impl Read for File {
    #[inline]
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        self.inner.read(buf)
    }
}

impl Write for File {
    #[inline]
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        self.inner.write(buf)
    }

    #[inline]
    fn flush(&mut self) -> io::Result<()> {
        self.inner.flush()
    }
}

impl Seek for File {
    #[inline]
    fn seek(&mut self, pos: SeekFrom) -> io::Result<u64> {
        self.inner.seek(pos)
    }
}

impl AsRef<std::fs::File> for File {
    #[inline]
    fn as_ref(&self) -> &std::fs::File {
        &self.inner
    }
}

/// Options for opening a synchronous Linux file.
///
/// This mirrors the API of `std::fs::OpenOptions` while returning a backend-specific
/// [`File`] handle.
#[derive(Debug, Clone)]
pub struct OpenOptions {
    inner: std::fs::OpenOptions,
}

impl OpenOptions {
    /// Creates a new `OpenOptions` with default options.
    #[must_use]
    pub fn new() -> Self {
        Self {
            inner: std::fs::OpenOptions::new(),
        }
    }

    /// Sets the read permission.
    ///
    /// If `true`, the file will be opened for reading.
    #[inline]
    pub fn read(&mut self, read: bool) -> &mut Self {
        self.inner.read(read);
        self
    }

    /// Sets the write permission.
    ///
    /// If `true`, the file will be opened for writing.
    #[inline]
    pub fn write(&mut self, write: bool) -> &mut Self {
        self.inner.write(write);
        self
    }

    /// Sets the append mode.
    ///
    /// If `true`, all writes will append to the end of the file regardless of the
    /// current cursor position.
    #[inline]
    pub fn append(&mut self, append: bool) -> &mut Self {
        self.inner.append(append);
        self
    }

    /// Sets the truncate mode.
    ///
    /// If `true`, the file will be truncated to length zero when opened for writing.
    #[inline]
    pub fn truncate(&mut self, truncate: bool) -> &mut Self {
        self.inner.truncate(truncate);
        self
    }

    /// Sets the create mode.
    ///
    /// If `true`, the file will be created if it does not exist.
    #[inline]
    pub fn create(&mut self, create: bool) -> &mut Self {
        self.inner.create(create);
        self
    }

    /// Sets the create-new mode.
    ///
    /// If `true`, the file will be created only if it does not already exist. Opening an
    /// existing file will fail.
    #[inline]
    pub fn create_new(&mut self, create_new: bool) -> &mut Self {
        self.inner.create_new(create_new);
        self
    }

    /// Opens the file at `path` with the configured options.
    pub fn open<P: AsRef<Path>>(&self, path: P) -> io::Result<File> {
        Ok(File {
            inner: self.inner.open(path)?,
        })
    }
}

impl Default for OpenOptions {
    fn default() -> Self {
        Self::new()
    }
}
