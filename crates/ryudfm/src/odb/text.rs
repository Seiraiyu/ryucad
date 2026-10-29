//! ODB++'s two kinds of text file: line records (`L 0 0 1 1 0 P 0;1=2`)
//! and structured text (`KEY=VALUE` lines and `NAME { … }` arrays).
//!
//! Files are ASCII by the specification; a file that isn't UTF-8 is read
//! as Latin-1, byte for character, so a stray accented byte in a part name
//! never fails a layer. `#` starts a comment line. Lines end in LF or CRLF.

use std::borrow::Cow;

/// A file's text.
pub fn decode(bytes: &[u8]) -> Cow<'_, str> {
    match std::str::from_utf8(bytes) {
        Ok(s) => Cow::Borrowed(s),
        Err(_) => Cow::Owned(bytes.iter().map(|&b| char::from(b)).collect()),
    }
}

/// What is wrong at a line and column (both from 1) of a file.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Bad {
    pub line: u32,
    pub column: u32,
    pub message: String,
}

impl Bad {
    pub fn new(line: u32, column: u32, message: impl Into<String>) -> Bad {
        Bad {
            line,
            column,
            message: message.into(),
        }
    }
}

/// The lines that hold something, numbered from 1, trimmed: no blank
/// lines and no comments.
pub fn lines(text: &str) -> impl Iterator<Item = (u32, &str)> {
    text.split('\n').enumerate().filter_map(|(i, l)| {
        let l = l.trim();
        (!l.is_empty() && !l.starts_with('#')).then_some((i as u32 + 1, l))
    })
}

/// A record line split at its first `;` outside quotes: its fields, and
/// what follows (attributes, then `ID=`), if anything does.
pub fn split_record(line: &str) -> (&str, Option<&str>) {
    let mut quoted = false;
    for (i, c) in line.char_indices() {
        match c {
            '\'' => quoted = !quoted,
            ';' if !quoted => return (&line[..i], Some(&line[i + 1..])),
            _ => {}
        }
    }
    (line, None)
}

/// A field of a record line and its column (from 1).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Token<'a> {
    pub text: &'a str,
    pub column: u32,
    /// Written in single quotes (which `text` leaves out).
    pub quoted: bool,
}

/// The fields of a record line, separated by white space; a field that
/// starts with `'` runs to the next `'` and may hold spaces.
pub fn tokens(line: &str) -> Result<Vec<Token<'_>>, Bad> {
    let mut out = Vec::new();
    let b = line.as_bytes();
    let mut i = 0;
    while i < b.len() {
        if b[i].is_ascii_whitespace() {
            i += 1;
            continue;
        }
        let column = i as u32 + 1;
        if b[i] == b'\'' {
            let Some(end) = line[i + 1..].find('\'') else {
                return Err(Bad::new(0, column, "a quote that doesn't close"));
            };
            out.push(Token {
                text: &line[i + 1..i + 1 + end],
                column,
                quoted: true,
            });
            i += end + 2;
        } else {
            let start = i;
            while i < b.len() && !b[i].is_ascii_whitespace() {
                i += 1;
            }
            out.push(Token {
                text: &line[start..i],
                column,
                quoted: false,
            });
        }
    }
    Ok(out)
}

/// One `KEY=VALUE` line.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Field {
    /// As written.
    pub key: String,
    /// Trimmed, quotes and all.
    pub value: String,
    pub line: u32,
}

/// A `NAME { … }` array.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Array {
    pub name: String,
    pub line: u32,
    pub fields: Vec<Field>,
}

fn find<'a>(fields: &'a [Field], key: &str) -> Option<&'a Field> {
    fields.iter().find(|f| f.key.eq_ignore_ascii_case(key))
}

impl Array {
    /// The first field named `key`, in any case.
    pub fn field(&self, key: &str) -> Option<&Field> {
        find(&self.fields, key)
    }

    pub fn get(&self, key: &str) -> Option<&str> {
        self.field(key).map(|f| f.value.as_str())
    }
}

/// A structured text file: its fields outside arrays and its arrays, in
/// order.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Structured {
    pub fields: Vec<Field>,
    pub arrays: Vec<Array>,
}

impl Structured {
    pub fn field(&self, key: &str) -> Option<&Field> {
        find(&self.fields, key)
    }

    pub fn get(&self, key: &str) -> Option<&str> {
        self.field(key).map(|f| f.value.as_str())
    }

    /// The arrays named `name`, in any case.
    pub fn arrays<'a>(&'a self, name: &'a str) -> impl Iterator<Item = &'a Array> {
        self.arrays
            .iter()
            .filter(move |a| a.name.eq_ignore_ascii_case(name))
    }
}

/// Reads structured text. An array's name may stand alone with its `{` on
/// the next line; arrays don't nest.
pub fn structured(text: &str) -> Result<Structured, Bad> {
    let mut out = Structured::default();
    let mut open: Option<Array> = None;
    // A name waiting for its `{` on the next line.
    let mut pending: Option<(String, u32)> = None;
    for (n, line) in lines(text) {
        if let Some((name, at)) = pending.take() {
            if line == "{" {
                open = Some(Array {
                    name,
                    line: at,
                    fields: Vec::new(),
                });
                continue;
            }
            return Err(Bad::new(at, 1, format!("{name} without = or {{")));
        }
        if line == "}" {
            match open.take() {
                Some(a) => out.arrays.push(a),
                None => return Err(Bad::new(n, 1, "a } that closes nothing")),
            }
            continue;
        }
        if let Some((key, value)) = line.split_once('=') {
            let field = Field {
                key: key.trim().to_string(),
                value: value.trim().to_string(),
                line: n,
            };
            match &mut open {
                Some(a) => a.fields.push(field),
                None => out.fields.push(field),
            }
            continue;
        }
        if open.is_some() {
            return Err(Bad::new(n, 1, "an array inside an array"));
        }
        match line.strip_suffix('{') {
            Some(name) => {
                open = Some(Array {
                    name: name.trim().to_string(),
                    line: n,
                    fields: Vec::new(),
                })
            }
            None => pending = Some((line.to_string(), n)),
        }
    }
    if let Some(a) = open {
        return Err(Bad::new(a.line, 1, format!("{} {{ doesn't close", a.name)));
    }
    if let Some((name, at)) = pending {
        return Err(Bad::new(at, 1, format!("{name} without = or {{")));
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn text_is_utf8_or_latin1() {
        assert_eq!(decode(b"R1 \xc2\xb5F"), "R1 µF");
        assert_eq!(decode(b"R1 \xb5F"), "R1 µF");
    }

    #[test]
    fn lines_skip_comments_and_blanks() {
        let t = "UNITS=MM\r\n#\n#Feature symbol names\n\n$0 r10\r\n  P 0 0 0 P 0 0  \n";
        let got: Vec<_> = lines(t).collect();
        assert_eq!(got, [(1, "UNITS=MM"), (5, "$0 r10"), (6, "P 0 0 0 P 0 0")]);
    }

    #[test]
    fn records_split_at_their_attributes() {
        assert_eq!(
            split_record("P 1 2 0 P 0 0;0,1=2;ID=7"),
            ("P 1 2 0 P 0 0", Some("0,1=2;ID=7"))
        );
        assert_eq!(
            split_record("T 0 0 standard P 0 1 1 1 'a;b' 0;0"),
            ("T 0 0 standard P 0 1 1 1 'a;b' 0", Some("0"))
        );
        assert_eq!(split_record("L 0 0 1 1 0 P 0"), ("L 0 0 1 1 0 P 0", None));
        let t = tokens("PRP VALUE '10 uF'  1.5").unwrap();
        let texts: Vec<_> = t.iter().map(|t| (t.text, t.column, t.quoted)).collect();
        assert_eq!(
            texts,
            [
                ("PRP", 1, false),
                ("VALUE", 5, false),
                ("10 uF", 11, true),
                ("1.5", 20, false)
            ]
        );
        assert_eq!(tokens("PRP X ''").unwrap()[2].text, "");
        assert_eq!(tokens("PRP X 'open").unwrap_err().column, 7);
    }

    #[test]
    fn structured_text_and_arrays() {
        let t = "UNITS=INCH\nX_DATUM = 0.5\n\nSTEP-REPEAT {\n  NAME=pcb\n  X=1\n}\nTOOLS\n{\nNUM=1\n}\nOLD_NAME=\nOLD_NAME=x\n";
        let s = structured(t).unwrap();
        assert_eq!(s.get("units"), Some("INCH"));
        assert_eq!(s.get("X_DATUM"), Some("0.5"));
        assert_eq!(s.get("OLD_NAME"), Some(""));
        let a: Vec<_> = s.arrays("step-repeat").collect();
        assert_eq!((a.len(), a[0].line, a[0].get("name")), (1, 4, Some("pcb")));
        assert_eq!(
            s.arrays("TOOLS").next().unwrap().field("NUM").unwrap().line,
            10
        );
    }

    #[test]
    fn structured_text_faults() {
        assert_eq!(structured("A {\nX=1\n").unwrap_err().line, 1);
        assert_eq!(structured("X=1\n}\n").unwrap_err().line, 2);
        assert_eq!(structured("A {\nB {\n}\n").unwrap_err().line, 2);
        assert_eq!(structured("JUNK\nX=1\n").unwrap_err().line, 1);
        assert_eq!(structured("X=1\nJUNK").unwrap_err().line, 2);
    }
}
