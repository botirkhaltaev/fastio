//! Linux `io_uring` file I/O.
//!
//! This module mirrors `std::fs` where possible and adds positioned I/O backed
//! by a thread-local `io_uring` instance.

use std::cell::RefCell;
use std::fs::{Metadata, Permissions};
use std::io::{self, Error, ErrorKind, Read, Seek, SeekFrom, Write};
use std::os::fd::AsRawFd;
use std::path::Path;

use io_uring::{IoUring, opcode, types};

use crate::{OwnedBytes, WriteSlice, WriteSlices};

const DEFAULT_RING_DEPTH: u32 = 256;
const DEFAULT_CHUNK_SIZE: usize = 4 * 1024 * 1024;
const MAX_IO_LEN: usize = u32::MAX as usize;

thread_local! {
    static CACHED_RING: RefCell<Option<IoUring>> = const { RefCell::new(None) };
}

fn with_ring<T>(depth: u32, f: impl FnOnce(&mut IoUring) -> io::Result<T>) -> io::Result<T> {
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

/// A Linux `io_uring` file handle.
#[derive(Debug)]
pub struct File {
    inner: std::fs::File,
    ring_depth: u32,
    chunk_size: usize,
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
            ring_depth: self.ring_depth,
            chunk_size: self.chunk_size,
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
        self.submit_read_exact_at(&self.inner, 0, &mut bytes)?;
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
        self.submit_read_exact_at(&self.inner, offset, buf)
    }

    /// Writes all bytes from `buf` at `offset`.
    pub fn write_all_at(&self, offset: u64, buf: &[u8]) -> io::Result<()> {
        self.submit_write_exact_at(&self.inner, offset, buf)
    }

    /// Writes non-overlapping slices at their offsets.
    pub fn write_slices_at(&self, writes: WriteSlices<'_>) -> io::Result<()> {
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
        with_ring(depth, |ring| {
            let total = buf.len();
            let chunk_size = self.chunk_size.min(total);
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
                    // `chunk_end <= total`. The ring holds a reference to this
                    // memory until the CQE is consumed below, and `buf` outlives
                    // the ring borrow.
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
        with_ring(depth, |ring| {
            let total = data.len();
            let chunk_size = self.chunk_size.min(total);
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
                    // SAFETY: `buf_offset < total` and `data` outlives the ring borrow.
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

    fn submit_write_slices_at(
        &self,
        file: &std::fs::File,
        writes: &[WriteSlice<'_>],
    ) -> io::Result<()> {
        if writes.is_empty() {
            return Ok(());
        }
        let depth = self.ring_depth;
        with_ring(depth, |ring| {
            let n = writes.len();
            let mut done = vec![0usize; n];
            let mut in_flight: u32 = 0;
            let mut next_submit = 0usize;

            loop {
                while in_flight < depth && next_submit < n {
                    let idx = next_submit;
                    let write = &writes[idx];
                    let so_far = done[idx];
                    let remaining = write.data.len() - so_far;
                    if remaining == 0 {
                        next_submit += 1;
                        continue;
                    }
                    let len = remaining.min(MAX_IO_LEN) as u32;
                    // SAFETY: `write.data` is tied to the caller's `WriteSlice`,
                    // and `writes` outlives the ring borrow.
                    let ptr = unsafe { write.data.as_ptr().add(so_far) };
                    let entry = opcode::Write::new(types::Fd(file.as_raw_fd()), ptr, len)
                        .offset(write.offset + so_far as u64)
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
        Self {
            inner: std::fs::OpenOptions::new(),
            ring_depth: DEFAULT_RING_DEPTH,
            chunk_size: DEFAULT_CHUNK_SIZE,
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
        Ok(File {
            inner: self.inner.open(path)?,
            ring_depth: self.ring_depth,
            chunk_size: self.chunk_size,
        })
    }
}

impl Default for OpenOptions {
    fn default() -> Self {
        Self::new()
    }
}
