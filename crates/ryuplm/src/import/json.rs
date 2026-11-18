//! The JSON reader (design 935): records found by a JSON Pointer, each
//! flattened to its leaf values named by their pointers relative to the
//! record, numbers kept exactly as written. Written here rather than read
//! through serde_json: serde_json keeps no number's digits (`1.10` becomes
//! `1.1`), and its `arbitrary_precision` feature would change every
//! crate's numbers through feature unification, ryu-doc's canonical JSON
//! among them. It reads as it goes and builds no tree.

use super::rows::{Leaves, Refusal};
use std::collections::BTreeSet;

/// The deepest nesting read: deeper is refused, which bounds the
/// recursion too.
pub const MAX_DEPTH: usize = 128;

struct Parser<'a> {
    s: &'a [u8],
    text: &'a str,
    i: usize,
    /// The line `i` is on, and the byte its line starts at.
    line: u64,
    line_start: usize,
    depth: usize,
    /// The children's pointer relative to a record, normalized.
    children: Option<String>,
    /// Put before a record's pointer in its place: `line N ` in JSON Lines.
    base: String,
}

impl<'a> Parser<'a> {
    fn new(text: &'a str, line: u64, children: Option<&str>) -> Parser<'a> {
        Parser {
            s: text.as_bytes(),
            text,
            i: 0,
            line,
            line_start: 0,
            depth: 0,
            children: children
                .and_then(tokens)
                .map(|t| t.iter().map(|x| format!("/{}", escape(x))).collect()),
            base: String::new(),
        }
    }

    fn err(&self, msg: impl std::fmt::Display) -> Refusal {
        let end = self.i.min(self.text.len());
        let col = self.text[self.line_start..end].chars().count() + 1;
        Refusal::file(format!("line {}, column {col}: {msg}", self.line))
    }

    fn peek(&self) -> Option<u8> {
        self.s.get(self.i).copied()
    }

    fn ws(&mut self) {
        while let Some(b) = self.peek() {
            match b {
                b' ' | b'\t' | b'\r' => self.i += 1,
                b'\n' => {
                    self.i += 1;
                    self.line += 1;
                    self.line_start = self.i;
                }
                _ => break,
            }
        }
    }

    /// Only whitespace is left.
    fn end(&mut self) -> Result<(), Refusal> {
        self.ws();
        if self.i < self.s.len() {
            Err(self.err("text after the JSON value"))
        } else {
            Ok(())
        }
    }

    fn literal(&mut self, word: &str) -> Result<(), Refusal> {
        if self.s[self.i..].starts_with(word.as_bytes()) {
            self.i += word.len();
            Ok(())
        } else {
            Err(self.err("not a value"))
        }
    }

    fn hex4(&mut self) -> Result<u16, Refusal> {
        let h = self
            .s
            .get(self.i..self.i + 4)
            .filter(|h| h.iter().all(u8::is_ascii_hexdigit))
            .and_then(|h| std::str::from_utf8(h).ok())
            .and_then(|h| u16::from_str_radix(h, 16).ok())
            .ok_or_else(|| self.err("\\u takes four hex digits"))?;
        self.i += 4;
        Ok(h)
    }

    /// A string, at its opening quote: its text, unescaped.
    fn string(&mut self) -> Result<String, Refusal> {
        self.i += 1;
        let mut out = String::new();
        loop {
            let start = self.i;
            while self
                .peek()
                .is_some_and(|b| b != b'"' && b != b'\\' && b >= 0x20)
            {
                self.i += 1;
            }
            out.push_str(&self.text[start..self.i]);
            match self.peek() {
                None => return Err(self.err("a string isn't closed")),
                Some(b'"') => {
                    self.i += 1;
                    return Ok(out);
                }
                Some(b'\\') => {
                    self.i += 1;
                    let c = match self.peek() {
                        Some(b'"') => '"',
                        Some(b'\\') => '\\',
                        Some(b'/') => '/',
                        Some(b'b') => '\u{8}',
                        Some(b'f') => '\u{c}',
                        Some(b'n') => '\n',
                        Some(b'r') => '\r',
                        Some(b't') => '\t',
                        Some(b'u') => {
                            self.i += 1;
                            let u = self.hex4()?;
                            let c = if (0xD800..0xDC00).contains(&u) {
                                if self.s.get(self.i..self.i + 2) == Some(b"\\u") {
                                    self.i += 2;
                                    let l = self.hex4()?;
                                    if !(0xDC00..0xE000).contains(&l) {
                                        return Err(self.err("a lone UTF-16 surrogate"));
                                    }
                                    char::from_u32(
                                        0x10000
                                            + ((u32::from(u) - 0xD800) << 10)
                                            + (u32::from(l) - 0xDC00),
                                    )
                                } else {
                                    None
                                }
                            } else {
                                char::from_u32(u32::from(u))
                            };
                            out.push(c.ok_or_else(|| self.err("a lone UTF-16 surrogate"))?);
                            continue;
                        }
                        _ => return Err(self.err("not an escape")),
                    };
                    self.i += 1;
                    out.push(c);
                }
                Some(_) => return Err(self.err("a control character in a string")),
            }
        }
    }

    /// A number, exactly as written.
    fn number(&mut self) -> Result<String, Refusal> {
        let s = self.s;
        let digits = |i: &mut usize| {
            let from = *i;
            while s.get(*i).is_some_and(u8::is_ascii_digit) {
                *i += 1;
            }
            *i > from
        };
        let start = self.i;
        let mut i = self.i;
        if s.get(i) == Some(&b'-') {
            i += 1;
        }
        match s.get(i).copied() {
            Some(b'0') => i += 1,
            Some(b'1'..=b'9') => {
                digits(&mut i);
            }
            _ => return Err(self.err("not a value")),
        }
        if s.get(i) == Some(&b'.') {
            i += 1;
            if !digits(&mut i) {
                self.i = i;
                return Err(self.err("a fraction needs digits"));
            }
        }
        if matches!(s.get(i).copied(), Some(b'e' | b'E')) {
            i += 1;
            if matches!(s.get(i).copied(), Some(b'+' | b'-')) {
                i += 1;
            }
            if !digits(&mut i) {
                self.i = i;
                return Err(self.err("an exponent needs digits"));
            }
        }
        self.i = i;
        Ok(self.text[start..i].to_owned())
    }

    /// The value here as text when it is a scalar (a string's text, a
    /// number's digits, `true`, `false`, and `null` as empty), or `None`,
    /// reading nothing, when it is an object or an array.
    fn scalar(&mut self) -> Result<Option<String>, Refusal> {
        Ok(Some(match self.peek() {
            Some(b'"') => self.string()?,
            Some(b't') => {
                self.literal("true")?;
                "true".to_owned()
            }
            Some(b'f') => {
                self.literal("false")?;
                "false".to_owned()
            }
            Some(b'n') => {
                self.literal("null")?;
                String::new()
            }
            Some(b'{' | b'[') => return Ok(None),
            None => return Err(self.err("the text ends where a value should be")),
            _ => self.number()?,
        }))
    }

    /// Past an object's or an array's opening bracket, one level deeper.
    fn open(&mut self) -> Result<(), Refusal> {
        self.depth += 1;
        if self.depth > MAX_DEPTH {
            return Err(self.err("nested deeper than 128 levels"));
        }
        self.i += 1;
        self.ws();
        Ok(())
    }

    /// After a member or an element: true past a comma, false past the
    /// closing bracket.
    fn more(&mut self, close: u8) -> Result<bool, Refusal> {
        self.ws();
        match self.peek() {
            Some(b',') => {
                self.i += 1;
                self.ws();
                Ok(true)
            }
            Some(c) if c == close => {
                self.i += 1;
                self.depth -= 1;
                Ok(false)
            }
            _ => Err(self.err(if close == b'}' {
                "expected , or }"
            } else {
                "expected , or ]"
            })),
        }
    }

    /// The object here, `f` called at each member's value with its key. A
    /// key twice in one object is refused.
    fn object(
        &mut self,
        f: &mut dyn FnMut(&mut Parser<'a>, String) -> Result<(), Refusal>,
    ) -> Result<(), Refusal> {
        self.open()?;
        if self.peek() == Some(b'}') {
            self.i += 1;
            self.depth -= 1;
            return Ok(());
        }
        let mut keys = BTreeSet::new();
        loop {
            if self.peek() != Some(b'"') {
                return Err(self.err("expected a key in quotes"));
            }
            let k = self.string()?;
            if !keys.insert(k.clone()) {
                return Err(self.err(format!("key {k:?} is in this object twice")));
            }
            self.ws();
            if self.peek() != Some(b':') {
                return Err(self.err("expected : after a key"));
            }
            self.i += 1;
            self.ws();
            f(self, k)?;
            if !self.more(b'}')? {
                return Ok(());
            }
        }
    }

    /// The array here, `f` called at each element with its index.
    fn array(
        &mut self,
        f: &mut dyn FnMut(&mut Parser<'a>, usize) -> Result<(), Refusal>,
    ) -> Result<(), Refusal> {
        self.open()?;
        if self.peek() == Some(b']') {
            self.i += 1;
            self.depth -= 1;
            return Ok(());
        }
        let mut i = 0;
        loop {
            f(self, i)?;
            i += 1;
            if !self.more(b']')? {
                return Ok(());
            }
        }
    }

    /// Past the value here, checking it.
    fn skip(&mut self) -> Result<(), Refusal> {
        match self.peek() {
            Some(b'{') => self.object(&mut |p, _| p.skip()),
            Some(b'[') => self.array(&mut |p, _| p.skip()),
            _ => self.scalar().map(|_| ()),
        }
    }
}

/// A JSON Pointer's reference tokens, unescaped: `""` has none, and a
/// pointer not starting with `/` isn't one.
pub fn tokens(pointer: &str) -> Option<Vec<String>> {
    if pointer.is_empty() {
        return Some(Vec::new());
    }
    let rest = pointer.strip_prefix('/')?;
    Some(
        rest.split('/')
            .map(|t| t.replace("~1", "/").replace("~0", "~"))
            .collect(),
    )
}

/// `key` escaped as one JSON Pointer reference token.
pub fn escape(key: &str) -> String {
    key.replace('~', "~0").replace('/', "~1")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn scalar(text: &str) -> Result<Option<String>, Refusal> {
        let mut p = Parser::new(text, 1, None);
        p.ws();
        let v = p.scalar()?;
        if v.is_some() {
            p.end()?;
        }
        Ok(v)
    }

    fn skip(text: &str) -> Result<(), Refusal> {
        let mut p = Parser::new(text, 1, None);
        p.ws();
        p.skip()?;
        p.end()
    }

    #[test]
    fn numbers_keep_their_digits() {
        for n in ["1.10", "1E-3", "-0", "0.5e+10", "12"] {
            assert_eq!(scalar(n).unwrap().as_deref(), Some(n));
        }
        for bad in ["01", "1.", ".5", "1e", "-", "+1"] {
            assert!(scalar(bad).is_err(), "{bad}");
        }
    }

    #[test]
    fn strings_unescape() {
        assert_eq!(
            scalar(r#""a\"b\\c\/\nø😀""#).unwrap().unwrap(),
            "a\"b\\c/\nø😀"
        );
        for bad in [r#""\ud83d""#, r#""\ude00x""#, "\"a\nb\"", r#""\x""#, "\"open"] {
            assert!(scalar(bad).is_err(), "{bad}");
        }
    }

    #[test]
    fn literals_and_containers() {
        assert_eq!(scalar("true").unwrap().as_deref(), Some("true"));
        assert_eq!(scalar("false").unwrap().as_deref(), Some("false"));
        assert_eq!(scalar("null").unwrap().as_deref(), Some(""));
        assert_eq!(scalar("{}").unwrap(), None);
        assert!(scalar("nul").is_err());
    }

    #[test]
    fn structure_is_checked() {
        skip(r#"{"a": [1, {"b": null}], "c": "d", "e": {}, "f": []}"#).unwrap();
        for bad in [
            r#"{"a": 1,}"#,
            "[1,]",
            r#"{"a" 1}"#,
            r#"{"a": 1 "b": 2}"#,
            "[1 2]",
            r#"{"a": 1, "a": 2}"#,
            "{a: 1}",
            "[1] [2]",
        ] {
            assert!(skip(bad).is_err(), "{bad}");
        }
    }

    #[test]
    fn depth_is_capped() {
        skip(&("[".repeat(128) + &"]".repeat(128))).unwrap();
        let e = skip(&("[".repeat(129) + &"]".repeat(129))).unwrap_err();
        assert!(e.message.contains("128"), "{}", e.message);
    }

    #[test]
    fn errors_name_their_line_and_column() {
        let e = skip("{\n  \"a\": tru\n}").unwrap_err();
        assert_eq!(e.row, 0);
        assert!(e.message.starts_with("line 2, column 8"), "{}", e.message);
    }
}
