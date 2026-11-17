//! Holds decoded data to the size (and CRC-32) the archive promised, so a wrong
//! password or a damaged archive becomes an error instead of a bad file.

use std::io::{self, Read};

pub struct Verify<R> {
    inner: R,
    size: u64,
    crc: Option<u32>,
    hasher: crc32fast::Hasher,
    seen: u64,
    done: bool,
    /// Set once this reader has reported bad data.
    pub failed: bool,
}

impl<R: Read> Verify<R> {
    pub fn new(inner: R, size: u64, crc: Option<u32>) -> Self {
        Self { inner, size, crc, hasher: crc32fast::Hasher::new(), seen: 0, done: false, failed: false }
    }

    fn fail(&mut self, msg: String) -> io::Error {
        self.failed = true;
        io::Error::new(io::ErrorKind::InvalidData, msg)
    }

    fn inner_read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        self.inner.read(buf).inspect_err(|_| self.failed = true)
    }

    /// Runs as soon as the last byte arrives, so a consumer that stops at exactly
    /// `size` bytes (like `Take`) still gets the check, and the decoder's own
    /// end-of-data checks (MACs, stream trailers) run too.
    fn finish(&mut self) -> io::Result<()> {
        if self.inner_read(&mut [0u8; 1])? != 0 {
            return Err(self.fail(format!("data is longer than the {} bytes recorded", self.size)));
        }
        if let Some(expected) = self.crc {
            let got = self.hasher.clone().finalize();
            if got != expected {
                return Err(self.fail(format!("CRC mismatch (expected {expected:08X}, got {got:08X})")));
            }
        }
        self.done = true;
        Ok(())
    }
}

impl<R: Read> Read for Verify<R> {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        if self.done || buf.is_empty() {
            return Ok(0);
        }
        let left = self.size - self.seen;
        if left == 0 {
            self.finish()?;
            return Ok(0);
        }
        let want = buf.len().min(usize::try_from(left).unwrap_or(usize::MAX));
        let n = self.inner_read(&mut buf[..want])?;
        if n == 0 {
            return Err(self.fail(format!("data ends after {} of {} bytes", self.seen, self.size)));
        }
        self.seen += n as u64;
        self.hasher.update(&buf[..n]);
        if self.seen == self.size {
            self.finish()?;
        }
        Ok(n)
    }
}
