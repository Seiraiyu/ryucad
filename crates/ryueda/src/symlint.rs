//! The symbol lint: a library's symbols judged against the standards that
//! apply to them (`Project::standards_for`). Its findings judge a symbol,
//! never a sheet: each costs the symbol's own score (100 less its
//! findings' points) in `lib check`, and `check` reports them. Every rule
//! carries what it finds, why, how to fix it and its evidence; its weight
//! and severity are `lint.json`'s, as the sheet lint's are.

use crate::standards::{Class, Standards};
use crate::symbol::{Pin, PinType, Point, Side, Symbol};
use ryusch::geom::Rect;
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

/// A unit's body as ryusch draws it: its graphics' boxes and its pins'
/// bases.
fn body(s: &Symbol, unit: u32) -> Rect {
    ryusch::shape::local(&crate::schem::unit_part(s, unit, String::new(), "U")).0
}

/// Down to a multiple of `p`, and up to one.
fn floor(v: i64, p: i64) -> i64 {
    v.div_euclid(p) * p
}
fn ceil(v: i64, p: i64) -> i64 {
    (v + p - 1).div_euclid(p) * p
}

/// The next free place on `side` of `unit`'s body for a pin `len` long:
/// a row (two grid steps) below the lowest pin there on the left and
/// right, a column (four) past the rightmost along the top and bottom, or
/// the body's first row or column when the side has none; `None` past the
/// body's end.
fn slot(s: &Symbol, unit: u32, side: Side, len: i64, g: i64) -> Option<Point> {
    let b = body(s, unit);
    let p = 2 * g;
    let on: Vec<Point> = unit_pins(s, unit)
        .iter()
        .filter(|(_, x)| x.side == side)
        .map(|(_, x)| x.at)
        .collect();
    match side {
        Side::Left | Side::Right => {
            let x = if side == Side::Left {
                b.min[0] - len
            } else {
                b.max[0] + len
            };
            let y = on
                .iter()
                .map(|a| a[1])
                .min()
                .map_or_else(|| floor(b.max[1], p) - p, |y| y - p);
            (y > b.min[1]).then_some([x, y])
        }
        Side::Up | Side::Down => {
            let y = if side == Side::Up {
                b.max[1] + len
            } else {
                b.min[1] - len
            };
            let x = on
                .iter()
                .map(|a| a[0])
                .max()
                .map_or_else(|| ceil(b.min[0], p) + p, |x| x + 2 * p);
            (x < b.max[0]).then_some([x, y])
        }
    }
}

fn side_word(s: Side) -> &'static str {
    match s {
        Side::Left => "left",
        Side::Right => "right",
        Side::Up => "top",
        Side::Down => "bottom",
    }
}

fn sides(s: &Symbol, class: Option<Class>, cx: &Context, out: &mut Vec<Raw>) {
    let Some(class) = class else { return };
    let rule = cx.standards.rule(class);
    for (n, p) in &s.pins {
        let Some(role) = cx.standards.role(&p.name, p.ty) else {
            continue;
        };
        let Some(&want) = rule.sides.get(&role) else {
            continue;
        };
        if p.side == want {
            continue;
        }
        let mut r = raw(
            "sym_pin_side",
            vec![n.clone()],
            format!(
                "pin {n} ({}) is a {} pin on the {} side; a {} puts it on the {}",
                p.name,
                role.as_str(),
                side_word(p.side),
                class.as_str(),
                side_word(want)
            ),
            json!({"role": role, "side": p.side, "wanted": want}),
        );
        let unit = p.unit.unwrap_or_else(|| home(s));
        if let Some(at) = slot(s, unit, want, p.length_nm, cx.standards.grid_nm) {
            r.fixes.push(vec![
                json!({"op": "move", "pin": n, "at": at}),
                json!({"op": "set", "pin": n, "side": want}),
            ]);
        }
        out.push(r);
    }
}

/// How far along its side a pin stands, in reading order: down the left
/// and right, across the top and bottom.
fn along(p: &Pin) -> i64 {
    match p.side {
        Side::Left | Side::Right => -p.at[1],
        Side::Up | Side::Down => p.at[0],
    }
}

fn groups(s: &Symbol, class: Option<Class>, cx: &Context, out: &mut Vec<Raw>) {
    let Some(class) = class else { return };
    let st = cx.standards;
    let rule = st.rule(class);
    if !rule.groups {
        return;
    }
    for unit in units(s) {
        let pins = unit_pins(s, unit);
        // Each group's pins whose role has no side of its own.
        let mut by_group: std::collections::BTreeMap<&str, Vec<(&String, &Pin, Option<u64>)>> =
            Default::default();
        for (n, p) in &pins {
            if st
                .role(&p.name, p.ty)
                .is_some_and(|r| rule.sides.contains_key(&r))
            {
                continue;
            }
            if let Some((g, num)) = st.group_of(&p.name) {
                by_group.entry(g).or_default().push((n, p, num));
            }
        }
        for (g, members) in by_group {
            if members.len() < 2 {
                continue;
            }
            let numbers: Vec<String> = members.iter().map(|m| m.0.clone()).collect();
            let number_of = |n: &String| members.iter().find(|m| m.0 == n).and_then(|m| m.2);
            let is_member = |n: &String| members.iter().any(|m| m.0 == n);
            let side0 = members[0].1.side;
            if members.iter().any(|m| m.1.side != side0) {
                out.push(raw(
                    "sym_pin_group",
                    numbers.clone(),
                    format!(
                        "{g}'s pins {} stand on more than one side",
                        numbers.join(", ")
                    ),
                    json!({"group": g, "split": true}),
                ));
                continue;
            }
            let mut on_side: Vec<(&String, &Pin)> = pins
                .iter()
                .filter(|(_, p)| p.side == side0)
                .copied()
                .collect();
            on_side.sort_by_key(|(n, p)| (along(p), (*n).clone()));
            let idx: Vec<usize> = on_side
                .iter()
                .enumerate()
                .filter(|(_, (n, _))| is_member(n))
                .map(|(i, _)| i)
                .collect();
            let (first, last) = (idx[0], idx[idx.len() - 1]);
            let contiguous = last - first + 1 == idx.len();
            let in_order: Vec<Option<u64>> = idx.iter().map(|&i| number_of(on_side[i].0)).collect();
            let ordered = in_order.windows(2).all(|w| w[0] <= w[1]);
            if contiguous && ordered {
                continue;
            }
            // The fix: the run from the group's first pin to its last, the
            // group first (in number order), then the others as they stand,
            // each pin keeping its distance from the body.
            let run = &on_side[first..=last];
            let slots: Vec<Point> = run.iter().map(|(_, p)| p.at).collect();
            let mut order: Vec<(&String, &Pin)> =
                run.iter().filter(|(n, _)| is_member(n)).copied().collect();
            order.sort_by_key(|(n, _)| number_of(n));
            order.extend(run.iter().filter(|(n, _)| !is_member(n)).copied());
            let ops: Vec<Value> = order
                .iter()
                .zip(&slots)
                .filter_map(|((n, p), slot)| {
                    let at = match side0 {
                        Side::Left | Side::Right => [p.at[0], slot[1]],
                        Side::Up | Side::Down => [slot[0], p.at[1]],
                    };
                    (at != p.at).then(|| json!({"op": "move", "pin": n, "at": at}))
                })
                .collect();
            let mut r = raw(
                "sym_pin_group",
                numbers.clone(),
                if contiguous {
                    format!(
                        "{g}'s pins aren't in number order along the {} side",
                        side_word(side0)
                    )
                } else {
                    format!(
                        "{g}'s pins have other pins between them on the {} side",
                        side_word(side0)
                    )
                },
                json!({"group": g, "contiguous": contiguous, "ordered": ordered}),
            );
            if !ops.is_empty() {
                r.fixes.push(ops);
            }
            out.push(r);
        }
    }
}

/// Where standards come from, in a message.
fn from_words(from: &str) -> String {
    if from == "built_in" {
        "the built-in standards".to_owned()
    } else {
        format!("{from}'s standards.json")
    }
}

fn shape(s: &Symbol, class: Option<Class>, cx: &Context, out: &mut Vec<Raw>) {
    use crate::standards::Shapes;
    use ryusch::lint::ResistorStyle;
    if class != Some(Class::Resistor) || s.pins.len() != 2 {
        return;
    }
    let part = crate::schem::unit_part(s, 1, String::new(), "R");
    let Some(drawn) = ryusch::lint::resistor_style(&part) else {
        return;
    };
    let drawn = match drawn {
        ResistorStyle::Us => Shapes::Ansi,
        ResistorStyle::Iec => Shapes::Iec,
    };
    let want = cx.standards.shapes;
    if drawn == want {
        return;
    }
    let word = |sh: Shapes| match sh {
        Shapes::Ansi => "the US zigzag (ANSI)",
        Shapes::Iec => "the IEC box",
    };
    let mut r = raw(
        "sym_shape",
        Vec::new(),
        format!(
            "the resistor is drawn as {}; {} draw {}",
            word(drawn),
            from_words(cx.from),
            word(want)
        ),
        json!({"drawn": drawn.as_str(), "standard": want.as_str(), "from": cx.from}),
    );
    if cx.from == "built_in" && cx.prefs.prefer.resistor == ResistorStyle::Iec {
        r.prefs.push("prefer.resistor = iec".to_owned());
    }
    let pins: Vec<&Pin> = s.pins.values().collect();
    if let Some(body) = crate::template::resistor_body(want, base(pins[0]), base(pins[1])) {
        let mut ops: Vec<Value> = s
            .graphics
            .keys()
            .map(|id| json!({"op": "remove", "graphic": id}))
            .collect();
        ops.push(crate::template::add_op(&body));
        r.fixes.push(ops);
    }
    out.push(r);
}

fn far_grown(r: Rect, side: ryusch::geom::Side, m: i64) -> Rect {
    use ryusch::geom::Side as S;
    let mut r = r;
    match side {
        S::Left => r.max[0] += m,
        S::Right => r.min[0] -= m,
        S::Up => r.min[1] -= m,
        S::Down => r.max[1] += m,
    }
    r
}

fn margins(s: &Symbol, cx: &Context, out: &mut Vec<Raw>) {
    let st = cx.standards;
    let m = st.body_margin_nm;
    let p = 2 * st.grid_nm;
    for unit in units(s) {
        let part = crate::schem::unit_part(s, unit, String::new(), "U");
        if !ryusch::paint::draws_names(&part) {
            continue;
        }
        let sh = ryusch::shape::Shape::of(&part, ryusch::geom::Place::new([0, 0], 0, false));
        let boxes: Vec<(usize, Rect)> = part
            .pins
            .iter()
            .enumerate()
            .filter(|(_, pin)| !pin.name.is_empty())
            .map(|(k, pin)| {
                let b = ryusch::paint::name_box_sized(&pin.name, &sh.pins[k], st.pin_name_size_nm);
                (k, far_grown(b, sh.pins[k].side, m))
            })
            .collect();
        for (i, (k, b)) in boxes.iter().enumerate() {
            let past = !sh.body.contains(b);
            let into = boxes[i + 1..].iter().find(|(_, o)| o.overlaps(b));
            if !past && into.is_none() {
                continue;
            }
            let pin = &part.pins[*k];
            let mut r = raw(
                "sym_body_margin",
                vec![pin.number.clone()],
                format!(
                    "pin {}'s name {} {} {} mm",
                    pin.number,
                    pin.name,
                    if past {
                        "comes nearer its body's edge than"
                    } else {
                        "comes nearer another pin's name than"
                    },
                    mm(m)
                ),
                json!({"past_edge": past, "margin_nm": m}),
            );
            // Widen the far edge by what it lacks, in two-grid steps, and
            // move the pins on that edge with it: only for a left or right
            // pin of a unit whose body is one rectangle.
            let over = match sh.pins[*k].side {
                ryusch::geom::Side::Left => {
                    let o = into.map_or(0, |(_, o)| b.max[0] - o.min[0]);
                    Some((b.max[0] - sh.body.max[0]).max(o))
                }
                ryusch::geom::Side::Right => {
                    let o = into.map_or(0, |(_, o)| o.max[0] - b.min[0]);
                    Some((sh.body.min[0] - b.min[0]).max(o))
                }
                _ => None,
            };
            let rects: Vec<(&crate::id::Id, &crate::symbol::Graphic)> = s
                .graphics
                .iter()
                .filter(|(_, g)| g.unit().is_none_or(|u| u == unit))
                .filter(|(_, g)| matches!(g, crate::symbol::Graphic::Rect { .. }))
                .collect();
            if let (Some(over), [(id, crate::symbol::Graphic::Rect { start, end, .. })]) =
                (over, rects.as_slice())
                && over > 0
            {
                let d = ceil(over, p);
                let r0 = Rect::new(*start, *end);
                let (far, moved, by) = if sh.pins[*k].side == ryusch::geom::Side::Left {
                    ([r0.min[0], r0.max[0] + d], Side::Right, d)
                } else {
                    ([r0.min[0] - d, r0.max[0]], Side::Left, -d)
                };
                let mut ops = vec![json!({
                    "op": "set", "graphic": id,
                    "start": [far[0], r0.max[1]], "end": [far[1], r0.min[1]],
                })];
                for (n, q) in unit_pins(s, unit) {
                    if q.side == moved {
                        ops.push(json!({"op": "move", "pin": n, "by": [by, 0]}));
                    }
                }
                r.fixes.push(ops);
            }
            out.push(r);
        }
    }
}

/// Every rule's raw findings, in the catalogue's order.
fn found(s: &Symbol, class: Option<Class>, cx: &Context) -> Vec<Raw> {
    let mut out = Vec::new();
    off_grid(s, cx, &mut out);
    sides(s, class, cx, &mut out);
    groups(s, class, cx, &mut out);
    shape(s, class, cx, &mut out);
    margins(s, cx, &mut out);
    length(s, class, cx, &mut out);
    unit_size(s, class, cx, &mut out);
    out
}

/// Whether `ops` remove the finding `code` on `pins` from `s` and add no
/// `library_*` error: a fix is offered only so (round 2's ask).
fn fixes_it(
    s: &Symbol,
    class: Option<Class>,
    cx: &Context,
    code: &str,
    pins: &[String],
    ops: &[Value],
) -> bool {
    if ops.is_empty() {
        return false;
    }
    let mut t = s.clone();
    if crate::edit::apply(&mut t, ops, crate::units::Unit::Mm).is_err() {
        return false;
    }
    let key = |f: &crate::fault::Fault| (f.error.code, f.error.details.get("pointer").cloned());
    let before: Vec<_> = s.check().iter().filter(|f| f.is_error()).map(key).collect();
    if t.check()
        .iter()
        .filter(|f| f.is_error())
        .any(|f| !before.contains(&key(f)))
    {
        return false;
    }
    !found(&t, class, cx)
        .iter()
        .any(|r| r.code == code && r.pins == pins)
}

/// `s`'s findings as `cx` judges a symbol of `class` (`None`: only the
/// rules every symbol answers to), weighted, severities applied (`off`
/// drops a rule), sorted by code then pins.
pub fn lint(s: &Symbol, class: Option<Class>, cx: &Context) -> Vec<Finding> {
    let mut out = Vec::new();
    for r in found(s, class, cx) {
        let Raw {
            code,
            pins,
            message,
            details,
            fixes,
            prefs: rp,
        } = r;
        let (severity, sp) = cx.prefs.severity(code);
        if severity == Severity::Off {
            continue;
        }
        let (points, wp) = weight(cx.prefs, code);
        let mut message = message;
        if let Some(w) = &wp {
            message.push_str(&format!(" ({w})"));
        }
        let mut prefs = rp;
        prefs.extend(sp);
        prefs.extend(wp);
        let fixes = fixes
            .into_iter()
            .filter(|ops| fixes_it(s, class, cx, code, &pins, ops))
            .collect();
        out.push(Finding {
            code,
            pins,
            message,
            details,
            points,
            severity,
            prefs,
            fixes,
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

    #[test]
    fn a_pin_on_the_wrong_side_moves_to_its_sides_next_free_place() {
        let st = Standards::default();
        let prefs = Prefs::default();
        let cx = Context {
            standards: &st,
            prefs: &prefs,
            from: "built_in",
        };
        let pins = vec![
            PinSpec {
                number: "1".into(),
                name: "VDD".into(),
                ty: PinType::PowerIn,
            },
            PinSpec {
                number: "2".into(),
                name: "GND".into(),
                ty: PinType::PowerIn,
            },
            PinSpec {
                number: "3".into(),
                name: "EN".into(),
                ty: PinType::Input,
            },
            PinSpec {
                number: "4".into(),
                name: "OUT".into(),
                ty: PinType::Output,
            },
        ];
        let mut s = template("U", Class::Ic, &pins, &st).unwrap();
        // The output drawn on the left, under the input.
        let en = s.pins["3"].clone();
        let out = s.pins.get_mut("4").unwrap();
        out.side = Side::Left;
        out.at = [en.at[0], en.at[1] - 2 * st.grid_nm];
        let found = lint(&s, Some(Class::Ic), &cx);
        assert_eq!(codes(&found), [("sym_pin_side", vec!["4".to_owned()])]);
        assert_eq!(found[0].details["wanted"], "right");
        let ops = &found[0].fixes[0];
        assert_eq!(
            ops[1],
            serde_json::json!({"op": "set", "pin": "4", "side": "right"})
        );
        // The class decides: a regulator wants its supply on the left.
        let found = lint(
            &template("U", Class::Ic, &pins, &st).unwrap(),
            Some(Class::Regulator),
            &cx,
        );
        assert!(codes(&found).contains(&("sym_pin_side", vec!["1".to_owned()])));
    }

    #[test]
    fn a_groups_pins_stand_together_in_order() {
        let st = Standards::default();
        let prefs = Prefs::default();
        let cx = Context {
            standards: &st,
            prefs: &prefs,
            from: "built_in",
        };
        let gpio = |n: &str, i: u32| PinSpec {
            number: n.into(),
            name: format!("GPIO{i}"),
            ty: PinType::Bidirectional,
        };
        let pins = vec![gpio("1", 0), gpio("2", 1), gpio("3", 2), gpio("4", 3)];
        let clean = template("U", Class::Ic, &pins, &st).unwrap();
        assert!(lint(&clean, Some(Class::Ic), &cx).is_empty());
        // GPIO2 and GPIO1 swapped: out of order.
        let mut s = clean.clone();
        let (a, b) = (s.pins["2"].at, s.pins["3"].at);
        s.pins.get_mut("2").unwrap().at = b;
        s.pins.get_mut("3").unwrap().at = a;
        let found = lint(&s, Some(Class::Ic), &cx);
        assert_eq!(
            codes(&found),
            [(
                "sym_pin_group",
                vec!["1".into(), "2".into(), "3".into(), "4".into()]
            )]
        );
        assert_eq!(found[0].details["ordered"], false);
        let mut fixed = s.clone();
        crate::edit::apply(&mut fixed, &found[0].fixes[0], crate::units::Unit::Mm).unwrap();
        assert_eq!(fixed, clean);
        // An ungrouped pin between them: not together.
        let mut s = clean.clone();
        s.pins.get_mut("2").unwrap().name = "XTAL".into();
        let found = lint(&s, Some(Class::Ic), &cx);
        let group = found.iter().find(|f| f.code == "sym_pin_group").unwrap();
        assert_eq!(group.pins, ["1", "3", "4"]);
        assert_eq!(group.details["contiguous"], false);
        // Split across sides: found, no fix.
        let mut s = clean.clone();
        let p = s.pins.get_mut("4").unwrap();
        p.side = Side::Right;
        p.at[0] = -p.at[0];
        let found = lint(&s, Some(Class::Ic), &cx);
        let group = found.iter().find(|f| f.code == "sym_pin_group").unwrap();
        assert_eq!(group.details["split"], true);
        assert!(group.fixes.is_empty());
    }

    #[test]
    fn a_resistor_drawn_to_the_other_standard_is_redrawn_between_its_pins() {
        let ansi = Standards::default();
        let iec = Standards {
            shapes: crate::standards::Shapes::Iec,
            ..Standards::default()
        };
        let prefs = Prefs::default();
        let zigzag = template("R", Class::Resistor, &[], &ansi).unwrap();
        let boxed = template("R", Class::Resistor, &[], &iec).unwrap();
        let cx = Context {
            standards: &ansi,
            prefs: &prefs,
            from: "self",
        };
        assert!(lint(&zigzag, Some(Class::Resistor), &cx).is_empty());
        let found = lint(&boxed, Some(Class::Resistor), &cx);
        assert_eq!(codes(&found), [("sym_shape", vec![])]);
        assert_eq!(found[0].details["drawn"], "iec");
        assert!(
            found[0].message.contains("self's standards.json"),
            "{}",
            found[0].message
        );
        let mut fixed = boxed.clone();
        crate::edit::apply(&mut fixed, &found[0].fixes[0], crate::units::Unit::Mm).unwrap();
        assert!(lint(&fixed, Some(Class::Resistor), &cx).is_empty());
        assert_eq!(fixed.pins, zigzag.pins);
        // Not a resistor, or not judged as one: nothing.
        assert!(lint(&boxed, None, &cx).is_empty());
        assert!(lint(&boxed, Some(Class::Capacitor), &cx).is_empty());
        // The built-in standards follow prefer.resistor, and say so.
        let mut prefs = Prefs::default();
        prefs.prefer.resistor = ryusch::lint::ResistorStyle::Iec;
        let cx = Context {
            standards: &iec,
            prefs: &prefs,
            from: "built_in",
        };
        let found = lint(&zigzag, Some(Class::Resistor), &cx);
        assert_eq!(found[0].prefs, ["prefer.resistor = iec"]);
    }

    #[test]
    fn names_keep_their_margin_from_the_edge_and_each_other() {
        let st = Standards::default();
        let prefs = Prefs::default();
        let cx = Context {
            standards: &st,
            prefs: &prefs,
            from: "self",
        };
        let wide = vec![
            PinSpec {
                number: "1".into(),
                name: "RESET_N_LONG_NAME".into(),
                ty: PinType::Input,
            },
            PinSpec {
                number: "2".into(),
                name: "IRQ".into(),
                ty: PinType::Output,
            },
            PinSpec {
                number: "3".into(),
                name: "VDD".into(),
                ty: PinType::PowerIn,
            },
        ];
        let clean = template("U", Class::Ic, &wide, &st).unwrap();
        assert!(
            lint(&clean, Some(Class::Ic), &cx).is_empty(),
            "{:?}",
            lint(&clean, Some(Class::Ic), &cx)
        );
        // The body narrowed: the long name runs into the right side's.
        let mut s = clean.clone();
        let (id, rect) = s.graphics.iter_mut().next().unwrap();
        let id = id.clone();
        let crate::symbol::Graphic::Rect { start, end, .. } = rect else {
            panic!()
        };
        let narrow = 2 * st.grid_nm * 2;
        start[0] += narrow;
        end[0] -= narrow;
        for p in s.pins.values_mut() {
            match p.side {
                Side::Left => p.at[0] += narrow,
                Side::Right => p.at[0] -= narrow,
                _ => {}
            }
        }
        let found = lint(&s, Some(Class::Ic), &cx);
        let margin: Vec<&Finding> = found
            .iter()
            .filter(|f| f.code == "sym_body_margin")
            .collect();
        assert_eq!(margin[0].pins, ["1"]);
        // Its fix widens the far edge and moves that edge's pins.
        let ops = &margin[0].fixes[0];
        assert_eq!(ops[0]["op"], "set");
        assert_eq!(ops[0]["graphic"], serde_json::json!(id));
        assert!(ops.iter().any(|o| o["pin"] == "2" && o["op"] == "move"));
        // A larger margin finds the template too narrow.
        let roomy = Standards {
            body_margin_nm: 5_000_000,
            ..Standards::default()
        };
        let cx = Context {
            standards: &roomy,
            prefs: &prefs,
            from: "self",
        };
        assert!(
            lint(&clean, Some(Class::Ic), &cx)
                .iter()
                .any(|f| f.code == "sym_body_margin")
        );
    }

    #[test]
    fn a_fix_is_offered_only_when_it_removes_its_finding() {
        let st = Standards::default();
        let prefs = Prefs::default();
        let cx = Context {
            standards: &st,
            prefs: &prefs,
            from: "self",
        };
        // An output on the left of a body with no room on its right: the
        // pin_side fix's slot is outside the body, so none is offered; and
        // a fix that would leave the finding is dropped.
        let mut s = template("U", Class::Ic, &inputs(2), &st).unwrap();
        let one = s.pins.get_mut("1").unwrap();
        one.ty = PinType::Output;
        let found = lint(&s, Some(Class::Ic), &cx);
        let side = found.iter().find(|f| f.code == "sym_pin_side").unwrap();
        for ops in &side.fixes {
            let mut t = s.clone();
            crate::edit::apply(&mut t, ops, crate::units::Unit::Mm).unwrap();
            assert!(
                !lint(&t, Some(Class::Ic), &cx)
                    .iter()
                    .any(|f| f.code == "sym_pin_side" && f.pins == side.pins)
            );
        }
        // Every fix the lint offers anywhere in this module's cases removes
        // its finding and adds no library_* error.
        for f in &found {
            for ops in &f.fixes {
                let mut t = s.clone();
                crate::edit::apply(&mut t, ops, crate::units::Unit::Mm).unwrap();
                assert!(t.check().iter().all(|x| !x.is_error()));
            }
        }
    }

    #[test]
    fn fixes_that_change_nothing_are_not_offered() {
        let st = Standards::default();
        let prefs = Prefs::default();
        let cx = Context {
            standards: &st,
            prefs: &prefs,
            from: "self",
        };
        // A group "out of order" only by equal numbers can't be fixed by
        // moving: GPIO1 twice (two names, one number) is in order.
        let gp = |n: &str, name: &str| PinSpec {
            number: n.into(),
            name: name.into(),
            ty: PinType::Bidirectional,
        };
        let s = template(
            "U",
            Class::Ic,
            &[gp("1", "GPIO1"), gp("2", "GPIO1_ALT")],
            &st,
        )
        .unwrap();
        assert!(
            lint(&s, Some(Class::Ic), &cx)
                .iter()
                .all(|f| f.fixes.iter().all(|o| !o.is_empty()))
        );
    }
}
