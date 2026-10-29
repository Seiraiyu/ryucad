//! `.kicad_sym` symbol libraries (KiCad 6 to 9). Each symbol becomes a
//! ryueda symbol and its properties; a derived symbol (`extends`) keeps
//! only its properties and draws its parent. KiCad's symbol y axis
//! already points up. Sub-symbols `<name>_<unit>_<style>` give units
//! (unit 0 is every unit's) and body styles: style 2, De Morgan's, is
//! dropped. Pins become ordinary pins, hidden or not; nothing connects
//! implicitly. Colours, widths and fonts are the style's, not data.

use super::sexpr::{self, Sexpr, unreadable};
use super::{Notes, hidden, yes};
use crate::error::{Code, Error, Result};
use crate::id::Id;
use crate::symbol::{Alternate, Fill, Graphic, Pin, PinShape, PinType, Point, Side, Symbol};
use std::collections::BTreeMap;

/// One symbol of a library, as read.
#[derive(Clone, Debug, PartialEq)]
pub struct Item {
    /// The symbol it is derived from, in the same library.
    pub extends: Option<String>,
    /// A power symbol: KiCad's global label in a symbol's clothes.
    pub power: bool,
    /// Whether a De Morgan body style was dropped.
    pub demorgan: bool,
    /// The drawing; `None` for a derived symbol, which draws its parent.
    pub symbol: Option<Symbol>,
    /// Every property, as written, in the file's order.
    pub properties: Vec<(String, String)>,
}

/// A library file, read: each symbol by name (or why it can't be read),
/// in the file's order, and what reading found besides.
#[derive(Clone, Debug, PartialEq)]
pub struct Library {
    pub items: Vec<(String, Result<Item>)>,
    pub notes: Notes,
}

/// Reads a `.kicad_sym` file. Fails as a whole only when it isn't a
/// symbol library: a legacy `.lib` or `.dcm` is
/// `kicad_legacy_unsupported`, anything else `kicad_unreadable`.
pub fn read(text: &str) -> Result<Library> {
    let start = text.trim_start();
    if start.starts_with("EESchema-LIBRARY") || start.starts_with("EESchema-DOCLIB") {
        return Err(Error::new(
            Code::KicadLegacyUnsupported,
            "a KiCad 5 symbol library (.lib or .dcm); open and save it in KiCad 6 or later",
        ));
    }
    let top = sexpr::parse(text)?;
    if top.head() != Some("kicad_symbol_lib") {
        return Err(unreadable(
            top.pos(),
            "not a KiCad symbol library (kicad_symbol_lib)",
        ));
    }
    let mut notes = Notes::default();
    super::version(&top, super::SYMBOL_VERSION, &mut notes)?;
    let mut items = Vec::new();
    for node in top.items() {
        match node.head() {
            Some("symbol") => {
                let name = node.arg(0, "the symbol's name").unwrap_or("").to_owned();
                items.push((name, item(node, &mut notes)));
            }
            Some("version" | "generator" | "generator_version") => {}
            Some(h) => notes.ignore(h),
            None => return Err(unreadable(node.pos(), "a list was expected")),
        }
    }
    Ok(Library { items, notes })
}

fn pin_type(node: &Sexpr, t: &str) -> Result<PinType> {
    Ok(match t {
        "input" => PinType::Input,
        "output" => PinType::Output,
        "bidirectional" => PinType::Bidirectional,
        "tri_state" => PinType::TriState,
        "passive" => PinType::Passive,
        "free" => PinType::Free,
        "unspecified" => PinType::Unspecified,
        "power_in" => PinType::PowerIn,
        "power_out" => PinType::PowerOut,
        "open_collector" => PinType::OpenCollector,
        "open_emitter" => PinType::OpenEmitter,
        "no_connect" => PinType::NoConnect,
        t => {
            return Err(unreadable(
                node.pos(),
                format!("pin type {t:?} isn't KiCad's"),
            ));
        }
    })
}

fn pin_shape(node: &Sexpr, t: &str) -> Result<PinShape> {
    Ok(match t {
        "line" => PinShape::Line,
        "inverted" => PinShape::Inverted,
        "clock" => PinShape::Clock,
        "inverted_clock" => PinShape::InvertedClock,
        "input_low" => PinShape::InputLow,
        "clock_low" => PinShape::ClockLow,
        "output_low" => PinShape::OutputLow,
        "edge_clock_high" => PinShape::EdgeClockHigh,
        "non_logic" => PinShape::NonLogic,
        t => {
            return Err(unreadable(
                node.pos(),
                format!("pin shape {t:?} isn't KiCad's"),
            ));
        }
    })
}

/// A point `(at x y …)` or `(xy x y)`, in nanometres.
fn point(node: &Sexpr, notes: &mut Notes) -> Result<Point> {
    Ok([notes.mm_at(node, 0, "x")?, notes.mm_at(node, 1, "y")?])
}

/// The child `name`'s point.
fn point_of(node: &Sexpr, name: &str, notes: &mut Notes) -> Result<Point> {
    match node.child(name) {
        Some(c) => point(c, notes),
        None => Err(unreadable(node.pos(), format!("({name} …) is missing"))),
    }
}

/// `(pts (xy …) …)`'s points.
fn points(node: &Sexpr, notes: &mut Notes) -> Result<Vec<Point>> {
    let pts = node
        .child("pts")
        .ok_or_else(|| unreadable(node.pos(), "(pts …) is missing"))?;
    pts.items()
        .iter()
        .map(|p| match p.head() {
            Some("xy") => point(p, notes),
            _ => Err(unreadable(p.pos(), "a symbol's points are (xy x y)")),
        })
        .collect()
}

fn fill(node: &Sexpr, notes: &mut Notes) -> Fill {
    let Some(f) = node.child("fill") else {
        return Fill::None;
    };
    if f.child("color").is_some() {
        notes.ignore("fill.color");
    }
    match f
        .child("type")
        .and_then(|t| t.items().first())
        .and_then(Sexpr::text)
    {
        Some("outline") => Fill::Outline,
        Some("background") => Fill::Body,
        Some("color") => Fill::Body,
        Some("none") | None => Fill::None,
        Some(other) => {
            notes.ignore(format!("fill.{other}"));
            Fill::None
        }
    }
}

/// `(sub)name_<unit>_<style>`'s unit and style.
fn unit_style(node: &Sexpr, top: &str, name: &str) -> Result<(u32, u32)> {
    let bad = || {
        unreadable(
            node.pos(),
            format!("sub-symbol {name:?} isn't {top}_<unit>_<style>"),
        )
    };
    let rest = name
        .strip_prefix(top)
        .and_then(|r| r.strip_prefix('_'))
        .ok_or_else(bad)?;
    let (u, s) = rest.split_once('_').ok_or_else(bad)?;
    Ok((u.parse().map_err(|_| bad())?, s.parse().map_err(|_| bad())?))
}

fn item(node: &Sexpr, notes: &mut Notes) -> Result<Item> {
    let name = node.arg(0, "the symbol's name")?;
    let mut it = Item {
        extends: None,
        power: false,
        demorgan: false,
        symbol: None,
        properties: Vec::new(),
    };
    let mut pins: BTreeMap<String, Pin> = BTreeMap::new();
    let mut graphics: BTreeMap<Id, Graphic> = BTreeMap::new();
    let mut units = 1;
    let mut drawn = false;
    for child in node.items().iter().skip(1) {
        match child.head() {
            Some("power") => it.power = true,
            Some("extends") => it.extends = Some(child.arg(0, "the parent's name")?.to_owned()),
            Some("property") => it.properties.push((
                child.arg(0, "the property's name")?.to_owned(),
                child.arg(1, "the property's value")?.to_owned(),
            )),
            Some("symbol") => {
                drawn = true;
                let sub = child.arg(0, "the sub-symbol's name")?;
                let (unit, style) = unit_style(child, name, sub)?;
                if style > 1 {
                    it.demorgan = true;
                    continue;
                }
                units = units.max(unit);
                let unit = (unit > 0).then_some(unit);
                for g in child.items().iter().skip(1) {
                    draw(g, unit, &mut pins, &mut graphics, notes)?;
                }
            }
            // Look, and KiCad's bookkeeping: the style's, not data.
            Some("pin_names" | "pin_numbers" | "embedded_fonts") => {}
            // KiCad's flags at their defaults say nothing.
            Some("in_bom" | "on_board") if yes(child) => {}
            Some("exclude_from_sim") if !yes(child) => {}
            Some(h) => notes.ignore(format!("symbol.{h}")),
            None => return Err(unreadable(child.pos(), "a list was expected")),
        }
    }
    if it.extends.is_some() {
        if drawn {
            notes.ignore("symbol.extends.symbol");
        }
        return Ok(it);
    }
    it.symbol = Some(Symbol {
        name: name.to_owned(),
        units,
        pins,
        graphics,
    });
    Ok(it)
}

/// One item of a sub-symbol: a pin or a graphic, into `unit`.
fn draw(
    g: &Sexpr,
    unit: Option<u32>,
    pins: &mut BTreeMap<String, Pin>,
    graphics: &mut BTreeMap<Id, Graphic>,
    notes: &mut Notes,
) -> Result<()> {
    let graphic = match g.head() {
        Some("pin") => {
            let (number, p) = pin(g, unit, notes)?;
            if pins.insert(number.clone(), p).is_some() {
                return Err(Error::new(
                    Code::LibraryPinInvalid,
                    format!("pin {number} appears more than once; ryueda gives a number one pin"),
                )
                .with("pin", number));
            }
            return Ok(());
        }
        Some("rectangle") => Graphic::Rect {
            start: point_of(g, "start", notes)?,
            end: point_of(g, "end", notes)?,
            fill: fill(g, notes),
            unit,
        },
        Some("polyline") => Graphic::Polyline {
            points: points(g, notes)?,
            fill: fill(g, notes),
            unit,
        },
        Some("arc") => {
            if g.child("mid").is_none() {
                return Err(unreadable(g.pos(), "an arc without its middle point"));
            }
            Graphic::Arc {
                start: point_of(g, "start", notes)?,
                mid: point_of(g, "mid", notes)?,
                end: point_of(g, "end", notes)?,
                fill: fill(g, notes),
                unit,
            }
        }
        Some("circle") => {
            let r = g
                .child("radius")
                .ok_or_else(|| unreadable(g.pos(), "(radius …) is missing"))?;
            Graphic::Circle {
                center: point_of(g, "center", notes)?,
                radius_nm: notes.mm_at(r, 0, "the radius")?,
                fill: fill(g, notes),
                unit,
            }
        }
        Some("bezier") => {
            let p = points(g, notes)?;
            let p: [Point; 4] = p
                .try_into()
                .map_err(|_| unreadable(g.pos(), "a Bézier has four points"))?;
            Graphic::Bezier {
                points: p,
                fill: fill(g, notes),
                unit,
            }
        }
        Some("text") => {
            let at = g
                .child("at")
                .ok_or_else(|| unreadable(g.pos(), "(at …) is missing"))?;
            let size = match g
                .child("effects")
                .and_then(|e| e.child("font"))
                .and_then(|f| f.child("size"))
            {
                Some(s) => notes.mm_at(s, 0, "the text's height")?,
                None => 1_270_000,
            };
            // A symbol's text angle is in tenths of a degree.
            let rotation = match at.items().get(2) {
                Some(a) => notes.scaled(a, 100)?,
                None => 0,
            };
            if hidden(g) {
                notes.ignore("text.hide");
            }
            Graphic::Text {
                text: g.arg(0, "the text")?.to_owned(),
                at: point(at, notes)?,
                size_nm: size,
                rotation_mdeg: rotation,
                unit,
            }
        }
        Some(h) => {
            notes.ignore(h);
            return Ok(());
        }
        None => return Err(unreadable(g.pos(), "a list was expected")),
    };
    let id = Id::parse(&format!("g{}", graphics.len() + 1), "graphic").expect("g<n> is an ID");
    graphics.insert(id, graphic);
    Ok(())
}

/// `(pin TYPE SHAPE (at x y angle) (length l) (name …) (number …)
/// (alternate …)…)`. KiCad's angle points from the connection point to
/// the body; ryueda's side points from the body to the end.
fn pin(g: &Sexpr, unit: Option<u32>, notes: &mut Notes) -> Result<(String, Pin)> {
    let ty = pin_type(g, g.arg(0, "the pin's type")?)?;
    let shape = pin_shape(g, g.arg(1, "the pin's shape")?)?;
    let at = g
        .child("at")
        .ok_or_else(|| unreadable(g.pos(), "(at …) is missing"))?;
    let angle = match at.items().get(2) {
        Some(a) => notes.deg(a)?,
        None => 0,
    };
    let side = match angle.rem_euclid(360_000) {
        0 => Side::Left,
        90_000 => Side::Down,
        180_000 => Side::Right,
        270_000 => Side::Up,
        _ => {
            return Err(unreadable(
                at.pos(),
                "a pin points along an axis: 0, 90, 180 or 270",
            ));
        }
    };
    let length = match g.child("length") {
        Some(l) => notes.mm_at(l, 0, "the pin's length")?,
        None => 0,
    };
    let text = |name: &str| -> Result<String> {
        let t = g
            .child(name)
            .ok_or_else(|| unreadable(g.pos(), format!("({name} …) is missing")))?
            .arg(0, name)?;
        // KiCad writes "~" for no name.
        Ok(if t == "~" {
            String::new()
        } else {
            t.to_owned()
        })
    };
    let mut alternates = Vec::new();
    for a in g.children("alternate") {
        alternates.push(Alternate {
            name: a.arg(0, "the alternate's name")?.to_owned(),
            ty: pin_type(a, a.arg(1, "the alternate's type")?)?,
            shape: pin_shape(a, a.arg(2, "the alternate's shape")?)?,
        });
    }
    for c in g.items().iter().skip(2) {
        match c.head() {
            Some("at" | "length" | "name" | "number" | "alternate" | "hide" | "uuid") => {}
            Some(h) => notes.ignore(format!("pin.{h}")),
            None => {}
        }
    }
    let number = g
        .child("number")
        .ok_or_else(|| unreadable(g.pos(), "(number …) is missing"))?
        .arg(0, "the pin's number")?
        .to_owned();
    Ok((
        number,
        Pin {
            name: text("name")?,
            ty,
            unit,
            at: point(at, notes)?,
            side,
            length_nm: length,
            shape,
            alternates,
        },
    ))
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;

    /// A KiCad 6 library of our own: a two-unit part with a common pin,
    /// a De Morgan style, a hidden pin, a derived symbol and a power
    /// symbol, written as KiCad 6 writes them.
    pub const SIX: &str = r##"(kicad_symbol_lib (version 20211014) (generator kicad_symbol_editor)
  (symbol "DUAL" (pin_names (offset 1.016)) (in_bom yes) (on_board yes)
    (property "Reference" "U" (id 0) (at 0 5.08 0) (effects (font (size 1.27 1.27))))
    (property "Value" "DUAL" (id 1) (at 0 -5.08 0) (effects (font (size 1.27 1.27))))
    (property "Footprint" "Lib:SO-8" (id 2) (at 0 0 0) (effects (font (size 1.27 1.27)) hide))
    (property "ki_description" "Two gates" (id 4) (at 0 0 0) (effects (font (size 1.27 1.27)) hide))
    (symbol "DUAL_0_1"
      (rectangle (start -2.54 2.54) (end 2.54 -2.54) (stroke (width 0.254) (type default) (color 0 0 0 0)) (fill (type background)))
      (text "&" (at 0 0 900) (effects (font (size 1.016 1.016))))
    )
    (symbol "DUAL_0_0"
      (pin power_in line (at 0 7.62 270) (length 2.54) hide (name "VCC" (effects (font (size 1.27 1.27)))) (number "8" (effects (font (size 1.27 1.27)))))
    )
    (symbol "DUAL_1_1"
      (pin input line (at -5.08 0 0) (length 2.54) (name "A" (effects (font (size 1.27 1.27)))) (number "1" (effects (font (size 1.27 1.27))))
        (alternate "SDA" bidirectional line))
    )
    (symbol "DUAL_1_2"
      (pin input inverted (at -5.08 0 0) (length 2.54) (name "A" (effects (font (size 1.27 1.27)))) (number "1" (effects (font (size 1.27 1.27)))))
    )
    (symbol "DUAL_2_1"
      (pin output clock (at 5.08 0 180) (length 2.54) (name "~" (effects (font (size 1.27 1.27)))) (number "2" (effects (font (size 1.27 1.27)))))
      (arc (start 0 1) (mid 1 0) (end 0 -1) (stroke (width 0) (type default) (color 0 0 0 0)) (fill (type none)))
      (circle (center 0 0) (radius 0.1234567) (stroke (width 0) (type default) (color 0 0 0 0)) (fill (type outline)))
    )
  )
  (symbol "DUAL_B" (extends "DUAL")
    (property "Reference" "U" (id 0) (at 0 5.08 0) (effects (font (size 1.27 1.27))))
    (property "Value" "DUAL_B" (id 1) (at 0 -5.08 0) (effects (font (size 1.27 1.27))))
  )
  (symbol "+3V3" (power) (pin_names (offset 0)) (in_bom yes) (on_board yes)
    (property "Reference" "#PWR" (id 0) (at 0 -3.81 0) (effects (font (size 1.27 1.27)) hide))
    (symbol "+3V3_0_1" (pin power_in line (at 0 0 90) (length 0) hide (name "+3V3" (effects (font (size 1.27 1.27)))) (number "1" (effects (font (size 1.27 1.27))))))
  )
)
"##;

    fn get<'a>(lib: &'a Library, name: &str) -> &'a Item {
        lib.items
            .iter()
            .find(|(n, _)| n == name)
            .unwrap()
            .1
            .as_ref()
            .unwrap()
    }

    #[test]
    fn a_kicad_6_library_reads() {
        let lib = read(SIX).unwrap();
        assert_eq!(lib.items.len(), 3);
        let dual = get(&lib, "DUAL");
        assert!(dual.demorgan && !dual.power);
        let s = dual.symbol.as_ref().unwrap();
        assert_eq!(s.units, 2);
        assert!(s.check().is_empty(), "{:?}", s.check());
        let vcc = &s.pins["8"];
        assert_eq!(
            (vcc.unit, vcc.side, vcc.length_nm),
            (None, Side::Up, 2_540_000)
        );
        let a = &s.pins["1"];
        assert_eq!(
            (a.unit, a.ty, a.shape, a.side),
            (Some(1), PinType::Input, PinShape::Line, Side::Left)
        );
        assert_eq!(a.at, [-5_080_000, 0]);
        assert_eq!(a.alternates[0].name, "SDA");
        let two = &s.pins["2"];
        assert_eq!(
            (two.name.as_str(), two.side, two.shape),
            ("", Side::Right, PinShape::Clock)
        );
        assert_eq!(s.graphics.len(), 4);
        let g = |n: &str| &s.graphics[&Id::parse(n, "g").unwrap()];
        assert!(matches!(
            g("g1"),
            Graphic::Rect {
                fill: Fill::Body,
                unit: None,
                ..
            }
        ));
        assert!(matches!(
            g("g2"),
            Graphic::Text {
                rotation_mdeg: 90_000,
                size_nm: 1_016_000,
                ..
            }
        ));
        assert!(matches!(g("g3"), Graphic::Arc { unit: Some(2), .. }));
        assert!(matches!(
            g("g4"),
            Graphic::Circle {
                radius_nm: 123_457,
                fill: Fill::Outline,
                ..
            }
        ));
        assert_eq!(lib.notes.rounded.iter().collect::<Vec<_>>(), ["0.1234567"]);
        assert_eq!(
            dual.properties[3],
            ("ki_description".into(), "Two gates".into())
        );
        let b = get(&lib, "DUAL_B");
        assert_eq!(
            (b.extends.as_deref(), b.symbol.is_none()),
            (Some("DUAL"), true)
        );
        assert!(get(&lib, "+3V3").power);
        assert_eq!(lib.notes.newer, None);
        assert!(lib.notes.ignored.is_empty(), "{:?}", lib.notes.ignored);
    }

    #[test]
    fn what_cant_be_held_is_refused_alone() {
        let text = r#"(kicad_symbol_lib (version 20241209)
  (symbol "TWICE" (symbol "TWICE_1_1"
    (pin input line (at 0 0 0) (length 1) (name "A") (number "1"))
    (pin input line (at 0 2 0) (length 1) (name "B") (number "1"))))
  (symbol "SLANT" (symbol "SLANT_1_1" (pin input line (at 0 0 45) (length 1) (name "A") (number "1"))))
  (symbol "BADSUB" (symbol "OTHER_1_1"))
  (symbol "OK" (symbol "OK_1_1" (pin passive line (at 0 0 0) (length 1) (name "A") (number "1"))) (future_field 1))
  (some_new_thing)
)"#;
        let lib = read(text).unwrap();
        let code = |n: &str| {
            lib.items
                .iter()
                .find(|(x, _)| x == n)
                .unwrap()
                .1
                .as_ref()
                .map(drop)
                .map_err(|e| e.code)
        };
        assert_eq!(code("TWICE"), Err(Code::LibraryPinInvalid));
        assert_eq!(code("SLANT"), Err(Code::KicadUnreadable));
        assert_eq!(code("BADSUB"), Err(Code::KicadUnreadable));
        assert_eq!(code("OK"), Ok(()));
        assert_eq!(
            lib.notes.ignored.iter().collect::<Vec<_>>(),
            ["some_new_thing", "symbol.future_field"]
        );
    }

    #[test]
    fn files_that_arent_libraries_are_refused() {
        let legacy = "EESchema-LIBRARY Version 2.4\n#encoding utf-8\n";
        assert_eq!(read(legacy).unwrap_err().code, Code::KicadLegacyUnsupported);
        assert_eq!(
            read("(footprint \"x\")").unwrap_err().code,
            Code::KicadUnreadable
        );
        assert_eq!(
            read("(kicad_symbol_lib").unwrap_err().code,
            Code::KicadUnreadable
        );
        let newer = read("(kicad_symbol_lib (version 20260101))").unwrap();
        assert_eq!(newer.notes.newer, Some(20260101));
    }
}
