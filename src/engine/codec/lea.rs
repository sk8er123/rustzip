//! LEA (KS X 3246), the Korean 128-bit block cipher. EGG only needs encryption,
//! because it runs LEA in counter mode.

const DELTA: [u32; 8] = [
    0xc3ef_e9db, 0x4462_6b02, 0x79e2_7c8a, 0x78df_30ec, 0x715e_a49e, 0xc785_da0a, 0xe04e_f22a, 0xe5c4_0957,
];

pub struct Lea {
    round_keys: Vec<[u32; 6]>,
}

fn words<const N: usize>(bytes: &[u8]) -> [u32; N] {
    std::array::from_fn(|i| u32::from_le_bytes([bytes[4 * i], bytes[4 * i + 1], bytes[4 * i + 2], bytes[4 * i + 3]]))
}

impl Lea {
    /// `key` is 16, 24 or 32 bytes; any other length gives `None`.
    pub fn new(key: &[u8]) -> Option<Self> {
        let round_keys = match key.len() {
            16 => {
                let mut t: [u32; 4] = words(key);
                (0..24)
                    .map(|i| {
                        let d = DELTA[i % 4].rotate_left(i as u32);
                        t[0] = t[0].wrapping_add(d).rotate_left(1);
                        t[1] = t[1].wrapping_add(d.rotate_left(1)).rotate_left(3);
                        t[2] = t[2].wrapping_add(d.rotate_left(2)).rotate_left(6);
                        t[3] = t[3].wrapping_add(d.rotate_left(3)).rotate_left(11);
                        [t[0], t[1], t[2], t[1], t[3], t[1]]
                    })
                    .collect()
            }
            24 => {
                let mut t: [u32; 6] = words(key);
                (0..28)
                    .map(|i| {
                        let d = DELTA[i % 6].rotate_left(i as u32);
                        const ROT: [u32; 6] = [1, 3, 6, 11, 13, 17];
                        for (k, r) in ROT.iter().enumerate() {
                            t[k] = t[k].wrapping_add(d.rotate_left(k as u32)).rotate_left(*r);
                        }
                        t
                    })
                    .collect()
            }
            32 => {
                let mut t: [u32; 8] = words(key);
                (0..32)
                    .map(|i| {
                        let d = DELTA[i % 8].rotate_left(i as u32 & 0x1f);
                        const ROT: [u32; 6] = [1, 3, 6, 11, 13, 17];
                        std::array::from_fn(|k| {
                            let j = (6 * i + k) % 8;
                            t[j] = t[j].wrapping_add(d.rotate_left(k as u32)).rotate_left(ROT[k]);
                            t[j]
                        })
                    })
                    .collect()
            }
            _ => return None,
        };
        Some(Lea { round_keys })
    }

    pub fn encrypt_block(&self, block: &mut [u8; 16]) {
        let mut x: [u32; 4] = words(block);
        for rk in &self.round_keys {
            let [a, b, c, d] = x;
            x = [
                (a ^ rk[0]).wrapping_add(b ^ rk[1]).rotate_left(9),
                (b ^ rk[2]).wrapping_add(c ^ rk[3]).rotate_right(5),
                (c ^ rk[4]).wrapping_add(d ^ rk[5]).rotate_right(3),
                a,
            ];
        }
        for (i, w) in x.iter().enumerate() {
            block[4 * i..4 * i + 4].copy_from_slice(&w.to_le_bytes());
        }
    }
}

#[cfg(test)]
mod tests {
    use super::Lea;

    fn hex(s: &str) -> Vec<u8> {
        (0..s.len()).step_by(2).map(|i| u8::from_str_radix(&s[i..i + 2], 16).unwrap()).collect()
    }

    fn check(key: &str, pt: &str, ct: &str) {
        let lea = Lea::new(&hex(key)).unwrap();
        let mut block: [u8; 16] = hex(pt).try_into().unwrap();
        lea.encrypt_block(&mut block);
        assert_eq!(block.to_vec(), hex(ct));
    }

    #[test]
    fn kisa_reference_vectors() {
        check("0f1e2d3c4b5a69788796a5b4c3d2e1f0", "101112131415161718191a1b1c1d1e1f", "9fc84e3528c6c6185532c7a704648bfd");
        check(
            "0f1e2d3c4b5a69788796a5b4c3d2e1f0f0e1d2c3b4a59687",
            "202122232425262728292a2b2c2d2e2f",
            "6fb95e325aad1b878cdcf5357674c6f2",
        );
        check(
            "0f1e2d3c4b5a69788796a5b4c3d2e1f0f0e1d2c3b4a5968778695a4b3c2d1e0f",
            "303132333435363738393a3b3c3d3e3f",
            "d651aff647b189c13a8900ca27f9e197",
        );
    }
}
