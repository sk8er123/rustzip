//! CPIO: new ASCII (newc/crc), old ASCII (odc) and old binary, plain or filtered.

use std::io::{self, Read};

use anyhow::{bail, Result};

use super::super::walk::{drain, Item, Source, Visitor};
use super::super::{decode_name, unix_to_naive};

const S_IFMT: u32 = 0o170000;
const S_IFDIR: u32 = 0o040000;
const S_IFREG: u32 = 0o100000;

struct Header {
    mode: u32,
    mtime: u64,
    namesize: u64,
    filesize: u64,
    /// Alignment of name and data (4 for newc, 2 for binary, 1 for odc).
    align: u64,
}

fn hex(b: &[u8]) -> Result<u64> {
    Ok(u64::from_str_radix(std::str::from_utf8(b)?, 16)?)
}
fn oct(b: &[u8]) -> Result<u64> {
    Ok(u64::from_str_radix(std::str::from_utf8(b)?.trim(), 8)?)
}

struct Counted<R> {
    r: R,
    pos: u64,
}

impl<R: Read> Read for Counted<R> {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        let n = self.r.read(buf)?;
        self.pos += n as u64;
        Ok(n)
    }
}

impl<R: Read> Counted<R> {
    fn skip_to(&mut self, align: u64) -> io::Result<()> {
        let pad = (align - self.pos % align) % align;
        io::copy(&mut self.by_ref().take(pad), &mut io::sink()).map(|_| ())
    }

    fn header(&mut self) -> Result<Header> {
        let mut magic = [0u8; 6];
        self.read_exact(&mut magic)?;
        match &magic {
            b"070701" | b"070702" => {
                let mut h = [0u8; 104];
                self.read_exact(&mut h)?;
                let f = |i: usize| hex(&h[i * 8..i * 8 + 8]);
                Ok(Header { mode: f(1)? as u32, mtime: f(5)?, filesize: f(6)?, namesize: f(11)?, align: 4 })
            }
            b"070707" => {
                let mut h = [0u8; 70];
                self.read_exact(&mut h)?;
                Ok(Header { mode: oct(&h[12..18])? as u32, mtime: oct(&h[42..53])?, namesize: oct(&h[53..59])?, filesize: oct(&h[59..70])?, align: 1 })
            }
            _ if magic[..2] == [0xC7, 0x71] || magic[..2] == [0x71, 0xC7] => {
                let mut rest = [0u8; 20];
                self.read_exact(&mut rest)?;
                let mut all = magic.to_vec();
                all.extend_from_slice(&rest);
                let le = magic[0] == 0xC7;
                let w = |i: usize| -> u64 {
                    let b = [all[i * 2], all[i * 2 + 1]];
                    (if le { u16::from_le_bytes(b) } else { u16::from_be_bytes(b) }) as u64
                };
                Ok(Header { mode: w(3) as u32, mtime: w(8) << 16 | w(9), namesize: w(10), filesize: w(11) << 16 | w(12), align: 2 })
            }
            _ => bail!("CPIO: bad header"),
        }
    }
}

pub fn walk(src: &Source<'_>, v: &mut dyn Visitor) -> Result<()> {
    let mut r = Counted { r: src.open_stream()?, pos: 0 };
    loop {
        let h = r.header()?;
        let mut name = vec![0u8; h.namesize as usize];
        r.read_exact(&mut name)?;
        while name.last() == Some(&0) {
            name.pop();
        }
        r.skip_to(h.align)?;
        if name == b"TRAILER!!!" {
            return Ok(());
        }
        let kind = h.mode & S_IFMT;
        let mut data = (&mut r).take(h.filesize);
        if kind == S_IFDIR || kind == S_IFREG {
            let mut item = Item::new(&decode_name(&name, false, src.encoding), kind == S_IFDIR);
            item.size = h.filesize;
            item.modified = unix_to_naive(h.mtime as i64);
            item.method = src.stream_method("CPIO");
            if v.entry(&item)? && kind == S_IFREG {
                v.data(&item, &mut data)?;
            }
        }
        drain(&mut data)?;
        r.skip_to(h.align)?;
    }
}
