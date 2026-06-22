//! Tokio async I/O backend.
//!
//! [`Tokio`] implements [`AsyncIo`] using direct `tokio::fs` I/O.
//! Batch methods use bounded concurrency configured by [`TokioOptions`].
//!
//! [`AsyncIo`]: crate::AsyncIo

use std::path::{Path, PathBuf};

use futures::StreamExt;

use crate::{
    AsyncIo, ByteRange, FileRange, IoResult, RangeRead, RequestIndex, WriteSlices,
    buffer::OwnedBytes,
};

#[cfg(not(any(target_os = "linux", target_os = "macos", target_os = "windows")))]
compile_error!("fastio tokio supports Linux, macOS, and Windows only");

const DEFAULT_BATCH_CONCURRENCY: usize = 64;

/// Options for the Tokio async I/O backend.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TokioOptions {
    /// Maximum number of concurrent tasks for batch reads/writes.
    pub batch_concurrency: usize,
    /// Controls how read buffers are allocated.
    pub allocator: crate::buffer::BufferAllocator,
}

impl Default for TokioOptions {
    fn default() -> Self {
        Self {
            batch_concurrency: DEFAULT_BATCH_CONCURRENCY,
            allocator: crate::buffer::BufferAllocator::default(),
        }
    }
}

/// Tokio async I/O backend.
#[derive(Debug, Clone, Copy, Default)]
pub struct Tokio {
    options: TokioOptions,
}

impl Tokio {
    #[inline]
    #[must_use]
    pub const fn new() -> Self {
        Self {
            options: TokioOptions {
                batch_concurrency: DEFAULT_BATCH_CONCURRENCY,
                allocator: Self::DEFAULT_ALLOCATOR,
            },
        }
    }

    #[cfg(feature = "pool")]
    const DEFAULT_ALLOCATOR: crate::buffer::BufferAllocator =
        crate::buffer::BufferAllocator::Pooled(crate::buffer::PoolConfig {
            num_shards: 8,
            tls_cache_size: 4,
            max_per_shard: 32,
            min_buffer_size: 1024 * 1024,
        });
    #[cfg(not(feature = "pool"))]
    const DEFAULT_ALLOCATOR: crate::buffer::BufferAllocator =
        crate::buffer::BufferAllocator::System;

    pub fn with_options(options: TokioOptions) -> IoResult<Self> {
        if options.batch_concurrency == 0 {
            return Err(std::io::Error::new(
                std::io::ErrorKind::InvalidInput,
                "batch_concurrency must be greater than zero",
            ));
        }
        Ok(Self { options })
    }

    #[inline]
    #[must_use]
    pub const fn options(&self) -> &TokioOptions {
        &self.options
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

impl AsyncIo for Tokio {
    async fn read_file(&self, path: &Path) -> IoResult<OwnedBytes> {
        let path = path.to_path_buf();
        let allocator = self.options.allocator;
        ::tokio::task::spawn_blocking(move || {
            let meta = std::fs::metadata(&path)?;
            let len = usize::try_from(meta.len()).map_err(|_| {
                std::io::Error::new(std::io::ErrorKind::InvalidData, "file too large")
            })?;
            if len == 0 {
                return Ok(OwnedBytes::Vec(Vec::new()));
            }
            let file = std::fs::File::open(&path)?;
            let mut buf = allocator.alloc(len);
            read_at_positioned(&file, 0, buf.as_mut_slice().unwrap())?;
            Ok(buf)
        })
        .await
        .map_err(std::io::Error::other)?
    }

    async fn read_range(&self, path: &Path, range: ByteRange) -> IoResult<OwnedBytes> {
        if range.is_empty() {
            return Ok(OwnedBytes::Vec(Vec::new()));
        }
        let path = path.to_path_buf();
        let len = range.len_usize()?;
        let start = range.start();
        let allocator = self.options.allocator;
        ::tokio::task::spawn_blocking(move || {
            let file = std::fs::File::open(&path)?;
            let mut buf = allocator.alloc(len);
            read_at_positioned(&file, start, buf.as_mut_slice().unwrap())?;
            Ok(buf)
        })
        .await
        .map_err(std::io::Error::other)?
    }

    async fn read_ranges(&self, ranges: &[FileRange<'_>]) -> IoResult<Vec<RangeRead>> {
        if ranges.is_empty() {
            return Ok(Vec::new());
        }
        let tasks: Vec<(RequestIndex, PathBuf, ByteRange)> = ranges
            .iter()
            .enumerate()
            .map(|(i, e)| (RequestIndex::new(i), e.path.to_path_buf(), e.range))
            .collect();

        let allocator = self.options.allocator;
        let stream = futures::stream::iter(tasks).map(|(request_index, path, range)| async move {
            let len = range.len_usize()?;
            let start = range.start();
            ::tokio::task::spawn_blocking(move || {
                let file = std::fs::File::open(&path)?;
                let mut buf = allocator.alloc(len);
                read_at_positioned(&file, start, buf.as_mut_slice().unwrap())?;
                Ok::<RangeRead, std::io::Error>(RangeRead {
                    request_index,
                    range,
                    bytes: buf,
                })
            })
            .await
            .map_err(std::io::Error::other)?
        });

        let mut results: Vec<RangeRead> = stream
            .buffer_unordered(self.options.batch_concurrency)
            .collect::<Vec<IoResult<RangeRead>>>()
            .await
            .into_iter()
            .collect::<IoResult<Vec<RangeRead>>>()?;
        results.sort_unstable_by_key(|r| r.request_index);
        Ok(results)
    }

    async fn write_file(&self, path: &Path, data: &[u8]) -> IoResult<()> {
        ::tokio::fs::write(path, data).await
    }

    async fn write_positioned_file(
        &self,
        path: &Path,
        len: u64,
        writes: WriteSlices<'_>,
    ) -> IoResult<()> {
        let slices = writes.as_slice();
        ::tokio::task::block_in_place(|| {
            let file = std::fs::OpenOptions::new()
                .write(true)
                .create(true)
                .truncate(true)
                .open(path)?;
            file.set_len(len)?;
            if slices.is_empty() {
                return Ok(());
            }
            std::thread::scope(|scope| {
                let handles = slices
                    .iter()
                    .map(|w| scope.spawn(|| write_at_positioned(&file, w.offset, w.data)))
                    .collect::<Vec<_>>();
                for handle in handles {
                    handle
                        .join()
                        .map_err(|_| std::io::Error::other("positioned write worker panicked"))??;
                }
                Ok(())
            })
        })
    }

    async fn write_at(&self, path: &Path, offset: u64, data: &[u8]) -> IoResult<()> {
        ::tokio::task::block_in_place(|| {
            let file = std::fs::OpenOptions::new().write(true).open(path)?;
            write_at_positioned(&file, offset, data)
        })
    }

    async fn write_slices(&self, path: &Path, writes: WriteSlices<'_>) -> IoResult<()> {
        if writes.is_empty() {
            return Ok(());
        }
        let slices = writes.as_slice();
        ::tokio::task::block_in_place(|| {
            let file = std::fs::OpenOptions::new().write(true).open(path)?;
            std::thread::scope(|scope| {
                let handles = slices
                    .iter()
                    .map(|w| scope.spawn(|| write_at_positioned(&file, w.offset, w.data)))
                    .collect::<Vec<_>>();
                for handle in handles {
                    handle
                        .join()
                        .map_err(|_| std::io::Error::other("positioned write worker panicked"))??;
                }
                Ok(())
            })
        })
    }

    async fn sync_data(&self, path: &Path) -> IoResult<()> {
        let path = path.to_path_buf();
        ::tokio::task::spawn_blocking(move || {
            std::fs::OpenOptions::new()
                .write(true)
                .open(&path)?
                .sync_data()
        })
        .await
        .map_err(std::io::Error::other)?
    }

    async fn sync_all(&self, path: &Path) -> IoResult<()> {
        let path = path.to_path_buf();
        ::tokio::task::spawn_blocking(move || {
            std::fs::OpenOptions::new()
                .write(true)
                .open(&path)?
                .sync_all()
        })
        .await
        .map_err(std::io::Error::other)?
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_utils::run_async;
    use crate::{AsyncIo, ByteRange, FileRange, WriteSlice, WriteSlices};
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
        let result = run_async(Tokio::new().read_file(&path)).unwrap();
        assert_eq!(result.as_ref(), &data[..]);
    }

    #[test]
    fn read_file_empty() {
        let dir = TempDir::new().unwrap();
        let path = write_tmp(&dir, "empty.bin", b"");
        let result = run_async(Tokio::new().read_file(&path)).unwrap();
        assert!(result.is_empty());
    }

    #[test]
    fn read_range_returns_correct_slice() {
        let dir = TempDir::new().unwrap();
        let path = write_tmp(&dir, "data.bin", b"hello world");
        let range = ByteRange::new(6, 11).unwrap();
        let result = run_async(Tokio::new().read_range(&path, range)).unwrap();
        assert_eq!(result.as_ref(), b"world");
    }

    #[test]
    fn read_range_zero_len() {
        let dir = TempDir::new().unwrap();
        let path = write_tmp(&dir, "data.bin", b"hello");
        let range = ByteRange::from_offset_len(0, 0).unwrap();
        let result = run_async(Tokio::new().read_range(&path, range)).unwrap();
        assert!(result.is_empty());
    }

    #[test]
    fn read_ranges_multiple_preserves_order() {
        let dir = TempDir::new().unwrap();
        let path = write_tmp(&dir, "data.bin", b"abcdefghij");
        let r0 = ByteRange::new(0, 3).unwrap();
        let r1 = ByteRange::new(3, 6).unwrap();
        let r2 = ByteRange::new(6, 9).unwrap();
        let ranges = vec![
            FileRange::new(&path, r0),
            FileRange::new(&path, r1),
            FileRange::new(&path, r2),
        ];
        let results = run_async(Tokio::new().read_ranges(&ranges)).unwrap();
        assert_eq!(results.len(), 3);
        assert_eq!(results[0].data(), b"abc");
        assert_eq!(results[1].data(), b"def");
        assert_eq!(results[2].data(), b"ghi");
    }

    #[test]
    fn write_file_roundtrip() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("out.bin");
        run_async(Tokio::new().write_file(&path, b"test data")).unwrap();
        let result = run_async(Tokio::new().read_file(&path)).unwrap();
        assert_eq!(result.as_ref(), b"test data");
    }

    #[test]
    fn write_file_truncates_existing() {
        let dir = TempDir::new().unwrap();
        let path = write_tmp(&dir, "existing.bin", b"old data here");
        run_async(Tokio::new().write_file(&path, b"new")).unwrap();
        let result = run_async(Tokio::new().read_file(&path)).unwrap();
        assert_eq!(result.as_ref(), b"new");
    }

    #[test]
    fn write_at_preserves_surrounding_bytes() {
        let dir = TempDir::new().unwrap();
        let path = write_tmp(&dir, "data.bin", b"hello world");
        run_async(Tokio::new().write_at(&path, 6, b"rust!")).unwrap();
        let result = run_async(Tokio::new().read_file(&path)).unwrap();
        assert_eq!(result.as_ref(), b"hello rust!");
    }

    #[test]
    fn write_slices_batches_into_existing_file() {
        let dir = TempDir::new().unwrap();
        let path = write_tmp(&dir, "data.bin", b"----------");
        let slices = vec![WriteSlice::new(0, b"AB"), WriteSlice::new(8, b"CD")];
        let ws = WriteSlices::new(&slices).unwrap();
        run_async(Tokio::new().write_slices(&path, ws)).unwrap();
        let result = run_async(Tokio::new().read_file(&path)).unwrap();
        assert_eq!(result.as_ref(), b"AB------CD");
    }

    #[test]
    fn write_slices_empty_batch_is_noop() {
        let dir = TempDir::new().unwrap();
        let path = write_tmp(&dir, "data.bin", b"unchanged");
        let ws = WriteSlices::new(&[]).unwrap();
        run_async(Tokio::new().write_slices(&path, ws)).unwrap();
        let result = run_async(Tokio::new().read_file(&path)).unwrap();
        assert_eq!(result.as_ref(), b"unchanged");
    }

    #[test]
    fn write_positioned_file_creates_exact_length() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("positioned.bin");
        let ws = WriteSlices::new(&[]).unwrap();
        run_async(Tokio::new().write_positioned_file(&path, 16, ws)).unwrap();
        let result = run_async(Tokio::new().read_file(&path)).unwrap();
        assert_eq!(result.as_ref().len(), 16);
    }

    #[test]
    fn write_positioned_file_empty_batch_creates_file() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("new.bin");
        let ws = WriteSlices::new(&[]).unwrap();
        run_async(Tokio::new().write_positioned_file(&path, 0, ws)).unwrap();
        assert!(path.exists());
    }
}
