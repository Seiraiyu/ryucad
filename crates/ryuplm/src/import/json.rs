//! The JSON reader (design 935): records found by a JSON Pointer, each
//! flattened to its leaf values named by their pointers relative to the
//! record, numbers kept exactly as written. Written here rather than read
//! through serde_json: serde_json keeps no number's digits (`1.10` becomes
//! `1.1`), and its `arbitrary_precision` feature would change every
//! crate's numbers through feature unification, ryu-doc's canonical JSON
//! among them. It reads as it goes and builds no tree.

use super::rows::{Leaves, Refusal};
use std::collections::{BTreeMap, BTreeSet};

/// The deepest nesting read: deeper is refused, which bounds the
/// recursion too.
pub const MAX_DEPTH: usize = 128;

/// What a JSON reader reads (a mapping's reader): the records' pointer
/// (unused with `lines`), the children's pointer relative to a record,
/// and whether the file is JSON Lines.
#[derive(Clone, Copy, Debug)]
pub struct Shape<'a> {
    pub records: &'a str,
    pub children: Option<&'a str>,
    pub lines: bool,
}

/// The records `bytes` hold, each its place and its leaves, children
/// after their parent with `#level` when `shape.children` is set.
pub fn read(bytes: &[u8], shape: Shape<'_>) -> Result<Vec<Leaves>, Refusal> {
    let text = utf8(bytes)?;
    let mut out = Vec::new();
    if shape.lines {
        for (i, line) in text.split('\n').enumerate() {
            let line = line.strip_suffix('\r').unwrap_or(line);
            if line.trim().is_empty() {
                continue;
            }
            let n = i as u64 + 1;
            let mut p = Parser::new(line, n, shape.children);
            p.base = format!("line {n} ");
            p.ws();
            p.record(&mut out, "", 0)?;
            p.end()?;
        }
        if out.is_empty() {
            return Err(Refusal::file("no records: every line is blank"));
        }
    } else {
        let segs = tokens(shape.records).ok_or_else(|| {
            Refusal::file(format!("records {:?} isn't a JSON Pointer", shape.records))
        })?;
        let mut p = Parser::new(text, 1, shape.children);
        p.ws();
        if !p.walk(&segs, &mut String::new(), &mut out)? {
            return Err(Refusal::file(format!("no value at {:?}", shape.records)));
        }
        p.end()?;
        if out.is_empty() {
            return Err(Refusal::file(format!("no records at {:?}", shape.records)));
        }
    }
    Ok(out)
}

/// `bytes` as text: JSON is UTF-8 (RFC 8259), a byte-order mark skipped.
fn utf8(bytes: &[u8]) -> Result<&str, Refusal> {
    let b = bytes.strip_prefix(&[0xEF, 0xBB, 0xBF]).unwrap_or(bytes);
    std::str::from_utf8(b).map_err(|e| {
        let line = 1 + b[..e.valid_up_to()].iter().filter(|c| **c == b'\n').count();
        Refusal::file(format!("line {line} isn't UTF-8; JSON is UTF-8"))
    })
}

/// What a survey knows of the arrays with one pattern (indices as `*`):
/// how many objects they hold, up to 1,024 of their keys, and when the
/// pattern was first seen.
#[derive(Default)]
struct Stat {
    objects: usize,
    keys: BTreeSet<String>,
    order: usize,
}

/// `import guess`'s proposal for a JSON file (design 935): the concrete
/// array holding the most objects (the first seen on a tie) as records,
/// unless `records` names them, and as children the array of objects one
/// key below a record that shares at least two keys with the records and
/// at least half of its own. JSON Lines' records are `""`.
pub fn survey(
    bytes: &[u8],
    lines: bool,
    records: Option<&str>,
) -> Result<(String, Option<String>), Refusal> {
    let text = utf8(bytes)?;
    let mut stats: BTreeMap<String, Stat> = BTreeMap::new();
    if lines {
        for (i, line) in text.split('\n').enumerate() {
            let line = line.strip_suffix('\r').unwrap_or(line);
            if line.trim().is_empty() {
                continue;
            }
            let mut p = Parser::new(line, i as u64 + 1, None);
            p.ws();
            p.survey_value(&mut "/*".to_owned(), Some(""), &mut stats)?;
            p.end()?;
        }
    } else {
        let mut p = Parser::new(text, 1, None);
        p.ws();
        p.survey_value(&mut String::new(), None, &mut stats)?;
        p.end()?;
    }
    let records = match records {
        Some(r) => tokens(r)
            .ok_or_else(|| Refusal::file(format!("records {r:?} isn't a JSON Pointer")))?
            .iter()
            .map(|x| format!("/{}", escape(x)))
            .collect::<String>(),
        None if lines => String::new(),
        None => stats
            .iter()
            .filter(|(k, s)| s.objects > 0 && !k.split('/').any(|t| t == "*"))
            .max_by_key(|(_, s)| (s.objects, std::cmp::Reverse(s.order)))
            .map(|(k, _)| k.clone())
            .ok_or_else(|| {
                Refusal::file("no array of objects to read as records; name one with --records")
            })?,
    };
    let Some(rs) = stats.get(&records) else {
        return Ok((records, None));
    };
    let prefix = format!("{records}/*/");
    let children = stats
        .iter()
        .filter(|(k, s)| {
            s.objects > 0
                && k.strip_prefix(&prefix)
                    .is_some_and(|rest| !rest.contains('/'))
        })
        .filter(|(_, s)| {
            let common = s.keys.intersection(&rs.keys).count();
            common >= 2 && common * 2 >= s.keys.len()
        })
        .min_by_key(|(_, s)| s.order)
        .map(|(k, _)| format!("/{}", &k[prefix.len()..]));
    Ok((records, children))
}

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

    /// Walks from the value here down `segs`; at their end, each element
    /// of the array there is a record. False when a step finds nothing.
    fn walk(
        &mut self,
        segs: &[String],
        ptr: &mut String,
        out: &mut Vec<Leaves>,
    ) -> Result<bool, Refusal> {
        let Some((first, rest)) = segs.split_first() else {
            if self.peek() != Some(b'[') {
                let what = if ptr.is_empty() {
                    "the document".to_owned()
                } else {
                    ptr.clone()
                };
                return Err(self.err(format!("{what} isn't an array of records")));
            }
            let base = ptr.clone();
            self.array(&mut |p, i| p.record(out, &format!("{base}/{i}"), 0))?;
            return Ok(true);
        };
        let mut found = false;
        let len = ptr.len();
        match self.peek() {
            Some(b'{') => self.object(&mut |p, k| {
                if k != *first {
                    return p.skip();
                }
                ptr.push('/');
                ptr.push_str(&escape(&k));
                found = p.walk(rest, ptr, out)?;
                ptr.truncate(len);
                Ok(())
            })?,
            Some(b'[') => {
                let want = first.parse::<usize>().ok();
                self.array(&mut |p, i| {
                    if want != Some(i) {
                        return p.skip();
                    }
                    ptr.push('/');
                    ptr.push_str(&i.to_string());
                    found = p.walk(rest, ptr, out)?;
                    ptr.truncate(len);
                    Ok(())
                })?;
            }
            _ => self.skip()?,
        }
        Ok(found)
    }

    /// The record here, at pointer `ptr`: pushed before its children, so
    /// rows come in document order. An error inside it that names no row
    /// is refused at this record.
    fn record(&mut self, out: &mut Vec<Leaves>, ptr: &str, level: usize) -> Result<(), Refusal> {
        let p = out.len();
        let at = p as u64 + 1;
        let place = format!("{}{ptr}", self.base).trim_end().to_owned();
        out.push(Leaves {
            place: place.clone(),
            cells: Vec::new(),
        });
        let mut cells = Vec::new();
        if self.children.is_some() {
            cells.push(("#level".to_owned(), level.to_string()));
        }
        let r = if self.peek() == Some(b'{') {
            self.flatten(&mut String::new(), &mut cells, out, ptr, level)
        } else {
            Err(self.err("a record is an object"))
        };
        r.map_err(|e| {
            if e.row == 0 {
                Refusal { row: at, ..e }.placed(Some(place.as_str()))
            } else {
                e
            }
        })?;
        out[p].cells = cells;
        Ok(())
    }

    /// The value here, at `path` within the record at `ptr`: its leaves
    /// into `cells`, or, at the children's pointer, the child records.
    fn flatten(
        &mut self,
        path: &mut String,
        cells: &mut Vec<(String, String)>,
        out: &mut Vec<Leaves>,
        ptr: &str,
        level: usize,
    ) -> Result<(), Refusal> {
        if !path.is_empty() && self.children.as_deref() == Some(path.as_str()) {
            return self.kids(out, &format!("{ptr}{path}"), level + 1);
        }
        let len = path.len();
        match self.peek() {
            Some(b'{') => self.object(&mut |p, k| {
                path.push('/');
                path.push_str(&escape(&k));
                let r = p.flatten(path, cells, out, ptr, level);
                path.truncate(len);
                r
            }),
            Some(b'[') => self.array(&mut |p, i| {
                path.push('/');
                path.push_str(&i.to_string());
                let r = p.flatten(path, cells, out, ptr, level);
                path.truncate(len);
                r
            }),
            _ => {
                let v = self.scalar()?.unwrap_or_default();
                cells.push((path.clone(), v));
                Ok(())
            }
        }
    }

    /// The children at `ptr`: an array of records, or `null` for none.
    fn kids(&mut self, out: &mut Vec<Leaves>, ptr: &str, level: usize) -> Result<(), Refusal> {
        match self.peek() {
            Some(b'[') => {
                let base = ptr.to_owned();
                self.array(&mut |p, i| p.record(out, &format!("{base}/{i}"), level))
            }
            Some(b'n') => self.scalar().map(|_| ()),
            _ => Err(self.err(format!("the children at {ptr} aren't an array"))),
        }
    }

    /// The value here at `pattern`, into `stats`: an object directly in
    /// an array counts under the array's pattern (`owner`), with its keys.
    fn survey_value(
        &mut self,
        pattern: &mut String,
        owner: Option<&str>,
        stats: &mut BTreeMap<String, Stat>,
    ) -> Result<(), Refusal> {
        match self.peek() {
            Some(b'{') => {
                if let Some(a) = owner {
                    let n = stats.len();
                    stats
                        .entry(a.to_owned())
                        .or_insert_with(|| Stat {
                            order: n,
                            ..Stat::default()
                        })
                        .objects += 1;
                }
                let len = pattern.len();
                self.object(&mut |p, k| {
                    if let Some(a) = owner
                        && let Some(s) = stats.get_mut(a)
                        && s.keys.len() < 1024
                    {
                        s.keys.insert(k.clone());
                    }
                    pattern.push('/');
                    pattern.push_str(&escape(&k));
                    let r = p.survey_value(pattern, None, stats);
                    pattern.truncate(len);
                    r
                })
            }
            Some(b'[') => {
                let here = pattern.clone();
                let len = pattern.len();
                self.array(&mut |p, _| {
                    pattern.push_str("/*");
                    let r = p.survey_value(pattern, Some(here.as_str()), stats);
                    pattern.truncate(len);
                    r
                })
            }
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
        for bad in [
            r#""\ud83d""#,
            r#""\ude00x""#,
            "\"a\nb\"",
            r#""\x""#,
            "\"open",
        ] {
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

    fn read_str(
        text: &str,
        records: &str,
        children: Option<&str>,
        lines: bool,
    ) -> Result<Vec<Leaves>, Refusal> {
        read(
            text.as_bytes(),
            Shape {
                records,
                children,
                lines,
            },
        )
    }

    fn cells(l: &Leaves) -> Vec<(&str, &str)> {
        l.cells
            .iter()
            .map(|(a, b)| (a.as_str(), b.as_str()))
            .collect()
    }

    #[test]
    fn pointers_parse_and_escape() {
        assert_eq!(
            tokens("/a~1b/~0c"),
            Some(vec!["a/b".to_owned(), "~c".to_owned()])
        );
        assert_eq!(tokens(""), Some(vec![]));
        assert_eq!(tokens("a"), None);
        assert_eq!(escape("a/b~c"), "a~1b~0c");
    }

    #[test]
    fn records_flatten_to_their_pointers() {
        let r = read_str(
            r#"{"meta": {"n": 2}, "data": {"items": [
                {"pn": "100-2", "w": 1.10, "mfr": {"name": "Acme"}, "alts": [{"mpn": "X1"}, {"mpn": "X2"}],
                 "tags": [], "note": null, "a/b": true},
                {"pn": "100-3"}]}}"#,
            "/data/items",
            None,
            false,
        )
        .unwrap();
        assert_eq!(r.len(), 2);
        assert_eq!(r[0].place, "/data/items/0");
        assert_eq!(
            cells(&r[0]),
            [
                ("/pn", "100-2"),
                ("/w", "1.10"),
                ("/mfr/name", "Acme"),
                ("/alts/0/mpn", "X1"),
                ("/alts/1/mpn", "X2"),
                ("/note", ""),
                ("/a~1b", "true")
            ]
        );
        assert_eq!(cells(&r[1]), [("/pn", "100-3")]);
    }

    #[test]
    fn a_document_array_and_a_byte_order_mark() {
        let mut b = vec![0xEF, 0xBB, 0xBF];
        b.extend_from_slice(br#"[{"pn": "A"}]"#);
        let r = read(
            &b,
            Shape {
                records: "",
                children: None,
                lines: false,
            },
        )
        .unwrap();
        assert_eq!(
            (r[0].place.as_str(), cells(&r[0])),
            ("/0", vec![("/pn", "A")])
        );
    }

    #[test]
    fn children_follow_their_parent_with_a_level() {
        let r = read_str(
            r#"{"bom": [{"pn": "A", "parts": [{"pn": "B", "parts": [{"pn": "C"}]}, {"pn": "D", "parts": null}]}, {"pn": "E"}]}"#,
            "/bom",
            Some("/parts"),
            false,
        )
        .unwrap();
        let got: Vec<(&str, Vec<(&str, &str)>)> =
            r.iter().map(|l| (l.place.as_str(), cells(l))).collect();
        assert_eq!(
            got,
            vec![
                ("/bom/0", vec![("#level", "0"), ("/pn", "A")]),
                ("/bom/0/parts/0", vec![("#level", "1"), ("/pn", "B")]),
                (
                    "/bom/0/parts/0/parts/0",
                    vec![("#level", "2"), ("/pn", "C")]
                ),
                ("/bom/0/parts/1", vec![("#level", "1"), ("/pn", "D")]),
                ("/bom/1", vec![("#level", "0"), ("/pn", "E")]),
            ]
        );
    }

    #[test]
    fn json_lines_are_records_by_line() {
        let r = read_str(
            "{\"pn\": \"A\"}\r\n\n{\"pn\": \"B\", \"parts\": [{\"pn\": \"C\"}]}\n",
            "",
            Some("/parts"),
            true,
        )
        .unwrap();
        let places: Vec<&str> = r.iter().map(|l| l.place.as_str()).collect();
        assert_eq!(places, ["line 1", "line 3", "line 3 /parts/0"]);
        let e = read_str("{\"pn\": \"A\"}\n[1]\n", "", None, true).unwrap_err();
        assert_eq!((e.row, e.place.as_deref()), (2, Some("line 2")));
        assert!(read_str("\n\n", "", None, true).is_err());
    }

    #[test]
    fn refusals_say_where() {
        let msg = |t: &str, r: &str, c: Option<&str>| read_str(t, r, c, false).unwrap_err().message;
        assert!(msg(r#"{"items": []}"#, "/data", None).contains("no value at"));
        assert!(msg(r#"{"items": {}}"#, "/items", None).contains("isn't an array"));
        assert!(msg(r#"{"items": []}"#, "/items", None).contains("no records"));
        assert!(
            msg(
                r#"{"items": [{"pn": "A", "parts": 3}]}"#,
                "/items",
                Some("/parts")
            )
            .contains("aren't an array")
        );
        let e = read_str(
            r#"{"items": [{"pn": "A"}, {"pn": "B", "pn": "C"}]}"#,
            "/items",
            None,
            false,
        )
        .unwrap_err();
        assert_eq!((e.row, e.place.as_deref()), (2, Some("/items/1")));
        assert!(e.message.contains("twice"));
        let e = read(
            b"{\"a\": \"\xFF\"}",
            Shape {
                records: "",
                children: None,
                lines: false,
            },
        )
        .unwrap_err();
        assert!(e.message.contains("UTF-8"));
    }

    #[test]
    fn a_survey_proposes_the_records_and_their_children() {
        let doc = br#"{"meta": {"tags": [{"k": 1}]}, "items": [
            {"id": 1, "pn": "A", "qty": 1, "mfrs": [{"name": "M"}], "parts": [{"id": 2, "pn": "B", "qty": 2}]},
            {"id": 3, "pn": "C", "qty": 1}]}"#;
        assert_eq!(
            survey(doc, false, None).unwrap(),
            ("/items".to_owned(), Some("/parts".to_owned()))
        );
        assert_eq!(
            survey(doc, false, Some("/meta/tags")).unwrap(),
            ("/meta/tags".to_owned(), None)
        );
        let lines = b"{\"id\": 1, \"pn\": \"A\"}\n{\"id\": 2, \"pn\": \"B\", \"parts\": [{\"id\": 3, \"pn\": \"C\"}]}\n";
        assert_eq!(
            survey(lines, true, None).unwrap(),
            (String::new(), Some("/parts".to_owned()))
        );
        assert!(survey(br#"{"a": 1}"#, false, None).is_err());
    }
}
