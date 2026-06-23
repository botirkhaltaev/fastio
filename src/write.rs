use std::io::{Error, ErrorKind};

use crate::IoResult;

#[derive(Debug, Clone, Copy)]
pub struct WriteSlice<'a> {
    pub offset: u64,
    pub data: &'a [u8],
}

impl<'a> WriteSlice<'a> {
    #[inline]
    #[must_use]
    pub const fn new(offset: u64, data: &'a [u8]) -> Self {
        Self { offset, data }
    }

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

#[derive(Debug, Clone, Copy)]
pub struct WriteSlices<'s, 'd>(&'s [WriteSlice<'d>]);

impl<'s, 'd> WriteSlices<'s, 'd> {
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

    #[inline]
    #[must_use]
    pub const fn as_slice(self) -> &'s [WriteSlice<'d>] {
        self.0
    }

    #[inline]
    #[must_use]
    pub const fn len(self) -> usize {
        self.0.len()
    }

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
}
