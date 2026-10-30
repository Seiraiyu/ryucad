//! The crate format's compression: `TfFastCompression`'s framing of LZ4
//! blocks, and `Sdf_IntegerCompression`'s coding of 32-bit integers
//! (OpenUSD's `pxr/base/tf/fastCompression.cpp` and
//! `pxr/usd/sdf/integerCoding.cpp`).
//!
//! A buffer is one byte holding the number of chunks (0 for one chunk
//! that isn't counted), then LZ4 blocks: a single block as it is, else
//! each of `LZ4_MAX_INPUT_SIZE` bytes or fewer after its length as a
//! little-endian `i32`. Integers are coded before they are compressed:
//! each the difference from the one before it, the most common
//! difference written once (the largest of those equally common), then
//! a two-bit code per integer (that value, an `i8`, an `i16` or an
//! `i32`) packed four to a byte from the low bits, then the differences
//! the codes say are written, at their sizes, little-endian.

/// LZ4's largest input to one block, `LZ4_MAX_INPUT_SIZE`.
const MAX_CHUNK: usize = 0x7E00_0000;

/// `bytes` LZ4-compressed in `TfFastCompression`'s framing.
pub fn fast(bytes: &[u8]) -> Vec<u8> {
    if bytes.len() <= MAX_CHUNK {
        let mut out = vec![0u8];
        out.extend_from_slice(&lz4_flex::block::compress(bytes));
        return out;
    }
    let chunks = bytes.len().div_ceil(MAX_CHUNK);
    // 127 chunks is the most the framing counts (OpenUSD's own limit).
    assert!(chunks <= 127, "more than 127 LZ4 chunks");
    let mut out = vec![chunks as u8];
    for chunk in bytes.chunks(MAX_CHUNK) {
        let packed = lz4_flex::block::compress(chunk);
        let n = i32::try_from(packed.len()).expect("an LZ4 block under 2 GiB");
        out.extend_from_slice(&n.to_le_bytes());
        out.extend_from_slice(&packed);
    }
    out
}

/// `ints` coded as `Sdf_IntegerCompression` codes them, uncompressed.
pub fn encode_ints(ints: &[i32]) -> Vec<u8> {
    if ints.is_empty() {
        return Vec::new();
    }
    let diffs: Vec<i32> = ints
        .iter()
        .scan(0i32, |prev, &x| {
            let d = x.wrapping_sub(*prev);
            *prev = x;
            Some(d)
        })
        .collect();
    // The most common difference, the larger of two equally common, as
    // OpenUSD counts them in order.
    let mut counts = std::collections::HashMap::new();
    let (mut common, mut most) = (0i32, 0usize);
    for &d in &diffs {
        let c = counts.entry(d).or_insert(0usize);
        *c += 1;
        if *c > most || (*c == most && d > common) {
            common = d;
            most = *c;
        }
    }
    let mut codes = vec![0u8; (ints.len() * 2).div_ceil(8)];
    let mut vints = Vec::with_capacity(ints.len());
    for (k, &d) in diffs.iter().enumerate() {
        let code = if d == common {
            0
        } else if let Ok(small) = i8::try_from(d) {
            vints.extend_from_slice(&small.to_le_bytes());
            1
        } else if let Ok(medium) = i16::try_from(d) {
            vints.extend_from_slice(&medium.to_le_bytes());
            2
        } else {
            vints.extend_from_slice(&d.to_le_bytes());
            3
        };
        codes[k / 4] |= code << (2 * (k % 4));
    }
    let mut out = common.to_le_bytes().to_vec();
    out.extend_from_slice(&codes);
    out.extend_from_slice(&vints);
    out
}

/// `ints` coded and compressed, as `Sdf_IntegerCompression::
/// CompressToBuffer` writes them.
pub fn ints(ints: &[i32]) -> Vec<u8> {
    fast(&encode_ints(ints))
}

/// The same for unsigned integers, read as their bits.
pub fn uints(ints: &[u32]) -> Vec<u8> {
    let signed: Vec<i32> = ints.iter().map(|&x| x as i32).collect();
    self::ints(&signed)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn integers_code_as_openusd_documents() {
        // integerCoding.cpp's own example: differences 123, 1, 1, 100000,
        // 0, 1, 0; the common one 1.
        let coded = encode_ints(&[123, 124, 125, 100125, 100125, 100126, 100126]);
        let mut want = 1i32.to_le_bytes().to_vec();
        // Codes 01 00 00 11 | 01 00 01, from the low bits.
        want.push(0b11_00_00_01);
        want.push(0b00_01_00_01);
        want.push(123);
        want.extend_from_slice(&100000i32.to_le_bytes());
        want.extend_from_slice(&[0, 0]);
        assert_eq!(coded, want);
    }

    #[test]
    fn ties_take_the_larger_and_sizes_follow_the_difference() {
        // Differences 5, -5 (each once): 5 is common; -5 an i8.
        assert_eq!(encode_ints(&[5, 0]), [5, 0, 0, 0, 0b01_00, 0xfb]);
        // A difference of 300 is an i16, of 70,000 an i32.
        let coded = encode_ints(&[0, 300, 70300, 70300]);
        assert_eq!(&coded[..4], &0i32.to_le_bytes());
        assert_eq!(coded[4], 0b00_11_10_00);
        assert_eq!(&coded[5..], [44, 1, 0x70, 0x11, 1, 0]);
        // Wrapping differences, as OpenUSD's two's complement arithmetic:
        // MIN, then MAX - MIN = -1, the larger and so the common one.
        let coded = encode_ints(&[i32::MIN, i32::MAX]);
        assert_eq!(&coded[..5], [0xff, 0xff, 0xff, 0xff, 0b00_11]);
        assert_eq!(&coded[5..], &i32::MIN.to_le_bytes());
        assert!(encode_ints(&[]).is_empty());
    }

    #[test]
    fn one_chunk_is_framed_by_a_zero() {
        let data = b"tokens\0tokens\0tokens\0tokens\0";
        let packed = fast(data);
        assert_eq!(packed[0], 0);
        assert_eq!(
            lz4_flex::block::decompress(&packed[1..], data.len()).unwrap(),
            data
        );
        let packed = uints(&[7; 40]);
        let coded = lz4_flex::block::decompress(&packed[1..], 4 + 10 + 1).unwrap();
        assert_eq!(coded, encode_ints(&[7; 40]));
    }
}
