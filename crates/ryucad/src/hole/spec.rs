//! Hole specs: the one-line callout an agent writes (`--spec "M6x1.0-6H
//! THRU"`), parsed into a hole's fields, and written back canonically
//! from a feature. See the plan's grammar (ryucad 28.A); `THRU NEXT` and
//! `SPOTFACE` arrive with 28.B.
//!
//! Canonical text is ASCII and exact: lengths are their literals' own
//! decimals, expressions are in parentheses, and nothing is formatted from
//! a float. Drawings write the symbol form.

use crate::doc::hole::{
    BoreTolerance, Bottom, Depth as HoleDepth, FastenerFit, Hole, HoleForm, ThreadDepth,
};
use crate::error::{Code, Error, Result};
use crate::units::{Quantity, Unit};
use ryu_standards::thread::Standard;
use serde_json::{Map, Value, json};

/// A length as the spec writes it: a number in the context's unit
/// (`6`), a number and a unit (`0.25 in`), or `(expression)`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Len(pub String);

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ThreadKind {
    Metric,
    /// `UNC`, `UNF`, `UNEF`, `UN`.
    Unified(String),
    /// `NPT`, `NPTF`, `NPSM`.
    Pipe(String),
    /// `Rc`, `Rp`, `G`: the size carries the prefix.
    British,
    /// A project standard's key.
    House(String),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MouthKind {
    Cbore,
    Csk,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum What {
    Thread {
        kind: ThreadKind,
        /// `M6`, `1/4`, `#10`, `Rc1/4`, `T10`.
        size: String,
        /// A metric pitch (mm) or a TPI, as written.
        pitch: Option<Len>,
        class: Option<String>,
        left: bool,
    },
    Fastener {
        /// Counterbore or countersink; none is a plain clearance hole.
        mouth: Option<MouthKind>,
        standard: String,
        size: String,
        fit: Option<FastenerFit>,
    },
    Dia {
        diameter: Len,
        class: Option<String>,
    },
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Depth {
    Thru,
    Full,
    Deep(Len),
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Drill {
    pub diameter: Option<Len>,
    /// A tapped hole's drill depth.
    pub depth: Option<Len>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Mouth {
    pub kind: MouthKind,
    pub diameter: Option<Len>,
    /// A counterbore's depth, or a countersink's angle in degrees.
    pub second: Option<Len>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Spec {
    pub what: What,
    pub starts: Option<u32>,
    pub depth: Option<Depth>,
    /// A drill point's included angle in degrees: after a plain hole's
    /// depth, in a tapped hole's DRILL clause.
    pub angle: Option<Len>,
    pub drill: Option<Drill>,
    pub mouth: Option<Mouth>,
}

/// The unit a spec's bare numbers take: a metric thread's millimetres,
/// an inch or pipe thread's inches, else the project's.
pub fn context(what: &What, project: Unit) -> Unit {
    match what {
        What::Thread {
            kind: ThreadKind::Metric,
            ..
        } => Unit::Mm,
        What::Thread {
            kind: ThreadKind::Unified(_) | ThreadKind::Pipe(_) | ThreadKind::British,
            ..
        } => Unit::In,
        _ => project,
    }
}

impl Len {
    /// A quantity as the spec writes it in unit `cu`.
    pub fn of(q: &Quantity, cu: Unit) -> Len {
        let t = q.text();
        if !q.is_literal() {
            return Len(format!("({t})"));
        }
        match t.split_once(' ') {
            Some((n, u)) if Unit::parse(u) == Some(cu) => Len(n.to_owned()),
            _ => Len(t),
        }
    }

    /// An angle in degrees as the spec writes it.
    pub fn of_angle(q: &Quantity) -> Len {
        let t = q.text();
        match t.strip_suffix(" deg") {
            Some(n) if q.is_literal() => Len(n.to_owned()),
            _ => Len(format!("({t})")),
        }
    }

    /// As a hole's field: `"6 mm"`, `"0.25 in"`, `"d * 2"`.
    pub fn field(&self, cu: Unit) -> String {
        let t = &self.0;
        if let Some(e) = t.strip_prefix('(').and_then(|x| x.strip_suffix(')')) {
            return e.to_owned();
        }
        if t.contains(' ') {
            t.clone()
        } else {
            format!("{t} {}", cu.as_str())
        }
    }

    pub fn angle_field(&self) -> String {
        match self.0.strip_prefix('(').and_then(|x| x.strip_suffix(')')) {
            Some(e) => e.to_owned(),
            None => format!("{} deg", self.0),
        }
    }
}

impl Spec {
    /// The spec of a hole: every field its grammar carries.
    pub fn of(h: &Hole, project: Unit) -> Spec {
        let what = match (&h.thread, &h.fastener) {
            (Some(t), _) => {
                let built = crate::doc::hole::resolve(t).ok().flatten();
                let kind = match Standard::parse(&t.standard) {
                    Some(Standard::IsoMetric) => ThreadKind::Metric,
                    Some(s @ (Standard::Unc | Standard::Unf | Standard::Unef | Standard::Un)) => {
                        ThreadKind::Unified(s.key().to_uppercase())
                    }
                    Some(Standard::Npt) => ThreadKind::Pipe("NPT".into()),
                    Some(Standard::Nptf) => ThreadKind::Pipe("NPTF".into()),
                    Some(Standard::Nps) => ThreadKind::Pipe("NPSM".into()),
                    Some(Standard::Bspt | Standard::Bspp) => ThreadKind::British,
                    None => ThreadKind::House(t.standard.clone()),
                };
                let pitch = match (&kind, &built) {
                    (ThreadKind::British | ThreadKind::House(_), _) => None,
                    (_, Some(b)) => Some(Len(b.pitch.value.text())),
                    _ => None,
                };
                let class = match &built {
                    Some(b) => b.class.clone(),
                    None => t.class.clone(),
                };
                What::Thread {
                    kind,
                    size: t.size.clone(),
                    pitch,
                    class,
                    left: t.left,
                }
            }
            (None, Some(f)) => What::Fastener {
                mouth: match h.form {
                    HoleForm::Simple
                    | HoleForm::Counterdrill { .. }
                    | HoleForm::Spotface { .. } => None,
                    HoleForm::Counterbore { .. } => Some(MouthKind::Cbore),
                    HoleForm::Countersink { .. } => Some(MouthKind::Csk),
                },
                standard: f.standard.clone(),
                size: f.size.clone(),
                fit: (f.fit != FastenerFit::Normal).then_some(f.fit),
            },
            (None, None) => What::Dia {
                diameter: Len::of(
                    h.diameter.as_ref().expect("a plain hole has its diameter"),
                    project,
                ),
                class: match &h.bore {
                    Some(BoreTolerance::Fit(c)) => Some(c.clone()),
                    _ => None,
                },
            },
        };
        let cu = context(&what, project);
        let tapped = h.thread.is_some();
        let angle = match &h.bottom {
            Bottom::Drill(a) => Some(Len::of_angle(a)),
            Bottom::Flat => None,
        };
        let depth = match (&h.thread, &h.depth) {
            (Some(t), d) => match (&t.depth, d) {
                (Some(ThreadDepth::Full), _) => Some(Depth::Full),
                (Some(ThreadDepth::Length(q)), _) => Some(Depth::Deep(Len::of(q, cu))),
                (None, HoleDepth::ThroughAll) => Some(Depth::Thru),
                (None, _) => None,
            },
            (None, HoleDepth::ThroughAll) => Some(Depth::Thru),
            (None, HoleDepth::Blind(q)) => Some(Depth::Deep(Len::of(q, cu))),
            (None, HoleDepth::UpTo { .. }) => None,
        };
        let derived = h.thread.is_some() || h.fastener.is_some();
        let drill_diameter = h
            .diameter
            .as_ref()
            .filter(|_| derived)
            .map(|q| Len::of(q, cu));
        let drill_depth = match (&h.depth, tapped) {
            (HoleDepth::Blind(q), true) => Some(Len::of(q, cu)),
            _ => None,
        };
        let drill = (drill_diameter.is_some() || drill_depth.is_some()).then_some(Drill {
            diameter: drill_diameter,
            depth: drill_depth,
        });
        let written = |q: &Option<Quantity>| q.as_ref().map(|q| Len::of(q, cu));
        let mouth = match &h.form {
            HoleForm::Simple | HoleForm::Counterdrill { .. } | HoleForm::Spotface { .. } => None,
            HoleForm::Counterbore { diameter, depth } => Some(Mouth {
                kind: MouthKind::Cbore,
                diameter: written(diameter),
                second: written(depth),
            }),
            HoleForm::Countersink { diameter, angle } => Some(Mouth {
                kind: MouthKind::Csk,
                diameter: written(diameter),
                second: angle.as_ref().map(Len::of_angle),
            }),
        }
        // A fastener's mouth with nothing written says nothing more.
        .filter(|m| h.fastener.is_none() || m.diameter.is_some() || m.second.is_some());
        Spec {
            what,
            starts: h.thread.as_ref().and_then(|t| t.starts),
            depth,
            angle,
            drill,
            mouth,
        }
    }

    /// The canonical text.
    pub fn text(&self) -> String {
        let mut s = match &self.what {
            What::Thread {
                kind,
                size,
                pitch,
                class,
                left,
            } => {
                let mut s = match (kind, pitch) {
                    (ThreadKind::Metric, Some(p)) => format!("{size}x{}", p.0),
                    (ThreadKind::Metric, None) => size.clone(),
                    (ThreadKind::Unified(w) | ThreadKind::Pipe(w), Some(p)) => {
                        format!("{size}-{} {w}", p.0)
                    }
                    (ThreadKind::Unified(w) | ThreadKind::Pipe(w), None) => format!("{size} {w}"),
                    (ThreadKind::British, _) => size.clone(),
                    (ThreadKind::House(std), _) => format!("THREAD {} {size}", std.to_uppercase()),
                };
                let house = matches!(kind, ThreadKind::House(_));
                if let Some(c) = class {
                    s.push_str(&if house {
                        format!(" CLASS {c}")
                    } else {
                        format!("-{c}")
                    });
                }
                if *left {
                    s.push_str(if house { " LH" } else { "-LH" });
                }
                s
            }
            What::Fastener {
                mouth,
                standard,
                size,
                fit,
            } => {
                let word = match mouth {
                    Some(MouthKind::Cbore) => "CBORE",
                    Some(MouthKind::Csk) => "CSK",
                    None => "CLEAR",
                };
                let mut s = format!("{word} {} {size}", standard.to_uppercase());
                if let Some(f) = fit {
                    s.push_str(&format!(" {}", f.key().to_uppercase()));
                }
                s
            }
            What::Dia { diameter, class } => {
                let mut s = format!("DIA {}", diameter.0);
                if let Some(c) = class {
                    s.push_str(&format!(" {c}"));
                }
                s
            }
        };
        if let Some(n) = self.starts {
            s.push_str(&format!(" {n} STARTS"));
        }
        let tapped = matches!(self.what, What::Thread { .. });
        match &self.depth {
            None => {}
            Some(Depth::Thru) => s.push_str(" THRU"),
            Some(Depth::Full) => s.push_str(" FULL"),
            Some(Depth::Deep(l)) => s.push_str(&format!(" {} DEEP", l.0)),
        }
        if !tapped && let Some(a) = &self.angle {
            s.push_str(&format!(" {}DEG", a.0));
        }
        let tapped_angle = self.angle.as_ref().filter(|_| tapped);
        if self.drill.is_some() || tapped_angle.is_some() {
            s.push_str(", DRILL");
            if let Some(d) = &self.drill {
                if let Some(x) = &d.diameter {
                    s.push_str(&format!(" {}", x.0));
                }
                if let Some(x) = &d.depth {
                    s.push_str(&format!(" {} DEEP", x.0));
                }
            }
            if let Some(a) = tapped_angle {
                s.push_str(&format!(" {}DEG", a.0));
            }
        }
        if let Some(m) = &self.mouth {
            s.push_str(match m.kind {
                MouthKind::Cbore => ", CBORE",
                MouthKind::Csk => ", CSK",
            });
            if let Some(d) = &m.diameter {
                s.push_str(&format!(" {}", d.0));
            }
            match (m.kind, &m.second) {
                (MouthKind::Cbore, Some(x)) => s.push_str(&format!(" {} DEEP", x.0)),
                (MouthKind::Csk, Some(x)) => s.push_str(&format!(" {}DEG", x.0)),
                _ => {}
            }
        }
        s
    }
}

/// `invalid_param` (`reason: spec_syntax`): where the spec stopped
/// reading, and what it expected there.
fn syntax(position: usize, expected: &str) -> Error {
    Error::new(
        Code::InvalidParam,
        format!("the spec doesn't read at byte {position}: expected {expected}"),
    )
    .with("reason", "spec_syntax")
    .with("position", position)
    .with("expected", expected)
}

/// A word of the spec and where it starts: runs of non-space, a comma,
/// `↧`, and `(...)` whole.
fn tokens(text: &str) -> Result<Vec<(usize, &str)>> {
    let mut out = Vec::new();
    let mut i = 0;
    let b = text.as_bytes();
    while i < b.len() {
        let c = text[i..].chars().next().expect("in bounds");
        if c.is_whitespace() {
            i += c.len_utf8();
        } else if c == ',' || c == '↧' {
            out.push((i, &text[i..i + c.len_utf8()]));
            i += c.len_utf8();
        } else if c == '(' {
            let mut depth = 0;
            let start = i;
            for (j, d) in text[i..].char_indices() {
                match d {
                    '(' => depth += 1,
                    ')' => {
                        depth -= 1;
                        if depth == 0 {
                            i = start + j + 1;
                            break;
                        }
                    }
                    _ => {}
                }
            }
            if depth != 0 {
                return Err(syntax(start, "a closing parenthesis"));
            }
            out.push((start, &text[start..i]));
        } else {
            let start = i;
            while i < b.len() {
                let d = text[i..].chars().next().expect("in bounds");
                if d.is_whitespace() || d == ',' || d == '↧' || d == '(' {
                    break;
                }
                i += d.len_utf8();
            }
            out.push((start, &text[start..i]));
        }
    }
    Ok(out)
}

fn is_number(s: &str) -> bool {
    !s.is_empty()
        && s.bytes().all(|b| b.is_ascii_digit() || b == b'.')
        && s.bytes().filter(|&b| b == b'.').count() <= 1
        && s != "."
}

struct Reader<'a> {
    toks: Vec<(usize, &'a str)>,
    at: usize,
    end: usize,
}

impl<'a> Reader<'a> {
    fn peek(&self) -> Option<&'a str> {
        self.toks.get(self.at).map(|t| t.1)
    }
    fn pos(&self) -> usize {
        self.toks.get(self.at).map_or(self.end, |t| t.0)
    }
    fn is(&self, word: &str) -> bool {
        self.peek().is_some_and(|t| t.eq_ignore_ascii_case(word))
    }
    fn take(&mut self) -> Option<&'a str> {
        let t = self.peek()?;
        self.at += 1;
        Some(t)
    }

    /// A length: a number with an optional unit word, or `(...)`.
    fn len(&mut self) -> Option<Len> {
        let t = self.peek()?;
        if t.starts_with('(') {
            self.at += 1;
            return Some(Len(t.to_owned()));
        }
        if !is_number(t) {
            // `0.25in`: a number with its unit run on.
            let split = t.find(|c: char| !(c.is_ascii_digit() || c == '.'))?;
            let (n, u) = t.split_at(split);
            if is_number(n) && Unit::parse(u).is_some() {
                self.at += 1;
                return Some(Len(format!("{n} {u}")));
            }
            return None;
        }
        self.at += 1;
        if let Some(u) = self.peek().filter(|u| Unit::parse(u).is_some()) {
            self.at += 1;
            return Some(Len(format!("{t} {u}")));
        }
        Some(Len(t.to_owned()))
    }

    /// Whether an angle comes next (so a length doesn't take its number).
    fn angle_next(&mut self) -> bool {
        let save = self.at;
        let found = self.angle().is_some();
        self.at = save;
        found
    }

    /// An angle: `118DEG`, `118°`, `118 DEG`, or `(expression)DEG`.
    fn angle(&mut self) -> Option<Len> {
        let t = self.peek()?;
        let deg_next = self
            .toks
            .get(self.at + 1)
            .is_some_and(|x| x.1.eq_ignore_ascii_case("DEG"));
        if t.starts_with('(') && deg_next {
            self.at += 2;
            return Some(Len(t.to_owned()));
        }
        let upper = t.to_ascii_uppercase();
        let n = upper.strip_suffix("DEG").or_else(|| {
            t.strip_suffix('°')
                .map(|_| &upper[..upper.len() - '°'.len_utf8()])
        });
        match n {
            Some(n) if is_number(n) => {
                self.at += 1;
                Some(Len(t[..n.len()].to_owned()))
            }
            _ if is_number(t)
                && self
                    .toks
                    .get(self.at + 1)
                    .is_some_and(|x| x.1.eq_ignore_ascii_case("DEG")) =>
            {
                self.at += 2;
                Some(Len(t.to_owned()))
            }
            _ => None,
        }
    }
}

impl Spec {
    pub fn parse(text: &str) -> Result<Spec> {
        let mut r = Reader {
            toks: tokens(text)?,
            at: 0,
            end: text.len(),
        };
        let what = Self::what(&mut r)?;
        let tapped = matches!(what, What::Thread { .. });
        let mut starts = None;
        if let (Some(n), Some(w)) = (r.peek(), r.toks.get(r.at + 1))
            && tapped
            && n.bytes().all(|b| b.is_ascii_digit())
            && w.1.eq_ignore_ascii_case("STARTS")
        {
            starts = Some(
                n.parse::<u32>()
                    .map_err(|_| syntax(r.pos(), "a number of starts"))?,
            );
            r.at += 2;
        }
        let depth = if r.is("THRU") {
            r.at += 1;
            Some(Depth::Thru)
        } else if tapped && r.is("FULL") {
            r.at += 1;
            Some(Depth::Full)
        } else if r.is("↧") {
            r.at += 1;
            let p = r.pos();
            Some(Depth::Deep(r.len().ok_or_else(|| syntax(p, "a depth"))?))
        } else {
            let save = r.at;
            match r.len() {
                Some(l) if r.is("DEEP") => {
                    r.at += 1;
                    Some(Depth::Deep(l))
                }
                Some(_) => {
                    r.at = save;
                    None
                }
                None => None,
            }
        };
        let mut angle = None;
        if !tapped && matches!(depth, Some(Depth::Deep(_))) {
            angle = r.angle();
        }
        let (mut drill, mut mouth) = (None, None);
        while r.is(",") {
            r.at += 1;
            let p = r.pos();
            if r.is("DRILL")
                && drill.is_none()
                && mouth.is_none()
                && !matches!(what, What::Dia { .. })
            {
                r.at += 1;
                let mut d = Drill {
                    diameter: None,
                    depth: None,
                };
                let save = r.at;
                let first = if r.angle_next() { None } else { r.len() };
                if let Some(l) = first {
                    if tapped && r.is("DEEP") {
                        r.at += 1;
                        d.depth = Some(l);
                    } else {
                        d.diameter = Some(l);
                        let save2 = r.at;
                        match r.len() {
                            Some(l2) if tapped && r.is("DEEP") => {
                                r.at += 1;
                                d.depth = Some(l2);
                            }
                            _ => r.at = save2,
                        }
                    }
                } else {
                    r.at = save;
                }
                if tapped {
                    angle = r.angle();
                }
                if d.diameter.is_some() || d.depth.is_some() {
                    drill = Some(d);
                } else if angle.is_none() {
                    return Err(syntax(r.pos(), "a drill's diameter, depth or angle"));
                }
            } else if (r.is("CBORE") || r.is("CSK")) && mouth.is_none() {
                let kind = if r.is("CBORE") {
                    MouthKind::Cbore
                } else {
                    MouthKind::Csk
                };
                if let What::Fastener { mouth: Some(m), .. } = &what
                    && *m != kind
                {
                    return Err(syntax(p, "the fastener's own mouth"));
                }
                r.at += 1;
                // A countersink's angle alone is no diameter.
                let first = if kind == MouthKind::Csk && r.angle_next() {
                    None
                } else {
                    r.len()
                };
                let (diameter, second) = match kind {
                    // `CBORE 6.8 DEEP` is a depth alone.
                    MouthKind::Cbore if first.is_some() && r.is("DEEP") => {
                        r.at += 1;
                        (None, first)
                    }
                    MouthKind::Cbore => {
                        let save = r.at;
                        let depth = match r.len() {
                            Some(l) if r.is("DEEP") => {
                                r.at += 1;
                                Some(l)
                            }
                            _ => {
                                r.at = save;
                                None
                            }
                        };
                        (first, depth)
                    }
                    MouthKind::Csk => (first, r.angle()),
                };
                mouth = Some(Mouth {
                    kind,
                    diameter,
                    second,
                });
            } else {
                return Err(syntax(p, "DRILL, CBORE or CSK"));
            }
        }
        if r.peek().is_some() {
            return Err(syntax(r.pos(), "a comma, or the end"));
        }
        Ok(Spec {
            what,
            starts,
            depth,
            angle,
            drill,
            mouth,
        })
    }

    fn what(r: &mut Reader) -> Result<What> {
        let p = r.pos();
        let Some(t) = r.take() else {
            return Err(syntax(p, "a thread, CBORE, CSK, CLEAR, THREAD or DIA"));
        };
        let up = t.to_ascii_uppercase();
        // Suffixes `-<class>` and `-LH`, run on to a thread's word.
        let suffixes = |rest: &str, p: usize| -> Result<(Option<String>, bool)> {
            let mut class = None;
            let mut left = false;
            for (i, part) in rest.split('-').enumerate().skip(1) {
                if part.eq_ignore_ascii_case("LH") {
                    left = true;
                } else if i == 1 && !part.is_empty() {
                    class = Some(normal_class(part));
                } else {
                    return Err(syntax(p, "a class, or LH"));
                }
            }
            Ok((class, left))
        };
        match up.as_str() {
            "CBORE" | "CSK" | "CLEAR" => {
                let mouth = match up.as_str() {
                    "CBORE" => Some(MouthKind::Cbore),
                    "CSK" => Some(MouthKind::Csk),
                    _ => None,
                };
                let p = r.pos();
                let standard = r
                    .take()
                    .ok_or_else(|| syntax(p, "a standard"))?
                    .to_ascii_lowercase();
                let p = r.pos();
                let size = r.take().ok_or_else(|| syntax(p, "a size"))?.to_owned();
                let fit = r
                    .peek()
                    .and_then(|f| FastenerFit::parse(&f.to_ascii_lowercase()));
                if fit.is_some() {
                    r.at += 1;
                }
                Ok(What::Fastener {
                    mouth,
                    standard,
                    size,
                    fit: fit.filter(|f| *f != FastenerFit::Normal),
                })
            }
            "DIA" => Self::dia(r),
            "THREAD" => {
                let p = r.pos();
                let standard = r
                    .take()
                    .ok_or_else(|| syntax(p, "a standard"))?
                    .to_ascii_lowercase();
                let p = r.pos();
                let size = r.take().ok_or_else(|| syntax(p, "a size"))?.to_owned();
                let mut class = None;
                if r.is("CLASS") {
                    r.at += 1;
                    let p = r.pos();
                    class = Some(r.take().ok_or_else(|| syntax(p, "a class"))?.to_owned());
                }
                let left = r.is("LH");
                if left {
                    r.at += 1;
                }
                Ok(What::Thread {
                    kind: ThreadKind::House(standard),
                    size,
                    pitch: None,
                    class,
                    left,
                })
            }
            _ if up.starts_with('Ø') || up.starts_with('⌀') => {
                let rest = &t[t.chars().next().expect("non-empty").len_utf8()..];
                if rest.is_empty() {
                    return Self::dia(r);
                }
                // `Ø6`: the number is this token's rest.
                r.at -= 1;
                r.toks[r.at] = (r.toks[r.at].0 + t.len() - rest.len(), rest);
                Self::dia(r)
            }
            _ if up.starts_with('M') && up[1..].starts_with(|c: char| c.is_ascii_digit()) => {
                let (head, rest) = t.split_once('-').map_or((t, ""), |(h, x)| (h, x));
                let rest = if rest.is_empty() {
                    String::new()
                } else {
                    format!("-{rest}")
                };
                let (size, pitch) = match head[1..].split_once(['x', 'X']) {
                    Some((d, pt)) => {
                        if !is_number(pt) {
                            return Err(syntax(p + 1 + d.len() + 1, "a pitch"));
                        }
                        (format!("M{d}"), Some(Len(pt.to_owned())))
                    }
                    None => (format!("M{}", &head[1..]), None),
                };
                let (class, left) = suffixes(&rest, p)?;
                Ok(What::Thread {
                    kind: ThreadKind::Metric,
                    size,
                    pitch,
                    class,
                    left,
                })
            }
            _ if ["RC", "RP", "G"].iter().any(|x| {
                up.starts_with(x) && up[x.len()..].starts_with(|c: char| c.is_ascii_digit())
            }) =>
            {
                let (head, rest) = t.split_once("-L").map_or((t, ""), |(h, _)| (h, "-LH"));
                let prefix = if up.starts_with("RC") {
                    "Rc"
                } else if up.starts_with("RP") {
                    "Rp"
                } else {
                    "G"
                };
                let size = format!("{prefix}{}", &head[prefix.len()..]);
                let (_, left) = suffixes(rest, p)?;
                Ok(What::Thread {
                    kind: ThreadKind::British,
                    size,
                    pitch: None,
                    class: None,
                    left,
                })
            }
            _ if t.contains('-') => {
                let (size, tpi) = t.rsplit_once('-').expect("has a dash");
                if !is_number(tpi) {
                    return Err(syntax(p + size.len() + 1, "threads per inch"));
                }
                let p = r.pos();
                let word = r
                    .take()
                    .ok_or_else(|| syntax(p, "UNC, UNF, UNEF, UN, NPT, NPTF or NPSM"))?;
                let (series, rest) = word
                    .split_once('-')
                    .map_or((word, String::new()), |(s, x)| (s, format!("-{x}")));
                let series = series.to_ascii_uppercase();
                let kind = match series.as_str() {
                    "UNC" | "UNF" | "UNEF" | "UN" => ThreadKind::Unified(series),
                    "NPT" | "NPTF" | "NPSM" => ThreadKind::Pipe(series),
                    _ => return Err(syntax(p, "UNC, UNF, UNEF, UN, NPT, NPTF or NPSM")),
                };
                let (class, left) = suffixes(&rest, p)?;
                Ok(What::Thread {
                    kind,
                    size: size.to_owned(),
                    pitch: Some(Len(tpi.to_owned())),
                    class,
                    left,
                })
            }
            _ => Err(syntax(p, "a thread, CBORE, CSK, CLEAR, THREAD or DIA")),
        }
    }

    fn dia(r: &mut Reader) -> Result<What> {
        let p = r.pos();
        let diameter = r.len().ok_or_else(|| syntax(p, "a length"))?;
        let class = r
            .peek()
            .filter(|c| {
                ryu_standards::fit::Class::parse(c)
                    .is_some_and(|c| c.member == ryu_standards::fit::Member::Hole)
            })
            .map(str::to_owned);
        if class.is_some() {
            r.at += 1;
        }
        Ok(What::Dia { diameter, class })
    }

    /// The hole fields the spec names, as `feature.add` lays them over the
    /// rest: `""` for a thread, fastener or fit it replaces. The second
    /// list names the fields it sets only by default (a derived drill's
    /// `diameter: ""`, a pointless depth's `bottom: "flat"`): given beside
    /// the spec, those are not usage, and the given value wins.
    pub fn apply(&self, project: Unit) -> Result<(Map<String, Value>, Vec<&'static str>)> {
        let cu = context(&self.what, project);
        let mut f = Map::new();
        let mut form = "simple";
        match &self.what {
            What::Thread {
                kind,
                size,
                pitch,
                class,
                left,
            } => {
                let standard = match kind {
                    ThreadKind::Metric => "iso_metric".to_owned(),
                    ThreadKind::Unified(w) => w.to_ascii_lowercase(),
                    ThreadKind::Pipe(w) if w == "NPSM" => "nps".to_owned(),
                    ThreadKind::Pipe(w) => w.to_ascii_lowercase(),
                    ThreadKind::British if size.starts_with('G') => "bspp".to_owned(),
                    ThreadKind::British => "bspt".to_owned(),
                    ThreadKind::House(s) => s.clone(),
                };
                let mut t = json!({"standard": standard, "size": size});
                // Defaults go unwritten: the size's own pitch and the
                // standard's class.
                let builtin = Standard::parse(&standard)
                    .and_then(|s| ryu_standards::thread::lookup(s, size, None, None).ok());
                if let (Some(p), Some(b)) = (pitch, &builtin) {
                    let own = b.pitch.value.text();
                    let same = match kind {
                        ThreadKind::Metric => {
                            let nm = |x: &str| {
                                Quantity::parse(&format!("{x} mm"), Unit::Mm)
                                    .map(|q| (q.meters() * 1e9).round() as i64)
                            };
                            nm(&p.0).ok() == nm(&own).ok()
                        }
                        _ => p.0 == own,
                    };
                    if !same {
                        match kind {
                            ThreadKind::Metric => t["pitch"] = json!(format!("{} mm", p.0)),
                            _ => t["tpi"] = json!(p.0),
                        }
                    }
                }
                if let Some(c) = class
                    && builtin.as_ref().and_then(|b| b.class.as_deref()) != Some(c.as_str())
                {
                    t["class"] = json!(c);
                }
                if *left {
                    t["hand"] = json!("left");
                }
                if let Some(n) = self.starts {
                    t["starts"] = json!(n);
                }
                match &self.depth {
                    Some(Depth::Full) => t["depth"] = json!("full"),
                    Some(Depth::Deep(l)) => t["depth"] = json!(l.field(cu)),
                    Some(Depth::Thru) => {
                        f.insert("depth".into(), json!("through_all"));
                    }
                    None => {}
                }
                f.insert("thread".into(), t);
                f.insert("fastener".into(), json!(""));
                f.insert("fit".into(), json!(""));
            }
            What::Fastener {
                mouth,
                standard,
                size,
                fit,
            } => {
                let mut x = json!({"standard": standard, "size": size});
                if let Some(fit) = fit {
                    x["fit"] = json!(fit.key());
                }
                f.insert("fastener".into(), x);
                f.insert("thread".into(), json!(""));
                form = match mouth {
                    Some(MouthKind::Cbore) => "counterbore",
                    Some(MouthKind::Csk) => "countersink",
                    None => "simple",
                };
            }
            What::Dia { diameter, class } => {
                f.insert("diameter".into(), json!(diameter.field(cu)));
                f.insert("thread".into(), json!(""));
                f.insert("fastener".into(), json!(""));
                f.insert("fit".into(), json!(class.clone().unwrap_or_default()));
            }
        }
        let tapped = matches!(self.what, What::Thread { .. });
        if !tapped {
            match &self.depth {
                Some(Depth::Thru) => {
                    f.insert("depth".into(), json!("through_all"));
                }
                Some(Depth::Deep(l)) => {
                    f.insert("depth".into(), json!({"blind": l.field(cu)}));
                }
                _ => {}
            }
        }
        if let Some(d) = &self.drill {
            if let Some(x) = &d.diameter {
                f.insert("diameter".into(), json!(x.field(cu)));
            }
            if let Some(x) = &d.depth {
                f.insert("depth".into(), json!({"blind": x.field(cu)}));
            }
        }
        let mut defaults = Vec::new();
        // A derived drill: a diameter written before goes.
        if matches!(self.what, What::Thread { .. } | What::Fastener { .. })
            && !f.contains_key("diameter")
        {
            f.insert("diameter".into(), json!(""));
            defaults.push("diameter");
        }
        match &self.angle {
            Some(a) => {
                f.insert("bottom".into(), json!({"drill": a.angle_field()}));
            }
            // A depth named with no point is flat.
            None if f.contains_key("depth") => {
                f.insert("bottom".into(), json!("flat"));
                defaults.push("bottom");
            }
            None => {}
        }
        if let Some(m) = &self.mouth {
            let (key, name) = match m.kind {
                MouthKind::Cbore => ("counterbore", "depth"),
                MouthKind::Csk => ("countersink", "angle"),
            };
            form = key;
            let mut o = json!({});
            if let Some(d) = &m.diameter {
                o["diameter"] = json!(d.field(cu));
            }
            if let Some(s) = &m.second {
                o[name] = json!(if m.kind == MouthKind::Csk {
                    s.angle_field()
                } else {
                    s.field(cu)
                });
            }
            f.insert(key.into(), o);
        } else if form != "simple" {
            f.insert(form.into(), json!({}));
        }
        f.insert("form".into(), json!(form));
        Ok((f, defaults))
    }
}

/// A class as specs write it: ISO's grade then an upper-case position
/// (`6H`), unified's number then `B` (`2B`).
fn normal_class(c: &str) -> String {
    c.to_ascii_uppercase()
}

/// Textconv's line for a hole's thread, as its file holds it: its
/// designation, its depth (`full` when none), and its tap drill and
/// starts when not the default. `None` when it isn't a thread's object.
pub fn thread_line(t: &Value) -> Option<String> {
    use ryu_standards::thread::{Pitch, lookup};
    let standard = t.get("standard")?.as_str()?;
    let size = t.get("size")?.as_str()?;
    let built = Standard::parse(standard);
    let pitch = built.and_then(|b| {
        let written = t
            .get("pitch")
            .and_then(Value::as_str)
            .and_then(|p| Quantity::parse(p, Unit::Mm).ok())
            .map(|q| Pitch::Mm((q.meters() * 1e9).round() as i64));
        let tpi = t
            .get("tpi")
            .and_then(|n| {
                n.as_str()
                    .map(str::to_owned)
                    .or(n.as_u64().map(|x| x.to_string()))
            })
            .and_then(|n| n.parse::<f64>().ok())
            .map(|n| {
                if b.is_pipe() {
                    Pitch::Tpi10((n * 10.0).round() as u32)
                } else {
                    Pitch::Tpi(n.round() as u16)
                }
            });
        lookup(b, size, written.or(tpi), None)
            .ok()
            .map(|x| x.pitch.value)
    });
    let class = t
        .get("class")
        .and_then(Value::as_str)
        .map(str::to_owned)
        .or_else(|| {
            built
                .and_then(|b| lookup(b, size, pitch, None).ok())
                .and_then(|x| x.class)
        });
    let left = t.get("hand").and_then(Value::as_str) == Some("left");
    let mut line = crate::hole::derive::designation(standard, size, pitch, class.as_deref(), left);
    line.push_str(", ");
    line.push_str(t.get("depth").and_then(Value::as_str).unwrap_or("full"));
    match t.get("drill") {
        Some(Value::String(s)) if s == "roll_form" => line.push_str(", roll form"),
        Some(d) => {
            if let Some(e) = d.get("engagement").and_then(Value::as_u64) {
                line.push_str(&format!(", engagement {e}%"));
            }
        }
        None => {}
    }
    if let Some(n) = t.get("starts").and_then(Value::as_u64) {
        line.push_str(&format!(", {n} starts"));
    }
    Some(line)
}

/// Textconv's line for a hole's fastener: `HOUSE_CAP M5, close`.
pub fn fastener_line(f: &Value) -> Option<String> {
    let standard = f.get("standard")?.as_str()?;
    let size = f.get("size")?.as_str()?;
    let fit = f.get("fit").and_then(Value::as_str).unwrap_or("normal");
    Some(format!("{} {size}, {fit}", standard.to_uppercase()))
}

/// A hole feature's canonical spec, from its file's JSON in a project of
/// `unit`; `None` when it isn't a hole or doesn't read alone (one reading
/// parameters, say).
pub fn of_json(v: &Value, unit: Unit) -> Option<String> {
    match crate::doc::files::Feature::from_json(v, unit, &crate::expr::Env::empty()).ok()? {
        crate::doc::files::Feature::Hole(h) => Some(canonical(&h, unit)),
        _ => None,
    }
}

/// The canonical spec of a hole.
pub fn canonical(h: &Hole, project: Unit) -> String {
    Spec::of(h, project).text()
}

/// A spec of any shape the grammar holds, from `seed` (xorshift):
/// every one `text` writes and `parse` reads back.
/// The fuzz target `hole_spec` and the identity test use it.
#[doc(hidden)]
pub fn arbitrary(seed: &mut u64) -> Spec {
    let mut next = |n: usize| {
        *seed ^= *seed << 13;
        *seed ^= *seed >> 7;
        *seed ^= *seed << 17;
        (*seed % n as u64) as usize
    };
    fn pick(next: &mut dyn FnMut(usize) -> usize, xs: &[&str]) -> String {
        xs[next(xs.len())].to_owned()
    }
    const LENS: [&str; 7] = ["6", "0.5", "12.25", ".5", "0.25 in", "(2 * d)", "20"];
    const ANGLES: [&str; 3] = ["118", "90", "(a + 1)"];
    let len = |next: &mut dyn FnMut(usize) -> usize| Len(pick(next, &LENS));
    let opt = |next: &mut dyn FnMut(usize) -> usize| next(2) == 0;
    let what = match next(7) {
        0 => What::Thread {
            kind: ThreadKind::Metric,
            size: pick(&mut next, &["M6", "M8", "M1.6"]),
            pitch: opt(&mut next).then(|| Len(pick(&mut next, &["1.0", "1.25", "0.35"]))),
            class: opt(&mut next).then(|| pick(&mut next, &["6H", "5G"])),
            left: opt(&mut next),
        },
        1 => What::Thread {
            kind: ThreadKind::Unified(pick(&mut next, &["UNC", "UNF", "UNEF", "UN"])),
            size: pick(&mut next, &["1/4", "#10", "1-1/2"]),
            pitch: Some(Len(pick(&mut next, &["20", "32", "6"]))),
            class: opt(&mut next).then(|| pick(&mut next, &["2B", "3B"])),
            left: opt(&mut next),
        },
        2 => What::Thread {
            kind: ThreadKind::Pipe(pick(&mut next, &["NPT", "NPTF", "NPSM"])),
            size: pick(&mut next, &["1/8", "1"]),
            pitch: Some(Len(pick(&mut next, &["27", "11.5"]))),
            class: None,
            left: opt(&mut next),
        },
        3 => What::Thread {
            kind: ThreadKind::British,
            size: pick(&mut next, &["Rc1/4", "Rp1/2", "G3/8"]),
            pitch: None,
            class: None,
            left: opt(&mut next),
        },
        4 => What::Thread {
            kind: ThreadKind::House(pick(&mut next, &["house_acme", "t2"])),
            size: pick(&mut next, &["T10", "X-1"]),
            pitch: None,
            class: opt(&mut next).then(|| pick(&mut next, &["2G", "6H"])),
            left: opt(&mut next),
        },
        5 => What::Fastener {
            mouth: [None, Some(MouthKind::Cbore), Some(MouthKind::Csk)][next(3)],
            standard: pick(&mut next, &["iso4762", "house_cap"]),
            size: pick(&mut next, &["M5", "#10"]),
            fit: [None, Some(FastenerFit::Close), Some(FastenerFit::Loose)][next(3)],
        },
        _ => What::Dia {
            diameter: len(&mut next),
            class: opt(&mut next).then(|| pick(&mut next, &["H7", "G6"])),
        },
    };
    let tapped = matches!(what, What::Thread { .. });
    let starts = (tapped && opt(&mut next)).then(|| 2 + next(4) as u32);
    let depth = match next(if tapped { 4 } else { 3 }) {
        0 => None,
        1 => Some(Depth::Thru),
        2 => Some(Depth::Deep(len(&mut next))),
        _ => Some(Depth::Full),
    };
    let angle = (opt(&mut next) && (tapped || matches!(depth, Some(Depth::Deep(_)))))
        .then(|| Len(pick(&mut next, &ANGLES)));
    let drill = match &what {
        What::Dia { .. } => None,
        _ if opt(&mut next) => None,
        What::Fastener { .. } => Some(Drill {
            diameter: Some(len(&mut next)),
            depth: None,
        }),
        _ => {
            let diameter = opt(&mut next).then(|| len(&mut next));
            let depth = (diameter.is_none() || opt(&mut next)).then(|| len(&mut next));
            Some(Drill { diameter, depth })
        }
    };
    let kind = match &what {
        What::Fastener { mouth: None, .. } => None,
        What::Fastener { mouth: Some(k), .. } => opt(&mut next).then_some(*k),
        _ => [None, Some(MouthKind::Cbore), Some(MouthKind::Csk)][next(3)],
    };
    let mouth = kind.map(|kind| Mouth {
        kind,
        diameter: opt(&mut next).then(|| len(&mut next)),
        second: opt(&mut next).then(|| match kind {
            MouthKind::Cbore => len(&mut next),
            MouthKind::Csk => Len(pick(&mut next, &ANGLES)),
        }),
    });
    Spec {
        what,
        starts,
        depth,
        angle,
        drill,
        mouth,
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use crate::hole::derive::tests::hole;

    fn text(fields: serde_json::Value) -> String {
        Spec::of(&hole(fields), Unit::Mm).text()
    }

    #[test]
    fn canonical_specs_of_holes() {
        assert_eq!(
            text(json!({"thread": {"standard": "iso_metric", "size": "M6"}})),
            "M6x1.0-6H THRU"
        );
        assert_eq!(
            text(
                json!({"thread": {"standard": "iso_metric", "size": "M8", "depth": "16 mm"},
                        "depth": {"blind": "20 mm"}, "bottom": {"drill": "118 deg"}})
            ),
            "M8x1.25-6H 16 DEEP, DRILL 20 DEEP 118DEG"
        );
        assert_eq!(
            text(
                json!({"thread": {"standard": "iso_metric", "size": "M8", "depth": "16 mm"},
                        "diameter": "6.8 mm", "depth": {"blind": "20 mm"}, "bottom": {"drill": "118 deg"}})
            ),
            "M8x1.25-6H 16 DEEP, DRILL 6.8 20 DEEP 118DEG"
        );
        assert_eq!(
            text(json!({"thread": {"standard": "unc", "size": "1/4"}})),
            "1/4-20 UNC-2B THRU"
        );
        assert_eq!(
            text(json!({"thread": {"standard": "npt", "size": "1/8"}})),
            "1/8-27 NPT THRU"
        );
        assert_eq!(
            text(json!({"thread": {"standard": "bspp", "size": "G1/4", "hand": "left"}})),
            "G1/4-LH THRU"
        );
        assert_eq!(
            text(
                json!({"fastener": {"standard": "iso4762", "size": "M5"}, "form": "counterbore",
                        "counterbore": {}})
            ),
            "CBORE ISO4762 M5 THRU"
        );
        assert_eq!(
            text(json!({"diameter": "6 mm", "fit": "H7"})),
            "DIA 6 H7 THRU"
        );
        assert_eq!(
            text(json!({"diameter": "6.6 mm", "form": "counterbore",
                        "counterbore": {"diameter": "11 mm", "depth": "6.8 mm"}})),
            "DIA 6.6 THRU, CBORE 11 6.8 DEEP"
        );
        // An expression's case is in Task 23, whose holes read `d`.
        assert_eq!(
            text(
                json!({"thread": {"standard": "house_acme", "size": "T10", "class": "2G", "hand": "left", "starts": 2}})
            ),
            "THREAD HOUSE_ACME T10 CLASS 2G LH 2 STARTS THRU"
        );
    }

    #[test]
    fn every_example_parses_to_its_canonical_text() {
        for (written, canonical) in [
            ("M6x1.0-6H THRU", "M6x1.0-6H THRU"),
            ("m6x1-6h thru", "M6x1-6H THRU"),
            ("M6 THRU", "M6 THRU"),
            (
                "M8x1.25-6H 16 DEEP, DRILL 6.8 20 DEEP 118DEG",
                "M8x1.25-6H 16 DEEP, DRILL 6.8 20 DEEP 118DEG",
            ),
            (
                "M8x1.25-6H ↧16, DRILL 20 DEEP 118°",
                "M8x1.25-6H 16 DEEP, DRILL 20 DEEP 118DEG",
            ),
            ("1/4-20 UNC-2B THRU", "1/4-20 UNC-2B THRU"),
            ("#10-32 unf thru", "#10-32 UNF THRU"),
            ("1-1/2-6 UNC-2B-LH THRU", "1-1/2-6 UNC-2B-LH THRU"),
            ("1/8-27 NPT THRU", "1/8-27 NPT THRU"),
            ("1-11.5 NPT", "1-11.5 NPT"),
            ("Rc1/4", "Rc1/4"),
            ("G1/4-LH THRU", "G1/4-LH THRU"),
            ("CBORE ISO4762 M5", "CBORE ISO4762 M5"),
            (
                "csk house_flat M5 close thru",
                "CSK HOUSE_FLAT M5 CLOSE THRU",
            ),
            ("DIA 6 H7 THRU", "DIA 6 H7 THRU"),
            ("Ø6 H7 THRU", "DIA 6 H7 THRU"),
            ("DIA 0.25 in THRU", "DIA 0.25 in THRU"),
            (
                "DIA 6.6 THRU, CBORE 11 6.8 DEEP",
                "DIA 6.6 THRU, CBORE 11 6.8 DEEP",
            ),
            ("DIA (d + 1) 12 DEEP 118DEG", "DIA (d + 1) 12 DEEP 118DEG"),
            (
                "THREAD house_acme T10 CLASS 2G LH 2 STARTS THRU",
                "THREAD HOUSE_ACME T10 CLASS 2G LH 2 STARTS THRU",
            ),
        ] {
            let s = Spec::parse(written).unwrap_or_else(|e| panic!("{written}: {e}"));
            assert_eq!(s.text(), canonical, "{written}");
        }
    }

    #[test]
    fn a_bad_spec_says_where_and_what() {
        for (written, position, expected) in [
            ("M6x", 3, "a pitch"),
            ("M6x1.0-6H THRU DEEP", 15, "a comma, or the end"),
            ("1/4-20 UNX", 7, "UNC, UNF, UNEF, UN, NPT, NPTF or NPSM"),
            ("CBORE ISO4762", 13, "a size"),
            ("DIA THRU", 4, "a length"),
            ("M6, DRILL 5 DEEP DEEP", 17, "a comma, or the end"),
            ("", 0, "a thread, CBORE, CSK, CLEAR, THREAD or DIA"),
        ] {
            let e = Spec::parse(written).unwrap_err();
            assert_eq!(
                (
                    e.code,
                    e.details["reason"].as_str(),
                    e.details["position"].as_u64(),
                    e.details["expected"].as_str()
                ),
                (
                    Code::InvalidParam,
                    Some("spec_syntax"),
                    Some(position),
                    Some(expected)
                ),
                "{written}: {e}"
            );
        }
    }

    #[test]
    fn a_spec_sets_the_fields_it_names() {
        let f = Spec::parse("M8x1.25-6H 16 DEEP, DRILL 20 DEEP 118DEG")
            .unwrap()
            .apply(Unit::Mm)
            .unwrap()
            .0;
        assert_eq!(
            f["thread"],
            json!({"standard": "iso_metric", "size": "M8", "depth": "16 mm"})
        );
        assert_eq!(f["depth"], json!({"blind": "20 mm"}));
        assert_eq!(f["bottom"], json!({"drill": "118 deg"}));
        assert_eq!(
            (&f["form"], &f["fastener"], &f["fit"]),
            (&json!("simple"), &json!(""), &json!(""))
        );
        assert_eq!(
            f["diameter"],
            json!(""),
            "the drill is derived: a written one goes"
        );
        let f = Spec::parse("M8x1.0-5H THRU")
            .unwrap()
            .apply(Unit::Mm)
            .unwrap()
            .0;
        assert_eq!(
            f["thread"],
            json!({"standard": "iso_metric", "size": "M8", "pitch": "1.0 mm", "class": "5H"})
        );
        let f = Spec::parse("CBORE ISO4762 M5 CLOSE")
            .unwrap()
            .apply(Unit::Mm)
            .unwrap()
            .0;
        assert_eq!(
            f["fastener"],
            json!({"standard": "iso4762", "size": "M5", "fit": "close"})
        );
        assert_eq!(
            (&f["form"], &f["counterbore"]),
            (&json!("counterbore"), &json!({}))
        );
        assert!(f.get("depth").is_none(), "no depth named");
        let f = Spec::parse("1/4-20 UNC-2B .5 DEEP, DRILL .625 DEEP")
            .unwrap()
            .apply(Unit::Mm)
            .unwrap()
            .0;
        assert_eq!(f["thread"]["depth"], ".5 in");
        assert_eq!(f["depth"], json!({"blind": ".625 in"}));
    }

    /// Seeded specs of every shape: parse ∘ text is the identity.
    #[test]
    fn parse_of_text_is_the_identity() {
        let mut seed = 0x2545_f491_4f6c_dd1du64;
        for _ in 0..20_000 {
            let s = arbitrary(&mut seed);
            let t = s.text();
            assert_eq!(
                Spec::parse(&t).unwrap_or_else(|e| panic!("{t}: {e}")),
                s,
                "{t}"
            );
        }
    }
}
