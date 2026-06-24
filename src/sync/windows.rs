use std::fs::{Metadata, Permissions};
use std::io::{self, Read, Seek, SeekFrom, Write};
use std::os::windows::fs::FileExt;
use std::path::Path;

use crate::{Allocator, DefaultAllocator, OwnedBytes, WriteSlices};

/// A synchronous Windows file handle.
#[derive(Debug)]
pub struct File<A = DefaultAllocator> {
    inner: std::fs::File,
    allocator: A,
}

impl<A> File<A> {
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
}

impl File<DefaultAllocator> {
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
}

impl<A: Allocator> File<A> {
    #[inline]
    pub fn try_clone(&self) -> io::Result<Self> {
        Ok(Self {
            inner: self.inner.try_clone()?,
            allocator: self.allocator.clone(),
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

    pub fn read_all(&self) -> io::Result<OwnedBytes> {
        let len = usize::try_from(self.inner.metadata()?.len())
            .map_err(|_| io::Error::other("file too large"))?;
        if len == 0 {
            return Ok(OwnedBytes::Vec(Vec::new()));
        }
        let mut bytes = self.allocator.allocate(len);
        let buf = bytes
            .as_mut_slice()
            .ok_or_else(|| io::Error::other("allocator returned immutable buffer"))?;
        if buf.len() != len {
            return Err(io::Error::other("allocator returned wrong-sized buffer"));
        }
        self.read_exact_at(0, buf)?;
        Ok(bytes)
    }

    pub fn read_at(&self, offset: u64, len: usize) -> io::Result<OwnedBytes> {
        if len == 0 {
            return Ok(OwnedBytes::Vec(Vec::new()));
        }
        let mut bytes = self.allocator.allocate(len);
        let buf = bytes
            .as_mut_slice()
            .ok_or_else(|| io::Error::other("allocator returned immutable buffer"))?;
        if buf.len() != len {
            return Err(io::Error::other("allocator returned wrong-sized buffer"));
        }
        self.read_exact_at(offset, buf)?;
        Ok(bytes)
    }

    pub fn read_exact_at(&self, offset: u64, buf: &mut [u8]) -> io::Result<()> {
        let mut handle = &self.inner;
        let original_position = handle.stream_position()?;
        let mut read = 0usize;
        let mut result = Ok(());
        while read < buf.len() {
            let read_offset = offset.checked_add(read as u64).ok_or_else(|| {
                io::Error::new(io::ErrorKind::InvalidInput, "read offset overflow")
            })?;
            let n = self.inner.seek_read(&mut buf[read..], read_offset)?;
            if n == 0 {
                result = Err(io::Error::new(
                    io::ErrorKind::UnexpectedEof,
                    "seek_read returned zero bytes before buffer was filled",
                ));
                break;
            }
            read += n;
        }
        let restore = handle.seek(SeekFrom::Start(original_position));
        result.and(restore.map(|_| ()))
    }

    pub fn write_all_at(&self, offset: u64, buf: &[u8]) -> io::Result<()> {
        let mut handle = &self.inner;
        let original_position = handle.stream_position()?;
        let mut written = 0usize;
        let mut result = Ok(());
        while written < buf.len() {
            let write_offset = offset.checked_add(written as u64).ok_or_else(|| {
                io::Error::new(io::ErrorKind::InvalidInput, "write offset overflow")
            })?;
            let n = self.inner.seek_write(&buf[written..], write_offset)?;
            if n == 0 {
                result = Err(io::Error::new(
                    io::ErrorKind::WriteZero,
                    "seek_write returned zero bytes",
                ));
                break;
            }
            written += n;
        }
        let restore = handle.seek(SeekFrom::Start(original_position));
        result.and(restore.map(|_| ()))
    }

    pub fn write_slices_at(&self, writes: WriteSlices<'_, '_>) -> io::Result<()> {
        for write in writes.as_slice() {
            self.write_all_at(write.offset, write.data)?;
        }
        Ok(())
    }
}

impl<A> Read for File<A> {
    #[inline]
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        self.inner.read(buf)
    }
}

impl<A> Write for File<A> {
    #[inline]
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        self.inner.write(buf)
    }

    #[inline]
    fn flush(&mut self) -> io::Result<()> {
        self.inner.flush()
    }
}

impl<A> Seek for File<A> {
    #[inline]
    fn seek(&mut self, pos: SeekFrom) -> io::Result<u64> {
        self.inner.seek(pos)
    }
}

impl<A> AsRef<std::fs::File> for File<A> {
    #[inline]
    fn as_ref(&self) -> &std::fs::File {
        &self.inner
    }
}

#[derive(Debug, Clone)]
pub struct OpenOptions<A = DefaultAllocator> {
    inner: std::fs::OpenOptions,
    allocator: A,
}

impl OpenOptions<DefaultAllocator> {
    #[must_use]
    pub fn new() -> Self {
        Self {
            inner: std::fs::OpenOptions::new(),
            allocator: DefaultAllocator::default(),
        }
    }
}

impl<A: Allocator> OpenOptions<A> {
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

    pub fn allocator<B: Allocator>(&self, allocator: B) -> OpenOptions<B> {
        OpenOptions {
            inner: self.inner.clone(),
            allocator,
        }
    }

    pub fn open<P: AsRef<Path>>(&self, path: P) -> io::Result<File<A>> {
        Ok(File {
            inner: self.inner.open(path)?,
            allocator: self.allocator.clone(),
        })
    }
}

impl Default for OpenOptions<DefaultAllocator> {
    fn default() -> Self {
        Self::new()
    }
}
