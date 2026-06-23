//! Synchronous blocking file I/O.
//!
//! This module intentionally mirrors [`std::fs`] where behavior matches, while
//! adding positioned I/O methods for large-file workloads.

use std::fs::{Metadata, Permissions};
use std::io::{self, Read, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};

use crate::{OwnedBytes, WriteSlice, WriteSlices};

#[cfg(target_os = "linux")]
mod linux;
#[cfg(target_os = "macos")]
mod macos;
#[cfg(target_os = "windows")]
mod windows;

#[cfg(not(any(target_os = "linux", target_os = "macos", target_os = "windows")))]
compile_error!("fastio sync supports Linux, macOS, and Windows only");

/// A synchronous file handle.
///
/// `File` follows [`std::fs::File`] for familiar operations and adds explicit
/// positioned read/write methods where the standard library has no portable
/// inherent API.
#[derive(Debug)]
pub struct File {
    inner: std::fs::File,
}

impl File {
    /// Opens a file in read-only mode.
    pub fn open<P: AsRef<Path>>(path: P) -> io::Result<Self> {
        OpenOptions::new().read(true).open(path)
    }

    /// Opens a file in write-only mode, truncating it if it exists.
    pub fn create<P: AsRef<Path>>(path: P) -> io::Result<Self> {
        OpenOptions::new()
            .write(true)
            .create(true)
            .truncate(true)
            .open(path)
    }

    /// Opens a file in write-only mode, failing if it already exists.
    pub fn create_new<P: AsRef<Path>>(path: P) -> io::Result<Self> {
        OpenOptions::new().write(true).create_new(true).open(path)
    }

    /// Returns a new options object for opening a file.
    #[must_use]
    pub fn options() -> OpenOptions {
        OpenOptions::new()
    }

    /// Creates a new `File` instance that shares the same underlying file handle.
    pub fn try_clone(&self) -> io::Result<Self> {
        Ok(Self {
            inner: self.inner.try_clone()?,
        })
    }

    /// Queries metadata about the underlying file.
    pub fn metadata(&self) -> io::Result<Metadata> {
        self.inner.metadata()
    }

    /// Truncates or extends the underlying file.
    pub fn set_len(&self, size: u64) -> io::Result<()> {
        self.inner.set_len(size)
    }

    /// Attempts to sync all OS-internal file content and metadata to disk.
    pub fn sync_all(&self) -> io::Result<()> {
        self.inner.sync_all()
    }

    /// Attempts to sync file content to disk.
    pub fn sync_data(&self) -> io::Result<()> {
        self.inner.sync_data()
    }

    /// Changes permissions on the underlying file.
    pub fn set_permissions(&self, perm: Permissions) -> io::Result<()> {
        self.inner.set_permissions(perm)
    }

    /// Reads the whole file into memory from offset 0.
    pub fn read_all(&self) -> io::Result<OwnedBytes> {
        let mut file = self.inner.try_clone()?;
        file.seek(SeekFrom::Start(0))?;
        let len = usize::try_from(file.metadata()?.len())
            .map_err(|_| io::Error::other("file too large"))?;
        if len == 0 {
            return Ok(OwnedBytes::Vec(Vec::new()));
        }
        let mut bytes = Vec::with_capacity(len);
        file.read_to_end(&mut bytes)?;
        Ok(OwnedBytes::Vec(bytes))
    }

    /// Reads `len` bytes at `offset` into a new buffer.
    pub fn read_at(&self, offset: u64, len: usize) -> io::Result<OwnedBytes> {
        if len == 0 {
            return Ok(OwnedBytes::Vec(Vec::new()));
        }
        let mut bytes = vec![0; len];
        self.read_exact_at(offset, &mut bytes)?;
        Ok(OwnedBytes::Vec(bytes))
    }

    /// Reads exactly enough bytes to fill `buf` at `offset`.
    pub fn read_exact_at(&self, offset: u64, buf: &mut [u8]) -> io::Result<()> {
        #[cfg(target_os = "linux")]
        return linux::read_exact_at(&self.inner, offset, buf);
        #[cfg(target_os = "macos")]
        return macos::read_exact_at(&self.inner, offset, buf);
        #[cfg(windows)]
        return windows::read_exact_at(&self.inner, offset, buf);
    }

    /// Writes all bytes from `buf` at `offset`.
    pub fn write_all_at(&self, offset: u64, buf: &[u8]) -> io::Result<()> {
        #[cfg(target_os = "linux")]
        return linux::write_all_at(&self.inner, offset, buf);
        #[cfg(target_os = "macos")]
        return macos::write_all_at(&self.inner, offset, buf);
        #[cfg(windows)]
        return windows::write_all_at(&self.inner, offset, buf);
    }

    /// Writes non-overlapping slices at their offsets.
    pub fn write_slices_at(&self, slices: &[WriteSlice<'_>]) -> io::Result<()> {
        let writes = WriteSlices::new(slices)?;
        for write in writes.as_slice() {
            self.write_all_at(write.offset, write.data)?;
        }
        Ok(())
    }
}

impl Read for File {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        self.inner.read(buf)
    }
}

impl Write for File {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        self.inner.write(buf)
    }

    fn flush(&mut self) -> io::Result<()> {
        self.inner.flush()
    }
}

impl Seek for File {
    fn seek(&mut self, pos: SeekFrom) -> io::Result<u64> {
        self.inner.seek(pos)
    }
}

impl AsRef<std::fs::File> for File {
    fn as_ref(&self) -> &std::fs::File {
        &self.inner
    }
}

/// Options and flags for opening a synchronous file.
#[derive(Debug, Clone)]
pub struct OpenOptions {
    inner: std::fs::OpenOptions,
    direct_io: bool,
}

impl OpenOptions {
    /// Creates a blank set of options.
    #[must_use]
    pub fn new() -> Self {
        Self {
            inner: std::fs::OpenOptions::new(),
            direct_io: false,
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

    /// Requests platform direct I/O.
    ///
    /// Direct I/O is currently supported only on Linux. Other platforms return
    /// [`io::ErrorKind::Unsupported`] when this is enabled.
    pub fn direct_io(&mut self, enabled: bool) -> &mut Self {
        self.direct_io = enabled;
        self
    }

    /// Opens a file with the configured options.
    pub fn open<P: AsRef<Path>>(&self, path: P) -> io::Result<File> {
        #[cfg(target_os = "linux")]
        let inner = linux::open(&self.inner, self.direct_io, path.as_ref())?;
        #[cfg(target_os = "macos")]
        let inner = macos::open(&self.inner, self.direct_io, path.as_ref())?;
        #[cfg(target_os = "windows")]
        let inner = windows::open(&self.inner, self.direct_io, path.as_ref())?;

        Ok(File { inner })
    }
}

impl Default for OpenOptions {
    fn default() -> Self {
        Self::new()
    }
}

/// Reads the entire contents of a file into bytes.
pub fn read<P: AsRef<Path>>(path: P) -> io::Result<OwnedBytes> {
    File::open(path)?.read_all()
}

/// Reads the entire contents of a file into a string.
pub fn read_to_string<P: AsRef<Path>>(path: P) -> io::Result<String> {
    std::fs::read_to_string(path)
}

/// Writes a slice as the entire contents of a file.
pub fn write<P: AsRef<Path>, C: AsRef<[u8]>>(path: P, contents: C) -> io::Result<()> {
    let mut file = File::create(path)?;
    file.write_all(contents.as_ref())
}

/// Copies one file to another.
pub fn copy<P: AsRef<Path>, Q: AsRef<Path>>(from: P, to: Q) -> io::Result<u64> {
    std::fs::copy(from, to)
}

/// Queries metadata for a path.
pub fn metadata<P: AsRef<Path>>(path: P) -> io::Result<Metadata> {
    std::fs::metadata(path)
}

/// Queries symlink metadata for a path.
pub fn symlink_metadata<P: AsRef<Path>>(path: P) -> io::Result<Metadata> {
    std::fs::symlink_metadata(path)
}

/// Returns the canonical absolute path.
pub fn canonicalize<P: AsRef<Path>>(path: P) -> io::Result<PathBuf> {
    std::fs::canonicalize(path)
}

/// Removes a file from the filesystem.
pub fn remove_file<P: AsRef<Path>>(path: P) -> io::Result<()> {
    std::fs::remove_file(path)
}

/// Renames a file or directory.
pub fn rename<P: AsRef<Path>, Q: AsRef<Path>>(from: P, to: Q) -> io::Result<()> {
    std::fs::rename(from, to)
}

/// Returns whether the path points at an existing entity.
pub fn exists<P: AsRef<Path>>(path: P) -> io::Result<bool> {
    std::fs::exists(path)
}

#[cfg(test)]
mod file_api_tests {
    use super::*;
    use tempfile::TempDir;

    #[test]
    fn read_reads_entire_file() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("model.bin");
        std::fs::write(&path, b"abcdef").unwrap();

        let bytes = read(&path).unwrap();

        assert_eq!(bytes.as_ref(), b"abcdef");
    }

    #[test]
    fn file_read_at_reads_positioned_bytes() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("model.bin");
        std::fs::write(&path, b"abcdef").unwrap();

        let file = File::open(&path).unwrap();
        let bytes = file.read_at(2, 3).unwrap();

        assert_eq!(bytes.as_ref(), b"cde");
    }

    #[test]
    fn file_write_all_at_writes_positioned_bytes() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("model.bin");
        std::fs::write(&path, b"abcdef").unwrap();

        let file = OpenOptions::new().write(true).open(&path).unwrap();
        file.write_all_at(2, b"XX").unwrap();

        assert_eq!(std::fs::read(&path).unwrap(), b"abXXef");
    }

    #[test]
    fn open_options_direct_io_errors_on_non_linux() {
        #[cfg(not(target_os = "linux"))]
        {
            let dir = TempDir::new().unwrap();
            let path = dir.path().join("model.bin");
            std::fs::write(&path, b"abcdef").unwrap();

            let err = OpenOptions::new()
                .read(true)
                .direct_io(true)
                .open(&path)
                .unwrap_err();

            assert_eq!(err.kind(), io::ErrorKind::Unsupported);
        }
    }
}
