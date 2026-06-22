//! Linux io_uring storage implementation with batched I/O.
//!
//! A thread-local ring is cached to amortize the `io_uring_setup` syscall
//! across repeated operations.  Batch operations (`read_ranges`,
//! `write_positioned_file`, `write_slices`) saturate the ring: up to
//! `ring_depth` ops are in-flight at once, completions are drained in a
//! tight loop, and partial results are immediately resubmitted.

use std::cell::RefCell;
use std::fs::{File, OpenOptions};
use std::io::{Error, ErrorKind};
use std::os::fd::AsRawFd;
use std::path::Path;

use io_uring::{IoUring as Uring, opcode, types};

use crate::{
    ByteRange, FileRange, IoResult, RangeRead, RequestIndex, WriteSlice, WriteSlices,
    buffer::OwnedBytes,
};

const DEFAULT_RING_DEPTH: u32 = 256;
const DEFAULT_CHUNK_SIZE: usize = 4 * 1024 * 1024;
/// Maximum single-op transfer size (io_uring length field is u32).
const MAX_IO_LEN: usize = u32::MAX as usize;

// ============================================================================
// Thread-local ring cache
// ============================================================================

thread_local! {
    /// Cached ring per thread — avoids repeated `io_uring_setup` syscalls.
    /// The ring depth is set on first use and stays for the thread's lifetime.
    static CACHED_RING: RefCell<Option<Uring>> = const { RefCell::new(None) };
}

/// Borrow the thread-local ring (creating or resizing it if necessary),
/// run `f`, then return it. If `f` returns an error the ring is still reusable.
fn with_ring<T>(depth: u32, f: impl FnOnce(&mut Uring) -> IoResult<T>) -> IoResult<T> {
    CACHED_RING.with(|cell| {
        let mut borrow = cell.borrow_mut();
        let needs_new = match borrow.as_ref() {
            None => true,
            Some(r) => r.params().sq_entries() < depth,
        };
        if needs_new {
            *borrow = Some(Uring::new(depth).map_err(|err| {
                Error::new(
                    err.kind(),
                    format!("failed to initialize io_uring with depth {depth}: {err}"),
                )
            })?);
        }
        f(borrow.as_mut().unwrap())
    })
}

// ============================================================================
// IoUring
// ============================================================================

/// High-throughput io_uring I/O backend (Linux only).
#[derive(Debug, Clone, Copy, Default)]
pub struct IoUring {
    options: IoUringOptions,
}

/// Options for the io_uring I/O backend.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct IoUringOptions {
    /// Depth of each io_uring instance. Also caps batch in-flight operations.
    pub ring_depth: u32,
    /// Size of each I/O chunk submitted to the ring (default 4 MiB).
    ///
    /// Larger values reduce submission overhead for big files; smaller values
    /// improve concurrency for many small operations.
    pub chunk_size: usize,
    /// Controls how read buffers are allocated.
    pub allocator: crate::buffer::BufferAllocator,
}

impl Default for IoUringOptions {
    fn default() -> Self {
        Self {
            ring_depth: DEFAULT_RING_DEPTH,
            chunk_size: DEFAULT_CHUNK_SIZE,
            allocator: crate::buffer::BufferAllocator::default(),
        }
    }
}

impl IoUring {
    /// Create a new `IoUring` backend with default options.
    #[inline]
    #[must_use]
    pub const fn new() -> Self {
        Self {
            options: IoUringOptions {
                ring_depth: DEFAULT_RING_DEPTH,
                chunk_size: DEFAULT_CHUNK_SIZE,
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

    pub fn with_options(options: IoUringOptions) -> IoResult<Self> {
        if options.ring_depth == 0 {
            return Err(Error::new(
                ErrorKind::InvalidInput,
                "ring_depth must be greater than zero",
            ));
        }
        if options.chunk_size == 0 {
            return Err(Error::new(
                ErrorKind::InvalidInput,
                "chunk_size must be greater than zero",
            ));
        }
        Ok(Self { options })
    }

    #[inline]
    #[must_use]
    pub const fn options(&self) -> &IoUringOptions {
        &self.options
    }

    /// Reads exactly `buf.len()` bytes from `file` at `offset` using the
    /// thread-local ring. Splits the buffer into chunks and keeps up to
    /// `ring_depth` reads in-flight simultaneously.
    fn read_exact_at(&self, file: &File, offset: u64, buf: &mut [u8]) -> IoResult<()> {
        if buf.is_empty() {
            return Ok(());
        }
        let depth = self.options.ring_depth;
        with_ring(depth, |ring| {
            let total = buf.len();
            let chunk_size = self.options.chunk_size.min(total);
            let num_chunks = total.div_ceil(chunk_size);

            let mut done = vec![0usize; num_chunks];
            let mut in_flight: u32 = 0;
            let mut next_submit = 0usize;

            loop {
                while in_flight < depth && next_submit < num_chunks {
                    let idx = next_submit;
                    let chunk_start = idx * chunk_size;
                    let chunk_end = total.min(chunk_start + chunk_size);
                    let chunk_len = chunk_end - chunk_start;
                    let so_far = done[idx];
                    let remaining = chunk_len - so_far;
                    if remaining == 0 {
                        next_submit += 1;
                        continue;
                    }
                    let len = remaining.min(MAX_IO_LEN) as u32;
                    let buf_offset = chunk_start + so_far;
                    // SAFETY: `buf_offset < total` since `chunk_start + so_far <
                    // chunk_end <= total`. The ring holds a reference to this
                    // memory until the CQE is consumed below — `buf` outlives
                    // the ring.
                    let ptr = unsafe { buf.as_mut_ptr().add(buf_offset) };
                    let file_offset = offset + buf_offset as u64;
                    let entry = opcode::Read::new(types::Fd(file.as_raw_fd()), ptr, len)
                        .offset(file_offset)
                        .build()
                        .user_data(idx as u64);
                    {
                        let mut sq = ring.submission();
                        unsafe {
                            sq.push(&entry)
                                .map_err(|_| Error::other("io_uring SQ full"))?;
                        }
                    }
                    in_flight += 1;
                    next_submit += 1;
                }

                if in_flight == 0 {
                    break;
                }

                ring.submit_and_wait(1)?;

                let cq: Vec<_> = ring.completion().collect();
                for cqe in cq {
                    in_flight -= 1;
                    let idx = cqe.user_data() as usize;
                    let result = cqe.result();
                    if result < 0 {
                        return Err(Error::from_raw_os_error(-result));
                    }
                    let n_read = result as usize;
                    if n_read == 0 {
                        return Err(Error::new(ErrorKind::UnexpectedEof, "short io_uring read"));
                    }
                    done[idx] += n_read;
                    let chunk_start = idx * chunk_size;
                    let chunk_end = total.min(chunk_start + chunk_size);
                    let chunk_len = chunk_end - chunk_start;
                    if done[idx] < chunk_len && next_submit > idx {
                        next_submit = idx;
                    }
                }
            }

            Ok(())
        })
    }

    /// Writes exactly `data.len()` bytes to `file` at `offset` using the
    /// thread-local ring with multiple in-flight chunks.
    fn write_exact_at(&self, file: &File, offset: u64, data: &[u8]) -> IoResult<()> {
        if data.is_empty() {
            return Ok(());
        }
        let depth = self.options.ring_depth;
        with_ring(depth, |ring| {
            let total = data.len();
            let chunk_size = self.options.chunk_size.min(total);
            let num_chunks = total.div_ceil(chunk_size);

            let mut done = vec![0usize; num_chunks];
            let mut in_flight: u32 = 0;
            let mut next_submit = 0usize;

            loop {
                while in_flight < depth && next_submit < num_chunks {
                    let idx = next_submit;
                    let chunk_start = idx * chunk_size;
                    let chunk_end = total.min(chunk_start + chunk_size);
                    let chunk_len = chunk_end - chunk_start;
                    let so_far = done[idx];
                    let remaining = chunk_len - so_far;
                    if remaining == 0 {
                        next_submit += 1;
                        continue;
                    }
                    let len = remaining.min(MAX_IO_LEN) as u32;
                    let buf_offset = chunk_start + so_far;
                    // SAFETY: `buf_offset < total` and `data` outlives the ring.
                    let ptr = unsafe { data.as_ptr().add(buf_offset) };
                    let file_offset = offset + buf_offset as u64;
                    let entry = opcode::Write::new(types::Fd(file.as_raw_fd()), ptr, len)
                        .offset(file_offset)
                        .build()
                        .user_data(idx as u64);
                    {
                        let mut sq = ring.submission();
                        unsafe {
                            sq.push(&entry)
                                .map_err(|_| Error::other("io_uring SQ full"))?;
                        }
                    }
                    in_flight += 1;
                    next_submit += 1;
                }

                if in_flight == 0 {
                    break;
                }

                ring.submit_and_wait(1)?;

                let cq: Vec<_> = ring.completion().collect();
                for cqe in cq {
                    in_flight -= 1;
                    let idx = cqe.user_data() as usize;
                    let result = cqe.result();
                    if result < 0 {
                        return Err(Error::from_raw_os_error(-result));
                    }
                    let n_written = result as usize;
                    if n_written == 0 {
                        return Err(Error::new(ErrorKind::WriteZero, "short io_uring write"));
                    }
                    done[idx] += n_written;
                    let chunk_start = idx * chunk_size;
                    let chunk_end = total.min(chunk_start + chunk_size);
                    let chunk_len = chunk_end - chunk_start;
                    if done[idx] < chunk_len && next_submit > idx {
                        next_submit = idx;
                    }
                }
            }

            Ok(())
        })
    }

    /// Writes every slice in `writes` to `file` using a saturated io_uring ring.
    ///
    /// Submits up to `ring_depth` ops simultaneously.  Partial writes are
    /// resubmitted immediately.  `writes` must be non-overlapping (guaranteed
    /// by [`WriteSlices`]).
    fn ring_batch_writes(&self, file: &File, writes: &[WriteSlice<'_>]) -> IoResult<()> {
        if writes.is_empty() {
            return Ok(());
        }
        let depth = self.options.ring_depth;
        with_ring(depth, |ring| {
            let n = writes.len();
            let mut done = vec![0usize; n];
            let mut in_flight: u32 = 0;
            let mut next_submit = 0usize;

            loop {
                while in_flight < depth && next_submit < n {
                    let idx = next_submit;
                    let w = &writes[idx];
                    let so_far = done[idx];
                    let remaining = w.data.len() - so_far;
                    if remaining == 0 {
                        next_submit += 1;
                        continue;
                    }
                    let len = remaining.min(MAX_IO_LEN) as u32;
                    // SAFETY: `w.data` is a shared slice with lifetime tied to
                    // the caller's `WriteSlice`. The ring holds a reference to
                    // this memory until the CQE is consumed below — `writes`
                    // outlives the ring borrow.
                    let ptr = unsafe { w.data.as_ptr().add(so_far) };
                    let entry = opcode::Write::new(types::Fd(file.as_raw_fd()), ptr, len)
                        .offset(w.offset + so_far as u64)
                        .build()
                        .user_data(idx as u64);
                    {
                        let mut sq = ring.submission();
                        unsafe {
                            sq.push(&entry)
                                .map_err(|_| Error::other("io_uring SQ full"))?;
                        }
                    }
                    in_flight += 1;
                    next_submit += 1;
                }

                if in_flight == 0 {
                    break;
                }

                ring.submit_and_wait(1)?;

                let cq: Vec<_> = ring.completion().collect();
                for cqe in cq {
                    in_flight -= 1;
                    let idx = cqe.user_data() as usize;
                    let result = cqe.result();
                    if result < 0 {
                        return Err(Error::from_raw_os_error(-result));
                    }
                    let n_written = result as usize;
                    if n_written == 0 {
                        return Err(Error::new(ErrorKind::WriteZero, "short io_uring write"));
                    }
                    done[idx] += n_written;
                    if done[idx] < writes[idx].data.len() && next_submit > idx {
                        next_submit = idx;
                    }
                }
            }

            Ok(())
        })
    }
}

impl super::BlockingIo for IoUring {
    fn read_file(&self, path: &Path) -> IoResult<OwnedBytes> {
        let file = OpenOptions::new().read(true).open(path)?;
        let len =
            usize::try_from(file.metadata()?.len()).map_err(|_| Error::other("file too large"))?;
        if len == 0 {
            return Ok(OwnedBytes::Vec(Vec::new()));
        }
        let mut buf = self.options.allocator.alloc(len);
        self.read_exact_at(&file, 0, buf.as_mut_slice().unwrap())?;
        Ok(buf)
    }

    fn read_range(&self, path: &Path, range: ByteRange) -> IoResult<OwnedBytes> {
        if range.is_empty() {
            return Ok(OwnedBytes::Vec(Vec::new()));
        }
        let file = OpenOptions::new().read(true).open(path)?;
        let len = range.len_usize()?;
        let mut buf = self.options.allocator.alloc(len);
        self.read_exact_at(&file, range.start(), buf.as_mut_slice().unwrap())?;
        Ok(buf)
    }

    fn read_ranges(&self, ranges: &[FileRange<'_>]) -> IoResult<Vec<RangeRead>> {
        if ranges.is_empty() {
            return Ok(Vec::new());
        }
        let n = ranges.len();
        let mut bufs: Vec<_> = ranges
            .iter()
            .map(|e| {
                e.range
                    .len_usize()
                    .map(|len| self.options.allocator.alloc(len))
            })
            .collect::<IoResult<_>>()?;
        let files: Vec<File> = ranges
            .iter()
            .map(|e| OpenOptions::new().read(true).open(e.path))
            .collect::<IoResult<_>>()?;

        let mut done = vec![0usize; n];
        let depth = self.options.ring_depth;

        with_ring(depth, |ring| {
            let mut in_flight: u32 = 0;
            let mut next_submit = 0usize;

            loop {
                while in_flight < depth && next_submit < n {
                    let idx = next_submit;
                    let range = ranges[idx].range;
                    let so_far = done[idx];
                    let remaining = range.len_usize()? - so_far;
                    if remaining == 0 {
                        next_submit += 1;
                        continue;
                    }
                    let len = remaining.min(MAX_IO_LEN) as u32;
                    // SAFETY: `bufs[idx]` is allocated with `range.len()` bytes.
                    // The ring holds a reference until the CQE is consumed —
                    // `bufs` outlives the ring borrow.
                    let ptr = unsafe { bufs[idx].as_mut_slice().unwrap().as_mut_ptr().add(so_far) };
                    let entry = opcode::Read::new(types::Fd(files[idx].as_raw_fd()), ptr, len)
                        .offset(range.start() + so_far as u64)
                        .build()
                        .user_data(idx as u64);
                    {
                        let mut sq = ring.submission();
                        unsafe {
                            sq.push(&entry)
                                .map_err(|_| Error::other("io_uring SQ full"))?;
                        }
                    }
                    in_flight += 1;
                    next_submit += 1;
                }

                if in_flight == 0 {
                    break;
                }

                ring.submit_and_wait(1)?;

                let cq: Vec<_> = ring.completion().collect();
                for cqe in cq {
                    in_flight -= 1;
                    let idx = cqe.user_data() as usize;
                    let result = cqe.result();
                    if result < 0 {
                        return Err(Error::from_raw_os_error(-result));
                    }
                    let n_read = result as usize;
                    if n_read == 0 {
                        return Err(Error::new(ErrorKind::UnexpectedEof, "short io_uring read"));
                    }
                    done[idx] += n_read;
                    let range = ranges[idx].range;
                    if done[idx] < range.len_usize()? && next_submit > idx {
                        next_submit = idx;
                    }
                }
            }

            Ok(())
        })?;

        let results = bufs
            .into_iter()
            .enumerate()
            .map(|(i, buf)| RangeRead {
                request_index: RequestIndex::new(i),
                range: ranges[i].range,
                bytes: buf,
            })
            .collect();
        Ok(results)
    }

    fn write_file(&self, path: &Path, data: &[u8]) -> IoResult<()> {
        let file = OpenOptions::new()
            .write(true)
            .create(true)
            .truncate(true)
            .open(path)?;
        self.write_exact_at(&file, 0, data)
    }

    fn write_positioned_file(
        &self,
        path: &Path,
        len: u64,
        writes: WriteSlices<'_>,
    ) -> IoResult<()> {
        let file = OpenOptions::new()
            .write(true)
            .create(true)
            .truncate(true)
            .open(path)?;
        file.set_len(len)?;
        self.ring_batch_writes(&file, writes.as_slice())
    }

    fn write_at(&self, path: &Path, offset: u64, data: &[u8]) -> IoResult<()> {
        let file = OpenOptions::new().write(true).open(path)?;
        self.write_exact_at(&file, offset, data)
    }

    fn write_slices(&self, path: &Path, writes: WriteSlices<'_>) -> IoResult<()> {
        if writes.is_empty() {
            return Ok(());
        }
        let file = OpenOptions::new().write(true).open(path)?;
        self.ring_batch_writes(&file, writes.as_slice())
    }

    fn sync_data(&self, path: &Path) -> IoResult<()> {
        OpenOptions::new().write(true).open(path)?.sync_data()
    }

    fn sync_all(&self, path: &Path) -> IoResult<()> {
        OpenOptions::new().write(true).open(path)?.sync_all()
    }
}

// ============================================================================
// Tests
// ============================================================================

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{BlockingIo, ByteRange, FileRange, WriteSlices};
    use tempfile::TempDir;

    fn write_tmp(dir: &TempDir, name: &str, data: &[u8]) -> std::path::PathBuf {
        let path = dir.path().join(name);
        std::fs::write(&path, data).unwrap();
        path
    }

    fn is_io_uring_init_error(err: &std::io::Error) -> bool {
        err.to_string().contains("failed to initialize io_uring")
    }

    fn skip_if_unavailable() -> bool {
        Uring::new(1).is_err()
    }

    #[test]
    fn read_file_roundtrip() {
        let dir = TempDir::new().unwrap();
        let data: Vec<u8> = (0u8..=255).cycle().take(4096).collect();
        let path = write_tmp(&dir, "file.bin", &data);

        let result = match IoUring::new().read_file(&path) {
            Ok(result) => result,
            Err(err) if is_io_uring_init_error(&err) => return,
            Err(err) => panic!("read_file failed: {err}"),
        };
        assert_eq!(result.as_ref(), &data[..]);
    }

    #[test]
    fn read_file_empty() {
        let dir = TempDir::new().unwrap();
        let path = write_tmp(&dir, "empty.bin", b"");

        let result = IoUring::new().read_file(&path).unwrap();
        assert!(result.is_empty());
    }

    #[test]
    fn read_range_returns_correct_slice() {
        if skip_if_unavailable() {
            return;
        }
        let dir = TempDir::new().unwrap();
        let data: Vec<u8> = (0u8..100).collect();
        let path = write_tmp(&dir, "range.bin", &data);

        let result = IoUring::new()
            .read_range(&path, ByteRange::from_offset_len(10, 20).unwrap())
            .unwrap();
        assert_eq!(result.as_ref(), &data[10..30]);
    }

    #[test]
    fn read_range_zero_len() {
        if skip_if_unavailable() {
            return;
        }
        let dir = TempDir::new().unwrap();
        let path = write_tmp(&dir, "z.bin", b"hello");

        let result = IoUring::new()
            .read_range(&path, ByteRange::from_offset_len(0, 0).unwrap())
            .unwrap();
        assert!(result.is_empty());
    }

    #[test]
    fn read_ranges_empty() {
        if skip_if_unavailable() {
            return;
        }
        let results = IoUring::new().read_ranges(&[]).unwrap();
        assert!(results.is_empty());
    }

    #[test]
    fn read_ranges_single() {
        if skip_if_unavailable() {
            return;
        }
        let dir = TempDir::new().unwrap();
        let data: Vec<u8> = (0u8..200).collect();
        let path = write_tmp(&dir, "batch.bin", &data);

        let entries = [FileRange::new(
            &path,
            ByteRange::from_offset_len(50, 30).unwrap(),
        )];
        let results = IoUring::new().read_ranges(&entries).unwrap();

        assert_eq!(results.len(), 1);
        assert_eq!(results[0].data(), &data[50..80]);
    }

    #[test]
    fn read_ranges_multiple_preserves_order() {
        if skip_if_unavailable() {
            return;
        }
        let dir = TempDir::new().unwrap();
        let data: Vec<u8> = (0u8..=255).collect();
        let path = write_tmp(&dir, "multi.bin", &data);

        let entries = [
            FileRange::new(&path, ByteRange::from_offset_len(0, 10).unwrap()),
            FileRange::new(&path, ByteRange::from_offset_len(20, 10).unwrap()),
            FileRange::new(&path, ByteRange::from_offset_len(100, 5).unwrap()),
        ];
        let results = IoUring::new().read_ranges(&entries).unwrap();

        assert_eq!(results.len(), 3);
        assert_eq!(results[0].data(), &data[0..10]);
        assert_eq!(results[1].data(), &data[20..30]);
        assert_eq!(results[2].data(), &data[100..105]);
    }

    #[test]
    fn write_file_roundtrip() {
        if skip_if_unavailable() {
            return;
        }
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("out.bin");
        let data = b"hello io_uring write";
        IoUring::new().write_file(&path, data).unwrap();
        IoUring::new().sync_all(&path).unwrap();
        let result = IoUring::new().read_file(&path).unwrap();
        assert_eq!(result.as_ref(), data);
    }

    #[test]
    fn write_positioned_file_creates_exact_length() {
        if skip_if_unavailable() {
            return;
        }
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("pos.bin");
        let writes = [WriteSlice::new(0, b"HELLO"), WriteSlice::new(10, b"WORLD")];
        IoUring::new()
            .write_positioned_file(&path, 15, WriteSlices::new(&writes).unwrap())
            .unwrap();
        let result = IoUring::new().read_file(&path).unwrap();
        assert_eq!(result.len(), 15);
        assert_eq!(&result.as_ref()[0..5], b"HELLO");
        assert_eq!(&result.as_ref()[10..15], b"WORLD");
    }

    #[test]
    fn write_positioned_file_empty_batch_creates_file() {
        if skip_if_unavailable() {
            return;
        }
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("empty_pos.bin");
        IoUring::new()
            .write_positioned_file(&path, 16, WriteSlices::new(&[]).unwrap())
            .unwrap();
        let meta = std::fs::metadata(&path).unwrap();
        assert_eq!(meta.len(), 16);
    }

    #[test]
    fn write_slices_batches_into_existing_file() {
        if skip_if_unavailable() {
            return;
        }
        let dir = TempDir::new().unwrap();
        let path = write_tmp(&dir, "batch_write.bin", &[0u8; 20]);
        let writes = [WriteSlice::new(0, b"HELLO"), WriteSlice::new(15, b"WORLD")];
        IoUring::new()
            .write_slices(&path, WriteSlices::new(&writes).unwrap())
            .unwrap();
        let result = IoUring::new().read_file(&path).unwrap();
        assert_eq!(&result.as_ref()[0..5], b"HELLO");
        assert_eq!(&result.as_ref()[15..20], b"WORLD");
    }

    #[test]
    fn write_slices_empty_batch_is_noop() {
        if skip_if_unavailable() {
            return;
        }
        let dir = TempDir::new().unwrap();
        let path = write_tmp(&dir, "noop.bin", b"unchanged");
        IoUring::new()
            .write_slices(&path, WriteSlices::new(&[]).unwrap())
            .unwrap();
        let result = IoUring::new().read_file(&path).unwrap();
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
    fn read_ranges_batch_preserves_all_request_indexes() {
        if skip_if_unavailable() {
            return;
        }
        let dir = TempDir::new().unwrap();
        let data: Vec<u8> = (0u8..=255).cycle().take(512).collect();
        let path = write_tmp(&dir, "big.bin", &data);

        let entries: Vec<FileRange<'_>> = (0..8)
            .map(|i| FileRange::new(&path, ByteRange::from_offset_len(i * 32, 32).unwrap()))
            .collect();
        let results = IoUring::new().read_ranges(&entries).unwrap();

        assert_eq!(results.len(), 8);
        for (i, r) in results.iter().enumerate() {
            assert_eq!(
                r.request_index,
                RequestIndex::new(i),
                "request_index mismatch at slot {i}"
            );
            let start = i * 32;
            assert_eq!(r.data(), &data[start..start + 32]);
        }
    }

    #[test]
    fn custom_chunk_size_reads_correctly() {
        if skip_if_unavailable() {
            return;
        }
        let dir = TempDir::new().unwrap();
        let data: Vec<u8> = (0u8..=255).cycle().take(32768).collect();
        let path = write_tmp(&dir, "chunk.bin", &data);
        let opts = IoUringOptions {
            chunk_size: 4096,
            ..Default::default()
        };
        let backend = IoUring::with_options(opts).unwrap();
        let result = backend.read_file(&path).unwrap();
        assert_eq!(result.as_ref(), &data[..]);
    }

    #[test]
    fn with_options_rejects_zero_chunk_size() {
        let opts = IoUringOptions {
            chunk_size: 0,
            ..Default::default()
        };
        assert!(IoUring::with_options(opts).is_err());
    }
}
