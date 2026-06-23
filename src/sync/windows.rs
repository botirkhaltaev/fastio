use std::fs::{Metadata, Permissions};
use std::io::{self, Read, Seek, SeekFrom, Write};
use std::os::windows::fs::FileExt;
use std::path::Path;

use crate::{Allocator, DefaultAllocator, OwnedBytes, WriteSlices};

#[derive(Debug)]
pub struct File<A = DefaultAllocator> {
    inner: std::fs::File,
    allocator: A,
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
    pub fn try_clone(&self) -> io::Result<Self> {
        Ok(Self {
            inner: self.inner.try_clone()?,
            allocator: self.allocator.clone(),
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

    pub fn read_all(&self) -> io::Result<OwnedBytes> {
        let mut file = self.inner.try_clone()?;
        file.seek(SeekFrom::Start(0))?;
        let len = usize::try_from(file.metadata()?.len())
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
        file.read_exact(buf)?;
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
        let mut read = 0usize;
        while read < buf.len() {
            let n = self
                .inner
                .seek_read(&mut buf[read..], offset + read as u64)?;
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

    pub fn write_all_at(&self, offset: u64, buf: &[u8]) -> io::Result<()> {
        let mut written = 0usize;
        while written < buf.len() {
            let n = self
                .inner
                .seek_write(&buf[written..], offset + written as u64)?;
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

    pub fn write_slices_at(&self, writes: WriteSlices<'_>) -> io::Result<()> {
        for write in writes.as_slice() {
            self.write_all_at(write.offset, write.data)?;
        }
        Ok(())
    }
}

impl<A> Read for File<A> {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        self.inner.read(buf)
    }
}

impl<A> Write for File<A> {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        self.inner.write(buf)
    }

    fn flush(&mut self) -> io::Result<()> {
        self.inner.flush()
    }
}

impl<A> Seek for File<A> {
    fn seek(&mut self, pos: SeekFrom) -> io::Result<u64> {
        self.inner.seek(pos)
    }
}

impl<A> AsRef<std::fs::File> for File<A> {
    fn as_ref(&self) -> &std::fs::File {
        &self.inner
    }
}

#[derive(Debug, Clone)]
pub struct OpenOptions<A = DefaultAllocator> {
    inner: std::fs::OpenOptions,
    direct_io: bool,
    allocator: A,
}

impl OpenOptions<DefaultAllocator> {
    #[must_use]
    pub fn new() -> Self {
        Self {
            inner: std::fs::OpenOptions::new(),
            direct_io: false,
            allocator: DefaultAllocator::default(),
        }
    }
}

impl<A: Allocator> OpenOptions<A> {
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

    pub fn direct_io(&mut self, enabled: bool) -> &mut Self {
        self.direct_io = enabled;
        self
    }

    pub fn allocator<B: Allocator>(&self, allocator: B) -> OpenOptions<B> {
        OpenOptions {
            inner: self.inner.clone(),
            direct_io: self.direct_io,
            allocator,
        }
    }

    pub fn open<P: AsRef<Path>>(&self, path: P) -> io::Result<File<A>> {
        if self.direct_io {
            return Err(io::Error::new(
                io::ErrorKind::Unsupported,
                "direct I/O is only supported on Linux",
            ));
        }
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
