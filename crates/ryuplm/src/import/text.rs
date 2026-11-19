//! A text file's bytes as text, in the encodings item masters come in
//! (design 734: "CSV, in any delimiter and encoding the mapping names").
//! Written here rather than taken from a crate: four encodings, a table of
//! 32 characters.

use super::rows::Refusal;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Encoding {
    Utf8,
    Utf16Le,
    Utf16Be,
    Latin1,
    Windows1252,
}

impl Encoding {
    pub fn parse(s: &str) -> Option<Encoding> {
        Some(match s.to_ascii_lowercase().as_str() {
            "utf-8" | "utf8" => Encoding::Utf8,
            "utf-16le" => Encoding::Utf16Le,
            "utf-16be" => Encoding::Utf16Be,
            "latin1" | "iso-8859-1" => Encoding::Latin1,
            "windows-1252" | "cp1252" => Encoding::Windows1252,
            _ => return None,
        })
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Encoding::Utf8 => "utf-8",
            Encoding::Utf16Le => "utf-16le",
            Encoding::Utf16Be => "utf-16be",
            Encoding::Latin1 => "latin1",
            Encoding::Windows1252 => "windows-1252",
        }
    }
}

/// Windows-1252's 0x80 to 0x9F; the five it leaves undefined map to the
/// C1 controls, as browsers do.
const CP1252: [char; 32] = [
    '\u{20AC}', '\u{81}', '\u{201A}', '\u{0192}', '\u{201E}', '\u{2026}', '\u{2020}', '\u{2021}',
    '\u{02C6}', '\u{2030}', '\u{0160}', '\u{2039}', '\u{0152}', '\u{8D}', '\u{017D}', '\u{8F}',
    '\u{90}', '\u{2018}', '\u{2019}', '\u{201C}', '\u{201D}', '\u{2022}', '\u{2013}', '\u{2014}',
    '\u{02DC}', '\u{2122}', '\u{0161}', '\u{203A}', '\u{0153}', '\u{9D}', '\u{017E}', '\u{0178}',
];

/// The encoding `bytes` are most likely in: a BOM's, else UTF-8 when they
/// are valid UTF-8, else Windows-1252.
pub fn sniff(bytes: &[u8]) -> Encoding {
    if bytes.starts_with(&[0xEF, 0xBB, 0xBF]) {
        Encoding::Utf8
    } else if bytes.starts_with(&[0xFF, 0xFE]) {
        Encoding::Utf16Le
    } else if bytes.starts_with(&[0xFE, 0xFF]) {
        Encoding::Utf16Be
    } else if std::str::from_utf8(bytes).is_ok() {
        Encoding::Utf8
    } else {
        Encoding::Windows1252
    }
}

/// The line (1-based) byte `at` of `bytes` is on.
fn line_of(bytes: &[u8], at: usize) -> u64 {
    1 + bytes[..at.min(bytes.len())]
        .iter()
        .filter(|b| **b == b'\n')
        .count() as u64
}

pub fn decode(bytes: &[u8], enc: Encoding) -> Result<String, Refusal> {
    match enc {
        Encoding::Utf8 => {
            let b = bytes.strip_prefix(&[0xEF, 0xBB, 0xBF]).unwrap_or(bytes);
            std::str::from_utf8(b).map(str::to_owned).map_err(|e| {
                Refusal::at(line_of(b, e.valid_up_to()), None, "this line isn't UTF-8; name the file's encoding in the mapping (reader.encoding)")
            })
        }
        Encoding::Latin1 => Ok(bytes.iter().map(|&b| char::from(b)).collect()),
        Encoding::Windows1252 => Ok(bytes
            .iter()
            .map(|&b| {
                if (0x80..0xA0).contains(&b) {
                    CP1252[usize::from(b - 0x80)]
                } else {
                    char::from(b)
                }
            })
            .collect()),
        Encoding::Utf16Le | Encoding::Utf16Be => {
            let bom: [u8; 2] = if enc == Encoding::Utf16Le {
                [0xFF, 0xFE]
            } else {
                [0xFE, 0xFF]
            };
            let b = bytes.strip_prefix(&bom).ok_or_else(|| {
                Refusal::file(format!(
                    "{} text starts with its byte order mark",
                    enc.as_str()
                ))
            })?;
            if b.len() % 2 == 1 {
                return Err(Refusal::file("UTF-16 text has an odd number of bytes"));
            }
            let units = b.as_chunks::<2>().0.iter().map(|p| {
                if enc == Encoding::Utf16Le {
                    u16::from_le_bytes([p[0], p[1]])
                } else {
                    u16::from_be_bytes([p[0], p[1]])
                }
            });
            let mut out = String::with_capacity(b.len() / 2);
            let mut line = 1u64;
            for c in char::decode_utf16(units) {
                let c = c.map_err(|_| {
                    Refusal::at(line, None, "this line has a lone UTF-16 surrogate")
                })?;
                if c == '\n' {
                    line += 1;
                }
                out.push(c);
            }
            Ok(out)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn each_encoding_decodes() {
        assert_eq!(
            decode(b"\xEF\xBB\xBFa,\xC3\xB8", Encoding::Utf8).unwrap(),
            "a,ø"
        );
        assert_eq!(decode(b"Gr\xF6\xDFe", Encoding::Latin1).unwrap(), "Größe");
        assert_eq!(
            decode(b"\x80 5\x96", Encoding::Windows1252).unwrap(),
            "€ 5–"
        );
        assert_eq!(decode(b"\x80", Encoding::Latin1).unwrap(), "\u{80}");
        assert_eq!(
            decode(b"\xFF\xFEa\x00\xF8\x00", Encoding::Utf16Le).unwrap(),
            "aø"
        );
        assert_eq!(decode(b"\xFE\xFF\x00a", Encoding::Utf16Be).unwrap(), "a");
    }

    #[test]
    fn bad_bytes_name_their_line() {
        let e = decode(b"a\nb\n\xF6", Encoding::Utf8).unwrap_err();
        assert_eq!(e.row, 3);
        assert!(e.message.contains("reader.encoding"));
        assert_eq!(decode(b"a\x00", Encoding::Utf16Le).unwrap_err().row, 0);
    }

    #[test]
    fn sniffing_prefers_a_bom_then_utf8() {
        assert_eq!(sniff(b"\xFF\xFEa\x00"), Encoding::Utf16Le);
        assert_eq!(sniff("ø".as_bytes()), Encoding::Utf8);
        assert_eq!(sniff(b"Gr\xF6\xDFe"), Encoding::Windows1252);
        assert_eq!(Encoding::parse("ISO-8859-1"), Some(Encoding::Latin1));
        assert_eq!(Encoding::parse("koi8-r"), None);
    }
}
