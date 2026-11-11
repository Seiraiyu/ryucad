//! The symbol lint: a library's symbols judged against the standards that
//! apply to them (`Project::standards_for`). Its findings judge a symbol,
//! never a sheet: each costs the symbol's own score (100 less its
//! findings' points) in `lib check`, and `check` reports them. Every rule
//! carries what it finds, why, how to fix it and its evidence; its weight
//! and severity are `lint.json`'s, as the sheet lint's are.

use crate::standards::{Class, Standards};
use crate::symbol::{Pin, PinType, Point, Side, Symbol};
use ryusch::lint::{Prefs, Rule, Severity};
use serde::Serialize;
use serde_json::{Value, json};
use std::collections::BTreeSet;

/// The old sheet rule whose judgement `sym_shape` took (plan 2.D):
/// `lint.json` and `lint configure` read it as the new one.
pub const ALIAS: (&str, &str) = ("sch_resistor_style", "sym_shape");

pub const RULES: [Rule; 7] = [
    Rule {
        code: "sym_pin_off_grid",
        weight: 2,
        unit: "a pin",
        what: "A pin's connection point isn't on the standards' grid (50 mil by default) from the symbol's origin.",
        why: "Wires meet pins on the grid; a pin off it takes a jog or a dangling end wherever the part is placed, and the engine places a part by its first pin.",
        fix: "Move the pin onto the grid (the finding's symbol edit ops), or set the grid the library draws to (standards set '{\"grid_nm\": …}').",
        evidence: "KiCad's library convention (KLC S4.1) puts every pin on a 100 mil grid and KiCad's schematic grid is 50 mil; ryusch places parts on the 50 mil grid and its sheet lint judges pins off it relative to each other (sch_pin_off_grid). A drafting convention; no round has judged it yet.",
    },
    Rule {
        code: "sym_pin_side",
        weight: 3,
        unit: "a pin",
        what: "A pin stands on a side its class's standards don't put its role on: a supply off the top, ground off the bottom, an input on the right, an output on the left (a regulator's supply runs left to right).",
        why: "Readers expect power from above, ground below and signals flowing left to right; a pin on the wrong side sends its wire across the body or against the flow.",
        fix: "Move the pin to the free place the finding's ops name, or change the class's sides (standards set).",
        evidence: "IEEE 315 and the hand-drawn references put supplies up, ground down, inputs left and outputs right; phase 1's lint already faults sideways power (sch_power_sideways) and signals drawn against the flow, which a pin on the wrong side causes. A drafting convention; no round has judged it yet.",
    },
    Rule {
        code: "sym_pin_group",
        weight: 2,
        unit: "a group",
        what: "A function's pins (I2C, SPI, USB, GPIO, by the standards' groups) stand on more than one side or with other pins between them, or a numbered group isn't in order down its side.",
        why: "A bus's pins together read as one interface and their wires run as a bundle; split, each wire goes its own way, and GPIO12 above GPIO3 hides the one being looked for.",
        fix: "Reorder the side's pins as the finding's ops do; bring a split group to one side first.",
        evidence: "Design 348: pins grouped by function, GPIOs in numeric order, as the references' MCUs and KiCad's library draw them. A drafting convention; no round has judged it yet.",
    },
    Rule {
        code: "sym_pin_length",
        weight: 1,
        unit: "a pin",
        what: "A pin of a part whose class judges lengths (an IC, a regulator, an op-amp) isn't the standards' length (100 mil by default).",
        why: "Pins of one length line a sheet's wires up at one distance from every body; a long or short pin reads as a different kind of pin.",
        fix: "Set the pin's length keeping where it meets the body (the finding's ops), or the standards' pin_length_nm.",
        evidence: "KiCad's library convention (KLC S4.2) draws pins 100 mil long; every IC of the corpus (tests/support/draw.rs) is drawn so. A drafting convention; no round has judged it yet.",
    },
    Rule {
        code: "sym_shape",
        weight: 1,
        unit: "a symbol",
        what: "A resistor is drawn to the other standard than its library's: the IEC box under ANSI standards, or the US zigzag under IEC ones.",
        why: "Two styles of one symbol on a sheet make the reader wonder whether they are different parts.",
        fix: "Redraw the body to the standard (the finding's ops), or set the standards' shapes (standards set '{\"shapes\": \"iec\"}').",
        evidence: "IEEE 315 draws the zigzag, IEC 60617 the box; an organisation keeps to one, ANSI by default (the owner's answer, design 348). Until plan 2.D the sheet lint judged it (sch_resistor_style, which reads as this rule); in round 2's blind review (plan 1.D, three judges a pair) no rubric criterion covered it alone.",
    },
    Rule {
        code: "sym_body_margin",
        weight: 2,
        unit: "a pin",
        what: "A pin's name, as drawn, comes nearer its body's far edge, or another pin's name, than the standards' margin (25 mil by default).",
        why: "A name outside its body reads as a label of the net, and names that touch can't be read: the cause of the sheet lint's sch_pin_name_outside, judged once in the library rather than on every sheet.",
        fix: "Widen the body (the finding's ops move its far edge and the pins on that edge), or shorten the name.",
        evidence: "Every committed reference's symbols hold their pin names inside the body with room to spare; the corpus's symbols are sized so their names fit (tests/support/draw.rs). A drafting convention; no round has judged it yet.",
    },
    Rule {
        code: "sym_units",
        weight: 2,
        unit: "a unit",
        what: "A unit of a symbol has more pins than its class draws in one (64 for an IC by default).",
        why: "A body with that many pins fills a sheet on its own and pulls every satellite far from its pin; drawn in units (power, ports, functions), each sits beside the circuit it serves.",
        fix: "Split the symbol into units in its library (symbol edit --units N, then each pin's unit): no op does it for you.",
        evidence: "The references draw large MCUs in units, power apart from I/O; design 143 asks for units past a pin count. A drafting convention; no round has judged it yet.",
    },
];

/// A rule of the symbol lint, by code.
pub fn rule(code: &str) -> Option<&'static Rule> {
    RULES.iter().find(|r| r.code == code)
}

/// A rule's weight, and the preference that set it if one did.
pub fn weight(prefs: &Prefs, code: &str) -> (i64, Option<String>) {
    match prefs.weights.get(code) {
        Some(w) => (*w, Some(format!("weights.{code} = {w}"))),
        None => (rule(code).map_or(0, |r| r.weight), None),
    }
}

/// What a symbol is judged with.
pub struct Context<'a> {
    pub standards: &'a Standards,
    pub prefs: &'a Prefs,
    /// Where the standards come from: an alias, or `built_in`.
    pub from: &'a str,
}

/// One finding of a symbol.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct Finding {
    pub code: &'static str,
    /// The pins concerned, by number (none for the whole symbol or a
    /// unit).
    pub pins: Vec<String>,
    pub message: String,
    #[serde(skip_serializing_if = "Value::is_null")]
    pub details: Value,
    /// Its rule's weight.
    pub points: i64,
    pub severity: Severity,
    /// The preferences that drove it.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub prefs: Vec<String>,
    /// Batches of `symbol edit` ops that remove it, each tried first.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub fixes: Vec<Vec<Value>>,
}

/// A finding before weights and severities.
struct Raw {
    code: &'static str,
    pins: Vec<String>,
    message: String,
    details: Value,
    fixes: Vec<Vec<Value>>,
    prefs: Vec<String>,
}

fn raw(code: &'static str, pins: Vec<String>, message: String, details: Value) -> Raw {
    Raw {
        code,
        pins,
        message,
        details,
        fixes: Vec::new(),
        prefs: Vec::new(),
    }
}

/// A symbol's score: 100 less its findings' points, at least 0.
pub fn score(found: &[Finding]) -> i64 {
    (100 - found.iter().map(|f| f.points).sum::<i64>()).max(0)
}

/// Which way a pin points from its body to its end.
fn dir(side: Side) -> Point {
    match side {
        Side::Left => [-1, 0],
        Side::Right => [1, 0],
        Side::Up => [0, 1],
        Side::Down => [0, -1],
    }
}

/// Where a pin meets its body: its connection point less its length.
pub fn base(p: &Pin) -> Point {
    let d = dir(p.side);
    [p.at[0] - d[0] * p.length_nm, p.at[1] - d[1] * p.length_nm]
}

/// The nearest multiple of `g` to `v`, halves away from zero.
fn snap(v: i64, g: i64) -> i64 {
    let h = g / 2;
    if v >= 0 {
        (v + h) / g * g
    } else {
        -((-v + h) / g * g)
    }
}

fn mm(v: i64) -> String {
    ryusch::geom::mm(v)
}

/// The unit common pins are drawn on: the first power pin's, else 1 (as
/// `schem::build` draws them).
pub fn home(s: &Symbol) -> u32 {
    s.pins
        .values()
        .find(|x| matches!(x.ty, PinType::PowerIn | PinType::PowerOut))
        .and_then(|x| x.unit)
        .unwrap_or(1)
}

/// The units that hold a pin (never more than the pins: a file may claim
/// billions of units).
pub fn units(s: &Symbol) -> BTreeSet<u32> {
    let h = home(s);
    s.pins.values().map(|p| p.unit.unwrap_or(h)).collect()
}

/// A unit's pins: its own and the common ones on its home unit.
pub fn unit_pins(s: &Symbol, unit: u32) -> Vec<(&String, &Pin)> {
    let h = home(s);
    s.pins
        .iter()
        .filter(|(_, p)| p.unit == Some(unit) || (p.unit.is_none() && unit == h))
        .collect()
}

fn off_grid(s: &Symbol, cx: &Context, out: &mut Vec<Raw>) {
    let g = cx.standards.grid_nm;
    for (n, p) in &s.pins {
        if p.at[0] % g == 0 && p.at[1] % g == 0 {
            continue;
        }
        let to = [snap(p.at[0], g), snap(p.at[1], g)];
        let mut r = raw(
            "sym_pin_off_grid",
            vec![n.clone()],
            format!(
                "pin {n} is at ({}, {}) mm, off the {} mm grid",
                mm(p.at[0]),
                mm(p.at[1]),
                mm(g)
            ),
            json!({"at": p.at, "grid_nm": g}),
        );
        r.fixes
            .push(vec![json!({"op": "move", "pin": n, "at": to})]);
        out.push(r);
    }
}

fn length(s: &Symbol, class: Option<Class>, cx: &Context, out: &mut Vec<Raw>) {
    let Some(class) = class else { return };
    if !cx.standards.rule(class).pin_length {
        return;
    }
    let want = cx.standards.pin_length_nm;
    for (n, p) in &s.pins {
        if p.length_nm == want {
            continue;
        }
        let (b, d) = (base(p), dir(p.side));
        let at = [b[0] + d[0] * want, b[1] + d[1] * want];
        let mut r = raw(
            "sym_pin_length",
            vec![n.clone()],
            format!(
                "pin {n} is {} mm long; a {}'s pins are {} mm",
                mm(p.length_nm),
                class.as_str(),
                mm(want)
            ),
            json!({"length_nm": p.length_nm, "standard_nm": want}),
        );
        r.fixes.push(vec![
            json!({"op": "set", "pin": n, "length": want}),
            json!({"op": "move", "pin": n, "at": at}),
        ]);
        out.push(r);
    }
}

fn unit_size(s: &Symbol, class: Option<Class>, cx: &Context, out: &mut Vec<Raw>) {
    let Some(class) = class else { return };
    let Some(max) = cx.standards.rule(class).max_pins_per_unit else {
        return;
    };
    for unit in units(s) {
        let n = unit_pins(s, unit).len();
        if n > max as usize {
            out.push(raw(
                "sym_units",
                Vec::new(),
                format!(
                    "unit {unit} has {n} pins; a {} draws at most {max} in one unit",
                    class.as_str()
                ),
                json!({"unit": unit, "pins": n, "max": max}),
            ));
        }
    }
}

/// Every rule's raw findings, in the catalogue's order.
fn found(s: &Symbol, class: Option<Class>, cx: &Context) -> Vec<Raw> {
    let mut out = Vec::new();
    off_grid(s, cx, &mut out);
    length(s, class, cx, &mut out);
    unit_size(s, class, cx, &mut out);
    out
}

/// `s`'s findings as `cx` judges a symbol of `class` (`None`: only the
/// rules every symbol answers to), weighted, severities applied (`off`
/// drops a rule), sorted by code then pins.
pub fn lint(s: &Symbol, class: Option<Class>, cx: &Context) -> Vec<Finding> {
    let mut out = Vec::new();
    for r in found(s, class, cx) {
        let (severity, sp) = cx.prefs.severity(r.code);
        if severity == Severity::Off {
            continue;
        }
        let (points, wp) = weight(cx.prefs, r.code);
        let mut message = r.message;
        if let Some(w) = &wp {
            message.push_str(&format!(" ({w})"));
        }
        let mut prefs = r.prefs;
        prefs.extend(sp);
        prefs.extend(wp);
        out.push(Finding {
            code: r.code,
            pins: r.pins,
            message,
            details: r.details,
            points,
            severity,
            prefs,
            fixes: r.fixes,
        });
    }
    out.sort_by(|a, b| (a.code, &a.pins).cmp(&(b.code, &b.pins)));
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::standards::{Class, ClassRule, Standards};
    use crate::symbol::PinType;
    use crate::template::{PinSpec, template};

    pub(crate) fn inputs(n: usize) -> Vec<PinSpec> {
        (1..=n)
            .map(|i| PinSpec {
                number: i.to_string(),
                name: format!("IN{i}"),
                ty: PinType::Input,
            })
            .collect()
    }

    pub(crate) fn codes(found: &[Finding]) -> Vec<(&str, Vec<String>)> {
        found.iter().map(|f| (f.code, f.pins.clone())).collect()
    }

    #[test]
    fn off_grid_long_pins_and_full_units_are_found_with_their_fixes() {
        let st = Standards::default();
        let prefs = Prefs::default();
        let cx = Context {
            standards: &st,
            prefs: &prefs,
            from: "built_in",
        };
        let clean = template("U", Class::Ic, &inputs(4), &st).unwrap();
        assert_eq!(codes(&lint(&clean, Some(Class::Ic), &cx)), []);
        let mut s = clean.clone();
        let one = s.pins.get_mut("1").unwrap();
        one.at[1] += 100_000;
        let snapped = [one.at[0], one.at[1] - 100_000];
        // Pin 2 a step longer, its base where it was.
        let two = s.pins.get_mut("2").unwrap();
        two.length_nm += 1_270_000;
        two.at[0] -= 1_270_000;
        let found = lint(&s, Some(Class::Ic), &cx);
        assert_eq!(
            codes(&found),
            [
                ("sym_pin_length", vec!["2".to_owned()]),
                ("sym_pin_off_grid", vec!["1".to_owned()]),
            ]
        );
        assert_eq!(
            found[1].fixes,
            [vec![
                serde_json::json!({"op": "move", "pin": "1", "at": snapped})
            ]]
        );
        let back = found[0].fixes[0].clone();
        assert_eq!(back[0]["length"], 2_540_000);
        // Unclassed, only the grid judges it; each finding costs its weight.
        assert_eq!(
            codes(&lint(&s, None, &cx)),
            [("sym_pin_off_grid", vec!["1".to_owned()])]
        );
        assert_eq!(score(&found), 100 - 1 - 2);
        // A unit of more pins than its class allows.
        let mut tight = Standards::default();
        tight.classes.insert(
            Class::Ic,
            ClassRule {
                max_pins_per_unit: Some(3),
                ..st.rule(Class::Ic)
            },
        );
        let cx = Context {
            standards: &tight,
            prefs: &prefs,
            from: "self",
        };
        let found = lint(&clean, Some(Class::Ic), &cx);
        assert_eq!(codes(&found), [("sym_units", vec![])]);
        assert_eq!(found[0].details["pins"], 4);
        // A severity off drops a rule; a weight set says so.
        let mut prefs = Prefs::default();
        prefs
            .severity
            .insert("sym_units".into(), ryusch::lint::Severity::Off);
        let cx = Context {
            standards: &tight,
            prefs: &prefs,
            from: "self",
        };
        assert!(lint(&clean, Some(Class::Ic), &cx).is_empty());
        let mut prefs = Prefs::default();
        prefs.weights.insert("sym_units".into(), 9);
        let cx = Context {
            standards: &tight,
            prefs: &prefs,
            from: "self",
        };
        let f = &lint(&clean, Some(Class::Ic), &cx)[0];
        assert_eq!(
            (f.points, f.prefs.as_slice()),
            (9, ["weights.sym_units = 9".to_owned()].as_slice())
        );
        assert!(
            f.message.ends_with("(weights.sym_units = 9)"),
            "{}",
            f.message
        );
    }
}
