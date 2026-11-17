//! AZO, ESTsoft's own EGG compression: LZ77 with adaptive binary arithmetic coding.
//! Ported from EggDotNet's decoder (MIT); every block is decoded independently.

use std::io::{self, Read};

const MATCH_MIN_LENGTH: u32 = 2;
const MATCH_MIN_DIST: u32 = 1;
const MATCH_DIST_SGAP: u32 = 16;
const MATCH_DIST_GAP: u32 = 4;
const CODE_SIZE: u32 = 128;
const DICTIONARY_SIZE: usize = 128;
/// ALZip writes 512 KiB blocks; anything far larger is a damaged header, and
/// refusing it keeps a bad size from allocating gigabytes.
const MAX_BLOCK: u32 = 64 << 20;

const LENGTH_TABLE: [u32; 128] = [
    2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12, 13, 14, 15, 16, 17, 18, 19, 20, 21, 22, 23, 24, 25, 26, 27, 28, 29, 30, 31,
    32, 33, 34, 35, 36, 37, 38, 39, 40, 41, 42, 44, 46, 48, 50, 52, 54, 56, 58, 62, 66, 70, 74, 78, 82, 86, 90, 98,
    106, 114, 122, 130, 138, 146, 154, 170, 186, 202, 218, 234, 250, 266, 282, 314, 346, 378, 410, 442, 474, 506,
    538, 602, 666, 730, 794, 858, 922, 986, 1050, 1178, 1306, 1434, 1562, 1690, 1818, 1946, 2074, 2330, 2586, 2842,
    3098, 3354, 3610, 3866, 4122, 4634, 5146, 5658, 6170, 6682, 7194, 7706, 8218, 9242, 10266, 11290, 12314, 13338,
    14362, 15386, 16410, 18458, 20506, 22554, 24602, 26650, 28698, 30746,
];

const DIST_TABLE: [u32; 128] = [
    1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12, 13, 14, 15, 16, 17, 18, 19, 20, 21, 23, 25, 27, 29, 33, 37, 41, 45, 53, 61,
    69, 77, 93, 109, 125, 141, 173, 205, 237, 269, 333, 397, 461, 525, 653, 781, 909, 1037, 1293, 1549, 1805, 2061,
    2573, 3085, 3597, 4109, 5133, 6157, 7181, 8205, 10253, 12301, 14349, 16397, 20493, 24589, 28685, 32781, 40973,
    49165, 57357, 65549, 81933, 98317, 114701, 131085, 163853, 196621, 229389, 262157, 327693, 393229, 458765,
    524301, 655373, 786445, 917517, 1048589, 1310733, 1572877, 1835021, 2097165, 2621453, 3145741, 3670029, 4194317,
    5242893, 6291469, 7340045, 8388621, 10485773, 12582925, 14680077, 16777229, 20971533, 25165837, 29360141,
    33554445, 41943053, 50331661, 58720269, 67108877, 83886093, 100663309, 117440525, 134217741, 167772173,
    201326605, 234881037, 268435469, 335544333, 402653197, 469762061, 536870925, 671088653, 805306381, 939524109,
];

/// Extra bits after a length code: 0 for the first 40 codes, then one more every 8.
fn length_extra_bits(code: u32) -> u32 {
    code.saturating_sub(32) / 8
}

/// Extra bits after a distance code: 0 for the first 20 codes, then one more every 4.
fn dist_extra_bits(code: u32) -> u32 {
    code.saturating_sub(16) / 4
}

fn dist_code(dist: u32) -> u32 {
    let v = dist.wrapping_sub(MATCH_MIN_DIST);
    if v < MATCH_DIST_SGAP {
        return v;
    }
    let v = v - MATCH_DIST_SGAP;
    let extra = (v / MATCH_DIST_GAP + 1).ilog2();
    let base = ((1u64 << extra) - 1) * MATCH_DIST_GAP as u64;
    let code = MATCH_DIST_SGAP as u64 + extra as u64 * MATCH_DIST_GAP as u64 + (v as u64 - base) / (1u64 << extra);
    code.min(CODE_SIZE as u64 - 1) as u32
}

/// Reads the compressed block MSB first; past the end it yields zero bits, like the reference.
struct Bits<'a> {
    data: &'a [u8],
    bit: u64,
}

impl Bits<'_> {
    fn bit(&mut self) -> u32 {
        let Some(&byte) = self.data.get((self.bit / 8) as usize) else { return 0 };
        let b = (byte >> (7 - self.bit % 8)) & 1;
        self.bit += 1;
        b as u32
    }
    fn byte(&mut self) -> u32 {
        if self.bit + 8 > self.data.len() as u64 * 8 {
            return 0;
        }
        (0..8).fold(0, |acc, _| (acc << 1) | self.bit())
    }
    fn bytes_used(&self) -> u64 {
        self.bit.div_ceil(8)
    }
}

const MSB: u32 = 1 << 31;
const SECOND_MSB: u32 = 1 << 30;

struct Entropy<'a> {
    bits: Bits<'a>,
    low: u32,
    up: u32,
    tag: u32,
    corrupt: bool,
}

impl<'a> Entropy<'a> {
    fn new(data: &'a [u8]) -> Self {
        let mut bits = Bits { data, bit: 0 };
        let tag = (0..4).fold(0u32, |acc, _| (acc << 8) | bits.byte());
        Entropy { bits, low: 0, up: u32::MAX, tag, corrupt: false }
    }

    fn rescale(&mut self) {
        while (self.low & MSB) == (self.up & MSB) {
            self.tag = (self.tag << 1) | self.bits.bit();
            self.low <<= 1;
            self.up = (self.up << 1) | 1;
        }
        while (self.low & SECOND_MSB) != 0 && (self.up & SECOND_MSB) == 0 {
            self.tag = ((self.tag << 1) | self.bits.bit()) ^ MSB;
            self.low = (self.low << 1) & (MSB - 1);
            self.up = (self.up << 1) | 1 | MSB;
        }
    }

    fn step(&mut self, total_bits: u32) -> (u32, u32) {
        // A full-range interval is 2^32 wide, which wraps to 0; the reference
        // computes the step with a 32-bit shift that wraps the same way.
        let mut t = if self.low == 0 && self.up == u32::MAX {
            1u32.wrapping_shl(32 - total_bits)
        } else {
            self.up.wrapping_sub(self.low).wrapping_add(1) >> total_bits
        };
        if t == 0 {
            self.corrupt = true;
            t = 1;
        }
        (t, self.tag.wrapping_sub(self.low) / t)
    }

    /// A uniformly distributed `total_bits`-bit number.
    fn value(&mut self, total_bits: u32) -> u32 {
        let (t, v) = self.step(total_bits);
        self.up = self.low.wrapping_add(t.wrapping_mul(v.wrapping_add(1))).wrapping_sub(1);
        self.low = self.low.wrapping_add(t.wrapping_mul(v));
        self.rescale();
        v
    }

    /// One bit whose probability of being 0 is `cum / 2^total_bits`.
    fn bit(&mut self, cum: u32, total_bits: u32) -> bool {
        let (t, v) = self.step(total_bits);
        let one = v >= cum;
        if one {
            self.low = self.low.wrapping_add(t.wrapping_mul(cum));
        } else {
            self.up = self.low.wrapping_add(t.wrapping_mul(cum)).wrapping_sub(1);
        }
        self.rescale();
        one
    }
}

/// Adaptive model for a number below `n` (a power of two), coded bit by bit.
#[derive(Clone)]
struct BitTree {
    bits: u32,
    prob: Vec<u32>,
}

const TREE_TOTAL_BITS: u32 = 10;

impl BitTree {
    fn new(n: u32) -> Self {
        let bits = 32 - (n - 1).leading_zeros();
        BitTree { bits, prob: vec![1 << (TREE_TOTAL_BITS - 1); 1 << bits] }
    }

    fn decode(&mut self, e: &mut Entropy) -> u32 {
        let mut pre = 1usize;
        for _ in 0..self.bits {
            let b = e.bit(self.prob[pre], TREE_TOTAL_BITS);
            pre = (pre << 1) | b as usize;
        }
        let value = (pre - (1 << self.bits)) as u32;
        self.update(value);
        value
    }

    fn update(&mut self, value: u32) {
        let mut pre = 1usize;
        for i in (0..self.bits).rev() {
            let b = (value >> i) & 1;
            let p = &mut self.prob[pre];
            if b == 0 {
                *p += ((1 << TREE_TOTAL_BITS) - *p) >> 4;
            } else {
                *p -= *p >> 4;
            }
            pre = (pre << 1) | b as usize;
        }
    }

    /// Which model gave `value` the higher probability (`>0`: self).
    fn compare(&self, other: &BitTree, value: u32) -> std::cmp::Ordering {
        let (mut p1, mut p2) = (1u32, 1u32);
        let mut pre = 1usize;
        for i in (0..self.bits).rev() {
            let b = (value >> i) & 1;
            if (p1 | p2) & (((1 << TREE_TOTAL_BITS) - 1) << (32 - TREE_TOTAL_BITS)) != 0 {
                p1 >>= TREE_TOTAL_BITS;
                p2 >>= TREE_TOTAL_BITS;
            }
            let pick = |p: u32| if b == 1 { (1 << TREE_TOTAL_BITS) - p } else { p };
            p1 = p1.wrapping_mul(pick(self.prob[pre]));
            p2 = p2.wrapping_mul(pick(other.prob[pre]));
            pre = (pre << 1) | b as usize;
        }
        p1.cmp(&p2)
    }
}

/// Adaptive bit whose probability depends on the last 8 outcomes.
struct BoolState {
    prob: [u32; 256],
    state: usize,
}

impl BoolState {
    fn new() -> Self {
        BoolState { prob: [1 << 11; 256], state: 0 }
    }

    fn decode(&mut self, e: &mut Entropy) -> bool {
        let p = &mut self.prob[self.state];
        let b = e.bit(*p, 12);
        if b {
            *p -= *p >> 6;
        } else {
            *p += ((1 << 12) - *p) >> 6;
        }
        self.state = ((self.state << 1) & 0xFF) | b as usize;
        b
    }
}

/// A context-keyed model blended with a coarser one, using whichever has been luckier.
struct Predict {
    fine: Vec<BitTree>,
    coarse: Vec<BitTree>,
    lucky: Vec<i32>,
    shift: u32,
}

impl Predict {
    fn new(keys: usize, n: u32, shift: u32) -> Self {
        Predict { fine: vec![BitTree::new(n); keys], coarse: vec![BitTree::new(n); keys >> shift], lucky: vec![0; keys], shift }
    }

    fn decode(&mut self, e: &mut Entropy, key: usize) -> u32 {
        let fine = &mut self.fine[key];
        let coarse = &mut self.coarse[key >> self.shift];
        let v = if self.lucky[key] >= 0 {
            let v = fine.decode(e);
            coarse.update(v);
            v
        } else {
            let v = coarse.decode(e);
            fine.update(v);
            v
        };
        match fine.compare(coarse, v) {
            std::cmp::Ordering::Greater => self.lucky[key] = self.lucky[key].wrapping_add(1),
            std::cmp::Ordering::Less => self.lucky[key] = self.lucky[key].wrapping_sub(1),
            std::cmp::Ordering::Equal => {}
        }
        v
    }
}

/// Recently used values, most recent first, with a flag for "reuse one of them".
struct History<const N: usize> {
    rep: [u32; N],
    reuse: BoolState,
    index: BitTree,
}

impl<const N: usize> History<N> {
    fn new(first: u32) -> Self {
        History { rep: std::array::from_fn(|i| first + i as u32), reuse: BoolState::new(), index: BitTree::new(N as u32) }
    }

    fn decode(&mut self, e: &mut Entropy) -> Option<u32> {
        if !self.reuse.decode(e) {
            return None;
        }
        let i = self.index.decode(e) as usize;
        let v = self.rep[i];
        self.rep.copy_within(0..i, 1);
        self.rep[0] = v;
        Some(v)
    }

    fn push(&mut self, v: u32) {
        self.rep.copy_within(0..N - 1, 1);
        self.rep[0] = v;
    }
}

/// Recent matches (position, length), reusable by index.
struct Dictionary {
    entries: [(u32, u32); DICTIONARY_SIZE],
    found: BoolState,
    history: History<2>,
    index: BitTree,
}

impl Dictionary {
    fn new() -> Self {
        Dictionary {
            entries: std::array::from_fn(|i| (0, MATCH_MIN_LENGTH + i as u32)),
            found: BoolState::new(),
            history: History::new(0),
            index: BitTree::new(DICTIONARY_SIZE as u32),
        }
    }

    fn decode(&mut self, e: &mut Entropy) -> Option<(u32, u32)> {
        if !self.found.decode(e) {
            return None;
        }
        let n = match self.history.decode(e) {
            Some(n) => n,
            None => {
                let n = self.index.decode(e);
                self.history.push(n);
                n
            }
        } as usize;
        let hit = *self.entries.get(n)?;
        self.entries.copy_within(0..n, 1);
        self.entries[0] = hit;
        Some(hit)
    }

    fn push(&mut self, pos: u32, len: u32) {
        self.entries.copy_within(0..DICTIONARY_SIZE - 1, 1);
        self.entries[0] = (pos, len);
    }
}

struct Models {
    is_match: BoolState,
    literal: Predict,
    dictionary: Dictionary,
    dist_history: History<2>,
    dist: BitTree,
    length: Predict,
}

impl Models {
    fn new() -> Self {
        Models {
            is_match: BoolState::new(),
            literal: Predict::new(256, 256, 5),
            dictionary: Dictionary::new(),
            dist_history: History::new(MATCH_MIN_DIST),
            dist: BitTree::new(CODE_SIZE),
            length: Predict::new(CODE_SIZE as usize, CODE_SIZE, 4),
        }
    }

    fn distance(&mut self, e: &mut Entropy) -> u32 {
        if let Some(d) = self.dist_history.decode(e) {
            return d;
        }
        let code = self.dist.decode(e);
        let d = DIST_TABLE[code as usize].wrapping_add(e.value(dist_extra_bits(code)));
        self.dist_history.push(d);
        d
    }

    fn length(&mut self, e: &mut Entropy, dist: u32) -> u32 {
        let code = self.length.decode(e, dist_code(dist) as usize);
        LENGTH_TABLE[code as usize].wrapping_add(e.value(length_extra_bits(code)))
    }
}

fn corrupt() -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, "AZO data is corrupt")
}

/// Decodes one block of `out.len()` bytes from `input`.
fn decode_block(input: &[u8], out: &mut [u8]) -> io::Result<()> {
    if input.len() as u64 + 8 > out.len() as u64 {
        if input.len() != out.len() {
            return Err(corrupt());
        }
        out.copy_from_slice(input);
        return Ok(());
    }
    let e = &mut Entropy::new(input);
    let m = &mut Models::new();
    out[0] = m.literal.decode(e, 0) as u8;
    let mut pos = 1usize;
    while pos < out.len() {
        if !m.is_match.decode(e) {
            out[pos] = m.literal.decode(e, out[pos - 1] as usize) as u8;
            pos += 1;
            continue;
        }
        let (dist, len) = match m.dictionary.decode(e) {
            Some((from, len)) => ((pos as u32).wrapping_sub(from), len),
            None => {
                let dist = m.distance(e);
                let len = m.length(e, dist);
                m.dictionary.push(pos as u32, len);
                (dist, len)
            }
        };
        let (dist, len) = (dist as usize, len as usize);
        if len == 0 || dist > pos || len > out.len() - pos {
            return Err(corrupt());
        }
        for i in pos..pos + len {
            out[i] = out[i - dist];
        }
        pos += len;
    }
    if e.corrupt || e.bits.bytes_used() != input.len() as u64 {
        return Err(corrupt());
    }
    Ok(())
}

/// Streams an AZO entry: a 2-byte header, then blocks of
/// (size, packed size, size ^ packed size) big-endian, ending at `size` bytes total.
pub struct AzoReader<R> {
    inner: R,
    left: u64,
    started: bool,
    block: Vec<u8>,
    pos: usize,
}

impl<R: Read> AzoReader<R> {
    pub fn new(inner: R, size: u64) -> Self {
        Self { inner, left: size, started: false, block: Vec::new(), pos: 0 }
    }

    fn next_block(&mut self) -> io::Result<bool> {
        if !self.started {
            let mut head = [0u8; 2];
            self.inner.read_exact(&mut head)?;
            if head[0] != b'1' {
                return Err(io::Error::new(io::ErrorKind::InvalidData, "unknown AZO version"));
            }
            self.started = true;
        }
        let mut head = [0u8; 12];
        self.inner.read_exact(&mut head)?;
        let word = |i: usize| u32::from_be_bytes([head[i], head[i + 1], head[i + 2], head[i + 3]]);
        let (size, packed, check) = (word(0), word(4), word(8));
        if size < packed || size ^ packed != check {
            return Err(corrupt());
        }
        if size == 0 || packed == 0 {
            return Ok(false);
        }
        if u64::from(size) > self.left || size > MAX_BLOCK {
            return Err(corrupt());
        }
        let mut input = Vec::new();
        (&mut self.inner).take(u64::from(packed)).read_to_end(&mut input)?;
        if input.len() != packed as usize {
            return Err(io::ErrorKind::UnexpectedEof.into());
        }
        self.block.clear();
        self.block.resize(size as usize, 0);
        decode_block(&input, &mut self.block)?;
        self.left -= u64::from(size);
        self.pos = 0;
        Ok(true)
    }
}

impl<R: Read> Read for AzoReader<R> {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        if self.pos == self.block.len() && (self.left == 0 || !self.next_block()?) {
            return Ok(0);
        }
        let n = buf.len().min(self.block.len() - self.pos);
        buf[..n].copy_from_slice(&self.block[self.pos..self.pos + n]);
        self.pos += n;
        Ok(n)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn extra_bit_formulas_match_reference_tables() {
        let length: [u32; 128] = std::array::from_fn(|i| length_extra_bits(i as u32));
        let dist: [u32; 128] = std::array::from_fn(|i| dist_extra_bits(i as u32));
        for i in 0..127 {
            assert_eq!(LENGTH_TABLE[i] + (1 << length[i]), LENGTH_TABLE[i + 1], "length code {i}");
            assert_eq!(DIST_TABLE[i] + (1 << dist[i]), DIST_TABLE[i + 1], "dist code {i}");
        }
    }

    #[test]
    fn dist_code_inverts_dist_table() {
        for (code, &base) in DIST_TABLE.iter().enumerate() {
            let last = base + (1 << dist_extra_bits(code as u32)) - 1;
            assert_eq!(dist_code(base), code as u32);
            assert_eq!(dist_code(last), code as u32);
        }
    }
}
