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
use crate::units::{Quantity, Unit};
use ryu_standards::thread::Standard;

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
                    HoleForm::Simple => None,
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
            HoleForm::Simple => None,
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

/// The canonical spec of a hole.
pub fn canonical(h: &Hole, project: Unit) -> String {
    Spec::of(h, project).text()
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use crate::hole::derive::tests::hole;
    use serde_json::json;

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
}
