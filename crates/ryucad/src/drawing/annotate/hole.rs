//! A hole's callout text (design 533, Drawings), from its feature and its
//! derived sizes, in the drawing's standard: a tapped hole's thread and
//! its depth, then its drill on its own line; any other hole's drill with
//! its fit; then one line each for its mouth (counterbore, countersink,
//! counterdrill, spotface), a near chamfer and a far side. A plain hole's
//! lines are what callouts always wrote. Hole tables write the same lines
//! with no count.

use super::text;
use crate::doc::drawing::Standard;
use crate::doc::hole::{Depth, Hole, HoleForm};
use crate::hole::derive::{FarSizes, FitSizes, Sizes, ThreadSizes};
use ryu_draw::symbol::{Run, Symbol};

/// How a count or a pair of sizes is crossed: `×` in ISO, `X` in ASME.
pub(crate) fn times(standard: Standard) -> &'static str {
    match standard {
        Standard::Asme => "X",
        Standard::Iso => "×",
    }
}

/// A thread's designation as a drawing writes it: an ISO metric thread's
/// pitch crossed as the standard crosses (`M6×1-6H`, `M8X1.25-6H`) and
/// without trailing zeros; every other standard's as the spec writes it
/// (`1/4-20 UNC-2B`, `1/8-27 NPT`).
pub(crate) fn designation(t: &ThreadSizes, standard: Standard) -> String {
    let d = &t.designation;
    if t.standard != "iso_metric" {
        return d.clone();
    }
    let Some((size, rest)) = d.split_once('x') else {
        return d.clone();
    };
    let (pitch, tail) = rest.split_at(rest.find('-').unwrap_or(rest.len()));
    let pitch = if pitch.contains('.') {
        pitch.trim_end_matches('0').trim_end_matches('.')
    } else {
        pitch
    };
    format!("{size}{}{pitch}{tail}", times(standard))
}

/// A bore's tolerance after its drill: its ISO 286 class, else its upper
/// and lower deviations (`+0.012/0`).
fn fit_text(f: &FitSizes, units: &text::Units) -> String {
    if let Some(c) = &f.class {
        return c.clone();
    }
    let signed = |v: f64| {
        let a = v.abs();
        let t = units.number(
            a,
            units.precision.max(text::needs(a / units.unit.factor())),
            false,
        );
        if v > 0.0 {
            format!("+{t}")
        } else if v < 0.0 {
            format!("-{t}")
        } else {
            t
        }
    };
    format!("{}/{}", signed(f.upper.si), signed(f.lower.si))
}

/// What hole `h` (its sizes `sizes`) calls out, line by line: `count` its
/// places, written `n×` before the first line when more than one.
pub(crate) fn lines(
    h: &Hole,
    sizes: &Sizes,
    units: &text::Units,
    standard: Standard,
    count: usize,
) -> Vec<Vec<Run>> {
    let x = times(standard);
    let len = |v: f64| {
        units.number(
            v,
            units.precision.max(text::needs(v / units.unit.factor())),
            false,
        )
    };
    let deg = |a: f64| format!("{}°", text::decimal(a.to_degrees(), 0, false));
    let depth = |d: &Depth| -> Vec<Run> {
        match d {
            Depth::ThroughAll => vec![Run::Text(" THRU".into())],
            Depth::Blind(q) | Depth::Tip(q) => vec![
                Run::Text(" ".into()),
                Run::Symbol(Symbol::Depth),
                Run::Text(len(q.meters())),
            ],
            Depth::UpTo { .. } | Depth::ThroughNext => Vec::new(),
        }
    };
    let mut out = Vec::new();
    let mut first = Vec::new();
    if count > 1 {
        first.push(Run::Text(format!("{count}{x} ")));
    }
    match &sizes.thread {
        Some(t) => {
            first.push(Run::Text(designation(t, standard)));
            match (&t.depth, &h.depth) {
                (Some(d), _) => first.extend([
                    Run::Text(" ".into()),
                    Run::Symbol(Symbol::Depth),
                    Run::Text(len(d.si)),
                ]),
                (None, Depth::ThroughAll) => first.push(Run::Text(" THRU".into())),
                (None, _) => {}
            }
            out.push(first);
            let mut drill = vec![
                Run::Symbol(Symbol::Diameter),
                Run::Text(len(sizes.drill.si)),
            ];
            drill.extend(depth(&h.depth));
            out.push(drill);
        }
        None => {
            first.push(Run::Symbol(Symbol::Diameter));
            first.push(Run::Text(len(sizes.drill.si)));
            if let Some(f) = &sizes.fit {
                first.push(Run::Text(format!(" {}", fit_text(f, units))));
            }
            first.extend(depth(&h.depth));
            out.push(first);
        }
    }
    let wide = |d: f64| {
        vec![
            Run::Text(" ".into()),
            Run::Symbol(Symbol::Diameter),
            Run::Text(len(d)),
        ]
    };
    match &h.form {
        HoleForm::Counterbore { .. } => {
            if let Some((d, depth)) = &sizes.counterbore {
                out.push(vec![
                    Run::Symbol(Symbol::Counterbore),
                    Run::Text(" ".into()),
                    Run::Symbol(Symbol::Diameter),
                    Run::Text(format!("{} ", len(d.si))),
                    Run::Symbol(Symbol::Depth),
                    Run::Text(len(depth.si)),
                ]);
            }
        }
        HoleForm::Countersink { .. } => {
            if let Some((d, a)) = &sizes.countersink {
                out.push(vec![
                    Run::Symbol(Symbol::Countersink),
                    Run::Text(" ".into()),
                    Run::Symbol(Symbol::Diameter),
                    Run::Text(format!("{} {x} {}", len(d.si), deg(a.si))),
                ]);
            }
        }
        HoleForm::Counterdrill { .. } => {
            if let Some((d, depth, a)) = &sizes.counterdrill {
                out.push(vec![
                    Run::Symbol(Symbol::Counterbore),
                    Run::Text(" ".into()),
                    Run::Symbol(Symbol::Diameter),
                    Run::Text(format!("{} ", len(d.si))),
                    Run::Symbol(Symbol::Depth),
                    Run::Text(format!("{} {x} {}", len(depth.si), deg(a.si))),
                ]);
            }
        }
        HoleForm::Spotface { .. } => {
            if let Some((d, depth)) = &sizes.spotface {
                let mut l = vec![Run::Text("SF".into())];
                l.extend(wide(d.si));
                if depth.si > 0.0 {
                    l.extend([
                        Run::Text(" ".into()),
                        Run::Symbol(Symbol::Depth),
                        Run::Text(len(depth.si)),
                    ]);
                }
                out.push(l);
            }
        }
        HoleForm::Simple => {}
    }
    if let Some((d, a)) = &sizes.near {
        out.push(vec![
            Run::Symbol(Symbol::Countersink),
            Run::Text(" ".into()),
            Run::Symbol(Symbol::Diameter),
            Run::Text(format!("{} {x} {}", len(d.si), deg(a.si))),
        ]);
    }
    match &sizes.far {
        Some(FarSizes::Countersink(d, a)) => out.push(vec![
            Run::Text("FAR SIDE ".into()),
            Run::Symbol(Symbol::Countersink),
            Run::Text(" ".into()),
            Run::Symbol(Symbol::Diameter),
            Run::Text(format!("{} {x} {}", len(d.si), deg(a.si))),
        ]),
        Some(FarSizes::Counterbore(d, depth)) => out.push(vec![
            Run::Text("FAR SIDE ".into()),
            Run::Symbol(Symbol::Counterbore),
            Run::Text(" ".into()),
            Run::Symbol(Symbol::Diameter),
            Run::Text(format!("{} ", len(d.si))),
            Run::Symbol(Symbol::Depth),
            Run::Text(len(depth.si)),
        ]),
        None => {}
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::hole::derive::{derive, tests::hole};
    use crate::units::Unit;
    use serde_json::{Value, json};

    fn units(unit: Unit, precision: u32, standard: Standard) -> text::Units {
        text::Units {
            unit,
            precision,
            standard,
        }
    }

    /// A hole from its spec (or its file's fields), at the latest format.
    fn of(spec_or_fields: Value) -> (Hole, Sizes) {
        let mut v = match spec_or_fields {
            Value::String(s) => Value::Object(
                crate::hole::spec::Spec::parse(&s)
                    .unwrap()
                    .apply(Unit::Mm)
                    .unwrap()
                    .0
                    .into_iter()
                    // `""` replaces a field: here there is none to.
                    .filter(|(k, x)| !(x.as_str() == Some("") || (k == "bottom" && x == "flat")))
                    .collect(),
            ),
            v => v,
        };
        v["format"] = json!(crate::doc::files::FEATURE_FORMAT);
        let h = hole(v);
        let s = derive(&h, None).unwrap();
        (h, s)
    }

    fn read(x: Value, u: text::Units, count: usize) -> Vec<String> {
        let (h, s) = of(x);
        lines(&h, &s, &u, u.standard, count)
            .iter()
            .map(|l| super::super::plain(l))
            .collect()
    }

    const ISO: Standard = Standard::Iso;
    const ASME: Standard = Standard::Asme;

    #[test]
    fn a_plain_hole_reads_as_it_always_has() {
        let u = units(Unit::Mm, 2, ASME);
        assert_eq!(read(json!("DIA 6 THRU"), u, 2), ["2X ⌀6.00 THRU"]);
        assert_eq!(
            read(
                json!({"diameter": "6.6 mm", "form": "counterbore",
                       "counterbore": {"diameter": "11 mm", "depth": "6.8 mm"}}),
                u,
                1
            ),
            ["⌀6.60 THRU", "⌴ ⌀11.00 ↧6.80"]
        );
        assert_eq!(
            read(
                json!({"diameter": "4 mm", "form": "countersink",
                       "countersink": {"diameter": "8 mm", "angle": "90 deg"},
                       "depth": {"blind": "8 mm"}}),
                units(Unit::Mm, 2, ISO),
                3
            ),
            ["3× ⌀4.00 ↧8.00", "⌵ ⌀8.00 × 90°"]
        );
    }

    #[test]
    fn a_tapped_hole_calls_out_its_thread_then_its_drill() {
        let blind = json!("M6x1.0-6H 10 DEEP, DRILL 14 DEEP 118DEG");
        assert_eq!(
            read(blind.clone(), units(Unit::Mm, 2, ISO), 4),
            ["4× M6×1-6H ↧10.00", "⌀5.00 ↧14.00"]
        );
        assert_eq!(
            read(blind, units(Unit::Mm, 2, ASME), 4),
            ["4X M6X1-6H ↧10.00", "⌀5.00 ↧14.00"]
        );
        assert_eq!(
            read(json!("M8x1.25-6H THRU"), units(Unit::Mm, 2, ISO), 1),
            ["M8×1.25-6H THRU", "⌀6.80 THRU"]
        );
        // Design 533's ASME example, in inches.
        assert_eq!(
            read(json!("1/4-20 UNC-2B THRU"), units(Unit::In, 3, ASME), 4),
            ["4X 1/4-20 UNC-2B THRU", "⌀.201 THRU"]
        );
    }

    #[test]
    fn a_fit_follows_the_drill_and_every_mouth_has_its_line() {
        let u = units(Unit::Mm, 2, ISO);
        assert_eq!(read(json!("DIA 6 H7 THRU"), u, 1), ["⌀6.00 H7 THRU"]);
        assert_eq!(
            read(json!("DIA 5 THRU, CDRILL 9 3 DEEP"), u, 1),
            ["⌀5.00 THRU", "⌴ ⌀9.00 ↧3.00 × 118°"]
        );
        assert_eq!(
            read(json!("DIA 5 THRU, NEAR 7"), u, 1),
            ["⌀5.00 THRU", "⌵ ⌀7.00 × 90°"]
        );
        assert_eq!(
            read(json!("DIA 4 THRU, FAR CSK 8 90DEG"), u, 1),
            ["⌀4.00 THRU", "FAR SIDE ⌵ ⌀8.00 × 90°"]
        );
        assert_eq!(
            read(
                json!({"diameter": "5 mm", "form": "spotface",
                       "spotface": {"diameter": "12 mm"}, "depth": {"blind": "8 mm"}}),
                u,
                1
            ),
            ["⌀5.00 ↧8.00", "SF ⌀12.00"]
        );
    }

    #[test]
    fn metric_designations_drop_trailing_zeros_and_take_the_standards_cross() {
        let (_, s) = of(json!("M6x1.0-6H THRU"));
        let mut t = s.thread.clone().unwrap();
        assert_eq!(designation(&t, ISO), "M6×1-6H");
        assert_eq!(designation(&t, ASME), "M6X1-6H");
        t.designation = "M6x1.0-6H-LH".into();
        assert_eq!(designation(&t, ISO), "M6×1-6H-LH");
        t.designation = "M1.6x0.35-6H".into();
        assert_eq!(designation(&t, ISO), "M1.6×0.35-6H");
        let (_, s) = of(json!("1/4-20 UNC-2B THRU"));
        assert_eq!(
            designation(s.thread.as_ref().unwrap(), ISO),
            "1/4-20 UNC-2B"
        );
    }
}
