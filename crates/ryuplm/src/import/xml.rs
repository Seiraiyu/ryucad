//! The XML reader (design 935): records found by an element path from the
//! root, each flattened to its attributes and its elements' text, named
//! by their paths relative to the record. Elements are matched by local
//! name, so a namespaced export reads without declaring its namespaces,
//! as XLSX's do; roxmltree refuses DTDs, so no entity expands.

use super::rows::{Leaves, Refusal};
use super::text::{self, Encoding};
use roxmltree::Document;

/// Whether `p` is an element path: `/A/B` when absolute, `A/B` when not,
/// each step a local name (no `@`, `[`, `]`, `:` or space).
pub fn path_ok(p: &str, absolute: bool) -> bool {
    let rest = if absolute {
        match p.strip_prefix('/') {
            Some(r) => r,
            None => return false,
        }
    } else {
        p
    };
    !rest.is_empty()
        && rest
            .split('/')
            .all(|s| !s.is_empty() && !s.contains(['@', '[', ']', ':', ' ']))
}

/// The records `bytes` hold (Task 7 of plan 8.A writes the reader).
pub fn read(_bytes: &[u8], records: &str, _children: Option<&str>) -> Result<Vec<Leaves>, Refusal> {
    Err(Refusal::file(format!("no records at {records}")))
}

/// The byte range of an XML declaration's `encoding="…"` at the start of
/// `s`, and the name it gives.
fn encoding_range(s: &str) -> Option<(usize, usize, &str)> {
    if !s.starts_with("<?xml") {
        return None;
    }
    let end = s.find("?>")?;
    let at = s[..end].find("encoding")?;
    let b = s.as_bytes();
    let mut i = at + "encoding".len();
    while i < end && b[i].is_ascii_whitespace() {
        i += 1;
    }
    if b.get(i) != Some(&b'=') {
        return None;
    }
    i += 1;
    while i < end && b[i].is_ascii_whitespace() {
        i += 1;
    }
    let q = *b.get(i)?;
    if q != b'"' && q != b'\'' {
        return None;
    }
    let v = i + 1;
    let close = v + s[v..end].find(char::from(q))?;
    Some((at, close + 1, &s[v..close]))
}

/// The document's text: in its byte-order mark's encoding, else its
/// declaration's, else UTF-8. The declaration's `encoding` is then blanked
/// with spaces, so the parser judges only the text it is given and no
/// position moves.
pub fn decode(bytes: &[u8]) -> Result<String, Refusal> {
    let enc = if bytes.starts_with(&[0xEF, 0xBB, 0xBF]) {
        Encoding::Utf8
    } else if bytes.starts_with(&[0xFF, 0xFE]) {
        Encoding::Utf16Le
    } else if bytes.starts_with(&[0xFE, 0xFF]) {
        Encoding::Utf16Be
    } else {
        let head: String = bytes.iter().take(256).map(|&b| char::from(b)).collect();
        match encoding_range(&head) {
            Some((_, _, name)) if name.eq_ignore_ascii_case("us-ascii") => Encoding::Utf8,
            Some((_, _, name)) => Encoding::parse(name).ok_or_else(|| {
                Refusal::file(format!(
                    "encoding {name:?} isn't one this reads: utf-8, utf-16le, utf-16be, iso-8859-1 or windows-1252"
                ))
            })?,
            None => Encoding::Utf8,
        }
    };
    let mut s = text::decode(bytes, enc).map_err(|r| {
        if enc == Encoding::Utf8 {
            Refusal::file(format!(
                "line {} isn't UTF-8; a document in another encoding names it in its XML declaration",
                r.row
            ))
        } else {
            r
        }
    })?;
    if let Some((from, to, _)) = encoding_range(&s) {
        s.replace_range(from..to, &" ".repeat(to - from));
    }
    Ok(s)
}

/// The document `text` holds; roxmltree refuses a DTD by default.
fn parse(text: &str) -> Result<Document<'_>, Refusal> {
    Document::parse(text).map_err(|e| Refusal::file(format!("not XML: {e}")))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn text_is_read_in_its_declared_encoding() {
        let mut b = b"<?xml version=\"1.0\" encoding=\"windows-1252\"?><a>".to_vec();
        b.extend_from_slice(&[0x80, 0x96, 0xD7]);
        b.extend_from_slice(b"</a>");
        let s = decode(&b).unwrap();
        assert!(!s.contains("windows-1252"), "{s}");
        assert_eq!(parse(&s).unwrap().root_element().text(), Some("€–×"));
        let utf16: Vec<u8> = [0xFF, 0xFE]
            .into_iter()
            .chain(
                "<?xml version=\"1.0\" encoding=\"UTF-16\"?><a>ø</a>"
                    .encode_utf16()
                    .flat_map(u16::to_le_bytes),
            )
            .collect();
        assert_eq!(
            parse(&decode(&utf16).unwrap())
                .unwrap()
                .root_element()
                .text(),
            Some("ø")
        );
        assert_eq!(
            parse(&decode("<a>ø</a>".as_bytes()).unwrap())
                .unwrap()
                .root_element()
                .text(),
            Some("ø")
        );
        let a = decode(b"<?xml version='1.0' encoding='US-ASCII'?><a>x</a>").unwrap();
        assert_eq!(parse(&a).unwrap().root_element().text(), Some("x"));
        let e = decode(b"<?xml version=\"1.0\" encoding=\"koi8-r\"?><a/>").unwrap_err();
        assert!(e.message.contains("koi8-r"), "{}", e.message);
        assert!(
            decode(b"<a>\xFF</a>")
                .unwrap_err()
                .message
                .contains("declaration")
        );
    }

    #[test]
    fn a_dtd_is_refused() {
        let s = decode(b"<!DOCTYPE a [<!ENTITY x \"y\">]><a>&x;</a>").unwrap();
        assert!(parse(&s).is_err());
    }
}
