//! Zip archives as OPC packages (3MF's) want them: entries in the order
//! given, each deflated by `miniz_oxide` at one fixed level, every time
//! the DOS epoch (1980-01-01 00:00), no extra fields, no comments, no
//! ZIP64. The same entries always give the same bytes.

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

/// Writes `entries` as a zip archive.
pub fn write(entries: &[Entry<'_>], out: &mut dyn Write) -> io::Result<()> {
    let count = u16::try_from(entries.len())
        .map_err(|_| io::Error::new(io::ErrorKind::InvalidInput, "more than 65,535 entries"))?;
    let mut central = Vec::new();
    let mut offset = 0usize;
    for e in entries {
        let packed = miniz_oxide::deflate::compress_to_vec(e.data, LEVEL);
        let crc = crc32(e.data);
        let (size, packed_size) = (u32_of(e.data.len(), e.name)?, u32_of(packed.len(), e.name)?);
        let name = e.name.as_bytes();
        let name_len = u16::try_from(name.len())
            .map_err(|_| io::Error::new(io::ErrorKind::InvalidInput, "an entry name too long"))?;
        // What the local and the central headers share, from the version
        // needed (2.0: deflate) to the extra field's length.
        let mut common = Vec::with_capacity(26);
        for v in [20u16, 0, 8, 0, EPOCH_DATE] {
            common.extend_from_slice(&v.to_le_bytes());
        }
        for v in [crc, packed_size, size] {
            common.extend_from_slice(&v.to_le_bytes());
        }
        common.extend_from_slice(&name_len.to_le_bytes());
        common.extend_from_slice(&0u16.to_le_bytes());
        out.write_all(&0x0403_4b50u32.to_le_bytes())?;
        out.write_all(&common)?;
        out.write_all(name)?;
        out.write_all(&packed)?;
        central.extend_from_slice(&0x0201_4b50u32.to_le_bytes());
        // Made by 2.0 on MS-DOS, so no attributes are a host's.
        central.extend_from_slice(&20u16.to_le_bytes());
        central.extend_from_slice(&common);
        // Comment length, disk, internal and external attributes.
        central.extend_from_slice(&[0; 10]);
        central.extend_from_slice(&u32_of(offset, "an entry's offset")?.to_le_bytes());
        central.extend_from_slice(name);
        offset += 30 + name.len() + packed.len();
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
}
