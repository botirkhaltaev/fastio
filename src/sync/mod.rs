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
    use tempfile::TempDir;

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
    fn open_options_direct_io_errors_on_non_linux() {
        #[cfg(not(target_os = "linux"))]
        {
            let dir = TempDir::new().unwrap();
            let path = dir.path().join("model.bin");
            std::fs::write(&path, b"abcdef").unwrap();

            let err = OpenOptions::new()
                .read(true)
                .direct_io(true)
                .open(&path)
                .unwrap_err();

            assert_eq!(err.kind(), std::io::ErrorKind::Unsupported);
        }
    }
}
