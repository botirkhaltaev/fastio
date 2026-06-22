use std::io::{Error, ErrorKind};
use std::path::Path;

use crate::{IoResult, OwnedBytes};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ByteRange {
    start: u64,
    end: u64,
}

impl ByteRange {
    #[inline]
    pub fn new(start: u64, end: u64) -> IoResult<Self> {
        if end < start {
            return Err(Error::new(
                ErrorKind::InvalidInput,
                "byte range end is before start",
            ));
        }
        Ok(Self { start, end })
    }

    #[inline]
    pub fn from_offset_len(offset: u64, len: usize) -> IoResult<Self> {
        let len = u64::try_from(len).map_err(|e| {
            Error::new(
                ErrorKind::InvalidInput,
                format!("range length too large: {e}"),
            )
        })?;
        let end = offset
            .checked_add(len)
            .ok_or_else(|| Error::new(ErrorKind::InvalidInput, "byte range overflow"))?;
        Self::new(offset, end)
    }

    #[inline]
    #[must_use]
    pub const fn start(self) -> u64 {
        self.start
    }

    #[inline]
    #[must_use]
    pub const fn end(self) -> u64 {
        self.end
    }

    #[inline]
    #[must_use]
    pub const fn len(self) -> u64 {
        self.end - self.start
    }

    #[inline]
    pub fn len_usize(self) -> IoResult<usize> {
        usize::try_from(self.len()).map_err(|e| {
            Error::new(
                ErrorKind::InvalidInput,
                format!("range length too large: {e}"),
            )
        })
    }

    #[inline]
    #[must_use]
    pub const fn is_empty(self) -> bool {
        self.start == self.end
    }
}

#[derive(Debug, Clone, Copy)]
pub struct FileRange<'a> {
    pub path: &'a Path,
    pub range: ByteRange,
}

impl<'a> FileRange<'a> {
    #[inline]
    #[must_use]
    pub const fn new(path: &'a Path, range: ByteRange) -> Self {
        Self { path, range }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct RequestIndex(usize);

impl RequestIndex {
    #[inline]
    #[must_use]
    pub const fn new(n: usize) -> Self {
        Self(n)
    }

    #[inline]
    #[must_use]
    pub const fn as_usize(self) -> usize {
        self.0
    }
}

impl std::fmt::Display for RequestIndex {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.0)
    }
}

#[derive(Debug)]
pub struct RangeRead {
    pub request_index: RequestIndex,
    pub range: ByteRange,
    pub bytes: OwnedBytes,
}

impl RangeRead {
    #[inline]
    #[must_use]
    pub fn data(&self) -> &[u8] {
        &self.bytes
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn byte_range_new_validates_order() {
        let range = ByteRange::new(10, 20).unwrap();
        assert_eq!(range.start(), 10);
        assert_eq!(range.end(), 20);
        assert_eq!(range.len(), 10);
        assert!(!range.is_empty());
        assert!(ByteRange::new(20, 10).is_err());
    }

    #[test]
    fn byte_range_from_offset_len_validates_overflow() {
        let range = ByteRange::from_offset_len(10, 5).unwrap();
        assert_eq!(range, ByteRange::new(10, 15).unwrap());
        assert!(ByteRange::from_offset_len(u64::MAX, 1).is_err());
    }
}
