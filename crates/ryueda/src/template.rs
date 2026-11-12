//! Templates: a symbol of a class drawn to the standards, as `symbol
//! create --template` asks. Two-pin parts in full, standing up with pin 1
//! on top, as the corpus and KiCad draw them; ICs, regulators and
//! connectors as a box with the pins given placed by the class's rules.
//! Authoring by command (design 143): nothing calls a template but that
//! command and the lint's fix for `sym_shape`.

use crate::error::{Code, Error, Result};
use crate::id::Id;
use crate::standards::{Class, Shapes, Standards};
use crate::symbol::{Fill, Graphic, Pin, PinShape, PinType, Point, Side, Symbol};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

/// The classes that have a template.
pub const TEMPLATES: [Class; 9] = [
    Class::Resistor,
    Class::Capacitor,
    Class::Inductor,
    Class::Diode,
    Class::Led,
    Class::Opamp,
    Class::Connector,
    Class::Regulator,
    Class::Ic,
];

/// A pin a box or op-amp template places: its number, name and type.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PinSpec {
    pub number: String,
    #[serde(default)]
    pub name: String,
    #[serde(rename = "type")]
    pub ty: PinType,
    /// The unit it belongs to, for a template of several units (an
    /// op-amp's amplifiers); `None` for a pin every unit has.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub unit: Option<u32>,
}

fn pin(name: &str, at: Point, side: Side, length_nm: i64, ty: PinType) -> Pin {
    Pin {
        name: name.to_owned(),
        ty,
        unit: None,
        at,
        side,
        length_nm,
        shape: PinShape::Line,
        alternates: Vec::new(),
    }
}

fn line(points: &[Point]) -> Graphic {
    Graphic::Polyline {
        points: points.to_vec(),
        fill: Fill::None,
        unit: None,
    }
}

/// Graphics as a symbol holds them: `g1`, `g2`, … in order.
fn numbered(list: Vec<Graphic>) -> BTreeMap<Id, Graphic> {
    list.into_iter()
        .enumerate()
        .map(|(i, g)| {
            (
                Id::parse(&format!("g{}", i + 1), "graphic").expect("g<n> is an ID"),
                g,
            )
        })
        .collect()
}

/// Half a resistor's body across its line: 40 mil, as KiCad's.
const HALF: i64 = 1_016_000;

/// A resistor's body between its pins' bases `a` and `b`, which lie on
/// one vertical or horizontal line: the zigzag (`ansi`, six turns at
/// twelfths of the span) or the box (`iec`). `None` when they don't.
pub fn resistor_body(shapes: Shapes, a: Point, b: Point) -> Option<Graphic> {
    let vertical = a[0] == b[0];
    if a == b || !(vertical || a[1] == b[1]) {
        return None;
    }
    Some(match shapes {
        Shapes::Iec => {
            let (start, end) = if vertical {
                ([a[0] - HALF, a[1]], [a[0] + HALF, b[1]])
            } else {
                ([a[0], a[1] + HALF], [b[0], a[1] - HALF])
            };
            Graphic::Rect {
                start,
                end,
                fill: Fill::None,
                unit: None,
            }
        }
        Shapes::Ansi => {
            let mut points = vec![a];
            for k in 1..=6i64 {
                let along = |p: i64, q: i64| p + (q - p) * (2 * k - 1) / 12;
                let off = if k % 2 == 1 { HALF } else { -HALF };
                points.push(if vertical {
                    [a[0] + off, along(a[1], b[1])]
                } else {
                    [along(a[0], b[0]), a[1] + off]
                });
            }
            points.push(b);
            line(&points)
        }
    })
}

/// A graphic as the `add` op that draws it (`edit::apply`'s names: a
/// rect's `start` and `end`, a polyline's `points`, each with `fill`).
pub fn add_op(g: &Graphic) -> serde_json::Value {
    let mut v = serde_json::to_value(g).expect("graphics serialize");
    let o = v.as_object_mut().expect("a graphic is an object");
    let ty = o.remove("type").expect("tagged by type");
    o.insert("op".into(), "add".into());
    o.insert("graphic".into(), ty);
    v
}

/// A two-pin part, standing up, pin 1 on top (a diode's and an LED's
/// cathode, pin 1, below, as KiCad numbers them).
fn two_pin(class: Class, st: &Standards) -> Option<Symbol> {
    let p = |n: &str, name: &str, y: i64, len: i64| {
        let side = if y > 0 { Side::Up } else { Side::Down };
        (n.to_owned(), pin(name, [0, y], side, len, PinType::Passive))
    };
    let (pins, graphics) = match class {
        Class::Resistor => (
            vec![
                p("1", "~", 3_810_000, 1_270_000),
                p("2", "~", -3_810_000, 1_270_000),
            ],
            vec![resistor_body(st.shapes, [0, 2_540_000], [0, -2_540_000]).expect("on one line")],
        ),
        Class::Capacitor => (
            vec![
                p("1", "~", 2_540_000, 2_159_000),
                p("2", "~", -2_540_000, 2_159_000),
            ],
            vec![
                line(&[[-1_524_000, 381_000], [1_524_000, 381_000]]),
                line(&[[-1_524_000, -381_000], [1_524_000, -381_000]]),
            ],
        ),
        Class::Inductor => {
            let mut humps = vec![[0, 2_540_000]];
            for i in 0..4 {
                let y = 2_540_000 - 1_270_000 * i;
                humps.extend([
                    [762_000, y - 254_000],
                    [762_000, y - 1_016_000],
                    [0, y - 1_270_000],
                ]);
            }
            (
                vec![
                    p("1", "1", 3_810_000, 1_270_000),
                    p("2", "2", -3_810_000, 1_270_000),
                ],
                vec![line(&humps)],
            )
        }
        Class::Diode | Class::Led => {
            let mut g = vec![
                line(&[
                    [-1_270_000, 1_270_000],
                    [1_270_000, 1_270_000],
                    [0, -1_270_000],
                    [-1_270_000, 1_270_000],
                ]),
                line(&[[-1_270_000, -1_270_000], [1_270_000, -1_270_000]]),
            ];
            if class == Class::Led {
                g.push(line(&[[1_778_000, 254_000], [2_794_000, -762_000]]));
                g.push(line(&[[2_286_000, 1_016_000], [3_302_000, 0]]));
            }
            (
                vec![
                    p("1", "K", -3_810_000, 2_540_000),
                    p("2", "A", 3_810_000, 2_540_000),
                ],
                g,
            )
        }
        _ => return None,
    };
    Some(Symbol {
        name: String::new(),
        units: 1,
        class: Some(class),
        pins: pins.into_iter().collect(),
        graphics: numbered(graphics),
    })
}

/// A symbol called `name` of `class`, drawn to `st`; `pins` places a
/// box's or an op-amp's pins (two-pin templates draw their own).
pub fn template(name: &str, class: Class, pins: &[PinSpec], st: &Standards) -> Result<Symbol> {
    template_units(name, class, 1, pins, st)
}

/// [`template`] of `units` units: an op-amp's amplifiers (1 to 4); every
/// other template draws one.
pub fn template_units(
    name: &str,
    class: Class,
    units: u32,
    pins: &[PinSpec],
    st: &Standards,
) -> Result<Symbol> {
    if units != 1 && class != Class::Opamp {
        return Err(Error::new(
            Code::InvalidParam,
            format!("a {} template draws one unit", class.as_str()),
        )
        .with("param", "units"));
    }
    let mut s = match class {
        Class::Resistor | Class::Capacitor | Class::Inductor | Class::Diode | Class::Led => {
            if !pins.is_empty() {
                return Err(Error::new(
                    Code::InvalidParam,
                    format!(
                        "a {} template draws its own two pins; give no pins",
                        class.as_str()
                    ),
                )
                .with("param", "pins"));
            }
            two_pin(class, st).expect("a two-pin class")
        }
        Class::Ic | Class::Regulator | Class::Connector => {
            if let Some(i) = pins.iter().position(|p| p.unit.is_some()) {
                return Err(
                    Error::new(Code::InvalidParam, "a box template draws one unit")
                        .with("pointer", format!("/pins/{i}/unit")),
                );
            }
            boxed(class, pins, st)?
        }
        Class::Opamp => opamp(pins, units, st)?,
        Class::Transistor => {
            return Err(Error::new(
                Code::InvalidParam,
                format!(
                    "there is no template for {}: draw it with ops",
                    class.as_str()
                ),
            )
            .with("param", "template")
            .with(
                "templates",
                TEMPLATES.iter().map(|c| c.as_str()).collect::<Vec<_>>(),
            ));
        }
    };
    s.name = name.to_owned();
    // Standards may ask for more than a symbol holds (a 50 mm pin on 64
    // rows): refused with the symbol's own fault, never written.
    if let Some(f) = s.check().into_iter().find(|f| f.is_error()) {
        return Err(f.error);
    }
    Ok(s)
}

/// Whether a pin's name is drawn: not empty, `~`, its number or KiCad's
/// `Pin_<n>` (as `schem` decides).
fn shown(s: &PinSpec) -> bool {
    !(s.name.is_empty()
        || s.name == "~"
        || s.name == s.number
        || s.name == format!("Pin_{}", s.number))
}

/// The pins given, checked: some, each numbered once, as pins are.
fn checked(pins: &[PinSpec]) -> Result<()> {
    if pins.is_empty() {
        return Err(Error::new(
            Code::InvalidParam,
            "a box template places the pins given; give some",
        )
        .with("param", "pins"));
    }
    let mut seen = std::collections::BTreeSet::new();
    for (i, p) in pins.iter().enumerate() {
        if !crate::symbol::pin_number_ok(&p.number) {
            return Err(Error::new(Code::InvalidParam, format!("pin number {:?} has white space, a control character or /, or is longer than 32", p.number))
                .with("pointer", format!("/pins/{i}/number")));
        }
        if !seen.insert(p.number.as_str()) {
            return Err(Error::new(
                Code::DuplicateId,
                format!("pin {} is given twice", p.number),
            )
            .with("pointer", format!("/pins/{i}/number")));
        }
    }
    Ok(())
}

/// A box: the pins given placed by `class`'s side rules (a role's side;
/// the rest, a group's pins together and a numbered group in order, each
/// run to the shorter of left and right), left and right rows two grid
/// steps apart from the top, top and bottom pins four apart about the
/// middle, the body grown until every name fits with the margin. A
/// connector is one column on the left, in the order given.
fn boxed(class: Class, pins: &[PinSpec], st: &Standards) -> Result<Symbol> {
    checked(pins)?;
    let g = st.grid_nm;
    let p = 2 * g;
    let len = st.pin_length_nm;
    let m = st.body_margin_nm;
    // Up to the next multiple of `p` (a step of two grid squares).
    let up = |v: i64| (v + p - 1).div_euclid(p) * p;
    let width = |s: &PinSpec| {
        if shown(s) {
            ryusch::font::width(&s.name, st.pin_name_size_nm)
        } else {
            0
        }
    };
    let widest = |l: &[&PinSpec]| l.iter().map(|s| width(s)).max().unwrap_or(0);
    let name_of = |s: &PinSpec| {
        if s.name.is_empty() {
            "~".to_owned()
        } else {
            s.name.clone()
        }
    };
    let mut out = BTreeMap::new();
    if class == Class::Connector {
        let all: Vec<&PinSpec> = pins.iter().collect();
        let right = -g + up(g / 2 + widest(&all) + m).max(p);
        for (i, s) in pins.iter().enumerate() {
            out.insert(
                s.number.clone(),
                pin(
                    &name_of(s),
                    [-g - len, -p * i as i64],
                    Side::Left,
                    len,
                    s.ty,
                ),
            );
        }
        let n = pins.len() as i64;
        return Ok(Symbol {
            name: String::new(),
            units: 1,
            class: Some(class),
            pins: out,
            graphics: numbered(vec![Graphic::Rect {
                start: [-g, g],
                end: [right, -p * n + g],
                fill: Fill::Body,
                unit: None,
            }]),
        });
    }
    let rule = st.rule(class);
    let (mut left, mut right, mut top, mut bottom): (
        Vec<&PinSpec>,
        Vec<&PinSpec>,
        Vec<&PinSpec>,
        Vec<&PinSpec>,
    ) = Default::default();
    let mut free = Vec::new();
    for s in pins {
        match st
            .role(&s.name, s.ty)
            .and_then(|r| rule.sides.get(&r).copied())
        {
            Some(Side::Left) => left.push(s),
            Some(Side::Right) => right.push(s),
            Some(Side::Up) => top.push(s),
            Some(Side::Down) => bottom.push(s),
            None => free.push(s),
        }
    }
    // The free pins in runs: a group's pins where its first stands, in
    // number order for a numbered group; every other pin a run of one.
    let mut runs: Vec<Vec<&PinSpec>> = Vec::new();
    let mut run_of: BTreeMap<String, usize> = BTreeMap::new();
    for s in free {
        match rule.groups.then(|| st.group_of(&s.name)).flatten() {
            Some((group, _)) => match run_of.get(group) {
                Some(&i) => runs[i].push(s),
                None => {
                    run_of.insert(group.to_owned(), runs.len());
                    runs.push(vec![s]);
                }
            },
            None => runs.push(vec![s]),
        }
    }
    for mut run in runs {
        run.sort_by_key(|s| st.group_of(&s.name).and_then(|(_, n)| n));
        if right.len() < left.len() {
            right.extend(run);
        } else {
            left.extend(run);
        }
    }
    // Room for the names reading in from the top and bottom, past the
    // margin; wide enough for a left and a right name apart by twice it.
    let room = |l: &[&PinSpec]| {
        if l.is_empty() {
            0
        } else {
            up(widest(l) + g + m).max(p)
        }
    };
    let (top_room, bottom_room) = (room(&top), room(&bottom));
    let rows = left.len().max(right.len()).max(1) as i64;
    let h = up(p * (rows + 1) + top_room + bottom_room);
    let across = |n: usize| 2 * p * (n.max(1) as i64 - 1) + 2 * p;
    let w = [
        4 * p,
        up(widest(&left) + widest(&right) + g + (2 * m).max(g)),
        across(top.len()),
        across(bottom.len()),
    ]
    .into_iter()
    .max()
    .expect("four widths");
    let (half_w, half_h) = (up(w / 2), up(h / 2));
    let first = (half_h - p - top_room).div_euclid(p) * p;
    for (i, s) in left.iter().enumerate() {
        out.insert(
            s.number.clone(),
            pin(
                &name_of(s),
                [-half_w - len, first - p * i as i64],
                Side::Left,
                len,
                s.ty,
            ),
        );
    }
    for (i, s) in right.iter().enumerate() {
        out.insert(
            s.number.clone(),
            pin(
                &name_of(s),
                [half_w + len, first - p * i as i64],
                Side::Right,
                len,
                s.ty,
            ),
        );
    }
    for (list, y, side) in [
        (&top, half_h + len, Side::Up),
        (&bottom, -half_h - len, Side::Down),
    ] {
        let start = -p * (list.len() as i64 - 1);
        for (i, s) in list.iter().enumerate() {
            out.insert(
                s.number.clone(),
                pin(&name_of(s), [start + 2 * p * i as i64, y], side, len, s.ty),
            );
        }
    }
    Ok(Symbol {
        name: String::new(),
        units: 1,
        class: Some(class),
        pins: out,
        graphics: numbered(vec![Graphic::Rect {
            start: [-half_w, half_h],
            end: [half_w, -half_h],
            fill: Fill::Body,
            unit: None,
        }]),
    })
}

/// Whether an op-amp's input is its inverting one (as `ryusch::topology`
/// reads it).
fn inverting(name: &str) -> bool {
    let n = name.to_ascii_uppercase();
    n.ends_with('-') || n.contains("IN-") || n.contains("INN")
}

/// Whether an op-amp's input is its non-inverting one.
fn non_inverting(name: &str) -> bool {
    let n = name.to_ascii_uppercase();
    n.ends_with('+') || n.contains("IN+") || n.contains("INP")
}

/// An op-amp (#758, design 519): each unit a triangle pointing right,
/// `(-h, h)`, `(h, 0)`, `(-h, -h)`, filled, `h` 6g (8g with three
/// supplies on the bottom); the inverting input's
/// base at `(-h, 2g)` above the non-inverting one's at `(-h, -2g)`, up
/// to two more inputs at `(-h, -4g)` then `(-h, 4g)` (below first, clear
/// of the way from the inverting input up to a stage's feedback), the output at the
/// apex; supplies, common to every unit and drawn on the first, from the
/// slanted edges (positive on the top, up to two; negative and ground on
/// the bottom, up to three), four steps apart, their ends in one row a pin
/// length past the triangle's box. The edges fall a grid step every two
/// across, so each base is on them and each end on the grid.
fn opamp(pins: &[PinSpec], units: u32, st: &Standards) -> Result<Symbol> {
    checked(pins)?;
    if !(1..=4).contains(&units) {
        return Err(Error::new(
            Code::InvalidParam,
            format!("an op-amp template draws 1 to 4 amplifiers, not {units}"),
        )
        .with("param", "units"));
    }
    let g = st.grid_nm;
    let len = st.pin_length_nm;
    // The triangle's half size: 6g, or 8g when three supplies stand on
    // the bottom edge, so each keeps its name inside, four steps apart.
    let down = |s: &PinSpec| {
        let name = s.name.to_ascii_uppercase();
        st.role(&s.name, s.ty) == Some(crate::standards::Role::Ground)
            || name.ends_with('-')
            || name.starts_with('-')
    };
    let supply = |s: &PinSpec| matches!(s.ty, PinType::PowerIn | PinType::PowerOut);
    let h = if pins.iter().filter(|s| supply(s) && down(s)).count() >= 3 {
        8 * g
    } else {
        6 * g
    };
    let refuse = |i: usize, why: String| {
        Error::new(Code::InvalidParam, why).with("pointer", format!("/pins/{i}"))
    };
    let mut out = BTreeMap::new();
    let (mut top, mut bottom): (Vec<String>, Vec<String>) = Default::default();
    // Per unit: inverting, non-inverting, output, extra inputs so far.
    let mut seen: BTreeMap<u32, (bool, bool, bool, usize)> = BTreeMap::new();
    for (i, s) in pins.iter().enumerate() {
        let power = matches!(s.ty, PinType::PowerIn | PinType::PowerOut);
        let unit = match (s.unit, power) {
            (Some(_), true) => {
                return Err(refuse(
                    i,
                    format!(
                        "supply {} is common to every unit: give it no unit",
                        s.number
                    ),
                ));
            }
            (None, true) => None,
            (None, false) if units == 1 => Some(1),
            (None, false) => {
                return Err(refuse(
                    i,
                    format!("pin {} needs its unit (1 to {units})", s.number),
                ));
            }
            (Some(u), false) if (1..=units).contains(&u) => Some(u),
            (Some(u), false) => {
                return Err(refuse(
                    i,
                    format!(
                        "pin {} is in unit {u}, but the template draws {units}",
                        s.number
                    ),
                ));
            }
        };
        let (at, side) = match unit {
            None => {
                let down = down(s);
                let (list, cap) = if down {
                    (&mut bottom, 3)
                } else {
                    (&mut top, 2)
                };
                if list.len() == cap {
                    return Err(refuse(
                        i,
                        format!(
                            "an op-amp template places at most {cap} supplies on its {} edge",
                            if down { "bottom" } else { "top" }
                        ),
                    ));
                }
                list.push(s.number.clone());
                // Placed once every supply is counted, below.
                ([0, 0], if down { Side::Down } else { Side::Up })
            }
            Some(u) => {
                let e = seen.entry(u).or_default();
                match s.ty {
                    PinType::Output | PinType::TriState if !e.2 => {
                        e.2 = true;
                        ([h + len, 0], Side::Right)
                    }
                    PinType::Input if inverting(&s.name) && !e.0 => {
                        e.0 = true;
                        ([-h - len, 2 * g], Side::Left)
                    }
                    PinType::Input if non_inverting(&s.name) && !e.1 => {
                        e.1 = true;
                        ([-h - len, -2 * g], Side::Left)
                    }
                    PinType::Input if e.3 < 2 => {
                        e.3 += 1;
                        let y = if e.3 == 1 { -4 * g } else { 4 * g };
                        ([-h - len, y], Side::Left)
                    }
                    _ => {
                        return Err(refuse(
                            i,
                            format!(
                                "an op-amp template has no place for pin {} ({}, {:?}) in unit {u}: \
                                 one inverting and one non-inverting input, one output and two \
                                 more inputs a unit, supplies with no unit",
                                s.number, s.name, s.ty
                            ),
                        ));
                    }
                }
            }
        };
        let name = if s.name.is_empty() { "~" } else { &s.name };
        let mut p = pin(name, at, side, len, s.ty);
        p.unit = if units == 1 { None } else { unit };
        out.insert(s.number.clone(), p);
    }
    // The supplies (plan 3.A's departure, #758): bases on the slanted
    // edge, where it stands at h/2 - x/2, four steps apart (x = -2g alone,
    // -4g and 0 for two, -6g, -2g and 2g for three), ends in one row a pin length past the triangle's
    // box, so no wire or label at an end meets the body.
    for (list, sign) in [(&top, 1), (&bottom, -1)] {
        for (k, number) in list.iter().enumerate() {
            // Three on the larger triangle, clear of the output's name by
            // the apex.
            let x = match list.len() {
                1 => -2 * g,
                2 => -4 * g + 4 * g * k as i64,
                _ => -6 * g + 4 * g * k as i64,
            };
            let edge = h / 2 - x / 2;
            let p = out.get_mut(number).expect("placed above");
            p.at = [x, sign * (h + len)];
            p.length_nm = h + len - edge;
        }
    }
    for u in 1..=units {
        let e = seen.get(&u).copied().unwrap_or_default();
        if !(e.0 && e.1 && e.2) {
            return Err(Error::new(
                Code::InvalidParam,
                format!("unit {u} needs an inverting input, a non-inverting input and an output"),
            )
            .with("param", "pins"));
        }
    }
    let triangle = |unit| Graphic::Polyline {
        points: vec![[-h, h], [h, 0], [-h, -h], [-h, h]],
        fill: Fill::Body,
        unit,
    };
    let graphics = if units == 1 {
        vec![triangle(None)]
    } else {
        (1..=units).map(|u| triangle(Some(u))).collect()
    };
    Ok(Symbol {
        name: String::new(),
        units,
        class: Some(Class::Opamp),
        pins: out,
        graphics: numbered(graphics),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn st(shapes: Shapes) -> Standards {
        Standards {
            shapes,
            ..Standards::default()
        }
    }

    #[test]
    fn two_pin_parts_are_drawn_in_full_standing_up() {
        for class in [
            Class::Resistor,
            Class::Capacitor,
            Class::Inductor,
            Class::Diode,
            Class::Led,
        ] {
            let s = template("X", class, &[], &st(Shapes::Ansi)).unwrap();
            assert_eq!((s.name.as_str(), s.units, s.class), ("X", 1, Some(class)));
            assert!(s.check().is_empty(), "{class:?}: {:?}", s.check());
            assert_eq!(s.pins.len(), 2);
            let top = s.pins.values().find(|p| p.side == Side::Up).unwrap();
            let bottom = s.pins.values().find(|p| p.side == Side::Down).unwrap();
            assert_eq!(top.at[0], 0);
            assert_eq!(bottom.at[0], 0);
            assert!(top.at[1] > 0 && bottom.at[1] < 0);
            for p in s.pins.values() {
                assert_eq!(p.at[1] % ryusch::geom::GRID, 0, "{class:?}");
            }
        }
        // The resistor follows the standards' shapes.
        let us = template("R", Class::Resistor, &[], &st(Shapes::Ansi)).unwrap();
        assert!(
            matches!(us.graphics.values().next(), Some(Graphic::Polyline { points, .. }) if points.len() == 8)
        );
        let iec = template("R", Class::Resistor, &[], &st(Shapes::Iec)).unwrap();
        assert!(matches!(
            iec.graphics.values().next(),
            Some(Graphic::Rect { .. })
        ));
        // A diode's cathode is pin 1, below.
        let d = template("D", Class::Diode, &[], &st(Shapes::Ansi)).unwrap();
        assert_eq!(
            (d.pins["1"].name.as_str(), d.pins["1"].side),
            ("K", Side::Down)
        );
        // Two-pin templates take no pins; some classes have no template.
        let pin = PinSpec {
            number: "1".into(),
            name: "A".into(),
            ty: PinType::Passive,
            unit: None,
        };
        assert_eq!(
            template("R", Class::Resistor, &[pin], &st(Shapes::Ansi))
                .unwrap_err()
                .code,
            Code::InvalidParam
        );
        let e = template("Q", Class::Transistor, &[], &st(Shapes::Ansi)).unwrap_err();
        assert_eq!(e.code, Code::InvalidParam);
        assert!(
            e.details["templates"]
                .as_array()
                .unwrap()
                .contains(&serde_json::json!("ic"))
        );
    }

    #[test]
    fn a_resistors_body_spans_its_pins_bases_either_way() {
        let z = resistor_body(Shapes::Ansi, [0, 2_540_000], [0, -2_540_000]).unwrap();
        let Graphic::Polyline { points, .. } = z else {
            panic!()
        };
        assert_eq!(points[1], [1_016_000, 2_540_000 - 423_333]);
        assert_eq!(points[2], [-1_016_000, 1_270_000]);
        assert_eq!(points[7], [0, -2_540_000]);
        let flat = resistor_body(Shapes::Iec, [-2_540_000, 0], [2_540_000, 0]).unwrap();
        assert_eq!(
            flat,
            Graphic::Rect {
                start: [-2_540_000, 1_016_000],
                end: [2_540_000, -1_016_000],
                fill: Fill::None,
                unit: None
            }
        );
        assert!(resistor_body(Shapes::Ansi, [0, 0], [1, 1]).is_none());
        assert!(resistor_body(Shapes::Ansi, [0, 0], [0, 0]).is_none());
    }

    fn spec(number: &str, name: &str, ty: PinType) -> PinSpec {
        PinSpec {
            number: number.into(),
            name: name.into(),
            ty,
            unit: None,
        }
    }

    /// An MCU's pins in a muddled order: power, ground, GPIOs out of
    /// order, I2C, an input and an output.
    fn mcu() -> Vec<PinSpec> {
        let mut v = vec![
            spec("1", "GPIO3", PinType::Bidirectional),
            spec("2", "VDD", PinType::PowerIn),
            spec("3", "SDA", PinType::Bidirectional),
            spec("4", "GPIO1", PinType::Bidirectional),
            spec("5", "GND", PinType::PowerIn),
            spec("6", "EN", PinType::Input),
            spec("7", "TX", PinType::Output),
            spec("8", "GPIO2", PinType::Bidirectional),
            spec("9", "SCL", PinType::Bidirectional),
            spec("10", "IOVDD", PinType::PowerIn),
            spec("11", "AGND", PinType::PowerIn),
        ];
        v.push(spec("12", "GPIO0", PinType::Bidirectional));
        v
    }

    #[test]
    fn an_ics_pins_stand_by_role_and_group_on_the_grid() {
        let st = Standards::default();
        let s = template("MCU", Class::Ic, &mcu(), &st).unwrap();
        assert!(s.check().is_empty(), "{:?}", s.check());
        let side = |n: &str| s.pins[n].side;
        assert_eq!((side("2"), side("10")), (Side::Up, Side::Up));
        assert_eq!((side("5"), side("11")), (Side::Down, Side::Down));
        assert_eq!(side("6"), Side::Left);
        assert_eq!(side("7"), Side::Right);
        // The GPIOs stand together, in number order down their side.
        let gpio: Vec<&Pin> = ["12", "4", "8", "1"].iter().map(|n| &s.pins[*n]).collect();
        assert!(gpio.iter().all(|p| p.side == gpio[0].side));
        assert!(
            gpio.windows(2)
                .all(|w| w[0].at[1] - w[1].at[1] == 2 * st.grid_nm)
        );
        // So do SDA and SCL.
        assert_eq!(side("3"), side("9"));
        assert_eq!(
            (s.pins["3"].at[1] - s.pins["9"].at[1]).abs(),
            2 * st.grid_nm
        );
        for p in s.pins.values() {
            assert_eq!(p.at[0] % st.grid_nm, 0);
            assert_eq!(p.at[1] % st.grid_nm, 0);
            assert_eq!(p.length_nm, st.pin_length_nm);
        }
        // One body, filled.
        assert!(matches!(
            s.graphics.values().collect::<Vec<_>>().as_slice(),
            [Graphic::Rect {
                fill: Fill::Body,
                ..
            }]
        ));
    }

    #[test]
    fn a_regulators_supply_runs_left_to_right_and_a_connector_is_one_column() {
        let st = Standards::default();
        let reg = template(
            "LDO",
            Class::Regulator,
            &[
                spec("1", "VIN", PinType::PowerIn),
                spec("2", "GND", PinType::PowerIn),
                spec("3", "VOUT", PinType::PowerOut),
                spec("4", "EN", PinType::Input),
            ],
            &st,
        )
        .unwrap();
        let side = |n: &str| reg.pins[n].side;
        assert_eq!(
            [side("1"), side("2"), side("3"), side("4")],
            [Side::Left, Side::Down, Side::Right, Side::Left]
        );
        let j = template(
            "J",
            Class::Connector,
            &[
                spec("1", "~", PinType::Passive),
                spec("2", "~", PinType::Passive),
                spec("3", "~", PinType::Passive),
            ],
            &st,
        )
        .unwrap();
        assert!(j.check().is_empty());
        let ys: Vec<i64> = ["1", "2", "3"].iter().map(|n| j.pins[*n].at[1]).collect();
        assert_eq!(ys, [0, -2_540_000, -5_080_000]);
        assert!(
            j.pins
                .values()
                .all(|p| p.side == Side::Left && p.at[0] == -3_810_000)
        );
        // Pins are given, numbered once each.
        assert_eq!(
            template("U", Class::Ic, &[], &st).unwrap_err().code,
            Code::InvalidParam
        );
        let twice = [
            spec("1", "A", PinType::Input),
            spec("1", "B", PinType::Input),
        ];
        let e = template("U", Class::Ic, &twice, &st).unwrap_err();
        assert_eq!(
            (e.code, &e.details["pointer"]),
            (Code::DuplicateId, &serde_json::json!("/pins/1/number"))
        );
    }

    fn amp(unit: Option<u32>, inv: &str, non: &str, out: &str) -> Vec<PinSpec> {
        [
            (inv, "-", PinType::Input),
            (non, "+", PinType::Input),
            (out, "~", PinType::Output),
        ]
        .iter()
        .map(|(n, name, ty)| PinSpec {
            number: (*n).into(),
            name: (*name).into(),
            ty: *ty,
            unit,
        })
        .collect()
    }

    fn supply(number: &str, name: &str) -> PinSpec {
        PinSpec {
            number: number.into(),
            name: name.into(),
            ty: PinType::PowerIn,
            unit: None,
        }
    }

    /// #758: a triangle pointing right, the inverting input above the
    /// non-inverting one, the output at the apex, supplies on the slanted
    /// edges, every pin the standards' length with its end on the grid.
    #[test]
    fn an_op_amp_is_a_triangle_with_its_supplies_on_its_edges() {
        let st = Standards::default();
        let g = st.grid_nm;
        let mut pins = amp(None, "2", "3", "1");
        pins.extend([supply("8", "V+"), supply("4", "V-")]);
        let s = template_units("OPA", Class::Opamp, 1, &pins, &st).unwrap();
        assert!(s.check().is_empty(), "{:?}", s.check());
        assert_eq!((s.units, s.class), (1, Some(Class::Opamp)));
        let at = |n: &str| (s.pins[n].at, s.pins[n].side);
        let len = st.pin_length_nm;
        assert_eq!(at("2"), ([-6 * g - len, 2 * g], Side::Left));
        assert_eq!(at("3"), ([-6 * g - len, -2 * g], Side::Left));
        assert_eq!(at("1"), ([6 * g + len, 0], Side::Right));
        // The supplies end a pin length past the triangle's box, their
        // bases on its edges (plan 3.A's departure: an end inside the box
        // left its label on the body).
        assert_eq!(at("8"), ([-2 * g, 6 * g + len], Side::Up));
        assert_eq!(at("4"), ([-2 * g, -6 * g - len], Side::Down));
        assert_eq!(crate::symlint::base(&s.pins["8"]), [-2 * g, 4 * g]);
        assert_eq!(crate::symlint::base(&s.pins["4"]), [-2 * g, -4 * g]);
        for (n, p) in &s.pins {
            assert_eq!((p.at[0] % g, p.at[1] % g), (0, 0));
            if !["8", "4"].contains(&n.as_str()) {
                assert_eq!(p.length_nm, len);
            }
        }
        assert_eq!(
            s.graphics.values().collect::<Vec<_>>(),
            [&Graphic::Polyline {
                points: vec![
                    [-6 * g, 6 * g],
                    [6 * g, 0],
                    [-6 * g, -6 * g],
                    [-6 * g, 6 * g]
                ],
                fill: Fill::Body,
                unit: None,
            }]
        );
    }

    /// A dual is two triangles; its supplies are common pins, drawn on
    /// unit 1. The OPA1622's enable stands on the left below the inputs,
    /// its two V- and its GND along the bottom edge.
    #[test]
    fn a_dual_op_amp_is_two_units_with_the_supplies_on_the_first() {
        let st = Standards::default();
        let g = st.grid_nm;
        let mut pins = amp(Some(1), "10", "1", "9");
        pins.push(PinSpec {
            number: "8".into(),
            name: "EN".into(),
            ty: PinType::Input,
            unit: Some(1),
        });
        pins.extend(amp(Some(2), "6", "5", "7"));
        pins.extend([
            supply("2", "V+"),
            supply("4", "V-"),
            supply("11", "V-"),
            supply("3", "GND"),
        ]);
        let s = template_units("OPA1622", Class::Opamp, 2, &pins, &st).unwrap();
        assert!(s.check().is_empty(), "{:?}", s.check());
        assert_eq!(s.units, 2);
        // Three bottom supplies: the triangle is 8g each way from its
        // middle.
        // The enable stands below the non-inverting input, clear of the
        // way from the inverting one up to the feedback.
        assert_eq!(s.pins["8"].at, [-8 * g - st.pin_length_nm, -4 * g]);
        // Four steps apart, ends in one row, bases on the bottom edge.
        let bottom: Vec<Point> = ["4", "11", "3"].iter().map(|n| s.pins[*n].at).collect();
        let row = -8 * g - st.pin_length_nm;
        assert_eq!(bottom, [[-6 * g, row], [-2 * g, row], [2 * g, row]]);
        let bases: Vec<Point> = ["4", "11", "3"]
            .iter()
            .map(|n| crate::symlint::base(&s.pins[*n]))
            .collect();
        assert_eq!(bases, [[-6 * g, -7 * g], [-2 * g, -5 * g], [2 * g, -3 * g]]);
        assert!(
            ["2", "4", "11", "3"]
                .iter()
                .all(|n| s.pins[*n].unit.is_none())
        );
        assert_eq!(crate::symlint::home(&s), 1);
        let triangles: Vec<Option<u32>> = s.graphics.values().map(|x| x.unit()).collect();
        assert_eq!(triangles, [Some(1), Some(2)]);
    }

    #[test]
    fn an_op_amp_template_refuses_what_it_cannot_place() {
        let st = Standards::default();
        let refused = |pins: &[PinSpec], units: u32| {
            template_units("A", Class::Opamp, units, pins, &st)
                .unwrap_err()
                .code
        };
        // No inverting input.
        let mut pins = amp(None, "2", "3", "1");
        pins[0].name = "X".into();
        pins[0].ty = PinType::Bidirectional;
        assert_eq!(refused(&pins, 1), Code::InvalidParam);
        // Unit 2 has no pins.
        assert_eq!(refused(&amp(Some(1), "2", "3", "1"), 2), Code::InvalidParam);
        // A supply in a unit.
        let mut pins = amp(None, "2", "3", "1");
        pins.push(PinSpec {
            unit: Some(1),
            ..supply("8", "V+")
        });
        assert_eq!(refused(&pins, 1), Code::InvalidParam);
        // Three top supplies.
        let mut pins = amp(None, "2", "3", "1");
        pins.extend([supply("4", "V+"), supply("5", "VCC"), supply("6", "VDD")]);
        assert_eq!(refused(&pins, 1), Code::InvalidParam);
        // Five units.
        assert_eq!(refused(&amp(None, "2", "3", "1"), 5), Code::InvalidParam);
        // A unit on a box template's pin.
        let mut pins = amp(Some(1), "2", "3", "1");
        pins.truncate(1);
        assert_eq!(
            template("U", Class::Ic, &pins, &st).unwrap_err().code,
            Code::InvalidParam
        );
    }
}
