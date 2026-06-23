//! Memory-mapped file I/O.
//!
//! This module exposes a small file-first API for read-only memory maps. The
//! returned [`MmapRegion`] is cheaply cloneable and implements `AsRef<[u8]>`.

use crate::{IoResult, buffer::MmapRegion};
use memmap2::MmapOptions as MemmapOptions;
use std::fs::{File as StdFile, Metadata};
use std::io::{Error, ErrorKind};
use std::path::Path;
use std::sync::Arc;

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

    /// Creates a new `File` instance sharing the same underlying handle.
    pub fn try_clone(&self) -> IoResult<Self> {
        Ok(Self {
            inner: self.inner.try_clone()?,
        })
    }

    /// Queries metadata about the underlying file.
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
        // SAFETY: the file is opened read-only and remains alive for the mapping
        // call; memmap2 owns the mapping after creation.
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
        // and the offset passed to memmap2 is page-aligned as required.
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
    pub fn new() -> Self {
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

    fn write_tmp(dir: &TempDir, name: &str, data: &[u8]) -> std::path::PathBuf {
        let path = dir.path().join(name);
        std::fs::write(&path, data).unwrap();
        path
    }

    #[test]
    fn map_file_roundtrip() {
        let dir = TempDir::new().unwrap();
        let data: Vec<u8> = (0u8..=255).cycle().take(4096).collect();
        let path = write_tmp(&dir, "file.bin", &data);

        let region = File::open(&path).unwrap().map().unwrap();
        assert_eq!(region.as_slice(), &data[..]);
        assert_eq!(region.len(), 4096);
        assert!(!region.is_empty());
    }

    #[test]
    fn map_file_empty_returns_error() {
        let dir = TempDir::new().unwrap();
        let path = write_tmp(&dir, "empty.bin", b"");

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
        let path = write_tmp(&dir, "range.bin", &data);

        let region = File::open(&path).unwrap().map_range(10, 20).unwrap();
        assert_eq!(region.as_slice(), &data[10..30]);
        assert_eq!(region.len(), 20);
    }

    #[test]
    fn map_range_zero_len_returns_error() {
        let dir = TempDir::new().unwrap();
        let path = write_tmp(&dir, "z.bin", b"hello");

        let err = File::open(&path).unwrap().map_range(0, 0).unwrap_err();
        assert_eq!(err.kind(), std::io::ErrorKind::InvalidInput);
    }

    #[test]
    fn map_range_beyond_file_returns_error() {
        let dir = TempDir::new().unwrap();
        let path = write_tmp(&dir, "short.bin", b"hi");

        let err = File::open(&path).unwrap().map_range(0, 1000).unwrap_err();
        assert_eq!(err.kind(), std::io::ErrorKind::UnexpectedEof);
    }

    #[test]
    fn map_file_large() {
        let dir = TempDir::new().unwrap();
        let data = vec![0xABu8; 1024 * 1024]; // 1 MiB
        let path = write_tmp(&dir, "large.bin", &data);

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
        let path = write_tmp(&dir, "pages.bin", &data);

        // Map 100 bytes starting at the second page boundary (4096)
        let region = File::open(&path).unwrap().map_range(4096, 100).unwrap();
        assert_eq!(region.len(), 100);
        assert_eq!(region.as_slice(), &data[4096..4196]);
    }

    #[test]
    fn region_is_cheaply_cloneable() {
        let dir = TempDir::new().unwrap();
        let path = write_tmp(&dir, "clone.bin", b"hello clone");

        let r1 = File::open(&path).unwrap().map().unwrap();
        let r2 = r1.clone();
        assert_eq!(r1.as_slice(), r2.as_slice());
        assert_eq!(r2.as_slice(), b"hello clone");
    }
}
