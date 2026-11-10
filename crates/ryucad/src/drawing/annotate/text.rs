//! What a dimension says: its value in the drawing's unit at its
//! precision (rounded half to even), its tolerance, and the runs of text
//! and symbols it draws.

use crate::doc::drawing::Standard;
use crate::doc::drawing_annotation::{Style, Tolerance};
use crate::error::{Code, Error, Result};
use crate::units::Unit;
use ryu_draw::symbol::{Run, Symbol};
use serde_json::{Value, json};

/// How a drawing shows lengths.
#[derive(Clone, Copy, Debug)]
pub(crate) struct Units {
    pub unit: Unit,
    /// Its decimals where an annotation says none.
    pub precision: u32,
    pub standard: Standard,
}

impl Units {
    /// `v` metres (or, for an angle, radians shown in degrees) as text at
    /// `places` decimals, rounded half to even; ASME drops an inch value's
    /// leading zero (`.500`).
    pub(crate) fn number(&self, v: f64, places: u32, angle: bool) -> String {
        let x = if angle {
            v.to_degrees()
        } else {
            v / self.unit.factor()
        };
        decimal(
            x,
            places,
            !angle && self.unit == Unit::In && self.standard == Standard::Asme,
        )
    }

    /// The decimals a value takes: the annotation's, else the drawing's
    /// (lengths) or none (angles).
    pub(crate) fn places(&self, style: &Style, angle: bool) -> u32 {
        style
            .precision
            .unwrap_or(if angle { 0 } else { self.precision })
    }
}

/// `x` at `places` decimals, rounded half to even, `-0` as `0`.
pub(crate) fn decimal(x: f64, places: u32, drop_lead: bool) -> String {
    let k = 10f64.powi(places as i32);
    let mut r = (x * k).round_ties_even() / k;
    if r == 0.0 {
        r = 0.0;
    }
    let mut s = format!("{r:.*}", places as usize);
    if drop_lead {
        if let Some(t) = s.strip_prefix("0.") {
            s = format!(".{t}");
        } else if let Some(t) = s.strip_prefix("-0.") {
            s = format!("-.{t}");
        }
    }
    s
}

/// The decimals `x` needs, at most 6: `0.005` three.
pub(crate) fn needs(x: f64) -> u32 {
    (0u32..=6)
        .find(|p| {
            let k = 10f64.powi(*p as i32);
            ((x * k).round() / k - x).abs() <= 1e-9 * x.abs().max(1e-9)
        })
        .unwrap_or(6)
}

/// A deviation's text: signed, `0` bare.
fn signed(u: &Units, v: f64, places: u32, angle: bool) -> String {
    let t = u.number(v, places, angle);
    if t.trim_start_matches(['-', '0', '.']).is_empty() {
        "0".to_owned()
    } else if v > 0.0 {
        format!("+{t}")
    } else {
        t
    }
}

/// What a dimension shows: the runs on its line, deviations stacked
/// after it (upper, lower), whether its text is boxed, and its value and
/// tolerance as `drawing.get` reports them.
#[derive(Clone, Debug, Default)]
pub(crate) struct Shown {
    pub runs: Vec<Run>,
    pub stack: Option<(String, String)>,
    pub boxed: bool,
    pub report: Value,
}

impl Shown {
    /// The runs as plain text, symbols by their signs.
    pub(crate) fn plain(&self) -> String {
        let mut s: String = self
            .runs
            .iter()
            .map(|r| match r {
                Run::Text(t) => t.clone(),
                Run::Symbol(Symbol::Diameter) => "⌀".into(),
                Run::Symbol(Symbol::Counterbore) => "⌴".into(),
                Run::Symbol(Symbol::Countersink) => "⌵".into(),
                Run::Symbol(Symbol::Depth) => "↧".into(),
                Run::Symbol(other) => format!("[{other:?}]"),
            })
            .collect();
        if let Some((a, b)) = &self.stack {
            s.push_str(&format!(" {a}/{b}"));
        }
        s
    }
}

/// The ISO 286 deviations of `fit` (a class, or a hole's and a shaft's)
/// at a nominal `size` in metres, in metres: `(class, upper, lower)` for
/// each class; `invalid_param` (`reason: fit_undefined`) where the
/// standard doesn't define one.
pub(crate) fn fit_deviations(fit: &str, size: f64) -> Result<Vec<(String, f64, f64)>> {
    use crate::drawing::fit::{Fit, deviations};
    let parsed = Fit::parse(fit).ok_or_else(|| {
        Error::new(Code::InvalidParam, format!("{fit:?} is no ISO 286 fit"))
            .with("reason", "fit_undefined")
    })?;
    let classes = match parsed {
        Fit::One(c) => vec![c],
        Fit::Pair(h, s) => vec![h, s],
    };
    let um = (size * 1e6).round();
    let mut out = Vec::new();
    for c in classes {
        let d = if um >= 1.0 {
            deviations(&c, um as u64)
        } else {
            None
        };
        let Some((upper, lower)) = d else {
            return Err(Error::new(
                Code::InvalidParam,
                format!(
                    "ISO 286 defines no {} at {} mm",
                    c.text(),
                    decimal(size * 1000.0, 3, false)
                ),
            )
            .with("reason", "fit_undefined")
            .with("fit", fit));
        };
        out.push((c.text(), upper as f64 * 1e-7, lower as f64 * 1e-7));
    }
    Ok(out)
}

/// What a dimension of `value` (metres, or radians when `angle`) with
/// `style` shows, `lead` before its value (`⌀`, `R`, `S⌀`).
pub(crate) fn shown(
    u: &Units,
    style: &Style,
    value: f64,
    angle: bool,
    lead: &[Run],
) -> Result<Shown> {
    let places = u.places(style, angle);
    let deg = if angle { "°" } else { "" };
    let num = |v: f64, p: u32| format!("{}{deg}", u.number(v, p, angle));
    let si = |q: &crate::units::Quantity| q.si();
    let tol_places = |t: f64| {
        let shown_in = if angle {
            t.to_degrees()
        } else {
            t / u.unit.factor()
        };
        places.max(needs(shown_in.abs()))
    };
    let mut value_text = num(value, places);
    let mut stack = None;
    let mut boxed = false;
    let mut report = json!({
        "value": value,
        "text": value_text,
        "unit": if angle { "deg" } else { u.unit.as_str() },
    });
    let mut after = String::new();
    match &style.tolerance {
        None => {}
        Some(Tolerance::Symmetric(t)) => {
            let p = tol_places(si(t));
            value_text = num(value, p);
            after = format!(" ±{}", num(si(t), p));
            report["tolerance"] = json!({"symmetric": si(t)});
        }
        Some(Tolerance::Deviation(a, b)) => {
            let p = tol_places(si(a).abs().max(si(b).abs()));
            value_text = num(value, p);
            stack = Some((signed(u, si(a), p, angle), signed(u, si(b), p, angle)));
            report["tolerance"] = json!({"deviation": [si(a), si(b)]});
        }
        Some(Tolerance::Limits(a, b)) => {
            let p = tol_places(si(a).abs().max(si(b).abs()));
            stack = Some((num(value + si(a), p), num(value + si(b), p)));
            value_text = String::new();
            report["tolerance"] = json!({"limits": [value + si(a), value + si(b)]});
        }
        Some(Tolerance::Fit(f)) => {
            let classes = fit_deviations(f, value)?;
            after = format!(" {f}");
            if let [(_, up, lo)] = classes.as_slice() {
                let p = places
                    .max(needs(up.abs() / u.unit.factor()))
                    .max(needs(lo.abs() / u.unit.factor()));
                after.push_str(&format!(
                    " ({}/{})",
                    signed(u, *up, p, false),
                    signed(u, *lo, p, false)
                ));
            }
            report["tolerance"] = json!({"fit": f, "classes": classes.iter().map(|(c, up, lo)| json!({
                "class": c, "upper": up, "lower": lo,
                "limits": [value + up, value + lo],
            })).collect::<Vec<_>>()});
        }
        Some(Tolerance::Basic) => {
            boxed = true;
            report["tolerance"] = json!({"basic": true});
        }
        Some(Tolerance::Reference) => {
            report["tolerance"] = json!({"reference": true});
        }
    }
    let mut body: Vec<Run> = lead.to_vec();
    if !value_text.is_empty() {
        body.push(Run::Text(value_text));
    }
    // `<>` in the annotation's text stands for the value.
    let mut runs: Vec<Run> = Vec::new();
    if let Some(p) = &style.prefix {
        runs.push(Run::Text(format!("{p} ")));
    }
    match &style.text {
        Some(t) => {
            let (a, b) = t.split_once("<>").expect("read with one <>");
            runs.push(Run::Text(a.to_owned()));
            runs.extend(body);
            runs.push(Run::Text(b.to_owned()));
        }
        None => runs.extend(body),
    }
    if !after.is_empty() {
        runs.push(Run::Text(after));
    }
    if let Some(s) = &style.suffix {
        runs.push(Run::Text(format!(" {s}")));
    }
    if matches!(style.tolerance, Some(Tolerance::Reference)) {
        runs.insert(0, Run::Text("(".into()));
        runs.push(Run::Text(")".into()));
    }
    // Adjacent texts as one run.
    let mut merged: Vec<Run> = Vec::new();
    for r in runs {
        match (merged.last_mut(), r) {
            (Some(Run::Text(a)), Run::Text(b)) => a.push_str(&b),
            (_, Run::Text(b)) if b.is_empty() => {}
            (_, r) => merged.push(r),
        }
    }
    let out = Shown {
        runs: merged,
        stack,
        boxed,
        report,
    };
    let mut out = out;
    out.report["shown"] = json!(out.plain());
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::units::Quantity;

    fn units(unit: Unit, standard: Standard) -> Units {
        Units {
            unit,
            precision: if unit == Unit::In { 3 } else { 2 },
            standard,
        }
    }

    fn q(s: &str) -> Quantity {
        Quantity::parse(s, Unit::Mm).unwrap()
    }

    #[test]
    fn values_round_half_to_even_in_the_drawings_unit() {
        let mm = units(Unit::Mm, Standard::Iso);
        assert_eq!(mm.number(0.012345, 2, false), "12.35");
        assert_eq!(decimal(0.125, 2, false), "0.12");
        assert_eq!(decimal(0.375, 2, false), "0.38");
        assert_eq!(decimal(-0.0001, 2, false), "0.00");
        let inch = units(Unit::In, Standard::Asme);
        assert_eq!(inch.number(0.0127, 3, false), ".500");
        assert_eq!(mm.number(std::f64::consts::FRAC_PI_4, 0, true), "45");
    }

    #[test]
    fn tolerances_show_as_their_standard_writes_them() {
        let mm = units(Unit::Mm, Standard::Iso);
        let st = |t: Tolerance| Style {
            tolerance: Some(t),
            ..Style::default()
        };
        let s = shown(
            &mm,
            &st(Tolerance::Symmetric(q("0.005 mm"))),
            0.012,
            false,
            &[],
        )
        .unwrap();
        assert_eq!(s.plain(), "12.000 ±0.005");
        let s = shown(
            &mm,
            &st(Tolerance::Deviation(q("0.1 mm"), q("0 mm"))),
            0.012,
            false,
            &[],
        )
        .unwrap();
        assert_eq!(s.plain(), "12.00 +0.10/0");
        let s = shown(
            &mm,
            &st(Tolerance::Limits(q("0.1 mm"), q("-0.1 mm"))),
            0.012,
            false,
            &[],
        )
        .unwrap();
        assert_eq!(s.plain(), " 12.10/11.90".trim_start());
        let s = shown(
            &mm,
            &st(Tolerance::Fit("H7".into())),
            0.025,
            false,
            &[Run::Symbol(Symbol::Diameter)],
        )
        .unwrap();
        assert_eq!(s.plain(), "⌀25.00 H7 (+0.021/0)");
        assert_eq!(s.report["tolerance"]["classes"][0]["upper"], 21e-6);
        let s = shown(&mm, &st(Tolerance::Fit("H7/g6".into())), 0.025, false, &[]).unwrap();
        assert_eq!(s.plain(), "25.00 H7/g6");
        let s = shown(&mm, &st(Tolerance::Reference), 0.025, false, &[]).unwrap();
        assert_eq!(s.plain(), "(25.00)");
        let e = shown(&mm, &st(Tolerance::Fit("H7".into())), 4.0, false, &[]).unwrap_err();
        assert_eq!(e.details["reason"], "fit_undefined");
        let t = Style {
            text: Some("4× <> THRU".into()),
            prefix: Some("TYP".into()),
            ..Style::default()
        };
        assert_eq!(
            shown(&mm, &t, 0.006, false, &[Run::Symbol(Symbol::Diameter)])
                .unwrap()
                .plain(),
            "TYP 4× ⌀6.00 THRU"
        );
    }
}
