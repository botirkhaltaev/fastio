//! Owned byte buffer types for storage I/O.
//!
//! [`OwnedBytes`] is a zero-copy owned byte container that can be backed by
//! pooled, aligned, memory-mapped, or plain `Vec` storage. All variants
//! implement `AsRef<[u8]>` and `Deref<Target = [u8]>` for uniform read access.
//!
//! # Buffer pool
//!
//! The process-wide buffer pool is configured via [`PoolConfig`]. Backends
//! that have pooling enabled obtain buffers via the pool and return
//! [`OwnedBytes::Pooled`]; when disabled they return [`OwnedBytes::Vec`].

use std::sync::Arc;

#[cfg(feature = "pool")]
use zeropool::BufferPool;
#[cfg(feature = "pool")]
pub(crate) use zeropool::PooledBuffer;

// ============================================================================
// AlignedBuffer — O_DIRECT aligned heap buffer (Linux only)
// ============================================================================

/// A page-aligned buffer for O_DIRECT I/O (Linux only).
#[cfg(target_os = "linux")]
pub struct AlignedBuffer {
    ptr: std::ptr::NonNull<u8>,
    layout: std::alloc::Layout,
    len: usize,
}

#[cfg(target_os = "linux")]
impl AlignedBuffer {
    const BLOCK_SIZE: usize = 4096;

    pub fn new(capacity: usize) -> std::io::Result<Self> {
        if capacity == 0 {
            let layout = std::alloc::Layout::from_size_align(0, 1).map_err(|_| {
                std::io::Error::new(std::io::ErrorKind::InvalidInput, "invalid alloc layout")
            })?;
            return Ok(Self {
                ptr: std::ptr::NonNull::dangling(),
                layout,
                len: 0,
            });
        }
        let layout =
            std::alloc::Layout::from_size_align(capacity, Self::BLOCK_SIZE).map_err(|_| {
                std::io::Error::new(std::io::ErrorKind::InvalidInput, "invalid alloc layout")
            })?;
        // SAFETY: layout was constructed by Layout::from_size_align and is valid
        // for allocation.
        let ptr = unsafe { std::alloc::alloc_zeroed(layout) };
        if ptr.is_null() {
            std::alloc::handle_alloc_error(layout);
        }
        Ok(Self {
            ptr: std::ptr::NonNull::new(ptr).unwrap(),
            layout,
            len: 0,
        })
    }

    pub fn as_slice(&self) -> &[u8] {
        // SAFETY: ptr is valid for at least self.len bytes, and set_len prevents
        // exposing bytes beyond the allocated layout size.
        unsafe { std::slice::from_raw_parts(self.ptr.as_ptr(), self.len) }
    }
    pub fn as_mut_slice(&mut self) -> &mut [u8] {
        // SAFETY: &mut self guarantees unique access, and ptr is valid for
        // self.len bytes by the same invariant as as_slice.
        unsafe { std::slice::from_raw_parts_mut(self.ptr.as_ptr(), self.len) }
    }
    pub fn as_mut_ptr(&mut self) -> *mut u8 {
        self.ptr.as_ptr()
    }
    pub fn set_len(&mut self, len: usize) {
        assert!(len <= self.layout.size());
        self.len = len;
    }
    pub fn capacity(&self) -> usize {
        self.layout.size()
    }
    pub const fn len(&self) -> usize {
        self.len
    }
    pub const fn is_empty(&self) -> bool {
        self.len == 0
    }
}

#[cfg(target_os = "linux")]
impl Drop for AlignedBuffer {
    fn drop(&mut self) {
        if self.layout.size() > 0 {
            // SAFETY: ptr was allocated with this exact layout in AlignedBuffer::new.
            unsafe { std::alloc::dealloc(self.ptr.as_ptr(), self.layout) }
        }
    }
}

#[cfg(target_os = "linux")]
impl std::fmt::Debug for AlignedBuffer {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("AlignedBuffer")
            .field("len", &self.len)
            .field("capacity", &self.capacity())
            .finish()
    }
}

#[cfg(target_os = "linux")]
// SAFETY: AlignedBuffer owns its allocation exclusively and only exposes
// shared/mutable access through Rust references; moving it to another thread
// transfers ownership of the allocation.
unsafe impl Send for AlignedBuffer {}

#[cfg(target_os = "linux")]
// SAFETY: &AlignedBuffer only provides &[u8] access (via Deref). Immutable
// byte slices are trivially safe to share across threads.
unsafe impl Sync for AlignedBuffer {}

// ============================================================================
// MmapRegion — Arc-backed memory-mapped file region
// ============================================================================

/// A memory-mapped file region backed by an `Arc<memmap2::Mmap>`.
///
/// Cheaply cloneable; `as_slice()` returns exactly `len` bytes starting at
/// `start` within the underlying mapping.
#[cfg(feature = "mmap")]
#[derive(Debug, Clone)]
pub struct MmapRegion {
    inner: Arc<memmap2::Mmap>,
    start: usize,
    len: usize,
}

#[cfg(feature = "mmap")]
impl MmapRegion {
    pub(crate) fn new(inner: Arc<memmap2::Mmap>, start: usize, len: usize) -> Self {
        debug_assert!(start.checked_add(len).is_some_and(|end| end <= inner.len()));
        Self { inner, start, len }
    }

    #[inline]
    #[must_use]
    pub fn subregion(&self, offset: usize, len: usize) -> Option<Self> {
        let relative_end = offset.checked_add(len)?;
        if relative_end > self.len {
            return None;
        }
        let start = self.start.checked_add(offset)?;
        Some(Self::new(Arc::clone(&self.inner), start, len))
    }

    #[inline]
    #[must_use]
    pub fn as_slice(&self) -> &[u8] {
        &self.inner[self.start..self.start + self.len]
    }
    #[inline]
    #[must_use]
    pub const fn len(&self) -> usize {
        self.len
    }
    #[inline]
    #[must_use]
    pub const fn is_empty(&self) -> bool {
        self.len == 0
    }
}

#[cfg(feature = "mmap")]
impl AsRef<[u8]> for MmapRegion {
    fn as_ref(&self) -> &[u8] {
        self.as_slice()
    }
}

#[cfg(feature = "mmap")]
impl std::ops::Deref for MmapRegion {
    type Target = [u8];
    fn deref(&self) -> &[u8] {
        self.as_slice()
    }
}

// ============================================================================
// BufferAllocator
// ============================================================================

/// Configuration for the buffer pool used by [`BufferAllocator::Pooled`].
#[cfg(feature = "pool")]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PoolConfig {
    /// Number of independent shards to reduce contention (default: 8).
    pub num_shards: usize,
    /// Thread-local cache capacity per thread (default: 4).
    pub tls_cache_size: usize,
    /// Maximum buffers retained per shard (default: 32).
    pub max_per_shard: usize,
    /// Minimum buffer size the pool will manage; smaller requests
    /// bypass the pool and allocate directly (default: 1 MiB).
    pub min_buffer_size: usize,
}

#[cfg(feature = "pool")]
impl Default for PoolConfig {
    fn default() -> Self {
        Self {
            num_shards: 8,
            tls_cache_size: 4,
            max_per_shard: 32,
            min_buffer_size: 1024 * 1024,
        }
    }
}

/// Controls how I/O backends allocate read buffers.
///
/// - [`Pooled`](Self::Pooled): Reuses buffers from a process-wide pool,
///   amortising allocation cost across repeated reads.
/// - [`System`](Self::System): Every read allocates a fresh `Vec<u8>` via the
///   system allocator and frees it on drop.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BufferAllocator {
    /// Reuse buffers from the process-wide pool.
    #[cfg(feature = "pool")]
    Pooled(PoolConfig),
    /// Allocate fresh `Vec<u8>` buffers via the system allocator.
    System,
}

impl Default for BufferAllocator {
    fn default() -> Self {
        #[cfg(feature = "pool")]
        {
            Self::Pooled(PoolConfig::default())
        }
        #[cfg(not(feature = "pool"))]
        {
            Self::System
        }
    }
}

impl BufferAllocator {
    /// Allocate a zeroed buffer of `len` bytes.
    #[inline]
    pub fn alloc(&self, len: usize) -> OwnedBytes {
        match self {
            #[cfg(feature = "pool")]
            Self::Pooled(_) => OwnedBytes::Pooled(get_buffer_pool().get(len)),
            Self::System => OwnedBytes::Vec(vec![0u8; len]),
        }
    }
}

/// Returns the process-wide sharded buffer pool.
///
/// Initialised on first call with [`PoolConfig::default`] settings.
#[cfg(feature = "pool")]
#[must_use]
pub(crate) fn get_buffer_pool() -> &'static BufferPool {
    use std::sync::OnceLock;
    static POOL: OnceLock<BufferPool> = OnceLock::new();
    POOL.get_or_init(|| {
        let cfg = PoolConfig::default();
        BufferPool::builder()
            .num_shards(cfg.num_shards)
            .tls_cache_size(cfg.tls_cache_size)
            .max_buffers_per_shard(cfg.max_per_shard)
            .min_buffer_size(cfg.min_buffer_size)
            .build()
    })
}

// ============================================================================
// OwnedBytes
// ============================================================================

/// Owned byte buffer backed by one of several storage strategies.
///
/// All variants provide uniform read access via `AsRef<[u8]>` / `Deref`.
/// Only the `Pooled`, `Aligned`, and `Vec` variants support mutable access.
pub enum OwnedBytes {
    /// A buffer returned from the global [`BufferPool`].
    #[cfg(feature = "pool")]
    Pooled(PooledBuffer),
    /// An O_DIRECT aligned buffer (Linux only).
    #[cfg(target_os = "linux")]
    Aligned(AlignedBuffer),
    /// A memory-mapped file region (zero-copy, read-only).
    #[cfg(feature = "mmap")]
    Mmap(MmapRegion),
    /// A plain heap-allocated buffer.
    Vec(Vec<u8>),
}

impl OwnedBytes {
    /// Wrap a pooled buffer.
    #[cfg(feature = "pool")]
    #[inline]
    #[must_use]
    pub fn from_pooled(buf: PooledBuffer) -> Self {
        Self::Pooled(buf)
    }

    /// Wrap an O_DIRECT aligned buffer (Linux only).
    #[cfg(target_os = "linux")]
    #[inline]
    #[must_use]
    pub fn from_aligned(buf: AlignedBuffer) -> Self {
        Self::Aligned(buf)
    }

    /// Wrap a plain `Vec<u8>`.
    #[inline]
    #[must_use]
    pub fn from_vec(v: Vec<u8>) -> Self {
        Self::Vec(v)
    }

    /// Number of bytes.
    #[inline]
    #[must_use]
    pub fn len(&self) -> usize {
        match self {
            #[cfg(feature = "pool")]
            Self::Pooled(b) => b.len(),
            #[cfg(target_os = "linux")]
            Self::Aligned(b) => b.len(),
            #[cfg(feature = "mmap")]
            Self::Mmap(b) => b.len(),
            Self::Vec(b) => b.len(),
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
    /// Returns `None` for [`OwnedBytes::Mmap`], which is immutable.
    #[inline]
    #[must_use]
    pub fn as_mut_slice(&mut self) -> Option<&mut [u8]> {
        match self {
            #[cfg(feature = "pool")]
            Self::Pooled(b) => Some(b.as_mut_slice()),
            #[cfg(target_os = "linux")]
            Self::Aligned(b) => Some(b.as_mut_slice()),
            #[cfg(feature = "mmap")]
            Self::Mmap(_) => None,
            Self::Vec(b) => Some(b.as_mut_slice()),
        }
    }

    /// Convert to a `Vec<u8>`.
    ///
    /// Copies when backed by `Aligned` or `Mmap` storage to avoid
    /// mismatched-allocator UB.
    #[must_use]
    pub fn into_vec(self) -> Vec<u8> {
        match self {
            #[cfg(feature = "pool")]
            Self::Pooled(b) => b.into_inner(),
            #[cfg(target_os = "linux")]
            Self::Aligned(b) => b.as_slice().to_vec(),
            #[cfg(feature = "mmap")]
            Self::Mmap(b) => b.as_slice().to_vec(),
            Self::Vec(b) => b,
        }
    }

    /// Convert to an `Arc<[u8]>`.
    ///
    /// Copies when backed by `Aligned` or `Mmap` storage.
    #[must_use]
    pub fn into_shared(self) -> Arc<[u8]> {
        match self {
            #[cfg(feature = "pool")]
            Self::Pooled(b) => b.into_inner().into(),
            #[cfg(target_os = "linux")]
            Self::Aligned(b) => b.as_slice().into(),
            #[cfg(feature = "mmap")]
            Self::Mmap(b) => Arc::from(b.as_slice()),
            Self::Vec(b) => b.into(),
        }
    }
}

impl AsRef<[u8]> for OwnedBytes {
    #[inline]
    fn as_ref(&self) -> &[u8] {
        match self {
            #[cfg(feature = "pool")]
            Self::Pooled(b) => b.as_ref(),
            #[cfg(target_os = "linux")]
            Self::Aligned(b) => b.as_slice(),
            #[cfg(feature = "mmap")]
            Self::Mmap(b) => b.as_slice(),
            Self::Vec(b) => b.as_ref(),
        }
    }
}

impl std::ops::Deref for OwnedBytes {
    type Target = [u8];

    #[inline]
    fn deref(&self) -> &Self::Target {
        self.as_ref()
    }
}

impl std::fmt::Debug for OwnedBytes {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("OwnedBytes")
            .field("len", &self.len())
            .finish_non_exhaustive()
    }
}

impl From<Vec<u8>> for OwnedBytes {
    #[inline]
    fn from(v: Vec<u8>) -> Self {
        Self::Vec(v)
    }
}

#[cfg(feature = "pool")]
impl From<PooledBuffer> for OwnedBytes {
    #[inline]
    fn from(buf: PooledBuffer) -> Self {
        Self::Pooled(buf)
    }
}

#[cfg(feature = "mmap")]
impl From<MmapRegion> for OwnedBytes {
    #[inline]
    fn from(region: MmapRegion) -> Self {
        Self::Mmap(region)
    }
}

#[cfg(target_os = "linux")]
impl From<AlignedBuffer> for OwnedBytes {
    #[inline]
    fn from(buf: AlignedBuffer) -> Self {
        Self::Aligned(buf)
    }
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
        let ob = OwnedBytes::from_vec(data.clone());
        assert_eq!(ob.as_ref(), &data[..]);
    }

    #[cfg(feature = "pool")]
    #[test]
    fn from_pooled_roundtrips() {
        let pool = get_buffer_pool();
        let mut buf = pool.get(8);
        buf[..4].copy_from_slice(&[10, 20, 30, 40]);
        let ob = OwnedBytes::from_pooled(buf);
        assert_eq!(&ob.as_ref()[..4], &[10, 20, 30, 40]);
    }

    #[test]
    fn vec_variant() {
        let data = vec![5u8, 6, 7];
        let ob = OwnedBytes::Vec(data.clone());
        assert_eq!(ob.as_ref(), &data[..]);
    }

    #[cfg(feature = "mmap")]
    #[test]
    fn mmap_variant_accessible() {
        let ob = OwnedBytes::Mmap(make_mmap_region());
        assert_eq!(ob.as_ref(), b"hello_mmap");
    }

    #[test]
    fn len_and_is_empty_vec() {
        let ob = OwnedBytes::from_vec(vec![1, 2]);
        assert_eq!(ob.len(), 2);
        assert!(!ob.is_empty());
        let empty = OwnedBytes::from_vec(vec![]);
        assert!(empty.is_empty());
    }

    #[test]
    fn deref_matches_as_ref() {
        let ob = OwnedBytes::from_vec(vec![42u8; 8]);
        let via_deref: &[u8] = &ob;
        assert_eq!(via_deref, ob.as_ref());
    }

    #[test]
    fn debug_shows_len() {
        let ob = OwnedBytes::from_vec(vec![0u8; 17]);
        assert!(format!("{ob:?}").contains("17"));
    }

    #[test]
    fn as_mut_slice_some_for_vec() {
        let mut ob = OwnedBytes::from_vec(vec![0u8; 4]);
        ob.as_mut_slice().unwrap()[0] = 99;
        assert_eq!(ob.as_ref()[0], 99);
    }

    #[cfg(feature = "pool")]
    #[test]
    fn as_mut_slice_some_for_pooled() {
        let pool = get_buffer_pool();
        let buf = pool.get(4);
        let mut ob = OwnedBytes::Pooled(buf);
        assert!(ob.as_mut_slice().is_some());
    }

    #[cfg(feature = "mmap")]
    #[test]
    fn as_mut_slice_none_for_mmap() {
        let mut ob = OwnedBytes::Mmap(make_mmap_region());
        assert!(ob.as_mut_slice().is_none());
    }

    #[test]
    fn into_vec_preserves_bytes() {
        let data = vec![7u8, 8, 9];
        assert_eq!(OwnedBytes::from_vec(data.clone()).into_vec(), data);
        #[cfg(feature = "mmap")]
        assert_eq!(
            OwnedBytes::Mmap(make_mmap_region()).into_vec(),
            b"hello_mmap"
        );
    }

    #[test]
    fn into_shared_preserves_bytes() {
        let data = vec![1u8, 2, 3];
        let shared = OwnedBytes::from_vec(data.clone()).into_shared();
        assert_eq!(shared.as_ref(), &data[..]);
        #[cfg(feature = "mmap")]
        let shared2 = OwnedBytes::Mmap(make_mmap_region()).into_shared();
        #[cfg(feature = "mmap")]
        assert_eq!(shared2.as_ref(), b"hello_mmap");
    }

    #[cfg(feature = "pool")]
    #[test]
    fn buffer_pool_is_singleton() {
        let p1 = get_buffer_pool() as *const _;
        let p2 = get_buffer_pool() as *const _;
        assert_eq!(p1, p2);
    }
}
