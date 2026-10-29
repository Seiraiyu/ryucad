//! KiCad's s-expressions: lists, unquoted tokens (keywords and numbers,
//! kept as written) and quoted strings with their escapes read. Every
//! node knows where it starts, so a fault names its line and column.

use crate::error::{Code, Error, Result};

/// Where a node starts: line and column (in characters), from 1.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Pos {
    pub line: u32,
    pub column: u32,
}

#[derive(Clone, Debug, PartialEq)]
pub enum Sexpr {
    /// `(head items…)`.
    List(Vec<Sexpr>, Pos),
    /// An unquoted token: a keyword or a number, as written.
    Atom(String, Pos),
    /// A quoted string, its escapes read.
    Str(String, Pos),
}

/// How deep lists may nest: KiCad's files go about ten deep.
pub const MAX_DEPTH: usize = 256;

/// A fault at `pos`: `kicad_unreadable` with its line and column.
pub fn unreadable(pos: Pos, message: impl Into<String>) -> Error {
    let message = message.into();
    Error::new(
        Code::KicadUnreadable,
        format!("line {}, column {}: {message}", pos.line, pos.column),
    )
    .with("line", pos.line)
    .with("column", pos.column)
}

impl Sexpr {
    pub fn pos(&self) -> Pos {
        match self {
            Sexpr::List(_, p) | Sexpr::Atom(_, p) | Sexpr::Str(_, p) => *p,
        }
    }

    /// An atom's or a string's text.
    pub fn text(&self) -> Option<&str> {
        match self {
            Sexpr::Atom(t, _) | Sexpr::Str(t, _) => Some(t),
            Sexpr::List(..) => None,
        }
    }

    /// A list's first item, when it is an atom: what the list is.
    pub fn head(&self) -> Option<&str> {
        match self {
            Sexpr::List(items, _) => match items.first() {
                Some(Sexpr::Atom(t, _)) => Some(t),
                _ => None,
            },
            _ => None,
        }
    }

    /// A list's items after its head; nothing for an atom or a string.
    pub fn items(&self) -> &[Sexpr] {
        match self {
            Sexpr::List(items, _) if !items.is_empty() => &items[1..],
            _ => &[],
        }
    }

    /// The first item that is a list headed `name`.
    pub fn child(&self, name: &str) -> Option<&Sexpr> {
        self.items().iter().find(|s| s.head() == Some(name))
    }

    /// Every item that is a list headed `name`.
    pub fn children<'a>(&'a self, name: &'a str) -> impl Iterator<Item = &'a Sexpr> {
        self.items().iter().filter(move |s| s.head() == Some(name))
    }

    /// Whether an item is the bare atom `name` (KiCad 6's `hide`).
    pub fn has_atom(&self, name: &str) -> bool {
        self.items()
            .iter()
            .any(|s| matches!(s, Sexpr::Atom(t, _) if t == name))
    }

    /// The text of item `i` after the head, or a fault naming `what`.
    pub fn arg(&self, i: usize, what: &str) -> Result<&str> {
        self.items()
            .get(i)
            .and_then(Sexpr::text)
            .ok_or_else(|| unreadable(self.pos(), format!("{what} is missing")))
    }
}

/// Reads one s-expression: the whole of `text` but white space.
pub fn parse(text: &str) -> Result<Sexpr> {
    let mut p = Parser {
        chars: text.chars().peekable(),
        pos: Pos { line: 1, column: 1 },
    };
    p.skip_space();
    let start = p.pos;
    if p.chars.peek() != Some(&'(') {
        return Err(unreadable(start, "a KiCad file is one list: ( … )"));
    }
    let top = p.list()?;
    p.skip_space();
    if p.chars.peek().is_some() {
        return Err(unreadable(p.pos, "text after the file's list"));
    }
    Ok(top)
}

struct Parser<'a> {
    chars: std::iter::Peekable<std::str::Chars<'a>>,
    pos: Pos,
}

impl Parser<'_> {
    fn bump(&mut self) -> Option<char> {
        let c = self.chars.next()?;
        if c == '\n' {
            self.pos.line += 1;
            self.pos.column = 1;
        } else {
            self.pos.column += 1;
        }
        Some(c)
    }

    fn skip_space(&mut self) {
        while self.chars.peek().is_some_and(|c| c.is_whitespace()) {
            self.bump();
        }
    }

    /// A list, its opening parenthesis next, read without recursion.
    fn list(&mut self) -> Result<Sexpr> {
        let mut stack: Vec<(Vec<Sexpr>, Pos)> = Vec::new();
        loop {
            self.skip_space();
            let at = self.pos;
            match self.chars.peek().copied() {
                None => {
                    let open = stack.last().map_or(at, |(_, p)| *p);
                    return Err(unreadable(open, "the file ends inside this list"));
                }
                Some('(') => {
                    if stack.len() == MAX_DEPTH {
                        return Err(unreadable(
                            at,
                            format!("lists nest more than {MAX_DEPTH} deep"),
                        ));
                    }
                    self.bump();
                    stack.push((Vec::new(), at));
                }
                Some(')') => {
                    self.bump();
                    let (items, start) = stack.pop().expect("inside a list");
                    let done = Sexpr::List(items, start);
                    match stack.last_mut() {
                        Some((parent, _)) => parent.push(done),
                        None => return Ok(done),
                    }
                }
                Some('"') => {
                    let s = self.string()?;
                    stack.last_mut().expect("inside a list").0.push(s);
                }
                Some(_) => {
                    let mut t = String::new();
                    while let Some(&c) = self.chars.peek() {
                        if c.is_whitespace() || c == '(' || c == ')' || c == '"' {
                            break;
                        }
                        t.push(c);
                        self.bump();
                    }
                    stack
                        .last_mut()
                        .expect("inside a list")
                        .0
                        .push(Sexpr::Atom(t, at));
                }
            }
        }
    }

    /// A quoted string, its opening quote next.
    fn string(&mut self) -> Result<Sexpr> {
        let at = self.pos;
        self.bump();
        let mut s = String::new();
        loop {
            match self.bump() {
                None => return Err(unreadable(at, "a string that never ends")),
                Some('"') => return Ok(Sexpr::Str(s, at)),
                Some('\\') => match self.bump() {
                    None => return Err(unreadable(at, "a string that never ends")),
                    Some(c) => s.push(match c {
                        'n' => '\n',
                        'r' => '\r',
                        't' => '\t',
                        'a' => '\u{7}',
                        'b' => '\u{8}',
                        'f' => '\u{c}',
                        'v' => '\u{b}',
                        // `\"`, `\\` and anything else: the character.
                        c => c,
                    }),
                },
                Some(c) => s.push(c),
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn lists_atoms_and_strings_read() {
        let s =
            parse("(pad \"1\" smd (at -0.51 0)\n\t(layers \"F.Cu\" \"a\\\"b\\\\\"))\n").unwrap();
        assert_eq!(s.head(), Some("pad"));
        assert_eq!(s.arg(0, "number").unwrap(), "1");
        assert_eq!(s.arg(1, "type").unwrap(), "smd");
        let at = s.child("at").unwrap();
        assert_eq!(at.arg(0, "x").unwrap(), "-0.51");
        let layers: Vec<&str> = s
            .child("layers")
            .unwrap()
            .items()
            .iter()
            .filter_map(Sexpr::text)
            .collect();
        assert_eq!(layers, ["F.Cu", "a\"b\\"]);
        assert_eq!(s.child("layers").unwrap().pos(), Pos { line: 2, column: 2 });
        assert!(parse("(a hide)").unwrap().has_atom("hide"));
        assert_eq!(parse("(a)").unwrap().items(), &[]);
    }

    #[test]
    fn faults_name_their_place() {
        for (text, line, column) in [
            ("(a (b)", 1, 1),
            ("(a \"b)", 1, 4),
            ("(a) b", 1, 5),
            ("x", 1, 1),
            ("(a\n  (b\n", 2, 3),
        ] {
            let e = parse(text).unwrap_err();
            assert_eq!(e.code, Code::KicadUnreadable, "{text}");
            assert_eq!(
                (e.details["line"].as_u64(), e.details["column"].as_u64()),
                (Some(line), Some(column)),
                "{text}: {}",
                e.message
            );
        }
        let deep = "(".repeat(MAX_DEPTH + 1) + &")".repeat(MAX_DEPTH + 1);
        assert_eq!(parse(&deep).unwrap_err().code, Code::KicadUnreadable);
        let ok = "(".repeat(MAX_DEPTH) + &")".repeat(MAX_DEPTH);
        assert!(parse(&ok).is_ok());
    }
}
