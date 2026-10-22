//! Tokens and parameters, read straight from the file's bytes.
//!
//! The grammar is ISO 10303-21's, with the liberties real files need:
//! keywords and enumerations in any case (held in upper case), any ASCII
//! white space between tokens, and line breaks inside strings dropped, as
//! writers that wrap long lines put them there.

use super::Param;

/// Lists nest no deeper than this, so no file can exhaust the stack.
const MAX_DEPTH: usize = 64;

/// Where a file breaks the grammar, and how.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct Fault {
    pub offset: usize,
    pub what: &'static str,
}

/// A position in `src`, reading no further than `end`.
pub(crate) struct Cursor<'a> {
    src: &'a [u8],
    pos: usize,
    end: usize,
}

impl<'a> Cursor<'a> {
    pub fn new(src: &'a [u8], start: usize, end: usize) -> Self {
        Cursor {
            src,
            pos: start,
            end,
        }
    }

    pub fn offset(&self) -> usize {
        self.pos
    }

    pub fn fault(&self, what: &'static str) -> Fault {
        Fault {
            offset: self.pos,
            what,
        }
    }

    fn byte(&self, at: usize) -> Option<u8> {
        if at < self.end {
            Some(self.src[at])
        } else {
            None
        }
    }

    /// Skips white space and comments.
    pub fn skip_space(&mut self) -> Result<(), Fault> {
        while let Some(b) = self.byte(self.pos) {
            match b {
                b' ' | b'\t' | b'\r' | b'\n' | 0x0b | 0x0c => self.pos += 1,
                b'/' if self.byte(self.pos + 1) == Some(b'*') => {
                    let start = self.pos;
                    self.pos += 2;
                    loop {
                        match self.byte(self.pos) {
                            None => {
                                return Err(Fault {
                                    offset: start,
                                    what: "unterminated comment",
                                });
                            }
                            Some(b'*') if self.byte(self.pos + 1) == Some(b'/') => {
                                self.pos += 2;
                                break;
                            }
                            Some(_) => self.pos += 1,
                        }
                    }
                }
                _ => break,
            }
        }
        Ok(())
    }

    /// The next byte after white space, not consumed.
    pub fn peek(&mut self) -> Result<Option<u8>, Fault> {
        self.skip_space()?;
        Ok(self.byte(self.pos))
    }

    /// Consumes `b` if it comes next.
    pub fn eat(&mut self, b: u8) -> Result<bool, Fault> {
        if self.peek()? == Some(b) {
            self.pos += 1;
            Ok(true)
        } else {
            Ok(false)
        }
    }

    pub fn expect(&mut self, b: u8, what: &'static str) -> Result<(), Fault> {
        if self.eat(b)? {
            Ok(())
        } else {
            Err(self.fault(what))
        }
    }

    pub fn at_end(&mut self) -> Result<bool, Fault> {
        Ok(self.peek()?.is_none())
    }

    /// Consumes `word` if the next token is exactly it (case-insensitively),
    /// such as `ISO-10303-21` or `ENDSEC`.
    pub fn eat_word(&mut self, word: &str) -> Result<bool, Fault> {
        self.skip_space()?;
        let w = word.as_bytes();
        let end = self.pos + w.len();
        if end > self.end || !self.src[self.pos..end].eq_ignore_ascii_case(w) {
            return Ok(false);
        }
        // `END` must not match the start of `END-ISO-10303-21`.
        if self.byte(end).is_some_and(|b| is_ident(b) || b == b'-') {
            return Ok(false);
        }
        self.pos = end;
        Ok(true)
    }

    /// A standard keyword, or a user-defined one with its `!`.
    pub fn keyword(&mut self) -> Result<Box<str>, Fault> {
        self.skip_space()?;
        let start = self.pos;
        if self.byte(self.pos) == Some(b'!') {
            self.pos += 1;
        }
        if !self.byte(self.pos).is_some_and(is_ident_start) {
            self.pos = start;
            return Err(self.fault("expected a keyword"));
        }
        while self.byte(self.pos).is_some_and(is_ident) {
            self.pos += 1;
        }
        Ok(upper(&self.src[start..self.pos]))
    }

    /// `( p, p, … )`.
    pub fn params(&mut self) -> Result<Vec<Param>, Fault> {
        self.list(0)
    }

    fn list(&mut self, depth: usize) -> Result<Vec<Param>, Fault> {
        if depth >= MAX_DEPTH {
            return Err(self.fault("lists nested too deeply"));
        }
        self.expect(b'(', "expected (")?;
        let mut out = Vec::new();
        if self.eat(b')')? {
            return Ok(out);
        }
        loop {
            out.push(self.param(depth)?);
            if self.eat(b')')? {
                return Ok(out);
            }
            self.expect(b',', "expected , or )")?;
        }
    }

    fn param(&mut self, depth: usize) -> Result<Param, Fault> {
        let Some(b) = self.peek()? else {
            return Err(self.fault("expected a parameter"));
        };
        match b {
            b'$' => {
                self.pos += 1;
                Ok(Param::Unset)
            }
            b'*' => {
                self.pos += 1;
                Ok(Param::Derived)
            }
            b'#' => self.reference().map(Param::Ref),
            b'\'' => self.string().map(Param::Str),
            b'"' => self.binary().map(Param::Binary),
            b'(' => self.list(depth + 1).map(Param::List),
            b'.' if self.byte(self.pos + 1).is_some_and(|c| c.is_ascii_digit()) => self.number(),
            b'.' => self.enumeration().map(Param::Enum),
            b'+' | b'-' | b'0'..=b'9' => self.number(),
            b'!' | b'A'..=b'Z' | b'a'..=b'z' | b'_' => {
                let name = self.keyword()?;
                self.expect(b'(', "expected ( after a type name")?;
                let value = self.param(depth + 1)?;
                self.expect(b')', "expected ) after a typed value")?;
                Ok(Param::Typed(name, Box::new(value)))
            }
            _ => Err(self.fault("expected a parameter")),
        }
    }

    /// `#n`.
    pub fn reference(&mut self) -> Result<u64, Fault> {
        self.expect(b'#', "expected #")?;
        let start = self.pos;
        while self.byte(self.pos).is_some_and(|c| c.is_ascii_digit()) {
            self.pos += 1;
        }
        if start == self.pos {
            return Err(self.fault("expected an instance number"));
        }
        let text = std::str::from_utf8(&self.src[start..self.pos]).expect("ASCII digits");
        text.parse().map_err(|_| Fault {
            offset: start,
            what: "instance number out of range",
        })
    }

    /// An integer, or a real if it has a point or an exponent.
    fn number(&mut self) -> Result<Param, Fault> {
        let start = self.pos;
        if matches!(self.byte(self.pos), Some(b'+' | b'-')) {
            self.pos += 1;
        }
        let digits = self.digits();
        let mut real = false;
        let mut fraction = 0;
        if self.byte(self.pos) == Some(b'.') {
            real = true;
            self.pos += 1;
            fraction = self.digits();
        }
        if digits + fraction == 0 {
            self.pos = start;
            return Err(self.fault("expected a number"));
        }
        if matches!(self.byte(self.pos), Some(b'E' | b'e')) {
            real = true;
            self.pos += 1;
            if matches!(self.byte(self.pos), Some(b'+' | b'-')) {
                self.pos += 1;
            }
            if self.digits() == 0 {
                return Err(self.fault("expected an exponent"));
            }
        }
        let text = std::str::from_utf8(&self.src[start..self.pos]).expect("ASCII number");
        let fault = |what| Fault {
            offset: start,
            what,
        };
        if real {
            // Rust's parser is correctly rounded and the same everywhere.
            let x: f64 = text.parse().map_err(|_| fault("malformed real"))?;
            if x.is_finite() {
                Ok(Param::Real(x))
            } else {
                Err(fault("real out of range"))
            }
        } else {
            text.parse()
                .map(Param::Int)
                .map_err(|_| fault("integer out of range"))
        }
    }

    fn digits(&mut self) -> usize {
        let start = self.pos;
        while self.byte(self.pos).is_some_and(|c| c.is_ascii_digit()) {
            self.pos += 1;
        }
        self.pos - start
    }

    /// `'…'`: a doubled quote is one quote, and line breaks are dropped.
    fn string(&mut self) -> Result<Box<str>, Fault> {
        let start = self.pos;
        self.pos += 1;
        let mut raw = Vec::new();
        loop {
            match self.byte(self.pos) {
                None => {
                    return Err(Fault {
                        offset: start,
                        what: "unterminated string",
                    });
                }
                Some(b'\'') if self.byte(self.pos + 1) == Some(b'\'') => {
                    raw.push(b'\'');
                    self.pos += 2;
                }
                Some(b'\'') => {
                    self.pos += 1;
                    break;
                }
                Some(b'\r' | b'\n') => self.pos += 1,
                Some(c) => {
                    raw.push(c);
                    self.pos += 1;
                }
            }
        }
        Ok(raw_text(&raw).into())
    }

    /// `.NAME.`, without its dots.
    fn enumeration(&mut self) -> Result<Box<str>, Fault> {
        self.pos += 1;
        let start = self.pos;
        if !self.byte(self.pos).is_some_and(is_ident_start) {
            return Err(self.fault("expected an enumeration"));
        }
        while self.byte(self.pos).is_some_and(is_ident) {
            self.pos += 1;
        }
        let name = upper(&self.src[start..self.pos]);
        if self.byte(self.pos) != Some(b'.') {
            return Err(self.fault("expected . after an enumeration"));
        }
        self.pos += 1;
        Ok(name)
    }

    /// `"…"`: a digit 0 to 3, then hex digits.
    fn binary(&mut self) -> Result<Box<str>, Fault> {
        self.pos += 1;
        let start = self.pos;
        if !matches!(self.byte(self.pos), Some(b'0'..=b'3')) {
            return Err(self.fault("expected 0 to 3 to start a binary"));
        }
        self.pos += 1;
        while self.byte(self.pos).is_some_and(|c| c.is_ascii_hexdigit()) {
            self.pos += 1;
        }
        let text = upper(&self.src[start..self.pos]);
        if self.byte(self.pos) != Some(b'"') {
            return Err(self.fault("expected \" to end a binary"));
        }
        self.pos += 1;
        Ok(text)
    }
}

/// Bytes between a string's quotes as text: UTF-8 if they are, else each
/// byte as ISO 8859-1, which is what a byte above 127 meant before UTF-8.
fn raw_text(raw: &[u8]) -> String {
    match std::str::from_utf8(raw) {
        Ok(s) => s.to_owned(),
        Err(_) => raw.iter().map(|&b| char::from(b)).collect(),
    }
}

fn is_ident_start(b: u8) -> bool {
    b.is_ascii_alphabetic() || b == b'_'
}

pub(crate) fn is_ident(b: u8) -> bool {
    b.is_ascii_alphanumeric() || b == b'_'
}

fn upper(bytes: &[u8]) -> Box<str> {
    let s = std::str::from_utf8(bytes).expect("ASCII identifier");
    s.to_ascii_uppercase().into()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn params(text: &str) -> Result<Vec<Param>, Fault> {
        let mut c = Cursor::new(text.as_bytes(), 0, text.len());
        let p = c.params()?;
        assert!(c.at_end()?, "{text}: trailing input");
        Ok(p)
    }

    fn one(text: &str) -> Param {
        let mut p = params(&format!("({text})")).expect(text);
        assert_eq!(p.len(), 1);
        p.pop().unwrap()
    }

    fn fault(text: &str) -> &'static str {
        params(text).expect_err(text).what
    }

    #[test]
    fn simple_values() {
        assert_eq!(one("$"), Param::Unset);
        assert_eq!(one("*"), Param::Derived);
        assert_eq!(one("#12"), Param::Ref(12));
        assert_eq!(one("42"), Param::Int(42));
        assert_eq!(one("-7"), Param::Int(-7));
        assert_eq!(one(".T."), Param::Enum("T".into()));
        assert_eq!(one(".unspecified."), Param::Enum("UNSPECIFIED".into()));
        assert_eq!(one("\"0ff\""), Param::Binary("0FF".into()));
        assert_eq!(params("()").unwrap(), vec![]);
    }

    #[test]
    fn reals() {
        let real = |t: &str| match one(t) {
            Param::Real(x) => x,
            p => panic!("{t}: {p:?}"),
        };
        assert_eq!(real("1."), 1.0);
        assert_eq!(real("0.5"), 0.5);
        assert_eq!(real("1.E-07"), 1e-7);
        assert_eq!(real("-2.5e3"), -2500.0);
        assert_eq!(real("+.25"), 0.25);
        assert_eq!(real("1E5"), 1e5);
        assert!(real("-0.").is_sign_negative());
        assert_eq!(real("4.286263797016E-16"), 4.286263797016e-16);
    }

    #[test]
    fn strings() {
        assert_eq!(one("''"), Param::Str("".into()));
        assert_eq!(one("'it''s'"), Param::Str("it's".into()));
        // OCCT wraps long strings across lines; the breaks aren't text.
        assert_eq!(
            one("'AP242_MIM_LF. {1 0 10303 442 1 1 4 \n}'"),
            Param::Str("AP242_MIM_LF. {1 0 10303 442 1 1 4 }".into())
        );
        assert_eq!(one("'caf\u{e9}'"), Param::Str("caf\u{e9}".into()));
        let mut c = Cursor::new(b"('caf\xe9')", 0, 8);
        assert_eq!(c.params().unwrap(), vec![Param::Str("caf\u{e9}".into())]);
    }

    #[test]
    fn lists_and_typed_values() {
        assert_eq!(
            one("(1.,(2,#3),$)"),
            Param::List(vec![
                Param::Real(1.0),
                Param::List(vec![Param::Int(2), Param::Ref(3)]),
                Param::Unset,
            ])
        );
        assert_eq!(
            one("length_measure ( 1.E-07 )"),
            Param::Typed("LENGTH_MEASURE".into(), Box::new(Param::Real(1e-7)))
        );
        assert_eq!(
            one("!MY_TYPE((1))"),
            Param::Typed(
                "!MY_TYPE".into(),
                Box::new(Param::List(vec![Param::Int(1)]))
            )
        );
    }

    #[test]
    fn space_and_comments() {
        assert_eq!(
            params(" ( /* a */ 1 ,\r\n\t/* b * / */ 2 ) /* c */").unwrap(),
            vec![Param::Int(1), Param::Int(2)]
        );
    }

    #[test]
    fn keywords() {
        let mut c = Cursor::new(b"  cartesian_point(", 0, 18);
        assert_eq!(&*c.keyword().unwrap(), "CARTESIAN_POINT");
        let mut c = Cursor::new(b"END-ISO-10303-21;", 0, 17);
        assert!(!c.eat_word("END").unwrap(), "END is followed by -");
        assert!(c.eat_word("end-iso-10303-21").unwrap());
        assert!(c.eat(b';').unwrap());
        let mut c = Cursor::new(b"ENDSECX", 0, 7);
        assert!(!c.eat_word("ENDSEC").unwrap());
    }

    #[test]
    fn faults() {
        assert_eq!(fault("(1"), "expected , or )");
        assert_eq!(fault("(1 2)"), "expected , or )");
        assert_eq!(fault("('abc)"), "unterminated string");
        assert_eq!(fault("(/* x)"), "unterminated comment");
        assert_eq!(fault("(1.E)"), "expected an exponent");
        assert_eq!(fault("(1E999)"), "real out of range");
        assert_eq!(fault("(99999999999999999999)"), "integer out of range");
        assert_eq!(fault("(#)"), "expected an instance number");
        assert_eq!(
            fault("(#99999999999999999999)"),
            "instance number out of range"
        );
        assert_eq!(fault("(.T)"), "expected . after an enumeration");
        assert_eq!(fault("(\"4F\")"), "expected 0 to 3 to start a binary");
        assert_eq!(fault("(X 1)"), "expected ( after a type name");
        assert_eq!(fault("(@)"), "expected a parameter");
        assert_eq!(fault("(-)"), "expected a number");
        let deep = format!("{}{}", "(".repeat(100), ")".repeat(100));
        assert_eq!(fault(&deep), "lists nested too deeply");
    }

    #[test]
    fn faults_point_at_the_problem() {
        let e = params("(1,   'abc").unwrap_err();
        assert_eq!(e.offset, 6);
    }
}
