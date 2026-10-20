//! Expressions: anywhere a quantity goes, a string such as
//! `2*wall + 3 mm` or `sqrt(a^2 + b^2)`.
//!
//! Literals carry their units (`3 mm`, `15 deg`); a bare literal is a
//! plain number, so `wall + 3` is a unit error, never a guess. Parameter
//! names are lowercase (`[a-z_][a-z0-9_]{0,62}`). A string keeps its
//! tokens as written; only the spacing is made canonical
//! (`2*wall+3mm` is written `2 * wall + 3 mm`).

use crate::error::{Code, Error, Result};
use crate::units::UNIT_NAMES;
use std::collections::BTreeSet;
use std::fmt::Write as _;

/// Deeper nesting than this is refused, so a hostile string can't
/// overflow the stack.
const MAX_DEPTH: usize = 64;

/// The functions an expression may call, from `ryukei::math`.
pub const FUNCTIONS: [&str; 14] = [
    "sin", "cos", "tan", "asin", "acos", "atan", "atan2", "sqrt", "abs", "min", "max", "floor",
    "ceil", "round",
];

/// Names that are always constants.
pub const CONSTANTS: [&str; 1] = ["pi"];

/// A parsed expression and its canonical text.
#[derive(Clone, Debug, PartialEq)]
pub struct Expr {
    node: Node,
    text: String,
}

#[derive(Clone, Debug, PartialEq)]
pub(crate) enum Node {
    /// A decimal as written, and the unit after it, if any.
    Num {
        text: String,
        unit: Option<String>,
    },
    /// A parameter, or a constant such as `pi`.
    Name(String),
    Call(String, Vec<Node>),
    Neg(Box<Node>),
    Pos(Box<Node>),
    Bin(Op, Box<Node>, Box<Node>),
    Paren(Box<Node>),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Op {
    Add,
    Sub,
    Mul,
    Div,
    Pow,
}

impl Op {
    fn symbol(self) -> &'static str {
        match self {
            Op::Add => " + ",
            Op::Sub => " - ",
            Op::Mul => " * ",
            Op::Div => " / ",
            Op::Pow => "^",
        }
    }
}

impl Expr {
    /// Parses `s`. Faults are `invalid_quantity`, with the byte `position`
    /// they were found at.
    pub fn parse(s: &str) -> Result<Expr> {
        let tokens = tokenize(s)?;
        let mut p = Parser {
            src: s,
            tokens,
            at: 0,
            depth: 0,
        };
        let node = p.sum()?;
        if let Some(t) = p.tokens.get(p.at) {
            return Err(p.fault(t.pos, &format!("unexpected {}", t.describe())));
        }
        Ok(Expr::from_node(node))
    }

    pub(crate) fn from_node(node: Node) -> Expr {
        let mut text = String::new();
        write_node(&mut text, &node);
        Expr { node, text }
    }

    /// The canonical text.
    pub fn text(&self) -> &str {
        &self.text
    }

    /// Whether this is a literal, possibly signed: `3 mm`, `-0.5`.
    pub fn is_literal(&self) -> bool {
        let mut n = &self.node;
        while let Node::Neg(x) | Node::Pos(x) = n {
            n = x;
        }
        matches!(n, Node::Num { .. })
    }

    /// The parameters it reads, in name order (constants are not
    /// parameters).
    pub fn names(&self) -> BTreeSet<String> {
        let mut out = BTreeSet::new();
        names(&self.node, &mut out);
        out
    }

    /// The same expression reading `to` wherever it read `from`.
    pub fn rename(&self, from: &str, to: &str) -> Expr {
        Expr::from_node(rename(&self.node, from, to))
    }
}

fn names(n: &Node, out: &mut BTreeSet<String>) {
    match n {
        Node::Num { .. } => {}
        Node::Name(s) => {
            if !CONSTANTS.contains(&s.as_str()) {
                out.insert(s.clone());
            }
        }
        Node::Call(_, args) => args.iter().for_each(|a| names(a, out)),
        Node::Neg(x) | Node::Pos(x) | Node::Paren(x) => names(x, out),
        Node::Bin(_, a, b) => {
            names(a, out);
            names(b, out);
        }
    }
}

fn rename(n: &Node, from: &str, to: &str) -> Node {
    let r = |x: &Node| Box::new(rename(x, from, to));
    match n {
        Node::Name(s) if s == from => Node::Name(to.to_owned()),
        Node::Num { .. } | Node::Name(_) => n.clone(),
        Node::Call(f, args) => Node::Call(
            f.clone(),
            args.iter().map(|a| rename(a, from, to)).collect(),
        ),
        Node::Neg(x) => Node::Neg(r(x)),
        Node::Pos(x) => Node::Pos(r(x)),
        Node::Paren(x) => Node::Paren(r(x)),
        Node::Bin(op, a, b) => Node::Bin(*op, r(a), r(b)),
    }
}

pub(crate) fn write_node(out: &mut String, n: &Node) {
    match n {
        Node::Num { text, unit } => {
            out.push_str(text);
            if let Some(u) = unit {
                let _ = write!(out, " {u}");
            }
        }
        Node::Name(s) => out.push_str(s),
        Node::Call(f, args) => {
            out.push_str(f);
            out.push('(');
            for (i, a) in args.iter().enumerate() {
                if i > 0 {
                    out.push_str(", ");
                }
                write_node(out, a);
            }
            out.push(')');
        }
        Node::Neg(x) => {
            out.push('-');
            write_node(out, x);
        }
        Node::Pos(x) => {
            out.push('+');
            write_node(out, x);
        }
        Node::Paren(x) => {
            out.push('(');
            write_node(out, x);
            out.push(')');
        }
        Node::Bin(op, a, b) => {
            write_node(out, a);
            out.push_str(op.symbol());
            write_node(out, b);
        }
    }
}

/// Whether `s` may name a parameter: lowercase, digits and `_`, not
/// starting with a digit, at most 63 bytes, and not a unit, function or
/// constant.
pub fn is_parameter_name(s: &str) -> bool {
    let b = s.as_bytes();
    !b.is_empty()
        && b.len() <= 63
        && (b[0].is_ascii_lowercase() || b[0] == b'_')
        && b.iter()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || *c == b'_')
        && !UNIT_NAMES.contains(&s)
        && !FUNCTIONS.contains(&s)
        && !CONSTANTS.contains(&s)
}

/// Checks a parameter name, as [`is_parameter_name`]; `invalid_id`
/// otherwise.
pub fn parameter_name(s: &str) -> Result<String> {
    if is_parameter_name(s) {
        return Ok(s.to_owned());
    }
    let why = if UNIT_NAMES.contains(&s) || FUNCTIONS.contains(&s) || CONSTANTS.contains(&s) {
        "is a unit, function or constant"
    } else {
        "must be lowercase letters, digits and _, not starting with a digit, at most 63 long"
    };
    Err(Error::new(Code::InvalidId, format!("parameter name {s:?} {why}")).with("parameter", s))
}

#[derive(Clone, Debug, PartialEq)]
enum Tok {
    Num(String),
    Ident(String),
    Sym(char),
}

#[derive(Clone, Debug)]
struct Token {
    tok: Tok,
    pos: usize,
}

impl Token {
    fn describe(&self) -> String {
        match &self.tok {
            Tok::Num(s) => format!("number {s}"),
            Tok::Ident(s) => format!("name {s}"),
            Tok::Sym(c) => format!("{c:?}"),
        }
    }
}

fn tokenize(s: &str) -> Result<Vec<Token>> {
    let b = s.as_bytes();
    let mut out = Vec::new();
    let mut i = 0;
    while i < b.len() {
        let c = b[i];
        if c.is_ascii_whitespace() {
            i += 1;
        } else if c.is_ascii_digit() || c == b'.' {
            let n = number_len(&s[i..]);
            if n == 0 {
                return Err(fault(s, i, "expected a number"));
            }
            out.push(Token {
                tok: Tok::Num(s[i..i + n].to_owned()),
                pos: i,
            });
            i += n;
        } else if c.is_ascii_alphabetic() || c == b'_' {
            let start = i;
            while i < b.len() && (b[i].is_ascii_alphanumeric() || b[i] == b'_') {
                i += 1;
            }
            out.push(Token {
                tok: Tok::Ident(s[start..i].to_owned()),
                pos: start,
            });
        } else if b"+-*/^(),".contains(&c) {
            out.push(Token {
                tok: Tok::Sym(c as char),
                pos: i,
            });
            i += 1;
        } else {
            let ch = s[i..].chars().next().expect("not at the end");
            return Err(fault(s, i, &format!("unexpected {ch:?}")));
        }
    }
    Ok(out)
}

/// The length of the unsigned decimal at the start of `s`: digits with an
/// optional fraction and exponent (`1`, `.5`, `2.`, `1e-3`), or 0.
fn number_len(s: &str) -> usize {
    let b = s.as_bytes();
    let mut i = 0;
    while i < b.len() && b[i].is_ascii_digit() {
        i += 1;
    }
    let mut digits = i;
    if i < b.len() && b[i] == b'.' {
        i += 1;
        let start = i;
        while i < b.len() && b[i].is_ascii_digit() {
            i += 1;
        }
        digits += i - start;
    }
    if digits == 0 {
        return 0;
    }
    if i < b.len() && (b[i] == b'e' || b[i] == b'E') {
        let mut j = i + 1;
        if j < b.len() && (b[j] == b'+' || b[j] == b'-') {
            j += 1;
        }
        let start = j;
        while j < b.len() && b[j].is_ascii_digit() {
            j += 1;
        }
        if j > start {
            i = j;
        }
    }
    i
}

fn fault(src: &str, pos: usize, why: &str) -> Error {
    Error::new(
        Code::InvalidQuantity,
        format!("bad expression {src:?} at {pos}: {why}"),
    )
    .with("value", src)
    .with("position", pos)
}

struct Parser<'a> {
    src: &'a str,
    tokens: Vec<Token>,
    at: usize,
    depth: usize,
}

impl Parser<'_> {
    fn fault(&self, pos: usize, why: &str) -> Error {
        fault(self.src, pos, why)
    }

    fn peek(&self) -> Option<&Tok> {
        self.tokens.get(self.at).map(|t| &t.tok)
    }

    fn pos(&self) -> usize {
        self.tokens.get(self.at).map_or(self.src.len(), |t| t.pos)
    }

    fn eat(&mut self, c: char) -> bool {
        if self.peek() == Some(&Tok::Sym(c)) {
            self.at += 1;
            true
        } else {
            false
        }
    }

    fn deeper(&mut self) -> Result<()> {
        self.depth += 1;
        if self.depth > MAX_DEPTH {
            return Err(self.fault(self.pos(), "nested too deeply"));
        }
        Ok(())
    }

    /// sum := product (('+' | '-') product)*
    fn sum(&mut self) -> Result<Node> {
        let mut n = self.product()?;
        loop {
            let op = if self.eat('+') {
                Op::Add
            } else if self.eat('-') {
                Op::Sub
            } else {
                return Ok(n);
            };
            n = Node::Bin(op, Box::new(n), Box::new(self.product()?));
        }
    }

    /// product := unary (('*' | '/') unary)*
    fn product(&mut self) -> Result<Node> {
        let mut n = self.unary()?;
        loop {
            let op = if self.eat('*') {
                Op::Mul
            } else if self.eat('/') {
                Op::Div
            } else {
                return Ok(n);
            };
            n = Node::Bin(op, Box::new(n), Box::new(self.unary()?));
        }
    }

    /// unary := ('-' | '+') unary | power. A sign binds looser than `^`:
    /// `-2^2` is `-(2^2)`.
    fn unary(&mut self) -> Result<Node> {
        self.deeper()?;
        let n = if self.eat('-') {
            Node::Neg(Box::new(self.unary()?))
        } else if self.eat('+') {
            Node::Pos(Box::new(self.unary()?))
        } else {
            self.power()?
        };
        self.depth -= 1;
        Ok(n)
    }

    /// power := atom ('^' unary)?, so `2^3^2` is `2^(3^2)`.
    fn power(&mut self) -> Result<Node> {
        let base = self.atom()?;
        if self.eat('^') {
            return Ok(Node::Bin(Op::Pow, Box::new(base), Box::new(self.unary()?)));
        }
        Ok(base)
    }

    /// atom := number unit? | name | name '(' sum (',' sum)* ')' | '(' sum ')'
    fn atom(&mut self) -> Result<Node> {
        let pos = self.pos();
        let Some(t) = self.peek().cloned() else {
            return Err(self.fault(pos, "expected a number, name or '('"));
        };
        self.at += 1;
        match t {
            Tok::Num(text) => {
                let unit = match self.peek() {
                    Some(Tok::Ident(u)) if UNIT_NAMES.contains(&u.as_str()) => {
                        let u = u.clone();
                        self.at += 1;
                        Some(u)
                    }
                    Some(Tok::Ident(u)) => {
                        let msg = format!(
                            "unknown unit {u:?}; units are {}, and a name needs an operator, as in 2 * {u}",
                            UNIT_NAMES.join(", ")
                        );
                        return Err(self.fault(self.pos(), &msg).with("unit", u.clone()));
                    }
                    _ => None,
                };
                Ok(Node::Num { text, unit })
            }
            Tok::Ident(name) => {
                if UNIT_NAMES.contains(&name.as_str()) {
                    return Err(
                        self.fault(pos, &format!("the unit {name} needs a number before it"))
                    );
                }
                if !self.eat('(') {
                    return Ok(Node::Name(name));
                }
                let mut args = vec![self.sum()?];
                while self.eat(',') {
                    args.push(self.sum()?);
                }
                if !self.eat(')') {
                    return Err(self.fault(self.pos(), "expected ',' or ')'"));
                }
                Ok(Node::Call(name, args))
            }
            Tok::Sym('(') => {
                self.deeper()?;
                let inner = self.sum()?;
                self.depth -= 1;
                if !self.eat(')') {
                    return Err(self.fault(self.pos(), "expected ')'"));
                }
                Ok(Node::Paren(Box::new(inner)))
            }
            t => Err(self.fault(
                pos,
                &format!("unexpected {}", Token { tok: t, pos }.describe()),
            )),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn text(s: &str) -> String {
        Expr::parse(s).unwrap().text().to_owned()
    }

    #[test]
    fn canonical_spacing_keeps_every_token() {
        for (s, t) in [
            ("2*wall+3mm", "2 * wall + 3 mm"),
            ("  2 *  wall  ", "2 * wall"),
            ("(a+b)/2", "(a + b) / 2"),
            ("-x^2", "-x^2"),
            ("+3 mm", "+3 mm"),
            ("atan2( a ,b )", "atan2(a, b)"),
            ("1E+2mm", "1E+2 mm"),
            ("((a))", "((a))"),
            ("2.^ -1", "2.^-1"),
            ("15deg", "15 deg"),
            (".5 in", ".5 in"),
        ] {
            assert_eq!(text(s), t, "{s}");
            // Canonical text parses back to itself.
            assert_eq!(text(t), t, "{t}");
        }
    }

    #[test]
    fn precedence() {
        let e = Expr::parse("1 + 2 * 3 ^ 2 ^ 2").unwrap();
        let Node::Bin(Op::Add, _, rhs) = &e.node else {
            panic!("{e:?}")
        };
        let Node::Bin(Op::Mul, _, pow) = rhs.as_ref() else {
            panic!("{rhs:?}")
        };
        let Node::Bin(Op::Pow, _, exp) = pow.as_ref() else {
            panic!("{pow:?}")
        };
        assert!(matches!(exp.as_ref(), Node::Bin(Op::Pow, _, _)));
        let e = Expr::parse("-2^2").unwrap();
        assert!(matches!(&e.node, Node::Neg(_)));
        let e = Expr::parse("a - b - c").unwrap();
        let Node::Bin(Op::Sub, lhs, _) = &e.node else {
            panic!()
        };
        assert!(matches!(lhs.as_ref(), Node::Bin(Op::Sub, _, _)));
    }

    #[test]
    fn faults_say_where() {
        for (s, pos) in [
            ("", 0),
            ("1 +", 3),
            ("2 wall", 2),
            ("1 furlong", 2),
            ("mm", 0),
            ("1 mm mm", 5),
            ("(1", 2),
            ("f(1 2)", 4),
            ("1..2", 2),
            ("1 # 2", 2),
            ("1 + ,", 4),
            ("a b", 2),
        ] {
            let e = Expr::parse(s).unwrap_err();
            assert_eq!(e.code, Code::InvalidQuantity, "{s}");
            assert_eq!(e.details["position"], pos, "{s}: {e}");
        }
        assert_eq!(
            Expr::parse("1 furlong").unwrap_err().details["unit"],
            "furlong"
        );
    }

    #[test]
    fn nesting_is_bounded() {
        let deep = format!("{}1{}", "(".repeat(100), ")".repeat(100));
        assert_eq!(Expr::parse(&deep).unwrap_err().code, Code::InvalidQuantity);
        let signs = format!("{}1", "-".repeat(100));
        assert_eq!(Expr::parse(&signs).unwrap_err().code, Code::InvalidQuantity);
        let ok = format!("{}1{}", "(".repeat(30), ")".repeat(30));
        assert!(Expr::parse(&ok).is_ok());
    }

    #[test]
    fn names_and_renames() {
        let e = Expr::parse("wall * 2 + max(gap, pi * r) - wall").unwrap();
        assert_eq!(
            e.names().into_iter().collect::<Vec<_>>(),
            ["gap", "r", "wall"]
        );
        let r = e.rename("wall", "skin");
        assert_eq!(r.text(), "skin * 2 + max(gap, pi * r) - skin");
        assert_eq!(e.rename("nobody", "x"), e);
    }

    #[test]
    fn literals() {
        for s in ["3 mm", "-3", "+-0.5 rad", "1e3"] {
            assert!(Expr::parse(s).unwrap().is_literal(), "{s}");
        }
        for s in ["(3 mm)", "a", "2 * 3", "pi"] {
            assert!(!Expr::parse(s).unwrap().is_literal(), "{s}");
        }
    }

    #[test]
    fn parameter_names() {
        for s in ["wall", "_x", "a1", "hole_d", &"a".repeat(63)] {
            assert!(is_parameter_name(s), "{s}");
        }
        for s in [
            "",
            "1a",
            "Wall",
            "a-b",
            "mm",
            "in",
            "sin",
            "pi",
            &"a".repeat(64),
        ] {
            assert!(!is_parameter_name(s), "{s}");
            assert_eq!(parameter_name(s).unwrap_err().code, Code::InvalidId);
        }
    }
}
