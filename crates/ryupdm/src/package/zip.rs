//! A package's archive: a zip written the same way every time, and read
//! back with every size bounded, since a package is something a person was
//! sent. Entries are deflated by `miniz_oxide` at level 6, stamped
//! 1980-01-01 00:00, made by Unix with mode 0644, with no extra fields,
//! comments, data descriptors or ZIP64 (design 436).

/// The deflate level: fixed, so the bytes are.
const LEVEL: u8 = 6;
/// The DOS date of 1980-01-01; its time is 00:00.
const EPOCH_DATE: u16 = (1 << 5) | 1;
/// Version needed: 2.0 (deflate). Made by: Unix (3), 2.0.
const NEEDED: u16 = 20;
const MADE_BY: u16 = (3 << 8) | 20;
/// A regular file, `rw-r--r--`, in the external attributes' high half.
const MODE: u32 = 0o100644 << 16;

const LOCAL: u32 = 0x0403_4b50;
const CENTRAL: u32 = 0x0201_4b50;
const END: u32 = 0x0605_4b50;

/// The largest number of entries read.
pub const MAX_ENTRIES: usize = 65_535;

/// The CRC-32 (ISO 3309) of `data`.
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

/// Why an archive can't be written: past what a zip without ZIP64 holds.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TooLarge(pub String);

/// The archive of `entries`, in the order given.
pub fn write(entries: &[(&str, &[u8])]) -> Result<Vec<u8>, TooLarge> {
    let count =
        u16::try_from(entries.len()).map_err(|_| TooLarge("more than 65,535 files".into()))?;
    let mut out = Vec::new();
    let mut central = Vec::new();
    for (name, data) in entries {
        let n =
            u16::try_from(name.len()).map_err(|_| TooLarge(format!("{name}: name too long")))?;
        let packed = miniz_oxide::deflate::compress_to_vec(data, LEVEL);
        let size =
            u32::try_from(data.len()).map_err(|_| TooLarge(format!("{name}: past 4 GiB")))?;
        let psize =
            u32::try_from(packed.len()).map_err(|_| TooLarge(format!("{name}: past 4 GiB")))?;
        let offset =
            u32::try_from(out.len()).map_err(|_| TooLarge("the archive: past 4 GiB".into()))?;
        // Version needed to the extra field's length, as both headers have it.
        let mut common = Vec::with_capacity(26);
        for v in [NEEDED, 0, 8, 0, EPOCH_DATE] {
            common.extend_from_slice(&v.to_le_bytes());
        }
        for v in [crc32(data), psize, size] {
            common.extend_from_slice(&v.to_le_bytes());
        }
        common.extend_from_slice(&n.to_le_bytes());
        common.extend_from_slice(&0u16.to_le_bytes());
        out.extend_from_slice(&LOCAL.to_le_bytes());
        out.extend_from_slice(&common);
        out.extend_from_slice(name.as_bytes());
        out.extend_from_slice(&packed);
        central.extend_from_slice(&CENTRAL.to_le_bytes());
        central.extend_from_slice(&MADE_BY.to_le_bytes());
        central.extend_from_slice(&common);
        // Comment length, disk, internal attributes.
        central.extend_from_slice(&[0; 6]);
        central.extend_from_slice(&MODE.to_le_bytes());
        central.extend_from_slice(&offset.to_le_bytes());
        central.extend_from_slice(name.as_bytes());
    }
    let at = u32::try_from(out.len()).map_err(|_| TooLarge("the archive: past 4 GiB".into()))?;
    let csize =
        u32::try_from(central.len()).map_err(|_| TooLarge("the archive: past 4 GiB".into()))?;
    out.extend_from_slice(&central);
    out.extend_from_slice(&END.to_le_bytes());
    out.extend_from_slice(&[0; 4]);
    out.extend_from_slice(&count.to_le_bytes());
    out.extend_from_slice(&count.to_le_bytes());
    out.extend_from_slice(&csize.to_le_bytes());
    out.extend_from_slice(&at.to_le_bytes());
    out.extend_from_slice(&0u16.to_le_bytes());
    Ok(out)
}

/// Why an archive doesn't read.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Unread {
    /// Not a zip as packages are written, with why.
    Malformed(String),
    /// An entry larger than allowed, by name.
    TooLarge(String),
    /// A name twice.
    Duplicate(String),
    /// An entry whose CRC-32 isn't its data's.
    Crc(String),
}

impl Unread {
    pub fn text(&self) -> String {
        match self {
            Unread::Malformed(why) => format!("isn't a package's zip: {why}"),
            Unread::TooLarge(n) => format!("holds {n}, larger than a package's files may be"),
            Unread::Duplicate(n) => format!("holds {n} twice"),
            Unread::Crc(n) => format!("holds {n}, whose checksum doesn't match its data"),
        }
    }
}

fn u16_at(b: &[u8], at: usize) -> Result<u16, Unread> {
    b.get(at..at + 2)
        .map(|s| u16::from_le_bytes([s[0], s[1]]))
        .ok_or_else(|| Unread::Malformed("it ends early".into()))
}

fn u32_at(b: &[u8], at: usize) -> Result<u32, Unread> {
    b.get(at..at + 4)
        .map(|s| u32::from_le_bytes([s[0], s[1], s[2], s[3]]))
        .ok_or_else(|| Unread::Malformed("it ends early".into()))
}

/// Every entry of `bytes`, by name in the central directory's order, each
/// unpacked to at most `max` bytes (`max_named` for the names it lists).
/// Stored and deflated entries are read; anything else is malformed.
pub fn read(
    bytes: &[u8],
    max: u64,
    max_named: &[(&str, u64)],
) -> Result<Vec<(String, Vec<u8>)>, Unread> {
    // The end record has no comment, as packages are written; a zip with
    // one is searched for its last 64 KiB.
    let floor = bytes.len().saturating_sub(22 + 65_535);
    let end = (floor..=bytes.len().saturating_sub(22))
        .rev()
        .find(|&i| u32_at(bytes, i).ok() == Some(END))
        .ok_or_else(|| Unread::Malformed("no end of central directory".into()))?;
    let count = usize::from(u16_at(bytes, end + 10)?);
    let csize = u32_at(bytes, end + 12)? as usize;
    let cat = u32_at(bytes, end + 16)? as usize;
    if cat.checked_add(csize).is_none_or(|e| e > end) {
        return Err(Unread::Malformed(
            "its central directory lies outside it".into(),
        ));
    }
    let mut out: Vec<(String, Vec<u8>)> = Vec::with_capacity(count.min(MAX_ENTRIES));
    let mut seen = std::collections::BTreeSet::new();
    let mut at = cat;
    for _ in 0..count {
        if u32_at(bytes, at)? != CENTRAL {
            return Err(Unread::Malformed("a central header is missing".into()));
        }
        let flags = u16_at(bytes, at + 8)?;
        let method = u16_at(bytes, at + 10)?;
        let crc = u32_at(bytes, at + 16)?;
        let psize = u32_at(bytes, at + 20)? as usize;
        let size = u64::from(u32_at(bytes, at + 24)?);
        let n = usize::from(u16_at(bytes, at + 28)?);
        let extra = usize::from(u16_at(bytes, at + 30)?);
        let comment = usize::from(u16_at(bytes, at + 32)?);
        let local = u32_at(bytes, at + 42)? as usize;
        let name = bytes
            .get(at + 46..at + 46 + n)
            .and_then(|s| std::str::from_utf8(s).ok())
            .ok_or_else(|| Unread::Malformed("a name isn't UTF-8".into()))?
            .to_owned();
        at += 46 + n + extra + comment;
        if flags & 1 != 0 {
            return Err(Unread::Malformed(format!("{name} is encrypted")));
        }
        let limit = max_named
            .iter()
            .find(|(m, _)| *m == name)
            .map_or(max, |(_, l)| *l);
        if size > limit {
            return Err(Unread::TooLarge(name));
        }
        if !seen.insert(name.clone()) {
            return Err(Unread::Duplicate(name));
        }
        if u32_at(bytes, local)? != LOCAL {
            return Err(Unread::Malformed(format!(
                "{name}'s local header is missing"
            )));
        }
        let ln = usize::from(u16_at(bytes, local + 26)?);
        let le = usize::from(u16_at(bytes, local + 28)?);
        let start = local + 30 + ln + le;
        let packed = bytes
            .get(start..start.saturating_add(psize))
            .ok_or_else(|| Unread::Malformed(format!("{name}'s data runs past the end")))?;
        let data = match method {
            0 => packed.to_vec(),
            8 => miniz_oxide::inflate::decompress_to_vec_with_limit(packed, size as usize)
                .map_err(|_| Unread::Malformed(format!("{name} doesn't inflate")))?,
            m => return Err(Unread::Malformed(format!("{name} is packed by method {m}"))),
        };
        if data.len() as u64 != size {
            return Err(Unread::Malformed(format!(
                "{name} isn't the size its header says"
            )));
        }
        if crc32(&data) != crc {
            return Err(Unread::Crc(name));
        }
        out.push((name, data));
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample() -> Vec<u8> {
        write(&[("package.json", b"{}\n"), ("a/b.txt", b"hello hello hello")]).unwrap()
    }

    #[test]
    fn the_same_files_give_the_same_bytes_and_read_back() {
        let z = sample();
        assert_eq!(z, sample());
        assert_eq!(
            blake3::hash(&z).to_hex().as_str(),
            "7f4a74362083fe887e96e150d7e75170bff01cdca79250666482c603cecbf759"
        );
        let back = read(&z, 1 << 20, &[]).unwrap();
        assert_eq!(
            back,
            vec![
                ("package.json".to_owned(), b"{}\n".to_vec()),
                ("a/b.txt".to_owned(), b"hello hello hello".to_vec())
            ]
        );
    }

    #[test]
    fn sizes_names_and_checksums_are_judged() {
        let z = sample();
        assert_eq!(read(&z, 4, &[]), Err(Unread::TooLarge("a/b.txt".into())));
        assert_eq!(
            read(&z, 1 << 20, &[("package.json", 2)]),
            Err(Unread::TooLarge("package.json".into()))
        );
        let mut bad = z.clone();
        // The first entry's stored CRC, in its central header.
        let c = bad
            .windows(4)
            .rposition(|w| w == CENTRAL.to_le_bytes())
            .unwrap();
        bad[c + 16] ^= 1;
        assert!(matches!(read(&bad, 1 << 20, &[]), Err(Unread::Crc(_))));
        assert!(matches!(
            read(b"not a zip", 1 << 20, &[]),
            Err(Unread::Malformed(_))
        ));
        for cut in 0..z.len() {
            let _ = read(&z[..cut], 1 << 20, &[]);
        }
    }
}
