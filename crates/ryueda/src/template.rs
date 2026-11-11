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
use serde::Deserialize;
use std::collections::BTreeMap;

/// The classes that have a template.
pub const TEMPLATES: [Class; 8] = [
    Class::Resistor,
    Class::Capacitor,
    Class::Inductor,
    Class::Diode,
    Class::Led,
    Class::Connector,
    Class::Regulator,
    Class::Ic,
];

/// A pin a box template places: its number, name and type.
#[derive(Clone, Debug, PartialEq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PinSpec {
    pub number: String,
    #[serde(default)]
    pub name: String,
    #[serde(rename = "type")]
    pub ty: PinType,
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
/// box's pins (two-pin templates draw their own).
pub fn template(name: &str, class: Class, pins: &[PinSpec], st: &Standards) -> Result<Symbol> {
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
        Class::Ic | Class::Regulator | Class::Connector => boxed(class, pins, st)?,
        Class::Opamp | Class::Transistor => {
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

/// Task 8 replaces this with the boxes.
fn boxed(class: Class, _pins: &[PinSpec], _st: &Standards) -> Result<Symbol> {
    Err(Error::new(
        Code::InvalidParam,
        format!("no {} template yet", class.as_str()),
    ))
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
}
