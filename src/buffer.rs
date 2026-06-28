//! Owned byte buffer types for storage I/O.
//!
//! [`Bytes`] is a zero-copy owned byte container backed by a pooled,
//! memory-mapped, or plain `Vec` storage. All variants provide uniform read
//! access via `AsRef<[u8]>` and `Deref<Target = [u8]>`.
//!
//! Read-capable backends allocate through the internal crate-only
//! `Bytes::allocate` path. It uses `zeropool::ZeroPool::alloc_uninit` to avoid
//! paying a zeroing cost, and hands the caller a mutable slice to fill. The
//! caller is responsible for fully initializing the buffer inside the closure;
//! once the closure succeeds, the returned `Bytes` is considered initialized.
//! Zero-length reads return an empty `Bytes::Vec` without touching the pool.

use std::fmt;

#[cfg(all(test, feature = "mmap"))]
use std::sync::Arc;

#[cfg(any(feature = "sync", feature = "tokio", feature = "io-uring"))]
use zeropool::ZeroPool;

#[cfg(feature = "mmap")]
use crate::mmap::MmapRegion;

// ============================================================================
// Bytes
// ============================================================================

/// Owned byte buffer backed by one of several storage strategies.
///
/// `Bytes` provides uniform read access via `AsRef<[u8]>` / `Deref`.
/// Only heap-backed and pooled storage support mutable access.
pub struct Bytes {
    inner: Storage,
}

#[derive(Debug)]
enum Storage {
    #[cfg(any(feature = "sync", feature = "tokio", feature = "io-uring"))]
    Pooled(zeropool::Buf<'static>),
    #[cfg(feature = "mmap")]
    Mmap(MmapRegion),
    Vec(Vec<u8>),
}

impl Bytes {
    /// Allocate a buffer for a read of `len` bytes and fill it via `init`.
    ///
    /// The closure receives a mutable byte slice of length `len`. The slice is
    /// backed by an uninitialized pooled buffer; the closure must fully
    /// initialize every byte before returning. On success, the now-initialized
    /// buffer is wrapped in a `Bytes`. On failure, the uninitialized buffer is
    /// returned to the pool and the error is propagated.
    #[cfg(any(feature = "sync", feature = "tokio", feature = "io-uring"))]
    pub(crate) fn allocate(
        len: usize,
        init: impl FnOnce(&mut [u8]) -> crate::IoResult<()>,
    ) -> crate::IoResult<Self> {
        if len == 0 {
            return Ok(Self { inner: Storage::Vec(Vec::new()) });
        }
        let mut uninit = global_pool().alloc_uninit(len);
        let uninit_slice = uninit.as_uninit_mut();
        // SAFETY: the closure is required to fully initialize every byte in the
        // slice before returning. `Bytes` is not exposed to safe code until the
        // closure succeeds, so no safe consumer can observe uninitialized bytes.
        let init_slice =
            unsafe { std::slice::from_raw_parts_mut(uninit_slice.as_mut_ptr().cast::<u8>(), len) };
        init(init_slice)?;
        // SAFETY: the closure has promised to initialize all bytes in `init_slice`.
        let buf = unsafe { uninit.assume_init() };
        Ok(Self { inner: Storage::Pooled(buf) })
    }

    /// Wrap a plain `Vec<u8>`.
    #[inline]
    #[must_use]
    pub fn from_vec(v: Vec<u8>) -> Self {
        Self { inner: Storage::Vec(v) }
    }

    /// Wrap a memory-mapped region.
    #[cfg(feature = "mmap")]
    #[inline]
    #[must_use]
    pub fn from_mmap(region: MmapRegion) -> Self {
        Self { inner: Storage::Mmap(region) }
    }

    /// Number of bytes.
    #[inline]
    #[must_use]
    pub fn len(&self) -> usize {
        match &self.inner {
            #[cfg(any(feature = "sync", feature = "tokio", feature = "io-uring"))]
            Storage::Pooled(b) => b.len(),
            #[cfg(feature = "mmap")]
            Storage::Mmap(b) => b.len(),
            Storage::Vec(b) => b.len(),
        }
    }

    /// Returns `true` if there are no bytes.
    #[inline]
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    /// Returns a mutable byte slice for the variants that own their memory.
    ///
    /// Returns `None` for memory-mapped storage, which is immutable.
    #[inline]
    #[must_use]
    pub fn as_mut_slice(&mut self) -> Option<&mut [u8]> {
        match &mut self.inner {
            #[cfg(any(feature = "sync", feature = "tokio", feature = "io-uring"))]
            Storage::Pooled(b) => Some(b.as_mut()),
            #[cfg(feature = "mmap")]
            Storage::Mmap(_) => None,
            Storage::Vec(b) => Some(b.as_mut_slice()),
        }
    }

    /// Convert to a `Vec<u8>`.
    ///
    /// Copies when backed by `Mmap` storage.
    #[must_use]
    pub fn into_vec(self) -> Vec<u8> {
        match self.inner {
            #[cfg(any(feature = "sync", feature = "tokio", feature = "io-uring"))]
            Storage::Pooled(b) => b.into_inner(),
            #[cfg(feature = "mmap")]
            Storage::Mmap(b) => b.as_slice().to_vec(),
            Storage::Vec(b) => b,
        }
    }

    /// Convert to an `Arc<[u8]>`.
    ///
    /// Copies when backed by `Mmap` storage.
    #[must_use]
    pub fn into_shared(self) -> std::sync::Arc<[u8]> {
        match self.inner {
            #[cfg(any(feature = "sync", feature = "tokio", feature = "io-uring"))]
            Storage::Pooled(b) => b.into_inner().into(),
            #[cfg(feature = "mmap")]
            Storage::Mmap(b) => std::sync::Arc::from(b.as_slice()),
            Storage::Vec(b) => b.into(),
        }
    }
}

impl AsRef<[u8]> for Bytes {
    #[inline]
    fn as_ref(&self) -> &[u8] {
        match &self.inner {
            #[cfg(any(feature = "sync", feature = "tokio", feature = "io-uring"))]
            Storage::Pooled(b) => b.as_ref(),
            #[cfg(feature = "mmap")]
            Storage::Mmap(b) => b.as_slice(),
            Storage::Vec(b) => b.as_ref(),
        }
    }
}

impl std::ops::Deref for Bytes {
    type Target = [u8];

    #[inline]
    fn deref(&self) -> &Self::Target {
        self.as_ref()
    }
}

impl fmt::Debug for Bytes {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Bytes").field("len", &self.len()).finish_non_exhaustive()
    }
}

impl From<Vec<u8>> for Bytes {
    #[inline]
    fn from(v: Vec<u8>) -> Self {
        Self::from_vec(v)
    }
}

#[cfg(feature = "mmap")]
impl From<MmapRegion> for Bytes {
    #[inline]
    fn from(region: MmapRegion) -> Self {
        Self::from_mmap(region)
    }
}

#[cfg(test)]
#[cfg(any(feature = "sync", feature = "tokio", feature = "io-uring"))]
impl Bytes {
    pub(crate) fn is_pooled(&self) -> bool {
        matches!(self.inner, Storage::Pooled(_))
    }
}

#[cfg(any(feature = "sync", feature = "tokio", feature = "io-uring"))]
fn global_pool() -> &'static ZeroPool {
    use std::sync::OnceLock;
    static POOL: OnceLock<ZeroPool> = OnceLock::new();
    POOL.get_or_init(|| {
        ZeroPool::new()
            .min_buffer_size(1024 * 1024)
            .tls_cache_size(4)
            .max_buffers_per_class(32)
    })
}

// ============================================================================
// Tests
// ============================================================================

#[cfg(test)]
mod tests {
    use super::*;

    #[cfg(feature = "mmap")]
    fn make_mmap_region() -> MmapRegion {
        use std::io::Write;
        let mut tmp = tempfile::NamedTempFile::new().unwrap();
        tmp.write_all(b"hello_mmap").unwrap();
        tmp.flush().unwrap();
        let file = std::fs::File::open(tmp.path()).unwrap();
        // SAFETY: the temp file remains alive for the duration of the mapping
        // setup, and memmap2 owns the resulting mapping independently.
        let mmap = unsafe { memmap2::MmapOptions::new().map(&file).unwrap() };
        MmapRegion::new(Arc::new(mmap), 0, 10)
    }

    #[test]
    fn from_vec_roundtrips() {
        let data = vec![1u8, 2, 3];
        let ob = Bytes::from_vec(data.clone());
        assert_eq!(ob.as_ref(), &data[..]);
    }

    #[cfg(any(feature = "sync", feature = "tokio", feature = "io-uring"))]
    #[test]
    fn allocate_pooled_roundtrips() {
        let mut ob = Bytes::allocate(8, |buf| {
            buf[..4].copy_from_slice(&[10, 20, 30, 40]);
            Ok(())
        })
        .unwrap();
        assert_eq!(&ob.as_ref()[..4], &[10, 20, 30, 40]);
        assert!(ob.as_mut_slice().is_some());
    }

    #[test]
    fn vec_variant() {
        let data = vec![5u8, 6, 7];
        let ob = Bytes::from_vec(data.clone());
        assert_eq!(ob.as_ref(), &data[..]);
    }

    #[cfg(feature = "mmap")]
    #[test]
    fn mmap_variant_accessible() {
        let ob = Bytes::from_mmap(make_mmap_region());
        assert_eq!(ob.as_ref(), b"hello_mmap");
    }

    #[test]
    fn len_and_is_empty_vec() {
        let ob = Bytes::from_vec(vec![1, 2]);
        assert_eq!(ob.len(), 2);
        assert!(!ob.is_empty());
        let empty = Bytes::from_vec(vec![]);
        assert!(empty.is_empty());
    }

    #[test]
    fn zero_length_vec_has_mutable_empty_slice() {
        let mut ob = Bytes::from_vec(Vec::new());

        let slice = ob.as_mut_slice().unwrap();

        assert!(slice.is_empty());
        assert!(ob.is_empty());
    }

    #[test]
    fn deref_matches_as_ref() {
        let ob = Bytes::from_vec(vec![42u8; 8]);
        let via_deref: &[u8] = &ob;
        assert_eq!(via_deref, ob.as_ref());
    }

    #[test]
    fn debug_shows_len() {
        let ob = Bytes::from_vec(vec![0u8; 17]);
        assert!(format!("{ob:?}").contains("17"));
    }

    #[test]
    fn as_mut_slice_some_for_vec() {
        let mut ob = Bytes::from_vec(vec![0u8; 4]);
        ob.as_mut_slice().unwrap()[0] = 99;
        assert_eq!(ob.as_ref()[0], 99);
    }

    #[cfg(feature = "mmap")]
    #[test]
    fn as_mut_slice_none_for_mmap() {
        let mut ob = Bytes::from_mmap(make_mmap_region());
        assert!(ob.as_mut_slice().is_none());
    }

    #[cfg(feature = "mmap")]
    #[test]
    fn mmap_subregion_returns_requested_window() {
        let region = make_mmap_region();

        let subregion = region.subregion(6, 4).unwrap();

        assert_eq!(subregion.as_slice(), b"mmap");
    }

    #[cfg(feature = "mmap")]
    #[test]
    fn mmap_subregion_rejects_out_of_bounds_window() {
        let region = make_mmap_region();

        assert!(region.subregion(8, 3).is_none());
        assert!(region.subregion(usize::MAX, 1).is_none());
    }

    #[test]
    fn into_vec_preserves_bytes() {
        let data = vec![7u8, 8, 9];
        assert_eq!(Bytes::from_vec(data.clone()).into_vec(), data);
        #[cfg(feature = "mmap")]
        assert_eq!(Bytes::from_mmap(make_mmap_region()).into_vec(), b"hello_mmap");
    }

    #[test]
    fn into_shared_preserves_bytes() {
        let data = vec![1u8, 2, 3];
        let shared = Bytes::from_vec(data.clone()).into_shared();
        assert_eq!(shared.as_ref(), &data[..]);
        #[cfg(feature = "mmap")]
        let shared2 = Bytes::from_mmap(make_mmap_region()).into_shared();
        #[cfg(feature = "mmap")]
        assert_eq!(shared2.as_ref(), b"hello_mmap");
    }

    #[cfg(any(feature = "sync", feature = "tokio", feature = "io-uring"))]
    #[test]
    fn allocate_returns_pooled_storage() {
        let ob = Bytes::allocate(4, |buf| {
            buf.copy_from_slice(&[1, 2, 3, 4]);
            Ok(())
        })
        .unwrap();
        assert!(matches!(ob.inner, Storage::Pooled(_)));
        assert_eq!(ob.len(), 4);
    }

    #[cfg(any(feature = "sync", feature = "tokio", feature = "io-uring"))]
    #[test]
    fn zero_length_allocation_is_empty_vec() {
        let ob = Bytes::allocate(0, |_| Ok(())).unwrap();
        assert!(matches!(ob.inner, Storage::Vec(_)));
        assert!(ob.is_empty());
    }

    #[cfg(any(feature = "sync", feature = "tokio", feature = "io-uring"))]
    #[test]
    fn allocate_propagates_init_error() {
        let err = Bytes::allocate(4, |_| Err(std::io::Error::other("boom"))).unwrap_err();
        assert_eq!(err.kind(), std::io::ErrorKind::Other);
    }
}
