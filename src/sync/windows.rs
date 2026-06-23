use std::fs::{File, OpenOptions};
use std::io;
use std::os::windows::fs::FileExt;
use std::path::Path;

pub(super) fn open(options: &OpenOptions, direct_io: bool, path: &Path) -> io::Result<File> {
    if direct_io {
        return Err(io::Error::new(
            io::ErrorKind::Unsupported,
            "direct I/O is only supported on Linux",
        ));
    }
    options.open(path)
}

pub(super) fn read_exact_at(file: &File, offset: u64, buf: &mut [u8]) -> io::Result<()> {
    let mut read = 0usize;
    while read < buf.len() {
        let n = file.seek_read(&mut buf[read..], offset + read as u64)?;
        if n == 0 {
            return Err(io::Error::new(
                io::ErrorKind::UnexpectedEof,
                "seek_read returned zero bytes before buffer was filled",
            ));
        }
        read += n;
    }
    Ok(())
}

pub(super) fn write_all_at(file: &File, offset: u64, buf: &[u8]) -> io::Result<()> {
    let mut written = 0usize;
    while written < buf.len() {
        let n = file.seek_write(&buf[written..], offset + written as u64)?;
        if n == 0 {
            return Err(io::Error::new(
                io::ErrorKind::WriteZero,
                "seek_write returned zero bytes",
            ));
        }
        written += n;
    }
    Ok(())
}
