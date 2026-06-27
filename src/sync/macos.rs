use std::fs::{Metadata, Permissions};
use std::io::{self, Read, Seek, SeekFrom, Write};
use std::os::unix::fs::FileExt;
use std::path::Path;

use crate::{Bytes, WriteSlices};

/// A synchronous macOS file handle.
#[derive(Debug)]
pub struct File {
    inner: std::fs::File,
}

impl File {
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

    pub fn try_clone(&self) -> io::Result<Self> {
        Ok(Self {
            inner: self.inner.try_clone()?,
        })
    }

    pub fn metadata(&self) -> io::Result<Metadata> {
        self.inner.metadata()
    }

    pub fn set_len(&self, size: u64) -> io::Result<()> {
        self.inner.set_len(size)
    }

    pub fn sync_all(&self) -> io::Result<()> {
        self.inner.sync_all()
    }

    pub fn sync_data(&self) -> io::Result<()> {
        self.inner.sync_data()
    }

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

    pub fn read_exact_at(&self, offset: u64, buf: &mut [u8]) -> io::Result<()> {
        self.inner.read_exact_at(buf, offset)
    }

    pub fn write_all_at(&self, offset: u64, buf: &[u8]) -> io::Result<()> {
        self.inner.write_all_at(buf, offset)
    }

    pub fn write_slices_at(&self, writes: WriteSlices<'_, '_>) -> io::Result<()> {
        for write in writes.as_slice() {
            self.write_all_at(write.offset, write.data)?;
        }
        Ok(())
    }
}

impl Read for File {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        self.inner.read(buf)
    }
}

impl Write for File {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        self.inner.write(buf)
    }

    fn flush(&mut self) -> io::Result<()> {
        self.inner.flush()
    }
}

impl Seek for File {
    fn seek(&mut self, pos: SeekFrom) -> io::Result<u64> {
        self.inner.seek(pos)
    }
}

impl AsRef<std::fs::File> for File {
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

    pub fn read(&mut self, read: bool) -> &mut Self {
        self.inner.read(read);
        self
    }

    pub fn write(&mut self, write: bool) -> &mut Self {
        self.inner.write(write);
        self
    }

    pub fn append(&mut self, append: bool) -> &mut Self {
        self.inner.append(append);
        self
    }

    pub fn truncate(&mut self, truncate: bool) -> &mut Self {
        self.inner.truncate(truncate);
        self
    }

    pub fn create(&mut self, create: bool) -> &mut Self {
        self.inner.create(create);
        self
    }

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
