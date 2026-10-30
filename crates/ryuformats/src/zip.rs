//! Zip archives as packages want them: entries in the order given, every
//! time the DOS epoch (1980-01-01 00:00), no comments, no ZIP64. OPC
//! packages (3MF's) deflate each entry with `miniz_oxide` at one fixed
//! level and carry no extra fields ([`write`]); usdz packages store each
//! entry as it is, its data starting on a 64-byte boundary, padded by an
//! extra field as OpenUSD pads it ([`write_stored`]). The same entries
//! always give the same bytes.

use std::io::{self, Write};

/// The deflate level: fixed, so the bytes are.
const LEVEL: u8 = 6;

/// The DOS date of 1980-01-01; its time is 00:00.
const EPOCH_DATE: u16 = (1 << 5) | 1;

/// One file of an archive: its name (with `/`) and its contents.
#[derive(Clone, Copy, Debug)]
pub struct Entry<'a> {
    pub name: &'a str,
    pub data: &'a [u8],
}

/// The CRC-32 (ISO 3309, as zip uses it) of `data`.
pub fn crc32(data: &[u8]) -> u32 {
    const TABLE: [u32; 256] = {
        let mut t = [0u32; 256];
        let mut i = 0;
        while i < 256 {
            let mut c = i as u32;
            let mut k = 0;
            while k < 8 {
                c = if c & 1 == 1 {
                    0xEDB8_8320 ^ (c >> 1)
                } else {
                    c >> 1
                };
                k += 1;
            }
            t[i] = c;
            i += 1;
        }
        t
    };
    !data.iter().fold(!0u32, |c, &b| {
        TABLE[((c ^ u32::from(b)) & 0xff) as usize] ^ (c >> 8)
    })
}

fn too_large(what: &str) -> io::Error {
    io::Error::new(
        io::ErrorKind::InvalidInput,
        format!("{what} past 4 GiB: zip64 isn't written"),
    )
}

fn u32_of(n: usize, what: &str) -> io::Result<u32> {
    u32::try_from(n).map_err(|_| too_large(what))
}

/// How an archive's entries are written.
#[derive(Clone, Copy)]
enum Pack {
    Deflate,
    /// Stored, each entry's data on a multiple of this.
    Stored(usize),
}

/// The extra field's ID OpenUSD's zip writer pads with: arbitrary and
/// unreserved.
const PADDING_ID: u16 = 0x1986;

/// The extra field that puts data otherwise at `at` on a multiple of
/// `align`: a header (its ID and its data's size) and zeros, at least a
/// header long, as OpenUSD's `_ComputeExtraFieldPaddingSize` sizes it.
fn padding(at: usize, align: usize) -> Vec<u8> {
    let mut n = (align - at % align) % align;
    if n == 0 {
        return Vec::new();
    }
    if n < 4 {
        n += align;
    }
    let mut extra = vec![0u8; n];
    extra[..2].copy_from_slice(&PADDING_ID.to_le_bytes());
    extra[2..4].copy_from_slice(&((n - 4) as u16).to_le_bytes());
    extra
}

/// Writes `entries` as a zip archive, each deflated.
pub fn write(entries: &[Entry<'_>], out: &mut dyn Write) -> io::Result<()> {
    archive(entries, Pack::Deflate, out)
}

/// Writes `entries` as a zip archive, each stored as it is, its data
/// starting on a multiple of `align` bytes from the archive's start.
pub fn write_stored(entries: &[Entry<'_>], align: usize, out: &mut dyn Write) -> io::Result<()> {
    archive(entries, Pack::Stored(align), out)
}

fn archive(entries: &[Entry<'_>], pack: Pack, out: &mut dyn Write) -> io::Result<()> {
    let count = u16::try_from(entries.len())
        .map_err(|_| io::Error::new(io::ErrorKind::InvalidInput, "more than 65,535 entries"))?;
    let mut central = Vec::new();
    let mut offset = 0usize;
    for e in entries {
        let name = e.name.as_bytes();
        let name_len = u16::try_from(name.len())
            .map_err(|_| io::Error::new(io::ErrorKind::InvalidInput, "an entry name too long"))?;
        // The version needed (2.0 for deflate, 1.0 stored), the method,
        // the data as written and the extra field.
        let (version, method, packed, extra) = match pack {
            Pack::Deflate => (
                20u16,
                8u16,
                std::borrow::Cow::Owned(miniz_oxide::deflate::compress_to_vec(e.data, LEVEL)),
                Vec::new(),
            ),
            Pack::Stored(align) => (
                10,
                0,
                std::borrow::Cow::Borrowed(e.data),
                padding(offset + 30 + name.len(), align),
            ),
        };
        let crc = crc32(e.data);
        let (size, packed_size) = (u32_of(e.data.len(), e.name)?, u32_of(packed.len(), e.name)?);
        // What the local and the central headers share, from the version
        // needed to the extra field's length.
        let mut common = Vec::with_capacity(26);
        for v in [version, 0, method, 0, EPOCH_DATE] {
            common.extend_from_slice(&v.to_le_bytes());
        }
        for v in [crc, packed_size, size] {
            common.extend_from_slice(&v.to_le_bytes());
        }
        common.extend_from_slice(&name_len.to_le_bytes());
        common.extend_from_slice(&(extra.len() as u16).to_le_bytes());
        out.write_all(&0x0403_4b50u32.to_le_bytes())?;
        out.write_all(&common)?;
        out.write_all(name)?;
        out.write_all(&extra)?;
        out.write_all(&packed)?;
        central.extend_from_slice(&0x0201_4b50u32.to_le_bytes());
        // Made by the version needed on MS-DOS, so no attributes are a
        // host's.
        central.extend_from_slice(&version.to_le_bytes());
        central.extend_from_slice(&common);
        // Comment length, disk, internal and external attributes.
        central.extend_from_slice(&[0; 10]);
        central.extend_from_slice(&u32_of(offset, "an entry's offset")?.to_le_bytes());
        central.extend_from_slice(name);
        central.extend_from_slice(&extra);
        offset += 30 + name.len() + extra.len() + packed.len();
    }
    out.write_all(&central)?;
    out.write_all(&0x0605_4b50u32.to_le_bytes())?;
    out.write_all(&[0; 4])?;
    out.write_all(&count.to_le_bytes())?;
    out.write_all(&count.to_le_bytes())?;
    out.write_all(&u32_of(central.len(), "the central directory")?.to_le_bytes())?;
    out.write_all(&u32_of(offset, "the central directory's offset")?.to_le_bytes())?;
    out.write_all(&0u16.to_le_bytes())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn crc32_check_value() {
        assert_eq!(crc32(b"123456789"), 0xCBF4_3926);
        assert_eq!(crc32(b""), 0);
    }

    #[test]
    fn archives_read_back_and_never_change() {
        let big = "a model ".repeat(10_000);
        let entries = [
            Entry {
                name: "[Content_Types].xml",
                data: b"<Types/>",
            },
            Entry {
                name: "3D/3dmodel.model",
                data: big.as_bytes(),
            },
            Entry {
                name: "empty",
                data: b"",
            },
        ];
        let mut bytes = Vec::new();
        write(&entries, &mut bytes).unwrap();
        let mut again = Vec::new();
        write(&entries, &mut again).unwrap();
        assert_eq!(bytes, again);
        let mut z = ::zip::ZipArchive::new(std::io::Cursor::new(&bytes)).unwrap();
        assert_eq!(z.len(), 3);
        for (k, e) in entries.iter().enumerate() {
            let mut f = z.by_index(k).unwrap();
            assert_eq!(f.name(), e.name);
            assert_eq!(f.compression(), ::zip::CompressionMethod::Deflated);
            assert_eq!(f.extra_data().map_or(0, <[u8]>::len), 0);
            let mut data = Vec::new();
            std::io::Read::read_to_end(&mut f, &mut data).unwrap();
            assert_eq!(data, e.data, "{}", e.name);
            let t = f.last_modified().unwrap();
            assert_eq!(
                (t.year(), t.month(), t.day(), t.hour(), t.minute()),
                (1980, 1, 1, 0, 0)
            );
        }
        // Deflated: the repeated text is far smaller.
        assert!(bytes.len() < 2_000, "{}", bytes.len());
    }

    #[test]
    fn padding_is_a_header_and_zeros() {
        assert!(padding(64, 64).is_empty());
        assert_eq!(padding(60, 64), [0x86, 0x19, 0, 0]);
        // Two bytes short can't hold a header: a whole alignment more.
        assert_eq!(padding(62, 64).len(), 66);
        assert_eq!(padding(62, 64)[2..4], [62, 0]);
        assert_eq!(padding(1, 64).len(), 63);
    }

    #[test]
    fn stored_archives_align_their_data() {
        let big = vec![7u8; 1000];
        let entries = [
            Entry {
                name: "frame.usdc",
                data: &big,
            },
            Entry {
                name: "a/b.txt",
                data: b"odd",
            },
            Entry {
                name: "c",
                data: b"",
            },
        ];
        let mut bytes = Vec::new();
        write_stored(&entries, 64, &mut bytes).unwrap();
        let mut again = Vec::new();
        write_stored(&entries, 64, &mut again).unwrap();
        assert_eq!(bytes, again);
        let mut z = ::zip::ZipArchive::new(std::io::Cursor::new(&bytes)).unwrap();
        for (k, e) in entries.iter().enumerate() {
            let mut f = z.by_index(k).unwrap();
            assert_eq!(f.name(), e.name);
            assert_eq!(f.compression(), ::zip::CompressionMethod::Stored);
            assert_eq!(f.data_start() % 64, 0, "{}", e.name);
            let mut data = Vec::new();
            std::io::Read::read_to_end(&mut f, &mut data).unwrap();
            assert_eq!(data, e.data);
        }
    }
}
