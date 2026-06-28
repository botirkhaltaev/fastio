//! Memory-mapped file I/O.
//!
//! This module exposes a small file-first API for read-only memory maps. The
//! returned [`MmapRegion`] is cheaply cloneable and implements `AsRef<[u8]>`.

use crate::IoResult;
use memmap2::MmapOptions as MemmapOptions;
use std::fs::{File as StdFile, Metadata};
use std::io::{Error, ErrorKind};
use std::path::Path;
use std::sync::Arc;

/// A memory-mapped file region backed by an `Arc<memmap2::Mmap>`.
///
/// Cheaply cloneable; `as_slice()` returns exactly `len` bytes starting at
/// `start` within the underlying mapping.
#[derive(Debug, Clone)]
pub struct MmapRegion {
    inner: Arc<memmap2::Mmap>,
    start: usize,
    len: usize,
}

impl MmapRegion {
    pub(crate) fn new(inner: Arc<memmap2::Mmap>, start: usize, len: usize) -> Self {
        debug_assert!(start.checked_add(len).is_some_and(|end| end <= inner.len()));
        Self { inner, start, len }
    }

    /// Returns a sub-region of `len` bytes starting at `offset` within this region.
    ///
    /// Returns `None` if the requested range is outside this region.
    #[inline]
    #[must_use]
    pub fn subregion(&self, offset: usize, len: usize) -> Option<Self> {
        let relative_end = offset.checked_add(len)?;
        if relative_end > self.len {
            return None;
        }
        let start = self.start.checked_add(offset)?;
        Some(Self::new(Arc::clone(&self.inner), start, len))
    }

    /// Returns the mapped bytes as a slice.
    #[inline]
    #[must_use]
    pub fn as_slice(&self) -> &[u8] {
        &self.inner[self.start..self.start + self.len]
    }

    /// Returns the length of the mapped region in bytes.
    #[inline]
    #[must_use]
    pub const fn len(&self) -> usize {
        self.len
    }

    /// Returns `true` if the region length is zero.
    #[inline]
    #[must_use]
    pub const fn is_empty(&self) -> bool {
        self.len == 0
    }
}

impl AsRef<[u8]> for MmapRegion {
    fn as_ref(&self) -> &[u8] {
        self.as_slice()
    }
}

impl std::ops::Deref for MmapRegion {
    type Target = [u8];
    fn deref(&self) -> &[u8] {
        self.as_slice()
    }
}

/// A file handle for memory-mapped reads.
#[derive(Debug)]
pub struct File {
    inner: StdFile,
}

impl File {
    /// Opens a file in read-only mode.
    pub fn open<P: AsRef<Path>>(path: P) -> IoResult<Self> {
        OpenOptions::new().open(path)
    }

    /// Returns a new options object for opening a memory-mapped file.
    #[must_use]
    pub fn options() -> OpenOptions {
        OpenOptions::new()
    }

    /// Returns a reference to the underlying `std::fs::File`.
    #[inline]
    #[must_use]
    pub const fn get_ref(&self) -> &StdFile {
        &self.inner
    }

    /// Consumes this handle, returning the underlying `std::fs::File`.
    #[inline]
    #[must_use]
    pub fn into_inner(self) -> StdFile {
        self.inner
    }

    /// Creates a new `File` instance sharing the same underlying handle.
    #[inline]
    pub fn try_clone(&self) -> IoResult<Self> {
        Ok(Self {
            inner: self.inner.try_clone()?,
        })
    }

    /// Queries metadata about the underlying file.
    #[inline]
    pub fn metadata(&self) -> IoResult<Metadata> {
        self.inner.metadata()
    }

    /// Memory-maps the entire file.
    pub fn map(&self) -> IoResult<MmapRegion> {
        let len_u64 = self.inner.metadata()?.len();
        let len = usize::try_from(len_u64)
            .map_err(|_| Error::new(ErrorKind::InvalidInput, "file too large"))?;
        if len == 0 {
            return Err(Error::new(ErrorKind::InvalidData, "cannot mmap empty file"));
        }
        // SAFETY: the file is opened read-only and remains alive for the map
        // call; memmap2 owns the mapping after creation. As with all mmap APIs,
        // external truncation or mutation of the file can still fault when the
        // mapping is accessed.
        let inner = unsafe { MemmapOptions::new().map(&self.inner)? };
        Ok(MmapRegion::new(Arc::new(inner), 0, len))
    }

    /// Memory-maps `len` bytes starting at `offset`.
    pub fn map_range(&self, offset: u64, len: usize) -> IoResult<MmapRegion> {
        if len == 0 {
            return Err(Error::new(
                ErrorKind::InvalidInput,
                "cannot mmap empty range",
            ));
        }
        let end = offset
            .checked_add(u64::try_from(len).map_err(|e| Error::new(ErrorKind::InvalidInput, e))?)
            .ok_or_else(|| Error::new(ErrorKind::InvalidInput, "range end overflow"))?;
        let file_len = self.inner.metadata()?.len();
        if end > file_len {
            return Err(Error::new(
                ErrorKind::UnexpectedEof,
                "range exceeds file size",
            ));
        }
        let page_size = u64::try_from(region::page::size()).map_err(|e| {
            Error::new(
                ErrorKind::InvalidInput,
                format!("invalid system page size: {e}"),
            )
        })?;
        if page_size == 0 {
            return Err(Error::new(
                ErrorKind::InvalidInput,
                "system page size must be non-zero",
            ));
        }
        let aligned_offset = (offset / page_size) * page_size;
        let start = usize::try_from(offset - aligned_offset)
            .map_err(|e| Error::new(ErrorKind::InvalidInput, e))?;
        let map_len = len
            .checked_add(start)
            .ok_or_else(|| Error::new(ErrorKind::InvalidInput, "mapping length overflow"))?;
        // SAFETY: the requested range has been bounds-checked against file_len,
        // and the offset passed to memmap2 is page-aligned as required. As with
        // all mmap APIs, external truncation or mutation can still fault when the
        // mapping is accessed.
        let inner = unsafe {
            MemmapOptions::new()
                .offset(aligned_offset)
                .len(map_len)
                .map(&self.inner)?
        };
        Ok(MmapRegion::new(Arc::new(inner), start, len))
    }
}

impl AsRef<StdFile> for File {
    #[inline]
    fn as_ref(&self) -> &StdFile {
        &self.inner
    }
}

/// Options and flags for opening a memory-mapped file.
#[derive(Debug, Clone)]
pub struct OpenOptions {
    _private: (),
}

impl OpenOptions {
    /// Creates a blank set of options.
    #[must_use]
    pub const fn new() -> Self {
        Self { _private: () }
    }

    /// Opens a file for read-only memory mapping.
    pub fn open<P: AsRef<Path>>(&self, path: P) -> IoResult<File> {
        Ok(File {
            inner: StdFile::open(path)?,
        })
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

    #[test]
    fn map_file_roundtrip() {
        let dir = TempDir::new().unwrap();
        let data: Vec<u8> = (0u8..=255).cycle().take(4096).collect();
        let path = dir.path().join("file.bin");
        std::fs::write(&path, &data).unwrap();

        let region = File::open(&path).unwrap().map().unwrap();
        assert_eq!(region.as_slice(), &data[..]);
        assert_eq!(region.len(), 4096);
        assert!(!region.is_empty());
    }

    #[test]
    fn map_file_empty_returns_error() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("empty.bin");
        std::fs::write(&path, b"").unwrap();

        let err = File::open(&path).unwrap().map().unwrap_err();
        assert!(
            err.kind() == std::io::ErrorKind::InvalidData
                || err.kind() == std::io::ErrorKind::Other,
            "unexpected error kind: {:?}",
            err.kind()
        );
    }

    #[test]
    fn map_range_returns_correct_slice() {
        let dir = TempDir::new().unwrap();
        let data: Vec<u8> = (0u8..100).collect();
        let path = dir.path().join("range.bin");
        std::fs::write(&path, &data).unwrap();

        let region = File::open(&path).unwrap().map_range(10, 20).unwrap();
        assert_eq!(region.as_slice(), &data[10..30]);
        assert_eq!(region.len(), 20);
    }

    #[test]
    fn map_range_zero_len_returns_error() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("z.bin");
        std::fs::write(&path, b"hello").unwrap();

        let err = File::open(&path).unwrap().map_range(0, 0).unwrap_err();
        assert_eq!(err.kind(), std::io::ErrorKind::InvalidInput);
    }

    #[test]
    fn map_range_beyond_file_returns_error() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("short.bin");
        std::fs::write(&path, b"hi").unwrap();

        let err = File::open(&path).unwrap().map_range(0, 1000).unwrap_err();
        assert_eq!(err.kind(), std::io::ErrorKind::UnexpectedEof);
    }

    #[test]
    fn map_range_rejects_offset_overflow() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("overflow.bin");
        std::fs::write(&path, b"hello").unwrap();

        let err = File::open(&path)
            .unwrap()
            .map_range(u64::MAX, 2)
            .unwrap_err();

        assert_eq!(err.kind(), std::io::ErrorKind::InvalidInput);
    }

    #[test]
    fn map_file_large() {
        let dir = TempDir::new().unwrap();
        let data = vec![0xABu8; 1024 * 1024]; // 1 MiB
        let path = dir.path().join("large.bin");
        std::fs::write(&path, &data).unwrap();

        let region = File::open(&path).unwrap().map().unwrap();
        assert_eq!(region.len(), data.len());
        assert!(region.as_slice().iter().all(|&b| b == 0xAB));
    }

    #[test]
    fn map_range_page_boundary() {
        let dir = TempDir::new().unwrap();
        // Write 2 pages worth of data (assuming ≤ 4 KiB pages)
        let mut data = vec![0u8; 8192];
        for (i, b) in data.iter_mut().enumerate() {
            *b = (i % 256) as u8;
        }
        let path = dir.path().join("pages.bin");
        std::fs::write(&path, &data).unwrap();

        // Map 100 bytes starting at the second page boundary (4096)
        let region = File::open(&path).unwrap().map_range(4096, 100).unwrap();
        assert_eq!(region.len(), 100);
        assert_eq!(region.as_slice(), &data[4096..4196]);
    }

    #[test]
    fn region_is_cheaply_cloneable() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("clone.bin");
        std::fs::write(&path, b"hello_mmap").unwrap();

        let region = File::open(&path).unwrap().map().unwrap();
        let cloned = region.clone();

        assert_eq!(cloned.as_slice(), b"hello_mmap");
        assert_eq!(region.as_slice(), b"hello_mmap");
    }
}
