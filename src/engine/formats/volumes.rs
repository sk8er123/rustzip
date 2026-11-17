//! One logical byte stream over parts of one or more files (split archives),
//! readable at any offset through shared references, so several entry readers
//! can exist at once without seeking each other's cursor.

use std::cell::Cell;
use std::fs::File;
use std::io::{self, BufReader, Read, Seek, SeekFrom};

use super::super::Progress;

struct Part {
    file: File,
    /// Where the part starts inside its file.
    offset: u64,
    /// Where the part starts in the logical stream.
    start: u64,
    len: u64,
}

pub struct Volumes<'p> {
    parts: Vec<Part>,
    progress: &'p Progress,
    /// Furthest logical offset read so far; progress counts its growth.
    reached: Cell<u64>,
}

#[cfg(unix)]
fn read_at(file: &File, buf: &mut [u8], offset: u64) -> io::Result<usize> {
    std::os::unix::fs::FileExt::read_at(file, buf, offset)
}

#[cfg(windows)]
fn read_at(file: &File, buf: &mut [u8], offset: u64) -> io::Result<usize> {
    std::os::windows::fs::FileExt::seek_read(file, buf, offset)
}

impl<'p> Volumes<'p> {
    pub fn new(progress: &'p Progress) -> Self {
        Volumes { parts: Vec::new(), progress, reached: Cell::new(0) }
    }

    /// Appends `len` bytes of `file` starting at `offset`.
    pub fn push(&mut self, file: File, offset: u64, len: u64) {
        let start = self.len();
        self.parts.push(Part { file, offset, start, len });
        self.progress.set_total(self.len());
    }

    pub fn len(&self) -> u64 {
        self.parts.last().map_or(0, |p| p.start + p.len)
    }

    pub fn read_at(&self, pos: u64, buf: &mut [u8]) -> io::Result<usize> {
        if self.progress.cancelled() {
            return Err(io::Error::other("cancelled"));
        }
        let i = self.parts.partition_point(|p| p.start + p.len <= pos);
        let Some(part) = self.parts.get(i) else { return Ok(0) };
        let within = pos - part.start;
        let want = buf.len().min(usize::try_from(part.len - within).unwrap_or(usize::MAX));
        let n = loop {
            match read_at(&part.file, &mut buf[..want], part.offset + within) {
                Err(e) if e.kind() == io::ErrorKind::Interrupted => {}
                r => break r?,
            }
        };
        if n == 0 && want > 0 {
            return Err(io::Error::new(io::ErrorKind::UnexpectedEof, "an archive volume is shorter than expected"));
        }
        let end = pos + n as u64;
        if end > self.reached.get() {
            self.progress.add(end - self.reached.get());
            self.reached.set(end);
        }
        Ok(n)
    }

    /// `len` bytes from `start`; reading stops early if the stream ends first.
    pub fn slice(&self, start: u64, len: u64) -> Slice<'_> {
        Slice { vols: self, pos: start, end: start.saturating_add(len).min(self.len()) }
    }

    /// A buffered, seekable reader from `start` to the end of the stream.
    pub fn reader(&self, start: u64) -> BufReader<Slice<'_>> {
        BufReader::with_capacity(16 * 1024, self.slice(start, u64::MAX))
    }
}

pub struct Slice<'a> {
    vols: &'a Volumes<'a>,
    pos: u64,
    end: u64,
}

impl Read for Slice<'_> {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        if self.pos >= self.end {
            return Ok(0);
        }
        let want = buf.len().min(usize::try_from(self.end - self.pos).unwrap_or(usize::MAX));
        let n = self.vols.read_at(self.pos, &mut buf[..want])?;
        self.pos += n as u64;
        Ok(n)
    }
}

impl Seek for Slice<'_> {
    fn seek(&mut self, to: SeekFrom) -> io::Result<u64> {
        let pos = match to {
            SeekFrom::Start(p) => Some(p),
            SeekFrom::Current(d) => self.pos.checked_add_signed(d),
            SeekFrom::End(d) => self.end.checked_add_signed(d),
        };
        self.pos = pos.ok_or_else(|| io::Error::new(io::ErrorKind::InvalidInput, "seek out of range"))?;
        Ok(self.pos)
    }
}

/// Little-endian field reads for header parsing.
pub trait ReadLe: Read {
    fn array<const N: usize>(&mut self) -> io::Result<[u8; N]> {
        let mut b = [0u8; N];
        self.read_exact(&mut b)?;
        Ok(b)
    }
    fn u8(&mut self) -> io::Result<u8> {
        Ok(self.array::<1>()?[0])
    }
    fn u16(&mut self) -> io::Result<u16> {
        self.array().map(u16::from_le_bytes)
    }
    fn u32(&mut self) -> io::Result<u32> {
        self.array().map(u32::from_le_bytes)
    }
    fn u64(&mut self) -> io::Result<u64> {
        self.array().map(u64::from_le_bytes)
    }
    /// A little-endian unsigned number `width` bytes wide (up to 8).
    fn uint(&mut self, width: usize) -> io::Result<u64> {
        let mut b = [0u8; 8];
        self.read_exact(&mut b[..width.min(8)])?;
        Ok(u64::from_le_bytes(b))
    }
    fn vec(&mut self, len: usize) -> io::Result<Vec<u8>> {
        let mut v = Vec::new();
        self.take(len as u64).read_to_end(&mut v)?;
        if v.len() != len {
            return Err(io::ErrorKind::UnexpectedEof.into());
        }
        Ok(v)
    }
    /// The next 4-byte signature, or `None` at the very end of the stream.
    fn signature(&mut self) -> io::Result<Option<u32>> {
        let mut b = [0u8; 4];
        let mut got = 0;
        while got < 4 {
            match self.read(&mut b[got..]) {
                Ok(0) => break,
                Ok(n) => got += n,
                Err(e) if e.kind() == io::ErrorKind::Interrupted => {}
                Err(e) => return Err(e),
            }
        }
        match got {
            0 => Ok(None),
            4 => Ok(Some(u32::from_le_bytes(b))),
            _ => Err(io::ErrorKind::UnexpectedEof.into()),
        }
    }
}

impl<R: Read + ?Sized> ReadLe for R {}
