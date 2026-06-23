//! Synchronous blocking file I/O.
//!
//! Each supported platform owns its `File` and `OpenOptions` implementation.

#[cfg(target_os = "linux")]
mod linux;
#[cfg(target_os = "macos")]
mod macos;
#[cfg(target_os = "windows")]
mod windows;

#[cfg(target_os = "linux")]
pub use linux::{File, OpenOptions};
#[cfg(target_os = "macos")]
pub use macos::{File, OpenOptions};
#[cfg(target_os = "windows")]
pub use windows::{File, OpenOptions};

#[cfg(not(any(target_os = "linux", target_os = "macos", target_os = "windows")))]
compile_error!("fastio sync supports Linux, macOS, and Windows only");

#[cfg(test)]
mod file_api_tests {
    use super::*;
    use crate::{Allocator, OwnedBytes, System, WriteSlice, WriteSlices};
    use std::io::{Read, Seek, SeekFrom};
    use tempfile::TempDir;

    #[derive(Debug, Clone)]
    struct ShortAllocator;

    impl Allocator for ShortAllocator {
        fn allocate(&self, len: usize) -> OwnedBytes {
            OwnedBytes::Vec(vec![0; len.saturating_sub(1)])
        }
    }

    #[test]
    fn file_read_all_reads_entire_file() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("model.bin");
        std::fs::write(&path, b"abcdef").unwrap();

        let bytes = File::open(&path).unwrap().read_all().unwrap();

        assert_eq!(bytes.as_ref(), b"abcdef");
    }

    #[test]
    fn file_read_at_reads_positioned_bytes() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("model.bin");
        std::fs::write(&path, b"abcdef").unwrap();

        let file = File::open(&path).unwrap();
        let bytes = file.read_at(2, 3).unwrap();

        assert_eq!(bytes.as_ref(), b"cde");
    }

    #[cfg(feature = "pool")]
    #[test]
    fn default_allocator_returns_pooled_read_buffer() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("model.bin");
        std::fs::write(&path, b"abcdef").unwrap();

        let bytes = File::open(&path).unwrap().read_all().unwrap();

        assert!(matches!(&bytes, OwnedBytes::Pooled(_)));
        assert_eq!(bytes.as_ref(), b"abcdef");
    }

    #[test]
    fn system_allocator_returns_vec_read_buffer() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("model.bin");
        std::fs::write(&path, b"abcdef").unwrap();

        let file = OpenOptions::new()
            .read(true)
            .allocator(System)
            .open(&path)
            .unwrap();
        let bytes = file.read_at(1, 3).unwrap();

        assert!(matches!(&bytes, OwnedBytes::Vec(_)));
        assert_eq!(bytes.as_ref(), b"bcd");
    }

    #[test]
    fn file_write_all_at_writes_positioned_bytes() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("model.bin");
        std::fs::write(&path, b"abcdef").unwrap();

        let file = OpenOptions::new().write(true).open(&path).unwrap();
        file.write_all_at(2, b"XX").unwrap();

        assert_eq!(std::fs::read(&path).unwrap(), b"abXXef");
    }

    #[test]
    fn read_all_does_not_move_cursor() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("model.bin");
        std::fs::write(&path, b"abcdef").unwrap();

        let mut file = OpenOptions::new().read(true).open(&path).unwrap();
        file.seek(SeekFrom::Start(2)).unwrap();
        let bytes = file.read_all().unwrap();
        let mut next = [0u8; 1];
        file.read_exact(&mut next).unwrap();

        assert_eq!(bytes.as_ref(), b"abcdef");
        assert_eq!(next, [b'c']);
    }

    #[test]
    fn create_new_fails_when_file_exists() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("existing.bin");
        std::fs::write(&path, b"already here").unwrap();

        let err = File::create_new(&path).unwrap_err();

        assert_eq!(err.kind(), std::io::ErrorKind::AlreadyExists);
    }

    #[test]
    fn write_slices_at_rejects_overlapping_slices() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("model.bin");
        std::fs::write(&path, b"----------").unwrap();
        let slices = [WriteSlice::new(0, b"abcdef"), WriteSlice::new(3, b"XYZ")];

        let err = WriteSlices::new(&slices).unwrap_err();

        assert_eq!(err.kind(), std::io::ErrorKind::InvalidInput);
    }

    #[test]
    fn read_reports_allocator_contract_violation() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("model.bin");
        std::fs::write(&path, b"abcdef").unwrap();
        let file = OpenOptions::new()
            .read(true)
            .allocator(ShortAllocator)
            .open(&path)
            .unwrap();

        let err = file.read_at(0, 3).unwrap_err();

        assert_eq!(err.kind(), std::io::ErrorKind::Other);
    }
}
