//! String contents and their escapes, both ways.
//!
//! Part 21 strings are printable ASCII; anything else is written with
//! directives: `\\` for a backslash, `\X\hh` for an ISO 8859-1 character,
//! `\X2\…\X0\` and `\X4\…\X0\` for UCS-2 and UCS-4 runs, and `\S\c` for
//! a character of the page `\P?\` chose. Reading is lenient where real
//! files are: a backslash that starts no directive is itself, bytes above
//! 127 are UTF-8 if they are and ISO 8859-1 otherwise, and pages other than
//! A (ISO 8859-1) give U+FFFD rather than failing the file.

/// The text a string's raw contents stand for, once the lexer has undone
/// doubled quotes and dropped line breaks.
pub(crate) fn decode(raw: &[u8]) -> String {
    let mut out = String::with_capacity(raw.len());
    let mut page_a = true;
    let mut i = 0;
    while i < raw.len() {
        if raw[i] != b'\\' {
            let start = i;
            while i < raw.len() && raw[i] != b'\\' {
                i += 1;
            }
            push_raw(&mut out, &raw[start..i]);
            continue;
        }
        let rest = &raw[i..];
        if rest.starts_with(b"\\\\") {
            out.push('\\');
            i += 2;
        } else if rest.len() >= 4 && rest.starts_with(b"\\S\\") {
            let c = rest[3];
            out.push(if page_a && (0x20..0x7f).contains(&c) {
                char::from(c + 0x80)
            } else {
                char::REPLACEMENT_CHARACTER
            });
            i += 4;
        } else if rest.len() >= 4
            && rest.starts_with(b"\\P")
            && (b'A'..=b'I').contains(&rest[2])
            && rest[3] == b'\\'
        {
            page_a = rest[2] == b'A';
            i += 4;
        } else if let Some(n) = run(rest, b"\\X2\\", 4, &mut out) {
            i += n;
        } else if let Some(n) = run(rest, b"\\X4\\", 8, &mut out) {
            i += n;
        } else if rest.len() >= 5
            && rest.starts_with(b"\\X\\")
            && let Some(c) = hex(&rest[3..5])
        {
            out.push(char::from(c as u8));
            i += 5;
        } else {
            out.push('\\');
            i += 1;
        }
    }
    out
}

/// A `\X2\` or `\X4\` run of `width` hex digits a character, ended by
/// `\X0\`: the bytes it took, or `None` if `rest` isn't one.
fn run(rest: &[u8], open: &[u8], width: usize, out: &mut String) -> Option<usize> {
    let body = rest.strip_prefix(open)?;
    let end = body.windows(4).position(|w| w == b"\\X0\\")?;
    let digits = &body[..end];
    if digits.is_empty() || digits.len() % width != 0 {
        return None;
    }
    let codes: Option<Vec<u32>> = digits.chunks(width).map(hex).collect();
    let codes = codes?;
    if width == 4 {
        let units: Vec<u16> = codes.iter().map(|&c| c as u16).collect();
        out.push_str(&String::from_utf16_lossy(&units));
    } else {
        out.extend(
            codes
                .iter()
                .map(|&c| char::from_u32(c).unwrap_or(char::REPLACEMENT_CHARACTER)),
        );
    }
    Some(open.len() + end + 4)
}

fn hex(digits: &[u8]) -> Option<u32> {
    let mut x = 0u32;
    for &d in digits {
        x = x * 16 + char::from(d).to_digit(16)?;
    }
    Some(x)
}

/// Bytes with no directive in them: UTF-8 if they are, else each byte as
/// ISO 8859-1, which is what a byte above 127 meant before UTF-8.
fn push_raw(out: &mut String, raw: &[u8]) {
    match std::str::from_utf8(raw) {
        Ok(s) => out.push_str(s),
        Err(_) => out.extend(raw.iter().map(|&b| char::from(b))),
    }
}

/// `text` as a string's contents, without the quotes: printable ASCII as
/// it is (quotes doubled, backslashes escaped), every other run as one
/// `\X2\` run, or `\X4\` if it holds a character beyond U+FFFF.
pub(crate) fn encode(text: &str) -> String {
    let printable = |c: char| (' '..='~').contains(&c);
    let mut out = String::with_capacity(text.len() + 2);
    let mut chars = text.chars().peekable();
    while let Some(&c) = chars.peek() {
        if printable(c) {
            match c {
                '\'' => out.push_str("''"),
                '\\' => out.push_str("\\\\"),
                _ => out.push(c),
            }
            chars.next();
            continue;
        }
        let mut other = Vec::new();
        while let Some(&c) = chars.peek() {
            if printable(c) {
                break;
            }
            other.push(c);
            chars.next();
        }
        if other.iter().all(|&c| (c as u32) <= 0xffff) {
            out.push_str("\\X2\\");
            for c in other {
                out.push_str(&format!("{:04X}", c as u32));
            }
        } else {
            out.push_str("\\X4\\");
            for c in other {
                out.push_str(&format!("{:08X}", c as u32));
            }
        }
        out.push_str("\\X0\\");
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use proptest::prelude::*;

    fn d(raw: &str) -> String {
        decode(raw.as_bytes())
    }

    #[test]
    fn directives() {
        assert_eq!(d("plain"), "plain");
        assert_eq!(d("a\\\\b"), "a\\b");
        assert_eq!(d("\\X\\E9t\\X\\E9"), "\u{e9}t\u{e9}");
        assert_eq!(d("\\X2\\00E900FC\\X0\\!"), "\u{e9}\u{fc}!");
        assert_eq!(d("\\X2\\D83DDE00\\X0\\"), "\u{1f600}");
        assert_eq!(d("\\X4\\0001F600\\X0\\"), "\u{1f600}");
        assert_eq!(d("\\S\\i"), "\u{e9}");
        assert_eq!(d("\\PA\\\\S\\i"), "\u{e9}");
        assert_eq!(d("\\PB\\\\S\\i\\PA\\\\S\\i"), "\u{fffd}\u{e9}");
        assert_eq!(d("\\X4\\00110000\\X0\\"), "\u{fffd}");
    }

    #[test]
    fn lenient() {
        // A backslash that starts no directive is itself.
        assert_eq!(d("C:\\temp\\x"), "C:\\temp\\x");
        assert_eq!(d("\\X2\\00E\\X0\\"), "\\X2\\00E\\X0\\");
        assert_eq!(d("\\X2\\00E9"), "\\X2\\00E9");
        assert_eq!(decode(b"caf\xc3\xa9"), "caf\u{e9}");
        assert_eq!(decode(b"caf\xe9"), "caf\u{e9}");
    }

    #[test]
    fn encoding() {
        assert_eq!(encode("it's a\\b"), "it''s a\\\\b");
        assert_eq!(encode("caf\u{e9}s"), "caf\\X2\\00E9\\X0\\s");
        assert_eq!(encode("a\nb"), "a\\X2\\000A\\X0\\b");
        assert_eq!(encode("\u{1f600}\u{e9}"), "\\X4\\0001F600000000E9\\X0\\");
    }

    proptest! {
        #[test]
        fn encoded_text_decodes_to_itself(text in any::<String>()) {
            let quoted = format!("('{}')", encode(&text));
            let mut c = super::super::lex::Cursor::new(quoted.as_bytes(), 0, quoted.len());
            let p = c.params().unwrap();
            prop_assert_eq!(p, vec![super::super::Param::Str(text.into())]);
        }
    }
}
