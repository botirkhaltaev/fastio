//! macOS std::fs synchronous storage implementation.

use std::io::{Read, Write};
use std::os::unix::fs::FileExt;
use std::path::Path;
use std::sync::Arc;

use crate::{
    ByteRange, FileRange, IoResult, RangeRead, RequestIndex, WriteSlices, buffer::OwnedBytes,
};

// ============================================================================
// SyncIo
// ============================================================================

/// Synchronous blocking I/O backend (macOS std::fs implementation).
#[derive(Debug, Clone)]
pub struct SyncIo {
    options: super::SyncOptions,
    pool: Option<Arc<rayon::ThreadPool>>,
}

impl Default for SyncIo {
    fn default() -> Self {
        Self::new()
    }
}

impl SyncIo {
    #[inline]
    #[must_use]
    pub fn new() -> Self {
        Self {
            options: super::SyncOptions::default(),
            pool: None,
        }
    }

    pub fn with_options(options: super::SyncOptions) -> IoResult<Self> {
        let pool = match options.batch_threads {
            Some(0) => {
                return Err(std::io::Error::new(
                    std::io::ErrorKind::InvalidInput,
                    "batch_threads must be greater than zero",
                ));
            }
            Some(threads) => Some(Arc::new(
                rayon::ThreadPoolBuilder::new()
                    .num_threads(threads)
                    .build()
                    .map_err(|e| std::io::Error::other(format!("rayon pool error: {e}")))?,
            )),
            None => None,
        };
        Ok(Self { options, pool })
    }

    #[inline]
    #[must_use]
    pub const fn options(&self) -> &super::SyncOptions {
        &self.options
    }

    fn in_pool<R: Send>(&self, f: impl FnOnce() -> R + Send) -> R {
        match &self.pool {
            Some(pool) => pool.install(f),
            None => f(),
        }
    }
}

impl super::super::BlockingIo for SyncIo {
    fn read_file(&self, path: &Path) -> IoResult<OwnedBytes> {
        let mut file = std::fs::File::open(path)?;
        let len = usize::try_from(file.metadata()?.len())
            .map_err(|_| std::io::Error::other("file too large"))?;
        if len == 0 {
            return Ok(OwnedBytes::Vec(Vec::new()));
        }
        let mut buf = self.options.allocator.alloc(len);
        file.read_exact(buf.as_mut_slice().unwrap())?;
        Ok(buf)
    }

    fn read_range(&self, path: &Path, range: ByteRange) -> IoResult<OwnedBytes> {
        if range.is_empty() {
            return Ok(OwnedBytes::Vec(Vec::new()));
        }
        let file = std::fs::File::open(path)?;
        let len = range.len_usize()?;
        let mut buf = self.options.allocator.alloc(len);
        file.read_exact_at(buf.as_mut_slice().unwrap(), range.start())?;
        Ok(buf)
    }

    fn read_ranges(&self, ranges: &[FileRange<'_>]) -> IoResult<Vec<RangeRead>> {
        use rayon::prelude::*;
        self.in_pool(|| {
            ranges
                .par_iter()
                .enumerate()
                .map(|(i, entry)| {
                    let bytes = self.read_range(entry.path, entry.range)?;
                    Ok(RangeRead {
                        request_index: RequestIndex::new(i),
                        range: entry.range,
                        bytes,
                    })
                })
                .collect()
        })
    }
    fn write_file(&self, path: &Path, data: &[u8]) -> IoResult<()> {
        let mut file = std::fs::OpenOptions::new()
            .write(true)
            .create(true)
            .truncate(true)
            .open(path)?;
        file.write_all(data)
    }

    fn write_positioned_file(
        &self,
        path: &Path,
        len: u64,
        writes: WriteSlices<'_>,
    ) -> IoResult<()> {
        let file = std::fs::OpenOptions::new()
            .write(true)
            .create(true)
            .truncate(true)
            .open(path)?;
        file.set_len(len)?;
        if writes.is_empty() {
            return Ok(());
        }
        use rayon::prelude::*;
        self.in_pool(|| {
            writes
                .as_slice()
                .par_iter()
                .try_for_each(|w| file.write_all_at(w.data, w.offset))
        })
    }

    fn write_at(&self, path: &Path, offset: u64, data: &[u8]) -> IoResult<()> {
        let file = std::fs::OpenOptions::new().write(true).open(path)?;
        file.write_all_at(data, offset)
    }

    fn write_slices(&self, path: &Path, writes: WriteSlices<'_>) -> IoResult<()> {
        if writes.is_empty() {
            return Ok(());
        }
        let file = std::fs::OpenOptions::new().write(true).open(path)?;
        use rayon::prelude::*;
        self.in_pool(|| {
            writes
                .as_slice()
                .par_iter()
                .try_for_each(|w| file.write_all_at(w.data, w.offset))
        })
    }

    fn sync_data(&self, path: &Path) -> IoResult<()> {
        std::fs::OpenOptions::new()
            .write(true)
            .open(path)?
            .sync_data()
    }

    fn sync_all(&self, path: &Path) -> IoResult<()> {
        std::fs::OpenOptions::new()
            .write(true)
            .open(path)?
            .sync_all()
    }
}

// ============================================================================
// Tests
// ============================================================================

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{BlockingIo, ByteRange, FileRange, WriteSlice, WriteSlices};
    use tempfile::TempDir;

    fn write_tmp(dir: &TempDir, name: &str, data: &[u8]) -> std::path::PathBuf {
        let path = dir.path().join(name);
        std::fs::write(&path, data).unwrap();
        path
    }

    #[test]
    fn read_file_roundtrip() {
        let dir = TempDir::new().unwrap();
        let data: Vec<u8> = (0u8..=255).cycle().take(4096).collect();
        let path = write_tmp(&dir, "file.bin", &data);

        let result = SyncIo::new().read_file(&path).unwrap();
        assert_eq!(result.as_ref(), &data[..]);
    }

    #[test]
    fn read_file_empty() {
        let dir = TempDir::new().unwrap();
        let path = write_tmp(&dir, "empty.bin", b"");

        let result = SyncIo::new().read_file(&path).unwrap();
        assert!(result.is_empty());
    }

    #[test]
    fn read_range_returns_correct_slice() {
        let dir = TempDir::new().unwrap();
        let data: Vec<u8> = (0u8..100).collect();
        let path = write_tmp(&dir, "range.bin", &data);

        let result = SyncIo::new()
            .read_range(&path, ByteRange::from_offset_len(10, 20).unwrap())
            .unwrap();
        assert_eq!(result.as_ref(), &data[10..30]);
    }

    #[test]
    fn read_range_zero_len() {
        let dir = TempDir::new().unwrap();
        let path = write_tmp(&dir, "z.bin", b"hello");

        let result = SyncIo::new()
            .read_range(&path, ByteRange::from_offset_len(0, 0).unwrap())
            .unwrap();
        assert!(result.is_empty());
    }

    #[test]
    fn read_ranges_empty() {
        let results = SyncIo::new().read_ranges(&[]).unwrap();
        assert!(results.is_empty());
    }

    #[test]
    fn read_ranges_single() {
        let dir = TempDir::new().unwrap();
        let data: Vec<u8> = (0u8..200).collect();
        let path = write_tmp(&dir, "batch.bin", &data);

        let entries = [FileRange::new(
            &path,
            ByteRange::from_offset_len(50, 30).unwrap(),
        )];
        let results = SyncIo::new().read_ranges(&entries).unwrap();

        assert_eq!(results.len(), 1);
        assert_eq!(results[0].data(), &data[50..80]);
    }

    #[test]
    fn read_ranges_multiple_preserves_order() {
        let dir = TempDir::new().unwrap();
        let data: Vec<u8> = (0u8..=255).collect();
        let path = write_tmp(&dir, "multi.bin", &data);

        let entries = [
            FileRange::new(&path, ByteRange::from_offset_len(0, 10).unwrap()),
            FileRange::new(&path, ByteRange::from_offset_len(20, 10).unwrap()),
            FileRange::new(&path, ByteRange::from_offset_len(100, 5).unwrap()),
        ];
        let results = SyncIo::new().read_ranges(&entries).unwrap();

        assert_eq!(results.len(), 3);
        assert_eq!(results[0].data(), &data[0..10]);
        assert_eq!(results[1].data(), &data[20..30]);
        assert_eq!(results[2].data(), &data[100..105]);
    }

    #[test]
    fn write_file_roundtrip() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("out.bin");
        let data = b"hello macos sync";
        SyncIo::new().write_file(&path, data).unwrap();
        SyncIo::new().sync_all(&path).unwrap();
        let result = SyncIo::new().read_file(&path).unwrap();
        assert_eq!(result.as_ref(), data);
    }

    #[test]
    fn write_file_truncates_existing() {
        let dir = TempDir::new().unwrap();
        let path = write_tmp(&dir, "trunc.bin", b"old content here");
        SyncIo::new().write_file(&path, b"new").unwrap();
        let result = SyncIo::new().read_file(&path).unwrap();
        assert_eq!(result.as_ref(), b"new");
    }

    #[test]
    fn write_at_preserves_surrounding_bytes() {
        let dir = TempDir::new().unwrap();
        let mut data = b"AAABBBCCC".to_vec();
        let path = write_tmp(&dir, "patch.bin", &data);
        SyncIo::new().write_at(&path, 3, b"XXX").unwrap();
        data[3..6].copy_from_slice(b"XXX");
        let result = SyncIo::new().read_file(&path).unwrap();
        assert_eq!(result.as_ref(), &data);
    }

    #[test]
    fn write_positioned_file_creates_exact_length() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("pos.bin");
        let writes = [WriteSlice::new(0, b"HELLO"), WriteSlice::new(10, b"WORLD")];
        SyncIo::new()
            .write_positioned_file(&path, 15, WriteSlices::new(&writes).unwrap())
            .unwrap();
        let result = SyncIo::new().read_file(&path).unwrap();
        assert_eq!(result.len(), 15);
        assert_eq!(&result.as_ref()[0..5], b"HELLO");
        assert_eq!(&result.as_ref()[10..15], b"WORLD");
    }

    #[test]
    fn write_slices_batches_into_existing_file() {
        let dir = TempDir::new().unwrap();
        let path = write_tmp(&dir, "batch_write.bin", &[0u8; 20]);
        let writes = [WriteSlice::new(0, b"HELLO"), WriteSlice::new(15, b"WORLD")];
        SyncIo::new()
            .write_slices(&path, WriteSlices::new(&writes).unwrap())
            .unwrap();
        let result = SyncIo::new().read_file(&path).unwrap();
        assert_eq!(&result.as_ref()[0..5], b"HELLO");
        assert_eq!(&result.as_ref()[15..20], b"WORLD");
    }

    #[test]
    fn write_slices_empty_batch_is_noop() {
        let dir = TempDir::new().unwrap();
        let path = write_tmp(&dir, "noop.bin", b"unchanged");
        SyncIo::new()
            .write_slices(&path, WriteSlices::new(&[]).unwrap())
            .unwrap();
        let result = SyncIo::new().read_file(&path).unwrap();
        assert_eq!(result.as_ref(), b"unchanged");
    }

    #[test]
    fn write_slices_rejects_overlap() {
        let writes = [WriteSlice::new(0, b"AAAAA"), WriteSlice::new(3, b"BBBBB")];
        let err = WriteSlices::new(&writes).unwrap_err();
        assert_eq!(err.kind(), std::io::ErrorKind::InvalidInput);
    }

    #[test]
    fn write_positioned_file_rejects_overlap() {
        let writes = [WriteSlice::new(0, b"AAAAA"), WriteSlice::new(3, b"BBBBB")];
        let err = WriteSlices::new(&writes).unwrap_err();
        assert_eq!(err.kind(), std::io::ErrorKind::InvalidInput);
    }

    #[test]
    fn write_positioned_file_empty_batch_creates_file() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("empty_pos.bin");
        SyncIo::new()
            .write_positioned_file(&path, 16, WriteSlices::new(&[]).unwrap())
            .unwrap();
        let meta = std::fs::metadata(&path).unwrap();
        assert_eq!(meta.len(), 16);
    }
}
