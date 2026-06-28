use std::fs::{Metadata, Permissions};
use std::io::{self, Read, Seek, SeekFrom, Write};
use std::os::windows::fs::FileExt;
use std::path::Path;

use crate::{Bytes, WriteSlices};

/// A synchronous Windows file handle.
#[derive(Debug)]
pub struct File {
    inner: std::fs::File,
}

impl File {
    /// Returns a reference to the underlying `std::fs::File`.
    #[inline]
    #[must_use]
    pub const fn get_ref(&self) -> &std::fs::File {
        &self.inner
    }

    /// Consumes this handle, returning the underlying `std::fs::File`.
    #[inline]
    #[must_use]
    pub fn into_inner(self) -> std::fs::File {
        self.inner
    }

    pub fn open<P: AsRef<Path>>(path: P) -> io::Result<Self> {
        OpenOptions::new().read(true).open(path)
    }

    pub fn create<P: AsRef<Path>>(path: P) -> io::Result<Self> {
        OpenOptions::new()
            .write(true)
            .create(true)
            .truncate(true)
            .open(path)
    }

    pub fn create_new<P: AsRef<Path>>(path: P) -> io::Result<Self> {
        OpenOptions::new().write(true).create_new(true).open(path)
    }

    #[must_use]
    pub fn options() -> OpenOptions {
        OpenOptions::new()
    }

    #[inline]
    pub fn try_clone(&self) -> io::Result<Self> {
        Ok(Self {
            inner: self.inner.try_clone()?,
        })
    }

    #[inline]
    pub fn metadata(&self) -> io::Result<Metadata> {
        self.inner.metadata()
    }

    #[inline]
    pub fn set_len(&self, size: u64) -> io::Result<()> {
        self.inner.set_len(size)
    }

    #[inline]
    pub fn sync_all(&self) -> io::Result<()> {
        self.inner.sync_all()
    }

    #[inline]
    pub fn sync_data(&self) -> io::Result<()> {
        self.inner.sync_data()
    }

    #[inline]
    pub fn set_permissions(&self, perm: Permissions) -> io::Result<()> {
        self.inner.set_permissions(perm)
    }

    pub fn read_all(&self) -> io::Result<Bytes> {
        let len = usize::try_from(self.inner.metadata()?.len())
            .map_err(|_| io::Error::other("file too large"))?;
        Bytes::allocate(len, |buf| self.read_exact_at(0, buf))
    }

    pub fn read_at(&self, offset: u64, len: usize) -> io::Result<Bytes> {
        Bytes::allocate(len, |buf| self.read_exact_at(offset, buf))
    }

    /// Positioned read that does not require a seek.
    ///
    /// # Cursor semantics on Windows
    ///
    /// This implementation saves the current file pointer, performs the read
    /// via `seek_read`, and then restores the saved pointer. The save/restore
    /// is not atomic: concurrent operations on the same underlying handle (or
    /// a clone that shares the same kernel file object) may observe the
    /// temporary cursor position. The original read error is preserved if the
    /// restore also fails.
    pub fn read_exact_at(&self, offset: u64, buf: &mut [u8]) -> io::Result<()> {
        let mut handle = &self.inner;
        let original_position = handle.stream_position()?;
        let result = self.seek_read_exact(offset, buf);
        let restore = handle.seek(SeekFrom::Start(original_position));
        match result {
            Ok(()) => restore.map(|_| ()),
            Err(err) => Err(err),
        }
    }

    /// Positioned write that does not require a seek.
    ///
    /// # Cursor semantics on Windows
    ///
    /// This implementation saves the current file pointer, performs the write
    /// via `seek_write`, and then restores the saved pointer. The save/restore
    /// is not atomic: concurrent operations on the same underlying handle (or
    /// a clone that shares the same kernel file object) may observe the
    /// temporary cursor position. The original write error is preserved if the
    /// restore also fails.
    pub fn write_all_at(&self, offset: u64, buf: &[u8]) -> io::Result<()> {
        let mut handle = &self.inner;
        let original_position = handle.stream_position()?;
        let result = self.seek_write_exact(offset, buf);
        let restore = handle.seek(SeekFrom::Start(original_position));
        match result {
            Ok(()) => restore.map(|_| ()),
            Err(err) => Err(err),
        }
    }

    fn seek_read_exact(&self, offset: u64, buf: &mut [u8]) -> io::Result<()> {
        let mut read = 0usize;
        while read < buf.len() {
            let read_offset = offset.checked_add(read as u64).ok_or_else(|| {
                io::Error::new(io::ErrorKind::InvalidInput, "read offset overflow")
            })?;
            let n = self.inner.seek_read(&mut buf[read..], read_offset)?;
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

    fn seek_write_exact(&self, offset: u64, buf: &[u8]) -> io::Result<()> {
        let mut written = 0usize;
        while written < buf.len() {
            let write_offset = offset.checked_add(written as u64).ok_or_else(|| {
                io::Error::new(io::ErrorKind::InvalidInput, "write offset overflow")
            })?;
            let n = self.inner.seek_write(&buf[written..], write_offset)?;
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

    pub fn write_slices_at(&self, writes: WriteSlices<'_, '_>) -> io::Result<()> {
        for write in writes.as_slice() {
            self.write_all_at(write.offset, write.data)?;
        }
        Ok(())
    }
}

impl Read for File {
    #[inline]
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        self.inner.read(buf)
    }
}

impl Write for File {
    #[inline]
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        self.inner.write(buf)
    }

    #[inline]
    fn flush(&mut self) -> io::Result<()> {
        self.inner.flush()
    }
}

impl Seek for File {
    #[inline]
    fn seek(&mut self, pos: SeekFrom) -> io::Result<u64> {
        self.inner.seek(pos)
    }
}

impl AsRef<std::fs::File> for File {
    #[inline]
    fn as_ref(&self) -> &std::fs::File {
        &self.inner
    }
}

#[derive(Debug, Clone)]
pub struct OpenOptions {
    inner: std::fs::OpenOptions,
}

impl OpenOptions {
    #[must_use]
    pub fn new() -> Self {
        Self {
            inner: std::fs::OpenOptions::new(),
        }
    }

    #[inline]
    pub fn read(&mut self, read: bool) -> &mut Self {
        self.inner.read(read);
        self
    }

    #[inline]
    pub fn write(&mut self, write: bool) -> &mut Self {
        self.inner.write(write);
        self
    }

    #[inline]
    pub fn append(&mut self, append: bool) -> &mut Self {
        self.inner.append(append);
        self
    }

    #[inline]
    pub fn truncate(&mut self, truncate: bool) -> &mut Self {
        self.inner.truncate(truncate);
        self
    }

    #[inline]
    pub fn create(&mut self, create: bool) -> &mut Self {
        self.inner.create(create);
        self
    }

    #[inline]
    pub fn create_new(&mut self, create_new: bool) -> &mut Self {
        self.inner.create_new(create_new);
        self
    }

    pub fn open<P: AsRef<Path>>(&self, path: P) -> io::Result<File> {
        Ok(File {
            inner: self.inner.open(path)?,
        })
    }
}

impl Default for OpenOptions {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(all(test, windows))]
mod tests {
    use super::*;
    use std::io::{Read, Seek, SeekFrom};
    use tempfile::TempDir;

    #[test]
    fn read_exact_at_preserves_cursor_position() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("cursor.bin");
        std::fs::write(&path, b"abcdefghij").unwrap();
        let mut file = File::open(&path).unwrap();

        file.seek(SeekFrom::Start(2)).unwrap();
        let mut buf = [0u8; 3];
        file.read_exact_at(5, &mut buf).unwrap();
        assert_eq!(&buf, b"fgh");

        let mut after = [0u8; 1];
        file.read_exact(&mut after).unwrap();
        assert_eq!(&after, b"c");
    }

    #[test]
    fn write_all_at_preserves_cursor_position() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("cursor.bin");
        std::fs::write(&path, b"abcdefghij").unwrap();
        let mut file = OpenOptions::new()
            .read(true)
            .write(true)
            .open(&path)
            .unwrap();

        file.seek(SeekFrom::Start(1)).unwrap();
        file.write_all_at(6, b"XYZ").unwrap();

        let mut after = [0u8; 1];
        file.read_exact(&mut after).unwrap();
        assert_eq!(&after, b"b");

        drop(file);
        assert_eq!(std::fs::read(&path).unwrap(), b"abcdefXYZj");
    }

    #[test]
    fn cloned_handle_sees_cursor_restore() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("clone.bin");
        std::fs::write(&path, b"abcdefghij").unwrap();
        let mut file = File::open(&path).unwrap();
        let cloned = file.try_clone().unwrap();

        file.seek(SeekFrom::Start(4)).unwrap();
        let mut buf = [0u8; 2];
        cloned.read_exact_at(0, &mut buf).unwrap();
        assert_eq!(&buf, b"ab");

        // Cloned read should not move the original cursor.
        let mut after = [0u8; 1];
        file.read_exact(&mut after).unwrap();
        assert_eq!(&after, b"e");
    }

    #[test]
    fn read_at_offset_zero_len_returns_empty() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("empty.bin");
        std::fs::write(&path, b"hello").unwrap();
        let file = File::open(&path).unwrap();
        let result = file.read_at(0, 0).unwrap();
        assert!(result.is_empty());
    }

    #[test]
    fn read_at_beyond_file_returns_short_read_error() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("short.bin");
        std::fs::write(&path, b"hello").unwrap();
        let file = File::open(&path).unwrap();
        let err = file.read_at(10, 4).unwrap_err();
        assert_eq!(err.kind(), io::ErrorKind::UnexpectedEof);
    }
}
