//! Traditional PKWARE ("ZIP 2.0") stream cipher, used by ALZ and by EGG's
//! "key base XOR" method.

use std::io::{self, Read};

const fn crc_table() -> [u32; 256] {
    let mut table = [0u32; 256];
    let mut i = 0;
    while i < 256 {
        let mut c = i as u32;
        let mut k = 0;
        while k < 8 {
            c = if c & 1 != 0 { 0xEDB8_8320 ^ (c >> 1) } else { c >> 1 };
            k += 1;
        }
        table[i] = c;
        i += 1;
    }
    table
}

static CRC_TABLE: [u32; 256] = crc_table();

/// ALZip runs on Windows and keys the cipher with the password in the ANSI code
/// page; for Korean that is CP949. Passwords it cannot encode are used as UTF-8.
pub fn password_bytes(password: &str) -> Vec<u8> {
    let (bytes, _, unmappable) = encoding_rs::EUC_KR.encode(password);
    if unmappable {
        password.as_bytes().to_vec()
    } else {
        bytes.into_owned()
    }
}

#[derive(Clone)]
pub struct Keys([u32; 3]);

impl Keys {
    pub fn new(password: &[u8]) -> Self {
        let mut k = Keys([0x1234_5678, 0x2345_6789, 0x3456_7890]);
        for &b in password {
            k.update(b);
        }
        k
    }

    fn update(&mut self, b: u8) {
        let [k0, k1, k2] = &mut self.0;
        *k0 = CRC_TABLE[((*k0 ^ b as u32) & 0xFF) as usize] ^ (*k0 >> 8);
        *k1 = k1.wrapping_add(*k0 & 0xFF).wrapping_mul(134_775_813).wrapping_add(1);
        *k2 = CRC_TABLE[((*k2 ^ (*k1 >> 24)) & 0xFF) as usize] ^ (*k2 >> 8);
    }

    fn decrypt_byte(&mut self, c: u8) -> u8 {
        let t = (self.0[2] | 2) & 0xFFFF;
        let p = c ^ ((t.wrapping_mul(t ^ 1) >> 8) as u8);
        self.update(p);
        p
    }

    /// Runs the 12-byte encryption header through the cipher and returns its
    /// last plain byte, which the archive compares with a known check byte.
    pub fn decrypt_header(&mut self, header: &[u8; 12]) -> u8 {
        header.iter().fold(0, |_, &c| self.decrypt_byte(c))
    }
}

pub struct ZipCryptoReader<R> {
    inner: R,
    keys: Keys,
}

impl<R: Read> ZipCryptoReader<R> {
    /// `keys` must already have consumed the encryption header.
    pub fn new(inner: R, keys: Keys) -> Self {
        Self { inner, keys }
    }
}

impl<R: Read> Read for ZipCryptoReader<R> {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        let n = self.inner.read(buf)?;
        for b in &mut buf[..n] {
            *b = self.keys.decrypt_byte(*b);
        }
        Ok(n)
    }
}
