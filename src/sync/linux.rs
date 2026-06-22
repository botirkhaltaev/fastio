//! Linux O_DIRECT-aware synchronous storage implementation.

use std::io::{Read, Seek, SeekFrom, Write};
use std::os::unix::fs::FileExt;
use std::path::Path;
use std::sync::Arc;

use super::DirectIo;
use crate::{
    ByteRange, FileRange, IoResult, RangeRead, RequestIndex, WriteSlices,
    buffer::{AlignedBuffer, OwnedBytes},
};

const BLOCK_SIZE: usize = 4096;
const BLOCK_SIZE_U64: u64 = 4096;
const MAX_SINGLE_READ: usize = 512 * 1024 * 1024;

// ============================================================================
// SyncIo
// ============================================================================

/// Synchronous blocking I/O backend (Linux O_DIRECT implementation).
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

    fn round_up_to_block(n: usize) -> usize {
        (n + BLOCK_SIZE - 1) & !(BLOCK_SIZE - 1)
    }

    /// Opens a file for reading, respecting the configured [`DirectIo`] mode.
    ///
    /// Returns the opened file and whether O_DIRECT is active on it.
    fn open_read(&self, path: &Path) -> IoResult<(std::fs::File, bool)> {
        use std::os::unix::fs::OpenOptionsExt;
        match self.options.direct_io {
            DirectIo::Disabled => Ok((std::fs::File::open(path)?, false)),
            DirectIo::Enabled => {
                let file = std::fs::OpenOptions::new()
                    .read(true)
                    .custom_flags(libc::O_DIRECT)
                    .open(path)?;
                Ok((file, true))
            }
            DirectIo::Auto => match std::fs::OpenOptions::new()
                .read(true)
                .custom_flags(libc::O_DIRECT)
                .open(path)
            {
                Ok(f) => Ok((f, true)),
                Err(e) if e.raw_os_error() == Some(libc::EINVAL) => {
                    Ok((std::fs::File::open(path)?, false))
                }
                Err(e) => Err(e),
            },
        }
    }

    fn read_direct(file: &mut std::fs::File, buf: &mut [u8], actual_len: usize) -> IoResult<()> {
        let mut pos = 0;
        while pos < actual_len {
            let n = file.read(&mut buf[pos..])?;
            if n == 0 {
                return Err(std::io::Error::new(
                    std::io::ErrorKind::UnexpectedEof,
                    format!("O_DIRECT short read: got {pos} of {actual_len} bytes"),
                ));
            }
            pos += n;
        }
        Ok(())
    }

    fn load_chunked(&self, path: &Path, chunks: usize) -> IoResult<OwnedBytes> {
        let (file, direct) = self.open_read(path)?;
        let file_size = usize::try_from(file.metadata()?.len())
            .map_err(|_| std::io::Error::other("file too large"))?;
        let raw_chunk = file_size.div_ceil(chunks).max(1);
        let chunk_size = if direct {
            Self::round_up_to_block(raw_chunk)
        } else {
            raw_chunk
        };
        drop(file);

        if direct {
            let aligned_total = Self::round_up_to_block(file_size);
            let mut final_buf = AlignedBuffer::new(aligned_total)?;
            final_buf.set_len(aligned_total);

            // Build (start_in_buf, buf_len, actual_len) metadata before
            // borrowing final_buf as a mutable slice.
            let task_meta: Vec<(usize, usize, usize)> = (0..chunks)
                .filter_map(|i| {
                    let start = i * chunk_size;
                    let end = start
                        .checked_add(chunk_size)
                        .map_or(file_size, |e| e.min(file_size));
                    if start >= end {
                        return None;
                    }
                    let actual_len = end - start;
                    let read_len = Self::round_up_to_block(actual_len);
                    let buf_len = read_len.min(aligned_total - start);
                    Some((start, buf_len, actual_len))
                })
                .collect();

            // Hand disjoint sub-slices to scoped threads via split_at_mut.
            // `std::thread::scope` joins every thread before `final_buf`
            // is accessed again — no unsafe needed.
            let path_ref: &Path = path;
            std::thread::scope(|s| -> IoResult<()> {
                let mut rest: &mut [u8] = final_buf.as_mut_slice();
                let mut consumed = 0usize;
                let mut handles = Vec::with_capacity(task_meta.len());

                for &(start, buf_len, actual_len) in &task_meta {
                    let (_, tail) = rest.split_at_mut(start - consumed);
                    let (chunk_slice, tail2) = tail.split_at_mut(buf_len);
                    rest = tail2;
                    consumed = start + buf_len;

                    let start_off = start as u64;
                    handles.push(s.spawn(move || -> IoResult<()> {
                        let (mut f, _) = self.open_read(path_ref)?;
                        f.seek(SeekFrom::Start(start_off))?;
                        Self::read_direct(&mut f, chunk_slice, actual_len)
                    }));
                }

                for h in handles {
                    h.join()
                        .map_err(|_| std::io::Error::other("thread panicked"))??;
                }
                Ok(())
            })?;

            final_buf.set_len(file_size);
            Ok(OwnedBytes::Aligned(final_buf))
        } else {
            let mut buf = self.options.allocator.alloc(file_size);

            let task_meta: Vec<(usize, usize)> = (0..chunks)
                .filter_map(|i| {
                    let start = i * chunk_size;
                    let end = start
                        .checked_add(chunk_size)
                        .map_or(file_size, |e| e.min(file_size));
                    if start >= end {
                        return None;
                    }
                    Some((start, end - start))
                })
                .collect();

            let path_ref: &Path = path;
            let buf_slice = buf.as_mut_slice().unwrap();
            std::thread::scope(|s| -> IoResult<()> {
                let mut rest: &mut [u8] = buf_slice;
                let mut consumed = 0usize;
                let mut handles = Vec::with_capacity(task_meta.len());

                for &(start, len) in &task_meta {
                    let (_, tail) = rest.split_at_mut(start - consumed);
                    let (chunk_slice, tail2) = tail.split_at_mut(len);
                    rest = tail2;
                    consumed = start + len;

                    let start_off = start as u64;
                    handles.push(s.spawn(move || -> IoResult<()> {
                        let mut f = std::fs::File::open(path_ref)?;
                        f.seek(SeekFrom::Start(start_off))?;
                        f.read_exact(chunk_slice)
                    }));
                }

                for h in handles {
                    h.join()
                        .map_err(|_| std::io::Error::other("thread panicked"))??;
                }
                Ok(())
            })?;

            Ok(buf)
        }
    }
}

impl super::super::BlockingIo for SyncIo {
    fn read_file(&self, path: &Path) -> IoResult<OwnedBytes> {
        let (mut file, direct) = self.open_read(path)?;
        let len = usize::try_from(file.metadata()?.len())
            .map_err(|_| std::io::Error::other("file too large"))?;
        if len == 0 {
            return Ok(OwnedBytes::Vec(Vec::new()));
        }
        if len > MAX_SINGLE_READ {
            let chunks = (len / (128 * 1024 * 1024)).clamp(1, 8);
            return self.load_chunked(path, chunks);
        }
        if direct {
            let aligned_len = Self::round_up_to_block(len);
            let mut buf = AlignedBuffer::new(aligned_len)?;
            buf.set_len(aligned_len);
            Self::read_direct(&mut file, buf.as_mut_slice(), len)?;
            buf.set_len(len);
            Ok(OwnedBytes::Aligned(buf))
        } else {
            let mut buf = self.options.allocator.alloc(len);
            file.read_exact(buf.as_mut_slice().unwrap())?;
            Ok(buf)
        }
    }

    fn read_range(&self, path: &Path, range: ByteRange) -> IoResult<OwnedBytes> {
        if range.is_empty() {
            return Ok(OwnedBytes::Vec(Vec::new()));
        }
        let len = range.len_usize()?;
        let offset = range.start();

        let (mut file, direct) = self.open_read(path)?;

        if direct {
            let head_skip =
                usize::try_from(offset % BLOCK_SIZE_U64).expect("block size fits in usize");
            let aligned_offset = offset - head_skip as u64;
            let aligned_len = Self::round_up_to_block(head_skip + len);
            let mut buf = AlignedBuffer::new(aligned_len)?;
            buf.set_len(aligned_len);
            file.seek(SeekFrom::Start(aligned_offset))?;
            Self::read_direct(&mut file, buf.as_mut_slice(), head_skip + len)?;
            if head_skip == 0 {
                buf.set_len(len);
                return Ok(OwnedBytes::Aligned(buf));
            }
            let slice = &buf.as_slice()[head_skip..head_skip + len];
            Ok(OwnedBytes::Vec(slice.to_vec()))
        } else {
            file.seek(SeekFrom::Start(offset))?;
            let mut buf = self.options.allocator.alloc(len);
            file.read_exact(buf.as_mut_slice().unwrap())?;
            Ok(buf)
        }
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
        let data = b"hello linux sync";
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
        // file is created and set to len even with no writes
        let meta = std::fs::metadata(&path).unwrap();
        assert_eq!(meta.len(), 16);
    }

    #[test]
    fn direct_io_disabled_reads_correctly() {
        let dir = TempDir::new().unwrap();
        let data: Vec<u8> = (0u8..=255).cycle().take(8192).collect();
        let path = write_tmp(&dir, "disabled.bin", &data);
        let opts = super::super::SyncOptions {
            direct_io: super::DirectIo::Disabled,
            ..Default::default()
        };
        let backend = SyncIo::with_options(opts).unwrap();
        let result = backend.read_file(&path).unwrap();
        assert_eq!(result.as_ref(), &data[..]);
    }

    #[test]
    fn direct_io_disabled_read_range() {
        let dir = TempDir::new().unwrap();
        let data: Vec<u8> = (0u8..200).collect();
        let path = write_tmp(&dir, "range_disabled.bin", &data);
        let opts = super::super::SyncOptions {
            direct_io: super::DirectIo::Disabled,
            ..Default::default()
        };
        let backend = SyncIo::with_options(opts).unwrap();
        let result = backend
            .read_range(&path, ByteRange::from_offset_len(10, 50).unwrap())
            .unwrap();
        assert_eq!(result.as_ref(), &data[10..60]);
    }

    #[test]
    fn direct_io_auto_reads_correctly() {
        let dir = TempDir::new().unwrap();
        let data: Vec<u8> = (0u8..=255).cycle().take(8192).collect();
        let path = write_tmp(&dir, "auto.bin", &data);
        let opts = super::super::SyncOptions {
            direct_io: super::DirectIo::Auto,
            ..Default::default()
        };
        let backend = SyncIo::with_options(opts).unwrap();
        let result = backend.read_file(&path).unwrap();
        assert_eq!(result.as_ref(), &data[..]);
    }
}
