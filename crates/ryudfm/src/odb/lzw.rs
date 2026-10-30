//! Unix `compress` (`.Z`): the LZW streams ODB++ allows for any single file.
//!
//! The format is `1f 9d`, a flags byte (the largest code width, 9 to 16
//! bits, and bit 7 for block mode, where code 256 clears the table), then
//! codes of 9 bits growing to the largest, least significant bit first.
//! As `compress` writes them, codes come in groups of eight: when the
//! width grows, or the table is cleared, the rest of the current group of
//! eight codes is skipped. This decoder follows ncompress 5.0.

/// Why a `.Z` stream doesn't decode.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum LzwError {
    /// Not `1f 9d`.
    Magic,
    /// A largest code width outside 9 to 16 bits.
    Width(u8),
    /// A code that isn't in the table yet, by its bit offset.
    Corrupt { at: u64 },
    /// More than the caller allows unpacked.
    TooLarge,
}

impl std::fmt::Display for LzwError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            LzwError::Magic => write!(f, "not a .Z stream"),
            LzwError::Width(w) => write!(f, "a .Z stream of {w}-bit codes"),
            LzwError::Corrupt { at } => write!(f, "a corrupt .Z stream at bit {at}"),
            LzwError::TooLarge => write!(f, "a .Z stream that unpacks too large"),
        }
    }
}

const CLEAR: u32 = 256;

/// Decodes `data`, refusing to unpack more than `limit` bytes.
pub fn decompress(data: &[u8], limit: usize) -> Result<Vec<u8>, LzwError> {
    if data.len() < 3 || data[0] != 0x1f || data[1] != 0x9d {
        return Err(LzwError::Magic);
    }
    let max_bits = u32::from(data[2] & 0x1f);
    let block = data[2] & 0x80 != 0;
    if !(9..=16).contains(&max_bits) {
        return Err(LzwError::Width(max_bits as u8));
    }
    let max_max = 1u32 << max_bits;
    let codes = &data[3..];
    let total = codes.len() as u64 * 8;

    // Entry `c` is `prefix[c]` followed by `suffix[c]`; 0 to 255 are bytes.
    let mut prefix = vec![0u16; max_max as usize];
    let mut suffix = vec![0u8; max_max as usize];
    for (c, s) in suffix.iter_mut().enumerate().take(256) {
        *s = c as u8;
    }
    let mut out = Vec::with_capacity(data.len().saturating_mul(3).min(limit));
    let mut stack: Vec<u8> = Vec::new();

    let mut bits = 9u32;
    let mut max_code = (1u32 << bits) - 1;
    let mut free = if block { 257 } else { 256 };
    let mut old: Option<u32> = None;
    let mut fin = 0u8;
    // The bit offset of the next code, and where its group of eight began.
    let mut pos = 0u64;
    let mut base = 0u64;
    let align = |pos: u64, base: u64, bits: u32| -> u64 {
        let group = u64::from(bits) * 8;
        base + (pos - base).div_ceil(group) * group
    };

    loop {
        if free > max_code {
            pos = align(pos, base, bits);
            base = pos;
            bits += 1;
            max_code = if bits == max_bits {
                max_max
            } else {
                (1 << bits) - 1
            };
        }
        if pos + u64::from(bits) > total {
            break;
        }
        let code = read(codes, pos, bits);
        let at = pos;
        pos += u64::from(bits);
        let Some(prev) = old else {
            if code >= 256 {
                return Err(LzwError::Corrupt { at });
            }
            fin = code as u8;
            push(&mut out, &[fin], limit)?;
            old = Some(code);
            continue;
        };
        if code == CLEAR && block {
            free = 256;
            pos = align(pos, base, bits);
            base = pos;
            bits = 9;
            max_code = (1 << bits) - 1;
            continue;
        }
        let mut c = code;
        stack.clear();
        if c >= free {
            // The KwKwK case: the code being defined now.
            if c > free {
                return Err(LzwError::Corrupt { at });
            }
            stack.push(fin);
            c = prev;
        }
        while c >= 256 {
            stack.push(suffix[c as usize]);
            c = u32::from(prefix[c as usize]);
        }
        fin = suffix[c as usize];
        stack.push(fin);
        stack.reverse();
        push(&mut out, &stack, limit)?;
        if free < max_max {
            prefix[free as usize] = prev as u16;
            suffix[free as usize] = fin;
            free += 1;
        }
        old = Some(code);
    }
    Ok(out)
}

fn push(out: &mut Vec<u8>, bytes: &[u8], limit: usize) -> Result<(), LzwError> {
    if out.len() + bytes.len() > limit {
        return Err(LzwError::TooLarge);
    }
    out.extend_from_slice(bytes);
    Ok(())
}

/// The `bits`-bit code at bit `pos`, least significant bit first.
fn read(data: &[u8], pos: u64, bits: u32) -> u32 {
    let byte = (pos / 8) as usize;
    let mut v = 0u32;
    for k in 0..3 {
        if let Some(b) = data.get(byte + k) {
            v |= u32::from(*b) << (8 * k);
        }
    }
    (v >> (pos % 8)) & ((1 << bits) - 1)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// `small` below, as ncompress 5.0 (`compress -c`) wrote it: block
    /// mode, 16 bits, crossing from 9-bit codes to 10.
    const SMALL_Z: &[&str] = &[
        "1f9d902050c8f0c1848b1b143eacc09101050e0e2e28a0806002020a0a2e38e0409101c78a0f146eb830f1210305083a",
        "70dcb8814327e0c08207132e6cf830e2c48a17336eecf831e4c8922753ae6c2990a041840a193a842891a2458c1a397a",
        "042992a449942a59ba341a33294da6379fea94dab32a50ac43b7c2443a73a94da739a3f2a4faf3aa50ad45d7ca545ab3",
        "294ea83ba7fab41a342bd19747f77e7dfb77ecdcc167ef1ee6ca962f58b880c9d2258c162fe2ae6dfb868d1bb86cddc2",
        "69f326f6ead6af58b982cdda35ac7675e8cb8d619be62cb93668cb8c5f97de1c99b6eadf8b5d93d60c7976eacf95938f",
        "cefc58366acf9415b79eee38f6e9ce93f5b2168db9fbeee2cfb38fc72d9cb975f0bea36f2faf9bb873ece26f075f5efd",
        "7befe3f29197db70cd5d4707",
    ];

    fn small() -> Vec<u8> {
        (0..900u32)
            .map(|i| (32 + (i * 7 + i * i) % 90) as u8)
            .collect()
    }

    fn unhex(lines: &[&str]) -> Vec<u8> {
        let s: String = lines.concat();
        (0..s.len())
            .step_by(2)
            .map(|i| u8::from_str_radix(&s[i..i + 2], 16).unwrap())
            .collect()
    }

    /// A `compress`-style encoder: greedy matches, codes growing from 9
    /// bits with the rest of each group of eight skipped, and a clear after
    /// the code that ends past each offset in `clear_at`.
    fn compress(data: &[u8], max_bits: u32, clear_at: &[usize]) -> Vec<u8> {
        struct Codes {
            bits: Vec<bool>,
            base: usize,
            width: u32,
            max_code: u32,
            max_bits: u32,
            // Codes since the start or the last clear, and which it was.
            k: u32,
            cleared: bool,
        }
        impl Codes {
            fn align(&mut self) {
                let group = self.width as usize * 8;
                let len = self.base + (self.bits.len() - self.base).div_ceil(group) * group;
                self.bits.resize(len, false);
                self.base = len;
            }
            fn code(&mut self, code: u32) {
                // The decoder's next free entry before it reads this code.
                let max_max = 1u32 << self.max_bits;
                let free = if self.cleared {
                    256 + self.k
                } else {
                    256 + self.k.max(1)
                };
                let max_code = if self.width == self.max_bits {
                    max_max
                } else {
                    (1 << self.width) - 1
                };
                if free.min(max_max) > max_code {
                    self.align();
                    self.width += 1;
                }
                for b in 0..self.width {
                    self.bits.push(code >> b & 1 == 1);
                }
                self.k += 1;
            }
        }
        let max_max = 1u32 << max_bits;
        let mut c = Codes {
            bits: Vec::new(),
            base: 0,
            width: 9,
            max_code: 511,
            max_bits,
            k: 0,
            cleared: false,
        };
        let mut table = std::collections::HashMap::new();
        let mut free = 257u32;
        let mut clears = clear_at.iter().copied().peekable();
        let mut ent = u32::from(data[0]);
        for (i, &b) in data.iter().enumerate().skip(1) {
            if let Some(&next) = table.get(&(ent, b)) {
                ent = next;
                continue;
            }
            c.code(ent);
            if clears.next_if(|&at| i > at).is_some() {
                c.code(CLEAR);
                c.align();
                c.width = 9;
                c.max_code = 511;
                c.k = 0;
                c.cleared = true;
                table.clear();
                free = 257;
            } else if free < max_max {
                table.insert((ent, b), free);
                free += 1;
            }
            ent = u32::from(b);
        }
        c.code(ent);
        let mut out = vec![0x1f, 0x9d, 0x80 | max_bits as u8];
        out.extend(c.bits.chunks(8).map(|byte| {
            byte.iter()
                .enumerate()
                .fold(0u8, |v, (k, &x)| v | (u8::from(x) << k))
        }));
        out
    }

    /// Bytes from a 64-bit LCG, `alphabet` letters wide.
    fn noise(n: usize, alphabet: u64, seed: u64) -> Vec<u8> {
        let mut s = seed;
        (0..n)
            .map(|_| {
                s = s
                    .wrapping_mul(6364136223846793005)
                    .wrapping_add(1442695040888963407);
                b'!' + ((s >> 33) % alphabet) as u8
            })
            .collect()
    }

    #[test]
    fn reads_what_ncompress_wrote() {
        let z = unhex(SMALL_Z);
        assert_eq!(z.len(), 300);
        assert_eq!(decompress(&z, 1 << 20).unwrap(), small());
    }

    #[test]
    fn codes_grow_to_every_width_and_clear() {
        // Enough distinct strings to fill a 16-bit table, then clears.
        let data = noise(400_000, 90, 1);
        for max_bits in [10, 12, 16] {
            let z = compress(&data, max_bits, &[]);
            assert_eq!(decompress(&z, 1 << 20).unwrap(), data, "{max_bits} bits");
        }
        let z = compress(&data, 16, &[1_000, 150_000, 150_200, 390_000]);
        assert_eq!(decompress(&z, 1 << 20).unwrap(), data);
        // Repetitive text, as features files are, and a single byte.
        let text = b"L 0.125 -1.5 0.25 -1.5 3 P 0\n".repeat(5_000);
        assert_eq!(
            decompress(&compress(&text, 16, &[]), 1 << 20).unwrap(),
            text
        );
        assert_eq!(decompress(&compress(b"x", 16, &[]), 8).unwrap(), b"x");
    }

    #[test]
    fn refuses_what_isnt_one() {
        assert_eq!(decompress(b"\x1f\x8b\x08", 10), Err(LzwError::Magic));
        assert_eq!(decompress(b"\x1f", 10), Err(LzwError::Magic));
        assert_eq!(decompress(b"\x1f\x9d\x88", 10), Err(LzwError::Width(8)));
        assert_eq!(decompress(b"\x1f\x9d\x91", 10), Err(LzwError::Width(17)));
        // A first code past the bytes, then a code past the table.
        assert_eq!(
            decompress(&[0x1f, 0x9d, 0x90, 0x00, 0x03], 10),
            Err(LzwError::Corrupt { at: 0 })
        );
        assert_eq!(
            decompress(&[0x1f, 0x9d, 0x90, 0x41, 0xfe, 0x03], 10),
            Err(LzwError::Corrupt { at: 9 })
        );
        let data = noise(10_000, 4, 2);
        let z = compress(&data, 16, &[]);
        assert_eq!(decompress(&z, 9_999), Err(LzwError::TooLarge));
        assert_eq!(decompress(&z, 10_000).unwrap(), data);
        // Truncated: what decodes before the end, never a panic.
        for n in 3..z.len() {
            let _ = decompress(&z[..n], 1 << 20);
        }
    }
}
