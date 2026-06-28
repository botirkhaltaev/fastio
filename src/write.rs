//! Shared vocabulary for non-overlapping batch writes.
//!
//! [`WriteSlice`] names a single `(offset, data)` pair and [`WriteSlices`]
//! validates that a collection of slices does not overlap.

use std::io::{Error, ErrorKind};

use crate::IoResult;

/// A single non-overlapping write region.
///
/// `WriteSlice` pairs a file offset with a byte slice. It is used by
/// [`WriteSlices`] to batch non-overlapping writes.
#[derive(Debug, Clone, Copy)]
pub struct WriteSlice<'a> {
    /// File offset where the slice begins.
    pub offset: u64,
    /// Bytes to write.
    pub data: &'a [u8],
}

impl<'a> WriteSlice<'a> {
    /// Creates a new write slice at `offset` with `data`.
    #[inline]
    #[must_use]
    pub const fn new(offset: u64, data: &'a [u8]) -> Self {
        Self { offset, data }
    }

    /// Returns the offset immediately after the slice.
    ///
    /// # Errors
    ///
    /// Returns `InvalidInput` if the slice length does not fit in `u64` or if
    /// `offset + data.len()` overflows.
    #[inline]
    pub fn end_offset(self) -> IoResult<u64> {
        let len = u64::try_from(self.data.len()).map_err(|e| {
            Error::new(
                ErrorKind::InvalidInput,
                format!("write length too large: {e}"),
            )
        })?;
        self.offset
            .checked_add(len)
            .ok_or_else(|| Error::new(ErrorKind::InvalidInput, "write offset overflow"))
    }
}

/// A validated, non-overlapping collection of write slices.
///
/// Constructed from a slice of [`WriteSlice`] values. Empty slices are allowed
/// and ignored when checking for overlaps.
#[derive(Debug, Clone, Copy)]
pub struct WriteSlices<'s, 'd>(&'s [WriteSlice<'d>]);

impl<'s, 'd> WriteSlices<'s, 'd> {
    /// Validates that the slices are non-overlapping and returns the collection.
    ///
    /// # Errors
    ///
    /// Returns `InvalidInput` if any slice overflows `u64` or if two
    /// non-empty slices overlap.
    pub fn new(slices: &'s [WriteSlice<'d>]) -> IoResult<Self> {
        let mut sorted: Vec<(u64, u64)> = slices
            .iter()
            .filter(|w| !w.data.is_empty())
            .map(|w| w.end_offset().map(|end| (w.offset, end)))
            .collect::<IoResult<_>>()?;
        sorted.sort_unstable_by_key(|&(start, _)| start);
        for pair in sorted.windows(2) {
            if pair[0].1 > pair[1].0 {
                return Err(Error::new(ErrorKind::InvalidInput, "write slices overlap"));
            }
        }
        Ok(Self(slices))
    }

    /// Returns the underlying slice.
    #[inline]
    #[must_use]
    pub const fn as_slice(self) -> &'s [WriteSlice<'d>] {
        self.0
    }

    /// Returns the number of slices, including empty ones.
    #[inline]
    #[must_use]
    pub const fn len(self) -> usize {
        self.0.len()
    }

    /// Returns `true` if there are no slices.
    #[inline]
    #[must_use]
    pub const fn is_empty(self) -> bool {
        self.0.is_empty()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn write_slice_end_offset_validates_overflow() {
        assert_eq!(WriteSlice::new(10, b"abc").end_offset().unwrap(), 13);
        assert!(WriteSlice::new(u64::MAX, b"x").end_offset().is_err());
    }

    #[test]
    fn write_slices_detects_overlap() {
        let a = WriteSlice::new(0, b"AAAAA");
        let b = WriteSlice::new(3, b"BBBBB");
        let err = WriteSlices::new(&[a, b]).unwrap_err();
        assert_eq!(err.kind(), ErrorKind::InvalidInput);
    }

    #[test]
    fn write_slices_allows_empty_slice_inside_non_empty_range() {
        let a = WriteSlice::new(0, b"AAAAA");
        let b = WriteSlice::new(3, b"");
        assert!(WriteSlices::new(&[a, b]).is_ok());
    }

    #[test]
    fn write_slices_allows_adjacent_ranges() {
        let a = WriteSlice::new(0, b"abc");
        let b = WriteSlice::new(3, b"def");
        let slices = [a, b];

        let writes = WriteSlices::new(&slices).unwrap();

        assert_eq!(writes.len(), 2);
    }

    #[test]
    fn write_slices_allows_unsorted_non_overlapping_ranges() {
        let first = WriteSlice::new(10, b"tail");
        let second = WriteSlice::new(0, b"head");
        let slices = [first, second];

        let writes = WriteSlices::new(&slices).unwrap();

        assert_eq!(writes.as_slice()[0].offset, 10);
        assert_eq!(writes.as_slice()[1].offset, 0);
    }
}
