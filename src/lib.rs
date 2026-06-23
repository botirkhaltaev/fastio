//! # fastio
//!
//! Explicit file I/O backends with backend-owned file handles.
//!
//! `fastio` intentionally has no default backend. Choose a backend module such
//! as [`sync`], `tokio`, `mmap`, or Linux `uring`, then use that module's file
//! API.
//!
//! # Features
//!
//! - `sync`: synchronous `std::fs`-like file I/O.
//! - `mmap`: read-only memory maps using `memmap2`.
//! - `pool`: pooled read buffers using `zeropool`.
//! - `tokio`: async I/O using Tokio, without a Rayon dependency.
//! - `io-uring`: Linux-only `io_uring` backend.
//!
//! # Example
//!
//! ```no_run
//! use fastio::sync::File;
//!
//! let file = File::open("model.bin")?;
//! let bytes = file.read_at(0, 4096)?;
//! # Ok::<(), std::io::Error>(())
//! ```

pub mod buffer;
pub mod write;

#[cfg(feature = "mmap")]
pub mod mmap;
#[cfg(feature = "sync")]
pub mod sync;
#[cfg(feature = "tokio")]
pub mod tokio;
#[cfg(all(target_os = "linux", feature = "io-uring"))]
pub mod uring;

pub use std::io::Result as IoResult;
pub use std::io::{Error, Result};

#[cfg(feature = "mmap")]
pub use buffer::MmapRegion;
#[cfg(feature = "pool")]
pub use buffer::Pool;
pub use buffer::{Allocator, DefaultAllocator, OwnedBytes, System};
pub use write::{WriteSlice, WriteSlices};
