use std::path::Path;

use crate::{ByteRange, FileRange, IoResult, OwnedBytes, RangeRead, WriteSlices};

pub trait BlockingIo {
    fn read_file(&self, path: &Path) -> IoResult<OwnedBytes>;
    fn read_range(&self, path: &Path, range: ByteRange) -> IoResult<OwnedBytes>;
    fn read_ranges(&self, ranges: &[FileRange<'_>]) -> IoResult<Vec<RangeRead>>;
    fn write_file(&self, path: &Path, data: &[u8]) -> IoResult<()>;
    fn write_positioned_file(&self, path: &Path, len: u64, writes: WriteSlices<'_>)
    -> IoResult<()>;
    fn write_at(&self, path: &Path, offset: u64, data: &[u8]) -> IoResult<()>;
    fn write_slices(&self, path: &Path, writes: WriteSlices<'_>) -> IoResult<()>;
    fn sync_data(&self, path: &Path) -> IoResult<()>;
    fn sync_all(&self, path: &Path) -> IoResult<()>;
}

#[cfg(feature = "tokio")]
pub trait AsyncIo {
    fn read_file<'a>(
        &'a self,
        path: &'a Path,
    ) -> impl std::future::Future<Output = IoResult<OwnedBytes>> + Send + 'a;

    fn read_range<'a>(
        &'a self,
        path: &'a Path,
        range: ByteRange,
    ) -> impl std::future::Future<Output = IoResult<OwnedBytes>> + Send + 'a;

    fn read_ranges<'a>(
        &'a self,
        ranges: &'a [FileRange<'a>],
    ) -> impl std::future::Future<Output = IoResult<Vec<RangeRead>>> + Send + 'a;

    fn write_file<'a>(
        &'a self,
        path: &'a Path,
        data: &'a [u8],
    ) -> impl std::future::Future<Output = IoResult<()>> + Send + 'a;

    fn write_positioned_file<'a>(
        &'a self,
        path: &'a Path,
        len: u64,
        writes: WriteSlices<'a>,
    ) -> impl std::future::Future<Output = IoResult<()>> + Send + 'a;

    fn write_at<'a>(
        &'a self,
        path: &'a Path,
        offset: u64,
        data: &'a [u8],
    ) -> impl std::future::Future<Output = IoResult<()>> + Send + 'a;

    fn write_slices<'a>(
        &'a self,
        path: &'a Path,
        writes: WriteSlices<'a>,
    ) -> impl std::future::Future<Output = IoResult<()>> + Send + 'a;

    fn sync_data<'a>(
        &'a self,
        path: &'a Path,
    ) -> impl std::future::Future<Output = IoResult<()>> + Send + 'a;

    fn sync_all<'a>(
        &'a self,
        path: &'a Path,
    ) -> impl std::future::Future<Output = IoResult<()>> + Send + 'a;
}

#[cfg(feature = "mmap")]
pub trait MmapIo {
    fn map_file(&self, path: &Path) -> IoResult<crate::MmapRegion>;
    fn map_range(&self, path: &Path, range: ByteRange) -> IoResult<crate::MmapRegion>;
}
