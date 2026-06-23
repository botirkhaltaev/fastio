//! Linux `io_uring` file I/O.
//!
//! This module mirrors `std::fs` where behavior matches. Cursor-based
//! [`Read`], [`Write`], and [`Seek`] operations and explicit positioned methods
//! are backed by a thread-local `io_uring` instance. Append mode is not
//! supported; use explicit offsets for writes that must target a known position.

use std::cell::RefCell;
use std::fs::{Metadata, Permissions};
use std::io::{self, Error, ErrorKind, Read, Seek, SeekFrom, Write};
use std::os::fd::AsRawFd;
use std::path::Path;
use std::sync::{Arc, Mutex};

use io_uring::{IoUring, opcode, types};

use crate::{Allocator, DefaultAllocator, OwnedBytes, WriteSlice, WriteSlices};

const DEFAULT_RING_DEPTH: u32 = 256;
const DEFAULT_CHUNK_SIZE: usize = 4 * 1024 * 1024;
const MAX_IO_LEN: usize = u32::MAX as usize;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum SubmissionState {
    Idle,
    InFlight,
    Done,
}

thread_local! {
    static CACHED_RING: RefCell<Option<IoUring>> = const { RefCell::new(None) };
}

/// A Linux `io_uring` file handle.
#[derive(Debug)]
pub struct File<A = DefaultAllocator> {
    inner: std::fs::File,
    ring_depth: u32,
    chunk_size: usize,
    position: Arc<Mutex<u64>>,
    allocator: A,
}

impl File<DefaultAllocator> {
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
}

impl<A: Allocator> File<A> {
    /// Creates a new `File` instance sharing the same underlying handle.
    pub fn try_clone(&self) -> io::Result<Self> {
        Ok(Self {
            inner: self.inner.try_clone()?,
            ring_depth: self.ring_depth,
            chunk_size: self.chunk_size,
            position: Arc::clone(&self.position),
            allocator: self.allocator.clone(),
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
        let mut bytes = self.allocator.allocate(len);
        let buf = bytes
            .as_mut_slice()
            .ok_or_else(|| io::Error::other("allocator returned immutable buffer"))?;
        if buf.len() != len {
            return Err(io::Error::other("allocator returned wrong-sized buffer"));
        }
        self.submit_read_exact_at(&self.inner, 0, buf)?;
        Ok(bytes)
    }

    /// Reads `len` bytes at `offset` into a new buffer.
    pub fn read_at(&self, offset: u64, len: usize) -> io::Result<OwnedBytes> {
        if len == 0 {
            return Ok(OwnedBytes::Vec(Vec::new()));
        }
        let mut bytes = self.allocator.allocate(len);
        let buf = bytes
            .as_mut_slice()
            .ok_or_else(|| io::Error::other("allocator returned immutable buffer"))?;
        if buf.len() != len {
            return Err(io::Error::other("allocator returned wrong-sized buffer"));
        }
        self.read_exact_at(offset, buf)?;
        Ok(bytes)
    }

    /// Reads exactly enough bytes to fill `buf` at `offset`.
    pub fn read_exact_at(&self, offset: u64, buf: &mut [u8]) -> io::Result<()> {
        self.submit_read_exact_at(&self.inner, offset, buf)
    }

    /// Writes all bytes from `buf` at `offset`.
    pub fn write_all_at(&self, offset: u64, buf: &[u8]) -> io::Result<()> {
        self.submit_write_exact_at(&self.inner, offset, buf)
    }

    /// Writes non-overlapping slices at their offsets.
    pub fn write_slices_at(&self, writes: WriteSlices<'_, '_>) -> io::Result<()> {
        self.submit_write_slices_at(&self.inner, writes.as_slice())
    }

    fn submit_read_exact_at(
        &self,
        file: &std::fs::File,
        offset: u64,
        buf: &mut [u8],
    ) -> io::Result<()> {
        if buf.is_empty() {
            return Ok(());
        }
        let depth = self.ring_depth;
        self.with_ring(|ring| {
            let total = buf.len();
            let chunk_size = self.chunk_size.min(total);
            let num_chunks = total.div_ceil(chunk_size);

            let mut done = vec![0usize; num_chunks];
            let mut state = vec![SubmissionState::Idle; num_chunks];
            let mut in_flight: u32 = 0;
            let mut pending_error: Option<io::Error> = None;

            loop {
                for idx in 0..num_chunks {
                    if pending_error.is_some() || in_flight >= depth {
                        break;
                    }
                    if state[idx] != SubmissionState::Idle {
                        continue;
                    }
                    let chunk_start = idx * chunk_size;
                    let chunk_end = total.min(chunk_start + chunk_size);
                    let chunk_len = chunk_end - chunk_start;
                    let so_far = done[idx];
                    let remaining = chunk_len - so_far;
                    if remaining == 0 {
                        state[idx] = SubmissionState::Done;
                        continue;
                    }
                    let len = remaining.min(MAX_IO_LEN) as u32;
                    let buf_offset = chunk_start + so_far;
                    // SAFETY: `buf_offset < total` since `chunk_start + so_far <
                    // `chunk_end <= total`. The ring holds a reference to this
                    // memory until the CQE is consumed below, and `buf` outlives
                    // the ring borrow.
                    let ptr = unsafe { buf.as_mut_ptr().add(buf_offset) };
                    let file_offset = offset.checked_add(buf_offset as u64).ok_or_else(|| {
                        Error::new(ErrorKind::InvalidInput, "read offset overflow")
                    })?;
                    let entry = opcode::Read::new(types::Fd(file.as_raw_fd()), ptr, len)
                        .offset(file_offset)
                        .build()
                        .user_data(idx as u64);
                    {
                        let mut sq = ring.submission();
                        if unsafe { sq.push(&entry) }.is_err() {
                            pending_error.get_or_insert_with(|| Error::other("io_uring SQ full"));
                            break;
                        }
                    }
                    state[idx] = SubmissionState::InFlight;
                    in_flight += 1;
                }

                if in_flight == 0 {
                    if let Some(err) = pending_error {
                        return Err(err);
                    }
                    break;
                }

                if let Err(err) = ring.submit_and_wait(1) {
                    pending_error.get_or_insert(err);
                }

                let cq: Vec<_> = ring.completion().collect();
                for cqe in cq {
                    in_flight -= 1;
                    let idx = cqe.user_data() as usize;
                    state[idx] = SubmissionState::Idle;
                    let result = cqe.result();
                    if result < 0 {
                        pending_error.get_or_insert_with(|| Error::from_raw_os_error(-result));
                        continue;
                    }
                    let n_read = result as usize;
                    if n_read == 0 {
                        pending_error.get_or_insert_with(|| {
                            Error::new(ErrorKind::UnexpectedEof, "short io_uring read")
                        });
                        continue;
                    }
                    done[idx] += n_read;
                    let chunk_start = idx * chunk_size;
                    let chunk_end = total.min(chunk_start + chunk_size);
                    let chunk_len = chunk_end - chunk_start;
                    if done[idx] >= chunk_len {
                        state[idx] = SubmissionState::Done;
                    }
                }
            }

            Ok(())
        })
    }

    fn submit_read_at(
        &self,
        file: &std::fs::File,
        offset: u64,
        buf: &mut [u8],
    ) -> io::Result<usize> {
        if buf.is_empty() {
            return Ok(0);
        }
        let len = buf.len().min(MAX_IO_LEN) as u32;
        self.with_ring(|ring| {
            let entry = opcode::Read::new(types::Fd(file.as_raw_fd()), buf.as_mut_ptr(), len)
                .offset(offset)
                .build();
            {
                let mut sq = ring.submission();
                unsafe { sq.push(&entry) }.map_err(|_| Error::other("io_uring SQ full"))?;
            }
            ring.submit_and_wait(1)?;
            let cqe = ring
                .completion()
                .next()
                .ok_or_else(|| Error::other("io_uring read completed without CQE"))?;
            let result = cqe.result();
            if result < 0 {
                return Err(Error::from_raw_os_error(-result));
            }
            Ok(result as usize)
        })
    }

    fn submit_write_exact_at(
        &self,
        file: &std::fs::File,
        offset: u64,
        data: &[u8],
    ) -> io::Result<()> {
        if data.is_empty() {
            return Ok(());
        }
        let depth = self.ring_depth;
        self.with_ring(|ring| {
            let total = data.len();
            let chunk_size = self.chunk_size.min(total);
            let num_chunks = total.div_ceil(chunk_size);

            let mut done = vec![0usize; num_chunks];
            let mut state = vec![SubmissionState::Idle; num_chunks];
            let mut in_flight: u32 = 0;
            let mut pending_error: Option<io::Error> = None;

            loop {
                for idx in 0..num_chunks {
                    if pending_error.is_some() || in_flight >= depth {
                        break;
                    }
                    if state[idx] != SubmissionState::Idle {
                        continue;
                    }
                    let chunk_start = idx * chunk_size;
                    let chunk_end = total.min(chunk_start + chunk_size);
                    let chunk_len = chunk_end - chunk_start;
                    let so_far = done[idx];
                    let remaining = chunk_len - so_far;
                    if remaining == 0 {
                        state[idx] = SubmissionState::Done;
                        continue;
                    }
                    let len = remaining.min(MAX_IO_LEN) as u32;
                    let buf_offset = chunk_start + so_far;
                    // SAFETY: `buf_offset < total` and `data` outlives the ring borrow.
                    let ptr = unsafe { data.as_ptr().add(buf_offset) };
                    let file_offset = offset.checked_add(buf_offset as u64).ok_or_else(|| {
                        Error::new(ErrorKind::InvalidInput, "write offset overflow")
                    })?;
                    let entry = opcode::Write::new(types::Fd(file.as_raw_fd()), ptr, len)
                        .offset(file_offset)
                        .build()
                        .user_data(idx as u64);
                    {
                        let mut sq = ring.submission();
                        if unsafe { sq.push(&entry) }.is_err() {
                            pending_error.get_or_insert_with(|| Error::other("io_uring SQ full"));
                            break;
                        }
                    }
                    state[idx] = SubmissionState::InFlight;
                    in_flight += 1;
                }

                if in_flight == 0 {
                    if let Some(err) = pending_error {
                        return Err(err);
                    }
                    break;
                }

                if let Err(err) = ring.submit_and_wait(1) {
                    pending_error.get_or_insert(err);
                }

                let cq: Vec<_> = ring.completion().collect();
                for cqe in cq {
                    in_flight -= 1;
                    let idx = cqe.user_data() as usize;
                    state[idx] = SubmissionState::Idle;
                    let result = cqe.result();
                    if result < 0 {
                        pending_error.get_or_insert_with(|| Error::from_raw_os_error(-result));
                        continue;
                    }
                    let n_written = result as usize;
                    if n_written == 0 {
                        pending_error.get_or_insert_with(|| {
                            Error::new(ErrorKind::WriteZero, "short io_uring write")
                        });
                        continue;
                    }
                    done[idx] += n_written;
                    let chunk_start = idx * chunk_size;
                    let chunk_end = total.min(chunk_start + chunk_size);
                    let chunk_len = chunk_end - chunk_start;
                    if done[idx] >= chunk_len {
                        state[idx] = SubmissionState::Done;
                    }
                }
            }

            Ok(())
        })
    }

    fn submit_write_at(&self, file: &std::fs::File, offset: u64, data: &[u8]) -> io::Result<usize> {
        if data.is_empty() {
            return Ok(0);
        }
        let len = data.len().min(MAX_IO_LEN) as u32;
        self.with_ring(|ring| {
            let entry = opcode::Write::new(types::Fd(file.as_raw_fd()), data.as_ptr(), len)
                .offset(offset)
                .build();
            {
                let mut sq = ring.submission();
                unsafe { sq.push(&entry) }.map_err(|_| Error::other("io_uring SQ full"))?;
            }
            ring.submit_and_wait(1)?;
            let cqe = ring
                .completion()
                .next()
                .ok_or_else(|| Error::other("io_uring write completed without CQE"))?;
            let result = cqe.result();
            if result < 0 {
                return Err(Error::from_raw_os_error(-result));
            }
            Ok(result as usize)
        })
    }

    fn submit_write_slices_at(
        &self,
        file: &std::fs::File,
        writes: &[WriteSlice<'_>],
    ) -> io::Result<()> {
        if writes.is_empty() {
            return Ok(());
        }
        let depth = self.ring_depth;
        self.with_ring(|ring| {
            let n = writes.len();
            let mut done = vec![0usize; n];
            let mut state = vec![SubmissionState::Idle; n];
            let mut in_flight: u32 = 0;
            let mut pending_error: Option<io::Error> = None;

            loop {
                for idx in 0..n {
                    if pending_error.is_some() || in_flight >= depth {
                        break;
                    }
                    if state[idx] != SubmissionState::Idle {
                        continue;
                    }
                    let write = &writes[idx];
                    let so_far = done[idx];
                    let remaining = write.data.len() - so_far;
                    if remaining == 0 {
                        state[idx] = SubmissionState::Done;
                        continue;
                    }
                    let len = remaining.min(MAX_IO_LEN) as u32;
                    // SAFETY: `write.data` is tied to the caller's `WriteSlice`,
                    // and `writes` outlives the ring borrow.
                    let ptr = unsafe { write.data.as_ptr().add(so_far) };
                    let file_offset = write.offset.checked_add(so_far as u64).ok_or_else(|| {
                        Error::new(ErrorKind::InvalidInput, "write offset overflow")
                    })?;
                    let entry = opcode::Write::new(types::Fd(file.as_raw_fd()), ptr, len)
                        .offset(file_offset)
                        .build()
                        .user_data(idx as u64);
                    {
                        let mut sq = ring.submission();
                        if unsafe { sq.push(&entry) }.is_err() {
                            pending_error.get_or_insert_with(|| Error::other("io_uring SQ full"));
                            break;
                        }
                    }
                    state[idx] = SubmissionState::InFlight;
                    in_flight += 1;
                }

                if in_flight == 0 {
                    if let Some(err) = pending_error {
                        return Err(err);
                    }
                    break;
                }

                if let Err(err) = ring.submit_and_wait(1) {
                    pending_error.get_or_insert(err);
                }

                let cq: Vec<_> = ring.completion().collect();
                for cqe in cq {
                    in_flight -= 1;
                    let idx = cqe.user_data() as usize;
                    state[idx] = SubmissionState::Idle;
                    let result = cqe.result();
                    if result < 0 {
                        pending_error.get_or_insert_with(|| Error::from_raw_os_error(-result));
                        continue;
                    }
                    let n_written = result as usize;
                    if n_written == 0 {
                        pending_error.get_or_insert_with(|| {
                            Error::new(ErrorKind::WriteZero, "short io_uring write")
                        });
                        continue;
                    }
                    done[idx] += n_written;
                    if done[idx] >= writes[idx].data.len() {
                        state[idx] = SubmissionState::Done;
                    }
                }
            }

            Ok(())
        })
    }

    fn with_ring<T>(&self, f: impl FnOnce(&mut IoUring) -> io::Result<T>) -> io::Result<T> {
        let depth = self.ring_depth;
        CACHED_RING.with(|cell| {
            let mut borrow = cell.borrow_mut();
            let needs_new = match borrow.as_ref() {
                None => true,
                Some(ring) => ring.params().sq_entries() < depth,
            };
            if needs_new {
                *borrow = Some(IoUring::new(depth).map_err(|err| {
                    Error::new(
                        err.kind(),
                        format!("failed to initialize io_uring with depth {depth}: {err}"),
                    )
                })?);
            }
            f(borrow.as_mut().unwrap())
        })
    }
}

impl<A: Allocator> Read for File<A> {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        let mut position = self
            .position
            .lock()
            .map_err(|_| Error::other("file position lock poisoned"))?;
        let n = self.submit_read_at(&self.inner, *position, buf)?;
        *position = position
            .checked_add(n as u64)
            .ok_or_else(|| Error::new(ErrorKind::InvalidInput, "file position overflow"))?;
        Ok(n)
    }
}

impl<A: Allocator> Write for File<A> {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        let mut position = self
            .position
            .lock()
            .map_err(|_| Error::other("file position lock poisoned"))?;
        let n = self.submit_write_at(&self.inner, *position, buf)?;
        if n == 0 && !buf.is_empty() {
            return Err(Error::new(ErrorKind::WriteZero, "short io_uring write"));
        }
        *position = position
            .checked_add(n as u64)
            .ok_or_else(|| Error::new(ErrorKind::InvalidInput, "file position overflow"))?;
        Ok(n)
    }

    fn flush(&mut self) -> io::Result<()> {
        self.inner.flush()
    }
}

impl<A> Seek for File<A> {
    fn seek(&mut self, pos: SeekFrom) -> io::Result<u64> {
        let mut position = self
            .position
            .lock()
            .map_err(|_| Error::other("file position lock poisoned"))?;
        let base = match pos {
            SeekFrom::Start(offset) => {
                *position = offset;
                return Ok(offset);
            }
            SeekFrom::Current(_) => i128::from(*position),
            SeekFrom::End(_) => i128::from(self.inner.metadata()?.len()),
        };
        let delta = match pos {
            SeekFrom::Current(delta) | SeekFrom::End(delta) => i128::from(delta),
            SeekFrom::Start(_) => unreachable!(),
        };
        let next = base
            .checked_add(delta)
            .ok_or_else(|| Error::new(ErrorKind::InvalidInput, "file position overflow"))?;
        let next = u64::try_from(next)
            .map_err(|_| Error::new(ErrorKind::InvalidInput, "invalid negative file position"))?;
        *position = next;
        Ok(next)
    }
}

impl<A> AsRef<std::fs::File> for File<A> {
    fn as_ref(&self) -> &std::fs::File {
        &self.inner
    }
}

/// Options and flags for opening an `io_uring` file.
#[derive(Debug, Clone)]
pub struct OpenOptions<A = DefaultAllocator> {
    inner: std::fs::OpenOptions,
    ring_depth: u32,
    chunk_size: usize,
    append: bool,
    allocator: A,
}

impl OpenOptions<DefaultAllocator> {
    /// Creates a blank set of options.
    #[must_use]
    pub fn new() -> Self {
        Self {
            inner: std::fs::OpenOptions::new(),
            ring_depth: DEFAULT_RING_DEPTH,
            chunk_size: DEFAULT_CHUNK_SIZE,
            append: false,
            allocator: DefaultAllocator::default(),
        }
    }
}

impl<A: Allocator> OpenOptions<A> {
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
        self.append = append;
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

    /// Sets the allocator used by reads on files opened with these options.
    pub fn allocator<B: Allocator>(&self, allocator: B) -> OpenOptions<B> {
        OpenOptions {
            inner: self.inner.clone(),
            ring_depth: self.ring_depth,
            chunk_size: self.chunk_size,
            append: self.append,
            allocator,
        }
    }

    /// Opens a file with the configured options.
    pub fn open<P: AsRef<Path>>(&self, path: P) -> io::Result<File<A>> {
        if self.ring_depth == 0 {
            return Err(Error::new(
                ErrorKind::InvalidInput,
                "ring_depth must be greater than zero",
            ));
        }
        if self.chunk_size == 0 {
            return Err(Error::new(
                ErrorKind::InvalidInput,
                "chunk_size must be greater than zero",
            ));
        }
        if self.append {
            return Err(Error::new(
                ErrorKind::Unsupported,
                "uring append mode is not supported; use explicit offsets",
            ));
        }
        Ok(File {
            inner: self.inner.open(path)?,
            ring_depth: self.ring_depth,
            chunk_size: self.chunk_size,
            position: Arc::new(Mutex::new(0)),
            allocator: self.allocator.clone(),
        })
    }
}

impl Default for OpenOptions<DefaultAllocator> {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{OwnedBytes, System, WriteSlice, WriteSlices};
    use tempfile::TempDir;

    fn allow_unavailable<T>(result: io::Result<T>) -> Option<T> {
        match result {
            Ok(value) => Some(value),
            Err(err) if err.to_string().contains("failed to initialize io_uring") => None,
            Err(err) => panic!("unexpected io_uring error: {err}"),
        }
    }

    #[test]
    fn read_all_roundtrips_file_contents() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("read-all.bin");
        std::fs::write(&path, b"hello uring").unwrap();
        let file = File::open(&path).unwrap();

        let Some(bytes) = allow_unavailable(file.read_all()) else {
            return;
        };

        assert_eq!(bytes.as_ref(), b"hello uring");
    }

    #[test]
    fn read_at_returns_positioned_bytes() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("read-at.bin");
        std::fs::write(&path, b"abcdef").unwrap();
        let file = File::open(&path).unwrap();

        let Some(bytes) = allow_unavailable(file.read_at(2, 3)) else {
            return;
        };

        assert_eq!(bytes.as_ref(), b"cde");
    }

    #[test]
    fn system_allocator_returns_vec_read_buffer() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("system.bin");
        std::fs::write(&path, b"abcdef").unwrap();
        let file = OpenOptions::new()
            .read(true)
            .allocator(System)
            .open(&path)
            .unwrap();

        let Some(bytes) = allow_unavailable(file.read_at(1, 3)) else {
            return;
        };

        assert!(matches!(&bytes, OwnedBytes::Vec(_)));
        assert_eq!(bytes.as_ref(), b"bcd");
    }

    #[test]
    fn write_all_at_preserves_surrounding_bytes() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("write-at.bin");
        std::fs::write(&path, b"hello world").unwrap();
        let file = OpenOptions::new().write(true).open(&path).unwrap();

        let Some(()) = allow_unavailable(file.write_all_at(6, b"uring")) else {
            return;
        };

        assert_eq!(std::fs::read(&path).unwrap(), b"hello uring");
    }

    #[test]
    fn write_slices_at_writes_non_overlapping_ranges() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("write-slices.bin");
        std::fs::write(&path, b"----------").unwrap();
        let file = OpenOptions::new().write(true).open(&path).unwrap();
        let slices = [WriteSlice::new(0, b"AB"), WriteSlice::new(8, b"YZ")];

        let Some(()) = allow_unavailable(file.write_slices_at(WriteSlices::new(&slices).unwrap()))
        else {
            return;
        };

        assert_eq!(std::fs::read(&path).unwrap(), b"AB------YZ");
    }

    #[test]
    fn ring_depth_one_and_tiny_chunks_roundtrip() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("chunked.bin");
        let file = OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .truncate(true)
            .ring_depth(1)
            .chunk_size(3)
            .open(&path)
            .unwrap();
        let data = b"chunked io_uring data";

        let Some(()) = allow_unavailable(file.write_all_at(0, data)) else {
            return;
        };
        let Some(bytes) = allow_unavailable(file.read_at(0, data.len())) else {
            return;
        };

        assert_eq!(bytes.as_ref(), data);
    }

    #[test]
    fn invalid_ring_options_return_invalid_input() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("invalid.bin");
        std::fs::write(&path, b"x").unwrap();

        let depth_err = OpenOptions::new()
            .read(true)
            .ring_depth(0)
            .open(&path)
            .unwrap_err();
        let chunk_err = OpenOptions::new()
            .read(true)
            .chunk_size(0)
            .open(&path)
            .unwrap_err();

        assert_eq!(depth_err.kind(), io::ErrorKind::InvalidInput);
        assert_eq!(chunk_err.kind(), io::ErrorKind::InvalidInput);
    }

    #[test]
    fn read_trait_uses_cursor_and_advances() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("read-trait.bin");
        std::fs::write(&path, b"abcdef").unwrap();
        let mut file = File::open(&path).unwrap();
        let mut first = [0u8; 2];
        let mut second = [0u8; 3];

        let Some(n_first) = allow_unavailable(file.read(&mut first)) else {
            return;
        };
        let Some(n_second) = allow_unavailable(file.read(&mut second)) else {
            return;
        };

        assert_eq!(n_first, 2);
        assert_eq!(first, *b"ab");
        assert_eq!(n_second, 3);
        assert_eq!(second, *b"cde");
    }

    #[test]
    fn write_trait_uses_cursor_and_advances() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("write-trait.bin");
        std::fs::write(&path, b"------").unwrap();
        let mut file = OpenOptions::new().write(true).open(&path).unwrap();

        let Some(n_first) = allow_unavailable(file.write(b"ab")) else {
            return;
        };
        let Some(n_second) = allow_unavailable(file.write(b"cd")) else {
            return;
        };

        assert_eq!(n_first, 2);
        assert_eq!(n_second, 2);
        assert_eq!(std::fs::read(&path).unwrap(), b"abcd--");
    }

    #[test]
    fn seek_trait_controls_read_position() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("seek-trait.bin");
        std::fs::write(&path, b"abcdef").unwrap();
        let mut file = File::open(&path).unwrap();
        let mut buf = [0u8; 2];

        assert_eq!(file.seek(SeekFrom::Start(3)).unwrap(), 3);
        let Some(n) = allow_unavailable(file.read(&mut buf)) else {
            return;
        };

        assert_eq!(n, 2);
        assert_eq!(buf, *b"de");
    }

    #[test]
    fn try_clone_shares_cursor() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("clone-cursor.bin");
        std::fs::write(&path, b"abcdef").unwrap();
        let mut file = File::open(&path).unwrap();
        let mut cloned = file.try_clone().unwrap();
        let mut first = [0u8; 2];
        let mut second = [0u8; 2];

        let Some(_) = allow_unavailable(file.read(&mut first)) else {
            return;
        };
        let Some(_) = allow_unavailable(cloned.read(&mut second)) else {
            return;
        };

        assert_eq!(first, *b"ab");
        assert_eq!(second, *b"cd");
    }

    #[test]
    fn positioned_reads_do_not_move_cursor() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("positioned-cursor.bin");
        std::fs::write(&path, b"abcdef").unwrap();
        let mut file = File::open(&path).unwrap();
        let mut cursor_bytes = [0u8; 2];

        let Some(bytes) = allow_unavailable(file.read_at(3, 2)) else {
            return;
        };
        let Some(_) = allow_unavailable(file.read(&mut cursor_bytes)) else {
            return;
        };

        assert_eq!(bytes.as_ref(), b"de");
        assert_eq!(cursor_bytes, *b"ab");
    }

    #[test]
    fn append_mode_is_rejected() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("append.bin");
        std::fs::write(&path, b"existing").unwrap();

        let err = OpenOptions::new().append(true).open(&path).unwrap_err();

        assert_eq!(err.kind(), io::ErrorKind::Unsupported);
    }
}
