//! Synchronous blocking I/O backend.
//!
//! [`Sync`] implements [`BlockingIo`](crate::BlockingIo).
//! Each OS has an explicit implementation:
//! - Linux: O_DIRECT-aware chunked reads with `write_at` positioned writes.
//! - macOS: `std::os::unix::fs::FileExt` positioned I/O.
//! - Windows: `std::os::windows::fs::FileExt` positioned I/O.

/// Controls O_DIRECT usage for bypassing the kernel page cache.
///
/// Only takes effect on Linux. On other platforms this option is accepted
/// but silently ignored (all I/O goes through the page cache).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum DirectIo {
    /// Never use O_DIRECT. All reads go through the kernel page cache.
    Disabled,
    /// Require O_DIRECT. Returns an error if the filesystem doesn't support it.
    Enabled,
    /// Try O_DIRECT, fall back to buffered I/O if the filesystem doesn't support it.
    #[default]
    Auto,
}

/// Options for the synchronous I/O backend.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct SyncOptions {
    /// Number of Rayon worker threads for batch reads/writes.
    ///
    /// `None` uses the global Rayon thread pool. `Some(n)` creates a backend-local
    /// pool with exactly `n` threads; `n == 0` is rejected by [`SyncIo::with_options`].
    pub batch_threads: Option<usize>,

    /// Controls whether to use O_DIRECT for reads (Linux only, ignored elsewhere).
    pub direct_io: DirectIo,

    /// Controls how read buffers are allocated.
    pub allocator: crate::buffer::BufferAllocator,
}

#[cfg(target_os = "linux")]
mod linux;
#[cfg(target_os = "linux")]
pub use linux::SyncIo;

#[cfg(target_os = "macos")]
mod macos;
#[cfg(target_os = "macos")]
pub use macos::SyncIo;

#[cfg(target_os = "windows")]
mod windows;
#[cfg(target_os = "windows")]
pub use windows::SyncIo;

#[cfg(not(any(target_os = "linux", target_os = "macos", target_os = "windows")))]
compile_error!("fastio sync supports Linux, macOS, and Windows only");
