//! EGG's AES and LEA encryption: PBKDF2-HMAC-SHA1 (1000 rounds) derives the key,
//! a MAC key and a 2-byte password check; data is the block cipher in counter mode,
//! followed by a 10-byte HMAC-SHA1 of the ciphertext. AES uses the WinZip AE layout
//! (little-endian counter from 1), LEA a big-endian counter from 0.

use std::io::{self, Read};

use aes::cipher::{BlockCipherEncrypt, KeyInit};
use aes::{Aes128, Aes256};
use hmac::{Hmac, Mac};
use sha1::Sha1;

use super::lea::Lea;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    Aes128,
    Aes256,
    Lea128,
    Lea256,
}

impl Kind {
    pub fn salt_len(self) -> usize {
        match self {
            Kind::Aes128 | Kind::Lea128 => 8,
            Kind::Aes256 | Kind::Lea256 => 16,
        }
    }
    fn key_len(self) -> usize {
        match self {
            Kind::Aes128 | Kind::Lea128 => 16,
            Kind::Aes256 | Kind::Lea256 => 32,
        }
    }
}

enum Cipher {
    Aes128(Box<Aes128>),
    Aes256(Box<Aes256>),
    Lea(Lea),
}

impl Cipher {
    fn keystream(&self, index: u64) -> [u8; 16] {
        match self {
            Cipher::Aes128(c) => aes_block(c.as_ref(), index),
            Cipher::Aes256(c) => aes_block(c.as_ref(), index),
            Cipher::Lea(c) => {
                let mut b = (index as u128).to_be_bytes();
                c.encrypt_block(&mut b);
                b
            }
        }
    }
}

fn aes_block(c: &impl BlockCipherEncrypt<BlockSize = aes::cipher::consts::U16>, index: u64) -> [u8; 16] {
    let mut b = aes::Block::from((index as u128 + 1).to_le_bytes());
    c.encrypt_block(&mut b);
    b.into()
}

/// Key material for one entry, already checked against the stored password check.
pub struct Keys {
    cipher: Cipher,
    mac_key: Vec<u8>,
}

impl Keys {
    /// `None` when the password does not match `check`.
    pub fn derive(kind: Kind, password: &[u8], salt: &[u8], check: [u8; 2]) -> Option<Keys> {
        let key_len = kind.key_len();
        let mut out = vec![0u8; 2 * key_len + 2];
        pbkdf2::pbkdf2_hmac::<Sha1>(password, salt, 1000, &mut out);
        if out[2 * key_len..] != check {
            return None;
        }
        let key = &out[..key_len];
        let cipher = match kind {
            Kind::Aes128 => Cipher::Aes128(Box::new(Aes128::new_from_slice(key).ok()?)),
            Kind::Aes256 => Cipher::Aes256(Box::new(Aes256::new_from_slice(key).ok()?)),
            Kind::Lea128 | Kind::Lea256 => Cipher::Lea(Lea::new(key)?),
        };
        Some(Keys { cipher, mac_key: out[key_len..2 * key_len].to_vec() })
    }
}

pub struct CtrReader<R> {
    inner: R,
    keys: std::rc::Rc<Keys>,
    mac: Hmac<Sha1>,
    expected_mac: [u8; 10],
    offset: u64,
    keystream: [u8; 16],
    finished: bool,
}

impl<R: Read> CtrReader<R> {
    pub fn new(inner: R, keys: std::rc::Rc<Keys>, expected_mac: [u8; 10]) -> Self {
        let mac = <Hmac<Sha1> as hmac::KeyInit>::new_from_slice(&keys.mac_key).expect("HMAC takes any key length");
        Self { inner, keys, mac, expected_mac, offset: 0, keystream: [0; 16], finished: false }
    }
}

impl<R: Read> Read for CtrReader<R> {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        let n = self.inner.read(buf)?;
        if n == 0 && !buf.is_empty() && !self.finished {
            self.finished = true;
            let tag = self.mac.clone().finalize().into_bytes();
            if tag[..10] != self.expected_mac {
                return Err(io::Error::new(io::ErrorKind::InvalidData, "encrypted data failed its MAC check"));
            }
        }
        self.mac.update(&buf[..n]);
        for b in &mut buf[..n] {
            let i = (self.offset % 16) as usize;
            if i == 0 {
                self.keystream = self.keys.cipher.keystream(self.offset / 16);
            }
            *b ^= self.keystream[i];
            self.offset += 1;
        }
        Ok(n)
    }
}
