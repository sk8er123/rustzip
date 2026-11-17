//! Decoder for Unix `compress` (.Z) files: LZW with 9..=16-bit codes.
//!
//! `compress` writes codes in groups of eight; when the code width grows or the
//! table is cleared, the unused rest of the current group is padding.

use std::io::{self, Read};

const CLEAR: u32 = 256;

pub struct LzwReader<R> {
    inner: R,
    max_bits: u32,
    block_mode: bool,
    bits: u32,
    /// Codes read at the current width (to find the end of an 8-code group).
    codes_in_width: u32,
    bitbuf: u64,
    bitcount: u32,
    prefix: Vec<u16>,
    suffix: Vec<u8>,
    next_free: u32,
    old: Option<u32>,
    first_char: u8,
    out: Vec<u8>,
    out_pos: usize,
    done: bool,
}

impl<R: Read> LzwReader<R> {
    pub fn new(mut inner: R) -> io::Result<Self> {
        let mut h = [0u8; 3];
        inner.read_exact(&mut h)?;
        if h[0] != 0x1f || h[1] != 0x9d {
            return Err(io::Error::new(io::ErrorKind::InvalidData, "not a .Z file"));
        }
        let max_bits = (h[2] & 0x1f) as u32;
        if !(9..=16).contains(&max_bits) {
            return Err(io::Error::new(io::ErrorKind::InvalidData, "bad .Z code size"));
        }
        let block_mode = h[2] & 0x80 != 0;
        Ok(Self {
            inner,
            max_bits,
            block_mode,
            bits: 9,
            codes_in_width: 0,
            bitbuf: 0,
            bitcount: 0,
            prefix: vec![0; 1 << 16],
            suffix: (0..1u32 << 16).map(|i| i as u8).collect(),
            next_free: if block_mode { 257 } else { 256 },
            old: None,
            first_char: 0,
            out: Vec::with_capacity(1 << 16),
            out_pos: 0,
            done: false,
        })
    }

    fn read_bits(&mut self, n: u32) -> io::Result<Option<u32>> {
        while self.bitcount < n {
            let mut b = [0u8];
            match self.inner.read(&mut b) {
                Ok(0) => return Ok(None),
                Ok(_) => {
                    self.bitbuf |= (b[0] as u64) << self.bitcount;
                    self.bitcount += 8;
                }
                Err(e) if e.kind() == io::ErrorKind::Interrupted => {}
                Err(e) => return Err(e),
            }
        }
        let v = (self.bitbuf & ((1u64 << n) - 1)) as u32;
        self.bitbuf >>= n;
        self.bitcount -= n;
        Ok(Some(v))
    }

    /// Skips the padding codes up to the end of the current group of eight.
    fn skip_group_rest(&mut self) -> io::Result<()> {
        let rest = (8 - self.codes_in_width % 8) % 8;
        for _ in 0..rest {
            if self.read_bits(self.bits)?.is_none() {
                break;
            }
        }
        self.codes_in_width = 0;
        Ok(())
    }

    fn max_code(&self) -> u32 {
        (1 << self.bits) - 1
    }

    /// Decodes one code into `out`; returns false at the end of the input.
    fn step(&mut self) -> io::Result<bool> {
        if self.next_free > self.max_code() && self.bits < self.max_bits {
            self.skip_group_rest()?;
            self.bits += 1;
        }
        let Some(code) = self.read_bits(self.bits)? else { return Ok(false) };
        self.codes_in_width += 1;
        let Some(old) = self.old else {
            if code > 255 {
                return Err(io::Error::new(io::ErrorKind::InvalidData, "bad first .Z code"));
            }
            self.first_char = code as u8;
            self.out.push(code as u8);
            self.old = Some(code);
            return Ok(true);
        };
        if code == CLEAR && self.block_mode {
            self.skip_group_rest()?;
            self.bits = 9;
            self.next_free = 256;
            self.old = None;
            return Ok(true);
        }
        let start = self.out.len();
        let mut c = code;
        if c >= self.next_free {
            if c > self.next_free {
                return Err(io::Error::new(io::ErrorKind::InvalidData, "corrupt .Z data"));
            }
            self.out.push(self.first_char);
            c = old;
        }
        while c >= 256 {
            self.out.push(self.suffix[c as usize]);
            c = self.prefix[c as usize] as u32;
        }
        self.first_char = c as u8;
        self.out.push(self.first_char);
        self.out[start..].reverse();
        if self.next_free < (1 << self.max_bits) {
            self.prefix[self.next_free as usize] = old as u16;
            self.suffix[self.next_free as usize] = self.first_char;
            self.next_free += 1;
        }
        self.old = Some(code);
        Ok(true)
    }
}

impl<R: Read> Read for LzwReader<R> {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        while self.out_pos == self.out.len() {
            if self.done {
                return Ok(0);
            }
            self.out.clear();
            self.out_pos = 0;
            while self.out.len() < 32 * 1024 {
                if !self.step()? {
                    self.done = true;
                    break;
                }
            }
        }
        let n = buf.len().min(self.out.len() - self.out_pos);
        buf[..n].copy_from_slice(&self.out[self.out_pos..self.out_pos + n]);
        self.out_pos += n;
        Ok(n)
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;

    /// A plain `compress -b16` encoder, only for tests (checked against the system
    /// `uncompress` in interop tests).
    pub fn compress(data: &[u8], max_bits: u32) -> Vec<u8> {
        use std::collections::HashMap;
        let mut out = vec![0x1f, 0x9d, 0x80 | max_bits as u8];
        let (mut acc, mut nacc) = (0u64, 0u32);
        let mut bits = 9u32;
        let mut in_width = 0u32;
        let put = |code: u32, bits: u32, out: &mut Vec<u8>, acc: &mut u64, nacc: &mut u32| {
            *acc |= (code as u64) << *nacc;
            *nacc += bits;
            while *nacc >= 8 {
                out.push(*acc as u8);
                *acc >>= 8;
                *nacc -= 8;
            }
        };
        let mut table: HashMap<(u32, u8), u32> = HashMap::new();
        let mut next = 257u32;
        let mut it = data.iter();
        let Some(&first) = it.next() else { return out };
        let mut w = first as u32;
        let emit = |code: u32, next: u32, bits: &mut u32, in_width: &mut u32, out: &mut Vec<u8>, acc: &mut u64, nacc: &mut u32| {
            put(code, *bits, out, acc, nacc);
            *in_width += 1;
            // `next` already counts the entry added for this code; real compress adds it
            // after writing, and widens once that count no longer fits.
            if next - 1 > (1 << *bits) - 1 && *bits < max_bits {
                while !(*in_width).is_multiple_of(8) {
                    put(0, *bits, out, acc, nacc);
                    *in_width += 1;
                }
                *in_width = 0;
                *bits += 1;
            }
        };
        for &b in it {
            if let Some(&c) = table.get(&(w, b)) {
                w = c;
                continue;
            }
            let grow = next < (1 << max_bits);
            if grow {
                table.insert((w, b), next);
                next += 1;
            }
            emit(w, next, &mut bits, &mut in_width, &mut out, &mut acc, &mut nacc);
            w = b as u32;
        }
        put(w, bits, &mut out, &mut acc, &mut nacc);
        if nacc > 0 {
            out.push(acc as u8);
        }
        out
    }

    #[test]
    fn lzw_roundtrip_through_test_encoder() {
        let mut data = Vec::new();
        for i in 0..200_000u32 {
            data.extend_from_slice(format!("{} ", i % 977).as_bytes());
        }
        let z = compress(&data, 16);
        let mut back = Vec::new();
        LzwReader::new(&z[..]).unwrap().read_to_end(&mut back).unwrap();
        assert_eq!(back, data);
    }
}
