//! SipHash-2-4 with 128-bit output (Aumasson and Bernstein, 2012).
//!
//! Only wrapping integer arithmetic, rotations and little-endian byte
//! order, so the result is identical on every platform. Names need no
//! protection against an attacker, only against accidental collisions, so
//! the key is a fixed constant ([`NAME_KEY`]).

/// The fixed key for name hashes. Changing it renames everything.
pub(crate) const NAME_KEY: [u8; 16] = *b"ryukei/names/v1.";

fn round(v: &mut [u64; 4]) {
    v[0] = v[0].wrapping_add(v[1]);
    v[1] = v[1].rotate_left(13);
    v[1] ^= v[0];
    v[0] = v[0].rotate_left(32);
    v[2] = v[2].wrapping_add(v[3]);
    v[3] = v[3].rotate_left(16);
    v[3] ^= v[2];
    v[0] = v[0].wrapping_add(v[3]);
    v[3] = v[3].rotate_left(21);
    v[3] ^= v[0];
    v[2] = v[2].wrapping_add(v[1]);
    v[1] = v[1].rotate_left(17);
    v[1] ^= v[2];
    v[2] = v[2].rotate_left(32);
}

fn compress(v: &mut [u64; 4], m: u64) {
    v[3] ^= m;
    round(v);
    round(v);
    v[0] ^= m;
}

fn le_u64(bytes: &[u8]) -> u64 {
    let mut b = [0u8; 8];
    b[..bytes.len()].copy_from_slice(bytes);
    u64::from_le_bytes(b)
}

/// SipHash-2-4-128 of `msg` under `key`. The 16 output bytes, read as a
/// little-endian `u128`.
pub(crate) fn siphash128(key: &[u8; 16], msg: &[u8]) -> u128 {
    let k0 = le_u64(&key[..8]);
    let k1 = le_u64(&key[8..]);
    let mut v = [
        k0 ^ 0x736f_6d65_7073_6575,
        k1 ^ 0x646f_7261_6e64_6f6d ^ 0xee,
        k0 ^ 0x6c79_6765_6e65_7261,
        k1 ^ 0x7465_6462_7974_6573,
    ];
    let (blocks, rest) = msg.as_chunks::<8>();
    for block in blocks {
        compress(&mut v, u64::from_le_bytes(*block));
    }
    let last = ((msg.len() as u64) << 56) | le_u64(rest);
    compress(&mut v, last);
    v[2] ^= 0xee;
    for _ in 0..4 {
        round(&mut v);
    }
    let lo = v[0] ^ v[1] ^ v[2] ^ v[3];
    v[1] ^= 0xdd;
    for _ in 0..4 {
        round(&mut v);
    }
    let hi = v[0] ^ v[1] ^ v[2] ^ v[3];
    u128::from(lo) | (u128::from(hi) << 64)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// From the reference implementation's `vectors.h` (`vectors_sip128`):
    /// key `00 01 .. 0f`, message `00 01 .. (len - 1)`.
    const VECTORS: [(usize, [u8; 16]); 7] = [
        (
            0,
            [
                0xa3, 0x81, 0x7f, 0x04, 0xba, 0x25, 0xa8, 0xe6, 0x6d, 0xf6, 0x72, 0x14, 0xc7, 0x55,
                0x02, 0x93,
            ],
        ),
        (
            1,
            [
                0xda, 0x87, 0xc1, 0xd8, 0x6b, 0x99, 0xaf, 0x44, 0x34, 0x76, 0x59, 0x11, 0x9b, 0x22,
                0xfc, 0x45,
            ],
        ),
        (
            7,
            [
                0xa1, 0xf1, 0xeb, 0xbe, 0xd8, 0xdb, 0xc1, 0x53, 0xc0, 0xb8, 0x4a, 0xa6, 0x1f, 0xf0,
                0x82, 0x39,
            ],
        ),
        (
            8,
            [
                0x3b, 0x62, 0xa9, 0xba, 0x62, 0x58, 0xf5, 0x61, 0x0f, 0x83, 0xe2, 0x64, 0xf3, 0x14,
                0x97, 0xb4,
            ],
        ),
        (
            15,
            [
                0x54, 0x93, 0xe9, 0x99, 0x33, 0xb0, 0xa8, 0x11, 0x7e, 0x08, 0xec, 0x0f, 0x97, 0xcf,
                0xc3, 0xd9,
            ],
        ),
        (
            16,
            [
                0x6e, 0xe2, 0xa4, 0xca, 0x67, 0xb0, 0x54, 0xbb, 0xfd, 0x33, 0x15, 0xbf, 0x85, 0x23,
                0x05, 0x77,
            ],
        ),
        (
            63,
            [
                0x51, 0x50, 0xd1, 0x77, 0x2f, 0x50, 0x83, 0x4a, 0x50, 0x3e, 0x06, 0x9a, 0x97, 0x3f,
                0xbd, 0x7c,
            ],
        ),
    ];

    #[test]
    fn reference_vectors() {
        let key: [u8; 16] = core::array::from_fn(|i| i as u8);
        for (len, expected) in VECTORS {
            let msg: Vec<u8> = (0..len as u8).collect();
            assert_eq!(siphash128(&key, &msg).to_le_bytes(), expected, "len {len}");
        }
    }
}
