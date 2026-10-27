//! Expressions: anywhere a quantity goes, a string such as
//! `2*wall + 3 mm` or `sqrt(a^2 + b^2)`.
//!
//! Literals carry their units (`3 mm`, `15 deg`); a bare literal is a
//! plain number, so `wall + 3` is a unit error, never a guess. Parameter
//! names are lowercase (`[a-z_][a-z0-9_]{0,62}`). A string keeps its
//! tokens as written; only the spacing is made canonical
//! (`2*wall+3mm` is written `2 * wall + 3 mm`).

use crate::error::{Code, Error, Result};
use crate::units::{Dim, UNIT_NAMES, literal_unit};
use ryu_math as math;
use serde_json::{Value, json};
use std::cell::RefCell;
use std::collections::{BTreeMap, BTreeSet};
use std::fmt::Write as _;

/// Deeper nesting than this is refused, so a hostile string can't
/// overflow the stack.
const MAX_DEPTH: usize = 64;

/// The functions an expression may call, from `ryu-math` (`ryukei::math`).
pub const FUNCTIONS: [&str; 14] = [
    "sin", "cos", "tan", "asin", "acos", "atan", "atan2", "sqrt", "abs", "min", "max", "floor",
    "ceil", "round",
];

/// Names that are always constants.
pub const CONSTANTS: [&str; 1] = ["pi"];

/// How many arguments a function takes: `(least, most)`.
fn arity(f: &str) -> Option<(usize, usize)> {
    match f {
        "atan2" => Some((2, 2)),
        "min" | "max" => Some((2, usize::MAX)),
        f if FUNCTIONS.contains(&f) => Some((1, 1)),
        _ => None,
    }
}

/// A value in SI units (meters, radians) and what it measures.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Val {
    pub x: f64,
    pub dim: Dim,
}

impl Val {
    pub fn new(x: f64, dim: Dim) -> Val {
        Val { x, dim }
    }

    /// How output reports a value: its kind and its SI value under the
    /// kind's key (`m`, `rad`, `n`, or `si`).
    pub fn report(&self) -> Value {
        let mut v = json!({"kind": self.dim.name()});
        v[self.dim.key()] = json!(self.x);
        v
    }
}

/// The lookup for a context with no parameters: every name is unknown.
pub fn no_parameters(name: &str) -> Result<Val> {
    Err(unknown_parameter(name))
}

/// A part's parameters, evaluated: each one's value, or why it has none.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Env {
    values: BTreeMap<String, Result<Val>>,
    /// Why the parameters can't be read at all, if they can't.
    broken: Option<Error>,
}

impl Env {
    /// No parameters.
    pub fn empty() -> Env {
        Env::default()
    }

    /// Parameters whose file can't be used: every name fails with `e`.
    pub fn broken(e: Error) -> Env {
        Env {
            values: BTreeMap::new(),
            broken: Some(e),
        }
    }

    /// Evaluates every parameter, each reading the others. A parameter on a
    /// cycle is `parameter_cycle`, naming the cycle; one that reads a
    /// failed parameter fails with that parameter's error.
    pub fn new(params: &BTreeMap<String, Expr>) -> Env {
        let solver = Cycles {
            params,
            done: RefCell::new(BTreeMap::new()),
            stack: RefCell::new(Vec::new()),
        };
        for name in params.keys() {
            let _ = solver.get(name);
        }
        Env {
            values: solver.done.into_inner(),
            broken: None,
        }
    }

    /// A parameter's value.
    pub fn get(&self, name: &str) -> Result<Val> {
        match (self.values.get(name), &self.broken) {
            (Some(v), _) => v.clone(),
            (None, Some(e)) => Err(e.clone()),
            (None, None) => Err(unknown_parameter(name)),
        }
    }

    /// Every parameter's value or error, by name.
    pub fn values(&self) -> &BTreeMap<String, Result<Val>> {
        &self.values
    }
}

/// Evaluation in dependency order, finding cycles on the way.
struct Cycles<'a> {
    params: &'a BTreeMap<String, Expr>,
    done: RefCell<BTreeMap<String, Result<Val>>>,
    stack: RefCell<Vec<String>>,
}

impl Cycles<'_> {
    fn get(&self, name: &str) -> Result<Val> {
        if let Some(v) = self.done.borrow().get(name) {
            return v.clone();
        }
        let Some(e) = self.params.get(name) else {
            return Err(unknown_parameter(name));
        };
        let on_stack = self.stack.borrow().iter().position(|n| n == name);
        if let Some(i) = on_stack {
            let mut cycle: Vec<String> = self.stack.borrow()[i..].to_vec();
            cycle.push(name.to_owned());
            let err = Error::new(
                Code::ParameterCycle,
                format!(
                    "parameters read each other in a circle: {}",
                    cycle.join(" -> ")
                ),
            )
            .with("cycle", &cycle);
            let mut done = self.done.borrow_mut();
            for n in &cycle {
                done.insert(n.clone(), Err(err.clone()));
            }
            return Err(err);
        }
        self.stack.borrow_mut().push(name.to_owned());
        let v = e.eval(&|n| self.get(n));
        self.stack.borrow_mut().pop();
        // A parameter on a cycle already has its error.
        self.done
            .borrow_mut()
            .entry(name.to_owned())
            .or_insert(v)
            .clone()
    }
}

pub fn unknown_parameter(name: &str) -> Error {
    Error::new(
        Code::UnknownParameter,
        format!("no parameter {name}; add it with `ryucad param set`"),
    )
    .with("parameter", name)
}

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

    /// The value, with `lookup` giving each parameter's. Evaluation is f64
    /// in a fixed order, with `ryu-math` for every function, so it gives
    /// the same bits on every platform. A result that isn't finite (a
    /// division by zero, `asin(2)`) is `invalid_quantity`; mixing kinds is
    /// `unit_mismatch`, naming the subexpression `at` fault.
    pub fn eval(&self, lookup: &dyn Fn(&str) -> Result<Val>) -> Result<Val> {
        Eval {
            whole: &self.text,
            lookup,
        }
        .node(&self.node)
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

struct Eval<'a> {
    whole: &'a str,
    lookup: &'a dyn Fn(&str) -> Result<Val>,
}

impl Eval<'_> {
    fn fault(&self, code: Code, at: &Node, why: &str) -> Error {
        let mut text = String::new();
        write_node(&mut text, at);
        let msg = if text == self.whole {
            format!("{}: {why}", self.whole)
        } else {
            format!("{}: in {text}: {why}", self.whole)
        };
        Error::new(code, msg)
            .with("value", self.whole)
            .with("at", text)
    }

    fn mismatch(&self, at: &Node, why: &str) -> Error {
        self.fault(Code::UnitMismatch, at, why)
    }

    fn node(&self, n: &Node) -> Result<Val> {
        let v = self.value(n)?;
        if !v.x.is_finite() {
            return Err(self.fault(Code::InvalidQuantity, n, "is not a finite number"));
        }
        Ok(v)
    }

    fn value(&self, n: &Node) -> Result<Val> {
        Ok(match n {
            Node::Num { text, unit } => {
                let x: f64 = text
                    .parse()
                    .map_err(|_| self.fault(Code::InvalidQuantity, n, "is not a number"))?;
                match unit {
                    None => Val::new(x, Dim::NUMBER),
                    Some(u) => {
                        let (factor, dim) = literal_unit(u).expect("the parser checked the unit");
                        Val::new(x * factor, dim)
                    }
                }
            }
            Node::Name(s) if s == "pi" => Val::new(core::f64::consts::PI, Dim::NUMBER),
            Node::Name(s) => (self.lookup)(s)?,
            Node::Neg(x) => {
                let v = self.node(x)?;
                Val::new(-v.x, v.dim)
            }
            Node::Pos(x) | Node::Paren(x) => self.node(x)?,
            Node::Bin(op, a, b) => {
                let (va, vb) = (self.node(a)?, self.node(b)?);
                let dim = match op {
                    Op::Add | Op::Sub if va.dim != vb.dim => {
                        let why = format!(
                            "can't {} a {} and a {}",
                            if *op == Op::Add { "add" } else { "subtract" },
                            va.dim.name(),
                            vb.dim.name()
                        );
                        return Err(self.mismatch(n, &why));
                    }
                    Op::Add | Op::Sub => Some(va.dim),
                    Op::Mul => va.dim.times(vb.dim),
                    Op::Div => va.dim.per(vb.dim),
                    Op::Pow => return self.pow(n, va, vb),
                };
                let dim = dim.ok_or_else(|| self.mismatch(n, "powers of units too high"))?;
                let x = match op {
                    Op::Add => va.x + vb.x,
                    Op::Sub => va.x - vb.x,
                    Op::Mul => va.x * vb.x,
                    _ => va.x / vb.x,
                };
                Val::new(x, dim)
            }
            Node::Call(f, args) => {
                let vals = args
                    .iter()
                    .map(|a| self.node(a))
                    .collect::<Result<Vec<Val>>>()?;
                self.call(n, f, &vals)?
            }
        })
    }

    fn pow(&self, n: &Node, base: Val, exp: Val) -> Result<Val> {
        if exp.dim != Dim::NUMBER {
            return Err(self.mismatch(n, "an exponent must be a plain number"));
        }
        let dim = if base.dim == Dim::NUMBER {
            Dim::NUMBER
        } else if exp.x.fract() == 0.0 && exp.x.abs() <= 16.0 {
            base.dim
                .pow(exp.x as i32)
                .ok_or_else(|| self.mismatch(n, "powers of units too high"))?
        } else {
            let why = format!("a {} can only be raised to a whole power", base.dim.name());
            return Err(self.mismatch(n, &why));
        };
        Ok(Val::new(math::pow(base.x, exp.x), dim))
    }

    fn call(&self, n: &Node, f: &str, a: &[Val]) -> Result<Val> {
        let want = |dim: Dim| -> Result<()> {
            match a.iter().find(|v| v.dim != dim) {
                Some(v) => Err(self.mismatch(
                    n,
                    &format!("{f} takes a {}, not a {}", dim.name(), v.dim.name()),
                )),
                None => Ok(()),
            }
        };
        let same = || -> Result<Dim> {
            want(a[0].dim)?;
            Ok(a[0].dim)
        };
        let x = a[0].x;
        Ok(match f {
            "sin" | "cos" | "tan" => {
                want(Dim::ANGLE)?;
                let y = match f {
                    "sin" => math::sin(x),
                    "cos" => math::cos(x),
                    _ => math::tan(x),
                };
                Val::new(y, Dim::NUMBER)
            }
            "asin" | "acos" | "atan" => {
                want(Dim::NUMBER)?;
                let y = match f {
                    "asin" => math::asin(x),
                    "acos" => math::acos(x),
                    _ => math::atan2(x, 1.0),
                };
                Val::new(y, Dim::ANGLE)
            }
            "atan2" => {
                same()?;
                Val::new(math::atan2(x, a[1].x), Dim::ANGLE)
            }
            "sqrt" => {
                let dim = a[0].dim.sqrt().ok_or_else(|| {
                    self.mismatch(n, &format!("the square root of a {}", a[0].dim.name()))
                })?;
                Val::new(math::sqrt(x), dim)
            }
            "abs" => Val::new(x.abs(), a[0].dim),
            "min" | "max" => {
                let dim = same()?;
                let pick = |m: f64, v: &Val| {
                    if (f == "min" && v.x < m) || (f == "max" && v.x > m) {
                        v.x
                    } else {
                        m
                    }
                };
                Val::new(a[1..].iter().fold(x, pick), dim)
            }
            _ => {
                want(Dim::NUMBER)?;
                let y = match f {
                    "floor" => x.floor(),
                    "ceil" => x.ceil(),
                    _ => x.round(),
                };
                Val::new(y, Dim::NUMBER)
            }
        })
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
                let Some((least, most)) = arity(&name) else {
                    let msg = format!(
                        "unknown function {name}; functions are {}",
                        FUNCTIONS.join(", ")
                    );
                    return Err(self.fault(pos, &msg));
                };
                self.deeper()?;
                let mut args = vec![self.sum()?];
                while self.eat(',') {
                    args.push(self.sum()?);
                }
                self.depth -= 1;
                if !self.eat(')') {
                    return Err(self.fault(self.pos(), "expected ',' or ')'"));
                }
                if args.len() < least || args.len() > most {
                    let n = if least == most {
                        format!("{least}")
                    } else {
                        format!("at least {least}")
                    };
                    return Err(self.fault(pos, &format!("{name} takes {n} argument(s)")));
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
            ("max(1 2)", 6),
            ("1..2", 2),
            ("1 # 2", 2),
            ("1 + ,", 4),
            ("a b", 2),
            ("cosh(1)", 0),
            ("sin(1, 2)", 0),
            ("2 * max(1)", 4),
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

    fn eval(s: &str) -> Result<Val> {
        let lookup = |name: &str| match name {
            "wall" => Ok(Val::new(0.003, Dim::LENGTH)),
            "n" => Ok(Val::new(4.0, Dim::NUMBER)),
            "tilt" => Ok(Val::new(0.5, Dim::ANGLE)),
            _ => no_parameters(name),
        };
        Expr::parse(s).unwrap().eval(&lookup)
    }

    fn val(s: &str) -> (f64, String) {
        let v = eval(s).unwrap();
        (v.x, v.dim.name())
    }

    #[test]
    fn evaluates_in_si_with_dimensions() {
        let deg = crate::units::DEG;
        for (s, x, dim) in [
            ("3 mm", 0.003, "length"),
            ("2 * wall + 3 mm", 2.0 * 0.003 + 0.003, "length"),
            ("wall * wall", 0.003 * 0.003, "length^2"),
            ("sqrt(wall^2)", math::sqrt(math::pow(0.003, 2.0)), "length"),
            ("n / 2", 2.0, "number"),
            ("15 deg", 15.0 * deg, "angle"),
            ("sin(30 deg)", math::sin(30.0 * deg), "number"),
            ("atan(1)", math::atan2(1.0, 1.0), "angle"),
            ("atan2(wall, 2 mm)", math::atan2(0.003, 0.002), "angle"),
            ("wall / tilt", 0.003 / 0.5, "length*angle^-1"),
            ("-2^2", -4.0, "number"),
            ("2^-1", 0.5, "number"),
            ("max(1 mm, wall, 2 mm)", 0.003, "length"),
            ("min(n, 2, 3)", 2.0, "number"),
            ("abs(-wall)", 0.003, "length"),
            (
                "floor(2.5) + ceil(0.5) + round(2.5)",
                2.0 + 1.0 + 3.0,
                "number",
            ),
            ("2 * pi * 1 m", 2.0 * core::f64::consts::PI * 1.0, "length"),
            ("1 in", 0.0254, "length"),
        ] {
            assert_eq!(val(s), (x, dim.to_owned()), "{s}");
        }
    }

    #[test]
    fn mixing_kinds_is_a_unit_mismatch() {
        for (s, at) in [
            ("wall + 3", "wall + 3"),
            ("2 * (wall - 1 deg)", "wall - 1 deg"),
            ("sin(wall)", "sin(wall)"),
            ("sqrt(wall)", "sqrt(wall)"),
            ("wall^tilt", "wall^tilt"),
            ("wall^0.5", "wall^0.5"),
            ("n^wall", "n^wall"),
            ("floor(wall)", "floor(wall)"),
            ("max(wall, n)", "max(wall, n)"),
            ("atan2(wall, n)", "atan2(wall, n)"),
            ("wall^16 * wall", "wall^16 * wall"),
        ] {
            let e = eval(s).unwrap_err();
            assert_eq!(e.code, Code::UnitMismatch, "{s}: {e}");
            assert_eq!(e.details["at"], at, "{s}");
            assert_eq!(e.details["value"], s, "{s}");
        }
        assert!(eval("wall^n").is_ok());
    }

    #[test]
    fn non_finite_results_are_refused() {
        for s in ["1 / 0", "asin(2)", "1e999", "sqrt(-1)", "10^400"] {
            assert_eq!(eval(s).unwrap_err().code, Code::InvalidQuantity, "{s}");
        }
    }

    #[test]
    fn unknown_names() {
        let e = eval("2 * gap").unwrap_err();
        assert_eq!(e.code, Code::UnknownParameter);
        assert_eq!(e.details["parameter"], "gap");
    }

    fn env(pairs: &[(&str, &str)]) -> Env {
        Env::new(
            &pairs
                .iter()
                .map(|(k, v)| (k.to_string(), Expr::parse(v).unwrap()))
                .collect(),
        )
    }

    #[test]
    fn parameters_read_each_other() {
        let e = env(&[("hole", "wall * 2"), ("wall", "3 mm"), ("n", "4")]);
        assert_eq!(e.get("hole").unwrap(), Val::new(0.006, Dim::LENGTH));
        assert_eq!(e.get("n").unwrap(), Val::new(4.0, Dim::NUMBER));
        assert_eq!(e.get("gap").unwrap_err().code, Code::UnknownParameter);
        assert_eq!(e.values().len(), 3);
    }

    #[test]
    fn cycles_are_named() {
        let e = env(&[
            ("a", "b + 1 mm"),
            ("b", "c"),
            ("c", "a"),
            ("d", "b"),
            ("z", "1 mm"),
        ]);
        for n in ["a", "b", "c"] {
            let err = e.get(n).unwrap_err();
            assert_eq!(err.code, Code::ParameterCycle, "{n}");
            assert_eq!(err.details["cycle"], json!(["a", "b", "c", "a"]), "{n}");
        }
        assert_eq!(e.get("d").unwrap_err().code, Code::ParameterCycle);
        assert!(e.get("z").is_ok());
        let me = env(&[("x", "x")]);
        assert_eq!(me.get("x").unwrap_err().details["cycle"], json!(["x", "x"]));
    }

    #[test]
    fn failures_spread_to_readers() {
        let e = env(&[("a", "gap * 2"), ("b", "a + 1 mm"), ("c", "1 mm + 1 deg")]);
        assert_eq!(e.get("a").unwrap_err().code, Code::UnknownParameter);
        assert_eq!(e.get("b").unwrap_err().details["parameter"], "gap");
        assert_eq!(e.get("c").unwrap_err().code, Code::UnitMismatch);
        let broken = Env::broken(Error::new(Code::InvalidJson, "bad"));
        assert_eq!(broken.get("a").unwrap_err().code, Code::InvalidJson);
    }

    #[test]
    fn reports_by_kind() {
        assert_eq!(
            Val::new(0.5, Dim::LENGTH).report(),
            json!({"kind": "length", "m": 0.5})
        );
        assert_eq!(
            Val::new(2.0, Dim::NUMBER).report(),
            json!({"kind": "number", "n": 2.0})
        );
        let area = Dim::LENGTH.times(Dim::LENGTH).unwrap();
        assert_eq!(
            Val::new(2.0, area).report(),
            json!({"kind": "length^2", "si": 2.0})
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
