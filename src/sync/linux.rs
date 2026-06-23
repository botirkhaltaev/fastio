use std::fs::{File, OpenOptions};
use std::io;
use std::os::unix::fs::{FileExt, OpenOptionsExt};
use std::path::Path;

pub(super) fn open(options: &OpenOptions, direct_io: bool, path: &Path) -> io::Result<File> {
    let mut options = options.clone();
    if direct_io {
        options.custom_flags(libc::O_DIRECT);
    }
    options.open(path)
}

pub(super) fn read_exact_at(file: &File, offset: u64, buf: &mut [u8]) -> io::Result<()> {
    file.read_exact_at(buf, offset)
}

pub(super) fn write_all_at(file: &File, offset: u64, buf: &[u8]) -> io::Result<()> {
    file.write_all_at(buf, offset)
}
