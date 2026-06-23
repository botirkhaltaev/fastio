//! Linux io_uring storage implementation with batched I/O.
//!
//! A thread-local ring is cached to amortize the `io_uring_setup` syscall
//! across repeated operations.  Batch operations (`read_ranges`,
//! `write_positioned_file`, `write_slices`) saturate the ring: up to
//! `ring_depth` ops are in-flight at once, completions are drained in a
//! tight loop, and partial results are immediately resubmitted.

use std::cell::RefCell;
use std::fs::File;
use std::io::{Error, ErrorKind};
use std::os::fd::AsRawFd;

use io_uring::{IoUring as Uring, opcode, types};

use crate::{IoResult, WriteSlice};

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
}

impl Default for IoUringOptions {
    fn default() -> Self {
        Self {
            ring_depth: DEFAULT_RING_DEPTH,
            chunk_size: DEFAULT_CHUNK_SIZE,
        }
    }
}

impl IoUring {
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

    /// Reads exactly `buf.len()` bytes from `file` at `offset` using the
    /// thread-local ring. Splits the buffer into chunks and keeps up to
    /// `ring_depth` reads in-flight simultaneously.
    pub(crate) fn read_exact_at(&self, file: &File, offset: u64, buf: &mut [u8]) -> IoResult<()> {
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
    pub(crate) fn write_exact_at(&self, file: &File, offset: u64, data: &[u8]) -> IoResult<()> {
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
    /// Submits up to `ring_depth` ops simultaneously. Partial writes are
    /// resubmitted immediately. Callers must pass non-overlapping writes.
    pub(crate) fn ring_batch_writes(&self, file: &File, writes: &[WriteSlice<'_>]) -> IoResult<()> {
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
