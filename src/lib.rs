//! # fastio
//!
//! Fast file I/O backends with a small trait vocabulary for blocking, async,
//! memory-mapped, and Linux `io_uring` access.
//!
//! Backends are cheap configuration values. Constructors validate only local
//! options; platform, kernel, filesystem, and permission failures are returned
//! by the I/O operation that encounters them.
//!
//! # Features
//!
//! - `sync`: synchronous positioned I/O with Rayon-backed batch operations.
//! - `mmap`: read-only memory maps using `memmap2`.
//! - `pool`: pooled read buffers using `zeropool`.
//! - `tokio`: async I/O using Tokio, without a Rayon dependency.
//! - `io-uring`: Linux-only `io_uring` backend.
//!
//! # Example
//!
//! ```no_run
//! use fastio::{BlockingIo, SyncIo};
//!
//! let bytes = SyncIo::new().read_file("model.bin".as_ref())?;
//! # Ok::<(), std::io::Error>(())
//! ```

pub mod buffer;
pub mod range;
pub mod traits;
pub mod write;

#[cfg(all(target_os = "linux", feature = "io-uring"))]
pub mod io_uring;
#[cfg(feature = "mmap")]
pub mod mmap;
#[cfg(feature = "sync")]
pub mod sync;
#[cfg(feature = "tokio")]
pub mod tokio;

pub use std::io::Result as IoResult;
pub use std::io::{Error, Result};

#[cfg(feature = "mmap")]
pub use buffer::MmapRegion;
#[cfg(feature = "pool")]
pub use buffer::PoolConfig;
pub use buffer::{BufferAllocator, OwnedBytes};
#[cfg(all(target_os = "linux", feature = "io-uring"))]
pub use io_uring::{IoUring, IoUringOptions};
#[cfg(feature = "mmap")]
pub use mmap::Mmap;
pub use range::{ByteRange, FileRange, RangeRead, RequestIndex};
#[cfg(feature = "sync")]
pub use sync::{DirectIo, SyncIo, SyncOptions};
#[cfg(feature = "tokio")]
pub use tokio::{Tokio, TokioOptions};
#[cfg(feature = "tokio")]
pub use traits::AsyncIo;
pub use traits::BlockingIo;
#[cfg(feature = "mmap")]
pub use traits::MmapIo;
pub use write::{WriteSlice, WriteSlices};

#[cfg(all(test, feature = "tokio"))]
pub(crate) mod test_utils {
    pub fn run_async<F>(future: F) -> F::Output
    where
        F: std::future::Future,
    {
        tokio::runtime::Runtime::new()
            .expect("tokio runtime creation failed")
            .block_on(future)
    }
}
