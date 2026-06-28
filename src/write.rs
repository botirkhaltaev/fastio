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

    #[test]
    fn write_slices_rejects_offset_overflow() {
        let a = WriteSlice::new(u64::MAX - 1, b"aa");
        let b = WriteSlice::new(0, b"x");
        let err = WriteSlices::new(&[a, b]).unwrap_err();
        assert_eq!(err.kind(), ErrorKind::InvalidInput);
    }

    #[test]
    fn write_slices_allows_many_non_overlapping_slices() {
        let data: Vec<Vec<u8>> = (0..100).map(|i| vec![i as u8; 2]).collect();
        let slices: Vec<_> = data
            .iter()
            .enumerate()
            .map(|(i, d)| WriteSlice::new((i * 2) as u64, d.as_slice()))
            .collect();
        assert!(WriteSlices::new(&slices).is_ok());
    }

    #[test]
    fn write_slices_detects_overlap_in_many_slices() {
        let overlap_data = b"overlap".to_vec();
        let data: Vec<Vec<u8>> = (0..50)
            .map(|i| {
                if i == 25 {
                    overlap_data.clone()
                } else {
                    vec![i as u8; 4]
                }
            })
            .collect();
        let mut slices: Vec<_> = data
            .iter()
            .enumerate()
            .map(|(i, d)| WriteSlice::new((i * 4) as u64, d.as_slice()))
            .collect();
        // Introduce an overlap.
        slices[25] = WriteSlice::new(90, data[25].as_slice());
        let err = WriteSlices::new(&slices).unwrap_err();
        assert_eq!(err.kind(), ErrorKind::InvalidInput);
    }

    #[test]
    fn write_slices_empty_input_is_ok() {
        let writes = WriteSlices::new(&[]).unwrap();
        assert!(writes.is_empty());
    }

    #[test]
    fn write_slices_all_empty_slices_are_ok() {
        let a = WriteSlice::new(0, b"");
        let b = WriteSlice::new(0, b"");
        let c = WriteSlice::new(100, b"");
        assert!(WriteSlices::new(&[a, b, c]).is_ok());
    }

    #[test]
    fn write_slices_boundary_touching_is_allowed() {
        let a = WriteSlice::new(0, b"abc");
        let b = WriteSlice::new(3, b"");
        let c = WriteSlice::new(3, b"def");
        let binding = [a, b, c];
        let writes = WriteSlices::new(&binding).unwrap();
        assert_eq!(writes.len(), 3);
    }
}
