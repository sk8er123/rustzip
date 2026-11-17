//! A bzip2 decoder that also reads ALZ's variant of bzip2.
//!
//! ALZ (method 1) writes ordinary bzip2 blocks with different framing: no "BZh"
//! stream header (block size is always 900k), each block starts with the bits
//! "DLZ\x01" instead of the 48-bit block magic, with no block CRC and no
//! "randomised" bit, and the stream ends with "DLZ\x02" and no combined CRC.
//! The block body is decoded the same way in both framings, so the standard
//! framing (checked against streams made by libbzip2) vouches for the body.

use std::io::{self, Read};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Framing {
    Standard,
    Alz,
}

const BLOCK_MAGIC: u64 = 0x3141_5926_5359;
const END_MAGIC: u64 = 0x1772_4538_5090;
const ALZ_BLOCK: u32 = u32::from_be_bytes(*b"DLZ\x01");
const ALZ_END: u32 = u32::from_be_bytes(*b"DLZ\x02");
const MAX_GROUPS: usize = 6;
const MAX_ALPHA: usize = 258;
const MAX_CODE_LEN: usize = 20;
const MAX_SELECTORS: usize = 18002;
const GROUP_SIZE: usize = 50;

fn bad(msg: &str) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, format!("bzip2 data is corrupt: {msg}"))
}

struct Bits<R> {
    inner: R,
    buf: Vec<u8>,
    pos: usize,
    acc: u64,
    count: u32,
}

impl<R: Read> Bits<R> {
    fn byte(&mut self) -> io::Result<u8> {
        if self.pos == self.buf.len() {
            self.buf.resize(8192, 0);
            self.pos = 0;
            let n = loop {
                match self.inner.read(&mut self.buf) {
                    Ok(n) => break n,
                    Err(e) if e.kind() == io::ErrorKind::Interrupted => {}
                    Err(e) => {
                        self.buf.clear();
                        return Err(e);
                    }
                }
            };
            self.buf.truncate(n);
            if n == 0 {
                return Err(io::ErrorKind::UnexpectedEof.into());
            }
        }
        self.pos += 1;
        Ok(self.buf[self.pos - 1])
    }

    /// The next `n` (at most 32) bits, most significant first.
    fn get(&mut self, n: u32) -> io::Result<u32> {
        while self.count < n {
            self.acc = (self.acc << 8) | self.byte()? as u64;
            self.count += 8;
        }
        self.count -= n;
        Ok(((self.acc >> self.count) & ((1u64 << n) - 1)) as u32)
    }

    fn bit(&mut self) -> io::Result<bool> {
        Ok(self.get(1)? == 1)
    }
}

/// Canonical Huffman decoding tables for one coding group (libbzip2's layout).
struct Table {
    min_len: usize,
    limit: [i32; MAX_CODE_LEN + 2],
    base: [i32; MAX_CODE_LEN + 2],
    perm: [u16; MAX_ALPHA],
}

impl Table {
    fn new(lengths: &[u8]) -> Self {
        let min_len = lengths.iter().copied().min().unwrap_or(1) as usize;
        let max_len = lengths.iter().copied().max().unwrap_or(1) as usize;
        let mut t = Table { min_len, limit: [0; MAX_CODE_LEN + 2], base: [0; MAX_CODE_LEN + 2], perm: [0; MAX_ALPHA] };
        let mut pp = 0;
        for len in min_len..=max_len {
            for (sym, _) in lengths.iter().enumerate().filter(|&(_, &l)| l as usize == len) {
                t.perm[pp] = sym as u16;
                pp += 1;
            }
        }
        for &l in lengths {
            t.base[l as usize + 1] += 1;
        }
        for i in 1..t.base.len() {
            t.base[i] += t.base[i - 1];
        }
        let mut vec = 0i32;
        for i in min_len..=max_len {
            vec += t.base[i + 1] - t.base[i];
            t.limit[i] = vec - 1;
            vec <<= 1;
        }
        for i in min_len + 1..=max_len {
            t.base[i] = ((t.limit[i - 1] + 1) << 1) - t.base[i];
        }
        t
    }

    fn decode<R: Read>(&self, bits: &mut Bits<R>) -> io::Result<u16> {
        let mut len = self.min_len;
        let mut code = bits.get(len as u32)? as i32;
        while code > self.limit[len] {
            len += 1;
            if len > MAX_CODE_LEN {
                return Err(bad("bad Huffman code"));
            }
            code = (code << 1) | bits.get(1)? as i32;
        }
        let i = code - self.base[len];
        if !(0..MAX_ALPHA as i32).contains(&i) {
            return Err(bad("bad Huffman code"));
        }
        Ok(self.perm[i as usize])
    }
}

fn crc_table() -> [u32; 256] {
    std::array::from_fn(|i| {
        (0..8).fold((i as u32) << 24, |c, _| if c & 0x8000_0000 != 0 { (c << 1) ^ 0x04c1_1db7 } else { c << 1 })
    })
}

/// Decodes one bzip2 stream in either framing.
pub struct Bzip2Reader<R> {
    bits: Bits<R>,
    framing: Framing,
    max_block: usize,
    started: bool,
    finished: bool,
    tt: Vec<u32>,
    // Inverse BWT and run-length state of the current block.
    pos: u32,
    left: usize,
    prev: Option<u8>,
    run: u8,
    repeat: u32,
    // Standard framing only.
    crc_table: [u32; 256],
    crc: u32,
    block_crc: u32,
    combined_crc: u32,
}

impl<R: Read> Bzip2Reader<R> {
    pub fn new(inner: R, framing: Framing) -> Self {
        Self {
            bits: Bits { inner, buf: Vec::new(), pos: 0, acc: 0, count: 0 },
            framing,
            max_block: 900_000,
            started: false,
            finished: false,
            tt: Vec::new(),
            pos: 0,
            left: 0,
            prev: None,
            run: 0,
            repeat: 0,
            crc_table: crc_table(),
            crc: 0,
            block_crc: 0,
            combined_crc: 0,
        }
    }

    /// Reads the next block header; false at the end of the stream.
    fn start_block(&mut self) -> io::Result<bool> {
        let b = &mut self.bits;
        if !self.started {
            self.started = true;
            if self.framing == Framing::Standard {
                if b.get(24)? != u32::from_be_bytes([0, b'B', b'Z', b'h']) {
                    return Err(bad("not a bzip2 stream"));
                }
                let level = b.get(8)?;
                if !(u32::from(b'1')..=u32::from(b'9')).contains(&level) {
                    return Err(bad("bad block size"));
                }
                self.max_block = 100_000 * (level - u32::from(b'0')) as usize;
            }
        }
        match self.framing {
            Framing::Standard => {
                let magic = (u64::from(b.get(24)?) << 24) | u64::from(b.get(24)?);
                if magic == END_MAGIC {
                    if b.get(32)? != self.combined_crc {
                        return Err(bad("stream CRC mismatch"));
                    }
                    return Ok(false);
                }
                if magic != BLOCK_MAGIC {
                    return Err(bad("bad block header"));
                }
                self.block_crc = b.get(32)?;
                if b.bit()? {
                    return Err(bad("randomised blocks (bzip2 0.9.0) are not supported"));
                }
            }
            Framing::Alz => match b.get(32)? {
                ALZ_END => return Ok(false),
                ALZ_BLOCK => {}
                _ => return Err(bad("bad ALZ block header")),
            },
        }
        let orig_ptr = self.bits.get(24)? as usize;
        let counts = self.read_block()?;
        if orig_ptr >= self.tt.len() {
            return Err(bad("bad BWT origin"));
        }
        let mut next = [0u32; 256];
        let mut sum = 0u32;
        for (n, c) in next.iter_mut().zip(counts) {
            *n = sum;
            sum += c;
        }
        for i in 0..self.tt.len() {
            let byte = (self.tt[i] & 0xFF) as usize;
            let slot = next[byte] as usize;
            self.tt[slot] |= (i as u32) << 8;
            next[byte] += 1;
        }
        self.pos = self.tt[orig_ptr] >> 8;
        self.left = self.tt.len();
        self.prev = None;
        self.run = 0;
        self.repeat = 0;
        self.crc = u32::MAX;
        Ok(true)
    }

    /// Decodes the Huffman/MTF/RLE2 body into `tt`; returns how often each byte occurs.
    fn read_block(&mut self) -> io::Result<[u32; 256]> {
        let b = &mut self.bits;
        let used_groups = b.get(16)?;
        let mut symbols = Vec::with_capacity(256);
        for i in 0..16 {
            if used_groups & (0x8000 >> i) != 0 {
                let used = b.get(16)?;
                symbols.extend((0..16).filter(|j| used & (0x8000 >> j) != 0).map(|j| (i * 16 + j) as u8));
            }
        }
        if symbols.is_empty() {
            return Err(bad("empty symbol map"));
        }
        let alpha_size = symbols.len() + 2;
        let groups = b.get(3)? as usize;
        if !(2..=MAX_GROUPS).contains(&groups) {
            return Err(bad("bad group count"));
        }
        let selector_count = b.get(15)? as usize;
        if selector_count == 0 {
            return Err(bad("no selectors"));
        }
        let mut order: Vec<u8> = (0..groups as u8).collect();
        let mut selectors = Vec::with_capacity(selector_count.min(MAX_SELECTORS));
        for _ in 0..selector_count {
            let mut j = 0;
            while b.bit()? {
                j += 1;
                if j >= groups {
                    return Err(bad("bad selector"));
                }
            }
            let g = order[j];
            order.copy_within(0..j, 1);
            order[0] = g;
            if selectors.len() < MAX_SELECTORS {
                selectors.push(g);
            }
        }
        let mut tables = Vec::with_capacity(groups);
        for _ in 0..groups {
            let mut len = b.get(5)? as i32;
            let mut lengths = [0u8; MAX_ALPHA];
            for l in lengths.iter_mut().take(alpha_size) {
                loop {
                    if !(1..=MAX_CODE_LEN as i32).contains(&len) {
                        return Err(bad("bad code length"));
                    }
                    if !b.bit()? {
                        break;
                    }
                    len += if b.bit()? { -1 } else { 1 };
                }
                *l = len as u8;
            }
            tables.push(Table::new(&lengths[..alpha_size]));
        }

        let end_of_block = alpha_size as u16 - 1;
        let mut mtf: [u8; 256] = std::array::from_fn(|i| i as u8);
        let mut counts = [0u32; 256];
        let mut selector = 0;
        let mut group_left = 0;
        let mut table = &tables[0];
        let max_block = self.max_block;
        let tt = &mut self.tt;
        tt.clear();
        let mut next_symbol = |b: &mut Bits<R>| -> io::Result<u16> {
            if group_left == 0 {
                let g = *selectors.get(selector).ok_or_else(|| bad("ran out of selectors"))?;
                table = &tables[g as usize];
                selector += 1;
                group_left = GROUP_SIZE;
            }
            group_left -= 1;
            table.decode(b)
        };
        let mut sym = next_symbol(b)?;
        while sym != end_of_block {
            if sym <= 1 {
                let mut run = 0usize;
                let mut weight = 1usize;
                while sym <= 1 {
                    if weight >= 1 << 21 {
                        return Err(bad("run too long"));
                    }
                    run += weight << sym;
                    weight <<= 1;
                    sym = next_symbol(b)?;
                }
                let byte = symbols[mtf[0] as usize];
                if tt.len() + run > max_block {
                    return Err(bad("block too long"));
                }
                counts[byte as usize] += run as u32;
                tt.resize(tt.len() + run, u32::from(byte));
            } else {
                if tt.len() >= max_block {
                    return Err(bad("block too long"));
                }
                let i = (sym - 1) as usize;
                let m = mtf[i];
                mtf.copy_within(0..i, 1);
                mtf[0] = m;
                let byte = *symbols.get(m as usize).ok_or_else(|| bad("bad symbol"))?;
                counts[byte as usize] += 1;
                tt.push(u32::from(byte));
                sym = next_symbol(b)?;
            }
        }
        Ok(counts)
    }

    fn next_bwt_byte(&mut self) -> io::Result<u8> {
        let t = *self.tt.get(self.pos as usize).ok_or_else(|| bad("bad BWT index"))?;
        self.pos = t >> 8;
        self.left -= 1;
        Ok(t as u8)
    }

    fn end_block(&mut self) -> io::Result<()> {
        if self.framing == Framing::Standard {
            let crc = !self.crc;
            if crc != self.block_crc {
                return Err(bad("block CRC mismatch"));
            }
            self.combined_crc = self.combined_crc.rotate_left(1) ^ crc;
        }
        Ok(())
    }

    fn emit(&mut self, b: u8) {
        if self.framing == Framing::Standard {
            self.crc = (self.crc << 8) ^ self.crc_table[((self.crc >> 24) as u8 ^ b) as usize];
        }
    }
}

impl<R: Read> Read for Bzip2Reader<R> {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        let mut n = 0;
        while n < buf.len() && !self.finished {
            if self.repeat > 0 {
                let b = self.prev.unwrap_or(0);
                buf[n] = b;
                self.emit(b);
                self.repeat -= 1;
                n += 1;
                continue;
            }
            if self.left == 0 {
                if self.started && !self.tt.is_empty() {
                    self.end_block()?;
                    self.tt.clear();
                }
                if !self.start_block()? {
                    self.finished = true;
                }
                continue;
            }
            let b = self.next_bwt_byte()?;
            if self.run == 4 {
                self.repeat = u32::from(b);
                self.run = 0;
                continue;
            }
            if self.prev == Some(b) {
                self.run += 1;
            } else {
                self.prev = Some(b);
                self.run = 1;
            }
            buf[n] = b;
            self.emit(b);
            n += 1;
        }
        Ok(n)
    }
}

/// Rewrites a libbzip2 stream in ALZ framing, for tests (there is no ALZ-bzip2
/// sample): finds every block magic in the bit stream, drops the stream header,
/// each block's magic, CRC and randomised bit, and the trailer.
#[cfg(test)]
pub fn to_alz_framing(standard: &[u8]) -> Vec<u8> {
    let bit = |i: usize| (standard[i / 8] >> (7 - i % 8)) & 1;
    let total = standard.len() * 8;
    let mut starts = Vec::new();
    let mut window = 0u64;
    let mut end = None;
    for i in 32..total {
        window = ((window << 1) | u64::from(bit(i))) & ((1 << 48) - 1);
        if i >= 32 + 47 {
            let at = i - 47;
            if window == BLOCK_MAGIC {
                starts.push(at);
            } else if window == END_MAGIC {
                end = Some(at);
                break;
            }
        }
    }
    let end = end.expect("stream trailer");
    let mut out = Vec::new();
    let mut acc = 0u8;
    let mut n = 0;
    let mut put = |b: u8| {
        acc = (acc << 1) | b;
        n += 1;
        if n == 8 {
            out.push(acc);
            acc = 0;
            n = 0;
        }
    };
    let put_word = |put: &mut dyn FnMut(u8), w: u32| (0..32).rev().for_each(|i| put(((w >> i) & 1) as u8));
    for (k, &s) in starts.iter().enumerate() {
        let next = starts.get(k + 1).copied().unwrap_or(end);
        assert_eq!(bit(s + 80), 0, "randomised block");
        put_word(&mut put, ALZ_BLOCK);
        (s + 81..next).for_each(|i| put(bit(i)));
    }
    put_word(&mut put, ALZ_END);
    (0..7).for_each(|_| put(0));
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use bzip2::{read::BzEncoder, Compression};

    fn sample() -> Vec<u8> {
        let mut seed = 0x2545_F491_4F6C_DD1Du64;
        let mut random = || {
            seed ^= seed << 13;
            seed ^= seed >> 7;
            seed ^= seed << 17;
            seed as u8
        };
        let mut data = Vec::new();
        data.extend((0..700_000).map(|_| random()));
        data.extend(std::iter::repeat_n(b'a', 5000));
        for i in 0..400_000u32 {
            data.extend_from_slice(format!("line {} {}\n", i % 977, i % 13).as_bytes());
            if i % 5000 == 0 {
                data.extend(std::iter::repeat_n(b'z', 300 + (i % 7) as usize));
            }
        }
        data.extend((0..300_000).map(|_| random() & 3));
        data.extend(std::iter::repeat_n(0u8, 1_000_000));
        data
    }

    fn compress(data: &[u8], level: u32) -> Vec<u8> {
        let mut out = Vec::new();
        BzEncoder::new(data, Compression::new(level)).read_to_end(&mut out).unwrap();
        out
    }

    fn decode(stream: &[u8], framing: Framing) -> io::Result<Vec<u8>> {
        let mut out = Vec::new();
        Bzip2Reader::new(stream, framing).read_to_end(&mut out)?;
        Ok(out)
    }

    #[test]
    fn standard_framing_matches_libbzip2() {
        let data = sample();
        assert!(data.len() > 6_000_000);
        for level in [1, 9] {
            let stream = compress(&data, level);
            assert!(decode(&stream, Framing::Standard).unwrap() == data, "level {level}");
        }
        for small in [&b""[..], b"x", b"aaaa", b"aaaaa", b"abababababab"] {
            assert_eq!(decode(&compress(small, 9), Framing::Standard).unwrap(), small);
        }
    }

    #[test]
    fn standard_framing_checks_crcs() {
        let data: Vec<u8> = (0..50_000u32).map(|i| (i * 7 % 251) as u8).collect();
        let mut stream = compress(&data, 9);
        let crc_byte = 4 + 6; // after "BZh9" and the 48-bit block magic
        stream[crc_byte] ^= 1;
        assert!(decode(&stream, Framing::Standard).is_err());
    }

    #[test]
    fn alz_framing_decodes_the_same_blocks() {
        let data = sample();
        for level in [1, 9] {
            let alz = to_alz_framing(&compress(&data, level));
            assert!(decode(&alz, Framing::Alz).unwrap() == data, "level {level}");
        }
    }

    #[test]
    fn damaged_streams_fail_without_panicking() {
        let data: Vec<u8> = (0..300_000u32).map(|i| (i % 97) as u8 ^ (i >> 9) as u8).collect();
        let standard = compress(&data, 1);
        let alz = to_alz_framing(&standard);
        for (stream, framing) in [(&standard, Framing::Standard), (&alz, Framing::Alz)] {
            for cut in (0..stream.len()).step_by(997) {
                assert!(decode(&stream[..cut], framing).is_err());
            }
            for pos in (0..stream.len()).step_by(331) {
                let mut bad = stream.clone();
                bad[pos] ^= 0x5A;
                let _ = decode(&bad, framing);
            }
        }
    }
}
