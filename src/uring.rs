//! Linux `io_uring` file I/O.
//!
//! This module mirrors `std::fs` where possible and adds positioned I/O backed
//! by a thread-local `io_uring` instance.

use std::fs::{Metadata, Permissions};
use std::io::{self, Read, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};

use crate::io_uring::{IoUring, IoUringOptions};
use crate::{OwnedBytes, WriteSlice, WriteSlices};

/// A Linux `io_uring` file handle.
#[derive(Debug)]
pub struct File {
    inner: std::fs::File,
    backend: IoUring,
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

    /// Creates a new `File` instance sharing the same underlying handle.
    pub fn try_clone(&self) -> io::Result<Self> {
        Ok(Self {
            inner: self.inner.try_clone()?,
            backend: self.backend,
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
        let len = usize::try_from(self.inner.metadata()?.len())
            .map_err(|_| io::Error::other("file too large"))?;
        if len == 0 {
            return Ok(OwnedBytes::Vec(Vec::new()));
        }
        let mut bytes = vec![0; len];
        self.backend.read_exact_at(&self.inner, 0, &mut bytes)?;
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
        self.backend.read_exact_at(&self.inner, offset, buf)
    }

    /// Writes all bytes from `buf` at `offset`.
    pub fn write_all_at(&self, offset: u64, buf: &[u8]) -> io::Result<()> {
        self.backend.write_exact_at(&self.inner, offset, buf)
    }

    /// Writes non-overlapping slices at their offsets.
    pub fn write_slices_at(&self, slices: &[WriteSlice<'_>]) -> io::Result<()> {
        let writes = WriteSlices::new(slices)?;
        self.backend
            .ring_batch_writes(&self.inner, writes.as_slice())
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

/// Options and flags for opening an `io_uring` file.
#[derive(Debug, Clone)]
pub struct OpenOptions {
    inner: std::fs::OpenOptions,
    ring_depth: u32,
    chunk_size: usize,
}

impl OpenOptions {
    /// Creates a blank set of options.
    #[must_use]
    pub fn new() -> Self {
        let defaults = IoUringOptions::default();
        Self {
            inner: std::fs::OpenOptions::new(),
            ring_depth: defaults.ring_depth,
            chunk_size: defaults.chunk_size,
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

    /// Sets the ring depth used by this file.
    pub fn ring_depth(&mut self, ring_depth: u32) -> &mut Self {
        self.ring_depth = ring_depth;
        self
    }

    /// Sets the maximum chunk size submitted to the ring.
    pub fn chunk_size(&mut self, chunk_size: usize) -> &mut Self {
        self.chunk_size = chunk_size;
        self
    }

    /// Opens a file with the configured options.
    pub fn open<P: AsRef<Path>>(&self, path: P) -> io::Result<File> {
        let backend = IoUring::with_options(IoUringOptions {
            ring_depth: self.ring_depth,
            chunk_size: self.chunk_size,
        })?;
        Ok(File {
            inner: self.inner.open(path)?,
            backend,
        })
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

/// Writes a slice as the entire contents of a file.
pub fn write<P: AsRef<Path>, C: AsRef<[u8]>>(path: P, contents: C) -> io::Result<()> {
    let mut file = File::create(path)?;
    file.write_all(contents.as_ref())
}

/// Reads the entire contents of a file into a string.
pub fn read_to_string<P: AsRef<Path>>(path: P) -> io::Result<String> {
    String::from_utf8(read(path)?.into_vec())
        .map_err(|err| io::Error::new(io::ErrorKind::InvalidData, err))
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
