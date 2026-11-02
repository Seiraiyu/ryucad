//! The corpus's symbols, our own drawings from datasheets, as `symbol
//! create` ops: passives drawn as KiCad's library draws them but resistors
//! as the US zigzag, and ICs as boxes with their pins round them, two grid
//! steps (2.54 mm) apart.

#![allow(dead_code)]

use serde_json::{Value, json};

/// A length in millimetres, as an op writes it.
fn mm(v: f64) -> String {
    format!("{v} mm")
}

/// A pin: `(number, name, type, x, y, side, length)`, millimetres.
pub type PinDef<'a> = (&'a str, &'a str, &'a str, f64, f64, &'a str, f64);

pub fn pin(p: &PinDef) -> Value {
    let (number, name, ty, x, y, side, len) = *p;
    json!({"op": "add", "pin": number, "name": name, "type": ty, "at": [mm(x), mm(y)], "side": side, "length": mm(len)})
}

pub fn rect(x0: f64, y0: f64, x1: f64, y1: f64, fill: &str) -> Value {
    json!({"op": "add", "graphic": "rect", "start": [mm(x0), mm(y0)], "end": [mm(x1), mm(y1)], "fill": fill})
}

pub fn line(points: &[(f64, f64)], fill: &str) -> Value {
    let pts: Vec<Value> = points
        .iter()
        .map(|(x, y)| json!([mm(*x), mm(*y)]))
        .collect();
    json!({"op": "add", "graphic": "polyline", "points": pts, "fill": fill})
}

/// Ops as the CLI's `--ops` takes them.
pub fn ops(list: Vec<Value>) -> String {
    serde_json::to_string(&list).unwrap()
}

/// A resistor's zigzag (the US symbol, the corpus's default), between
/// 2.54 mm above and below its middle.
pub fn zigzag() -> Value {
    line(
        &[
            (0.0, 2.54),
            (1.016, 2.117),
            (-1.016, 1.27),
            (1.016, 0.423),
            (-1.016, -0.423),
            (1.016, -1.27),
            (-1.016, -2.117),
            (0.0, -2.54),
        ],
        "none",
    )
}

/// A resistor standing up, pin 1 on top, drawn as a zigzag.
pub fn resistor() -> String {
    ops(vec![
        zigzag(),
        pin(&("1", "~", "passive", 0.0, 3.81, "up", 1.27)),
        pin(&("2", "~", "passive", 0.0, -3.81, "down", 1.27)),
    ])
}

/// A resistor drawn as a box (the IEC symbol): allowed, never the default.
pub fn resistor_box() -> String {
    ops(vec![
        rect(-1.016, 2.54, 1.016, -2.54, "none"),
        pin(&("1", "~", "passive", 0.0, 3.81, "up", 1.27)),
        pin(&("2", "~", "passive", 0.0, -3.81, "down", 1.27)),
    ])
}

/// A capacitor standing up, pin 1 on top: plates 3 mm wide, 0.76 mm
/// apart (KiCad's are 4 mm and 1 mm).
pub fn capacitor() -> String {
    ops(vec![
        line(&[(-1.524, 0.381), (1.524, 0.381)], "none"),
        line(&[(-1.524, -0.381), (1.524, -0.381)], "none"),
        pin(&("1", "~", "passive", 0.0, 2.54, "up", 2.159)),
        pin(&("2", "~", "passive", 0.0, -2.54, "down", 2.159)),
    ])
}

/// A small capacitor, as KiCad's `C_Small`: plates 2 mm wide, 0.5 mm
/// apart, its pins two steps apart (2.54 mm).
pub fn capacitor_small() -> String {
    ops(vec![
        line(&[(-1.016, 0.254), (1.016, 0.254)], "none"),
        line(&[(-1.016, -0.254), (1.016, -0.254)], "none"),
        pin(&("1", "~", "passive", 0.0, 1.27, "up", 1.016)),
        pin(&("2", "~", "passive", 0.0, -1.27, "down", 1.016)),
    ])
}

/// A polarized capacitor: a plate and a curve, + on top, as wide as
/// [`capacitor`].
pub fn capacitor_polarized() -> String {
    ops(vec![
        rect(-1.524, 0.635, 1.524, 0.381, "none"),
        line(&[(-1.524, -0.381), (0.0, -0.635), (1.524, -0.381)], "none"),
        line(&[(-1.27, 1.524), (-0.508, 1.524)], "none"),
        line(&[(-0.889, 1.905), (-0.889, 1.143)], "none"),
        pin(&("1", "~", "passive", 0.0, 2.54, "up", 1.905)),
        pin(&("2", "~", "passive", 0.0, -2.54, "down", 1.905)),
    ])
}

/// A small polarized capacitor, as [`capacitor_small`]: its pins two
/// steps apart.
pub fn capacitor_polarized_small() -> String {
    ops(vec![
        rect(-1.016, 0.381, 1.016, 0.254, "none"),
        line(&[(-1.016, -0.254), (0.0, -0.381), (1.016, -0.254)], "none"),
        line(&[(-1.016, 0.889), (-0.508, 0.889)], "none"),
        line(&[(-0.762, 1.143), (-0.762, 0.635)], "none"),
        pin(&("1", "~", "passive", 0.0, 1.27, "up", 0.889)),
        pin(&("2", "~", "passive", 0.0, -1.27, "down", 0.889)),
    ])
}

/// An LED pointing down, numbered as KiCad's: cathode (1) below, anode
/// (2) on top.
pub fn led() -> String {
    ops(vec![
        line(
            &[(-1.27, 1.27), (1.27, 1.27), (0.0, -1.27), (-1.27, 1.27)],
            "none",
        ),
        line(&[(-1.27, -1.27), (1.27, -1.27)], "none"),
        line(&[(1.778, 0.254), (2.794, -0.762)], "none"),
        line(&[(2.286, 1.016), (3.302, 0.0)], "none"),
        pin(&("1", "K", "passive", 0.0, -3.81, "down", 2.54)),
        pin(&("2", "A", "passive", 0.0, 3.81, "up", 2.54)),
    ])
}

/// A diode pointing down, numbered as KiCad's: cathode (1) below.
pub fn diode() -> String {
    ops(vec![
        line(
            &[(-1.27, 1.27), (1.27, 1.27), (0.0, -1.27), (-1.27, 1.27)],
            "none",
        ),
        line(&[(-1.27, -1.27), (1.27, -1.27)], "none"),
        pin(&("1", "K", "passive", 0.0, -3.81, "down", 2.54)),
        pin(&("2", "A", "passive", 0.0, 3.81, "up", 2.54)),
    ])
}

/// A solder jumper of two pads lying down.
pub fn jumper2() -> String {
    ops(vec![
        rect(-1.27, 0.762, -0.254, -0.762, "outline"),
        rect(0.254, 0.762, 1.27, -0.762, "outline"),
        pin(&("1", "A", "passive", -3.81, 0.0, "left", 2.54)),
        pin(&("2", "B", "passive", 3.81, 0.0, "right", 2.54)),
    ])
}

/// A solder jumper of three pads, the middle one's pin below; its pins'
/// names aren't drawn, as KiCad's aren't.
pub fn jumper3() -> String {
    ops(vec![
        rect(-2.032, 0.762, -1.016, -0.762, "outline"),
        rect(-0.508, 0.762, 0.508, -0.762, "outline"),
        rect(1.016, 0.762, 2.032, -0.762, "outline"),
        pin(&("1", "~", "passive", -5.08, 0.0, "left", 3.048)),
        pin(&("3", "~", "passive", 5.08, 0.0, "right", 3.048)),
        pin(&("2", "~", "input", 0.0, -2.54, "down", 1.778)),
    ])
}

/// `op` in unit `unit` of a symbol of several.
pub fn in_unit(mut op: Value, unit: u32) -> Value {
    op["unit"] = json!(unit);
    op
}

/// An IC unit: [`ic`]'s box and pins, all in `unit`.
pub fn ic_unit(w: f64, h: f64, pins: &[PinDef], unit: u32) -> Vec<Value> {
    let mut list = vec![in_unit(
        rect(-w / 2.0, h / 2.0, w / 2.0, -h / 2.0, "body"),
        unit,
    )];
    list.extend(pins.iter().map(|p| in_unit(pin(p), unit)));
    list
}

/// A crystal lying down, with its case's two ground pins below, their
/// names not drawn.
pub fn crystal_gnd() -> String {
    ops(vec![
        rect(-0.762, 1.524, 0.762, -1.524, "none"),
        line(&[(-1.524, 1.27), (-1.524, -1.27)], "none"),
        line(&[(1.524, 1.27), (1.524, -1.27)], "none"),
        pin(&("1", "1", "passive", -3.81, 0.0, "left", 2.286)),
        pin(&("3", "3", "passive", 3.81, 0.0, "right", 2.286)),
        pin(&("2", "~", "passive", -1.27, -3.81, "down", 1.27)),
        pin(&("4", "~", "passive", 1.27, -3.81, "down", 1.27)),
    ])
}

/// A push button lying down.
pub fn button() -> String {
    ops(vec![
        line(&[(-1.524, 0.0), (1.524, 1.016)], "none"),
        line(&[(0.0, 1.016), (0.0, 1.778)], "none"),
        line(&[(-1.016, 1.778), (1.016, 1.778)], "none"),
        pin(&("1", "~", "passive", -3.81, 0.0, "left", 2.286)),
        pin(&("2", "~", "passive", 3.81, 0.0, "right", 2.286)),
    ])
}

/// A connector of `n` pins down its left, numbered from the top, its
/// names hidden.
pub fn connector(n: usize) -> String {
    let mut list = vec![rect(-1.27, 1.27, 1.27, -2.54 * n as f64 + 1.27, "body")];
    for i in 0..n {
        let num = (i + 1).to_string();
        list.push(pin(&(
            &num,
            "~",
            "passive",
            -3.81,
            -2.54 * i as f64,
            "left",
            2.54,
        )));
    }
    ops(list)
}

/// An IC: a box of `w` by `h` millimetres centred on the origin, its pins
/// as given (each at the box's edge plus its length).
pub fn ic(w: f64, h: f64, pins: &[PinDef]) -> String {
    let mut list = vec![rect(-w / 2.0, h / 2.0, w / 2.0, -h / 2.0, "body")];
    list.extend(pins.iter().map(pin));
    ops(list)
}

/// Pins round a box: left and right in rows two steps (2.54 mm) apart from
/// the top, top and bottom four steps apart about the middle; the box's
/// height and width, at least `w` wide and grown until every pin's name
/// fits inside it (vertical names reading up from the top and bottom, the
/// left and right names apart), so no name runs past the body.
pub fn sides<'a>(
    left: &[(&'a str, &'a str, &'a str)],
    right: &[(&'a str, &'a str, &'a str)],
    top: &[(&'a str, &'a str, &'a str)],
    bottom: &[(&'a str, &'a str, &'a str)],
    w: f64,
) -> (Vec<PinDef<'a>>, f64, f64) {
    let snap = |v: f64| (v / 2.54).round() * 2.54;
    let up = |v: f64| (v / 2.54).ceil() * 2.54;
    // A name as ryusch measures and draws it (not drawn: `~`, the number,
    // KiCad's `Pin_<n>`), in millimetres.
    let width = |(n, name, _): &(&str, &str, &str)| -> f64 {
        if name.is_empty() || *name == "~" || name == n || name.starts_with("Pin_") {
            return 0.0;
        }
        ryusch::font::width(name, ryusch::font::SIZE) as f64 / 1e6
    };
    let widest = |list: &[(&str, &str, &str)]| list.iter().map(width).fold(0.0, f64::max);
    // Room for the names reading in from the top and bottom: half a step
    // from the edge, half a step clear of the side rows.
    let room = |list: &[(&str, &str, &str)]| {
        if list.is_empty() {
            0.0
        } else {
            up(widest(list) + 1.27).max(2.54)
        }
    };
    let (top_room, bottom_room) = (room(top), room(bottom));
    let rows = left.len().max(right.len()).max(1) as f64;
    let h = up(2.54 * (rows + 1.0) + top_room + bottom_room);
    // Wide enough for a left and a right name side by side, and for the
    // top and bottom pins with a step to spare at either end.
    let across = |list: &[(&str, &str, &str)]| 5.08 * (list.len().max(1) as f64 - 1.0) + 5.08;
    let w = [
        w,
        up(widest(left) + widest(right) + 2.54),
        across(top),
        across(bottom),
    ]
    .into_iter()
    .fold(0.0, f64::max);
    let w = 2.0 * up(w / 2.0);
    let first = up(h / 2.0) - 2.54 - top_room;
    let first = (first / 2.54).floor() * 2.54;
    let mut out = Vec::new();
    for (i, (n, name, ty)) in left.iter().enumerate() {
        out.push((
            *n,
            *name,
            *ty,
            -w / 2.0 - 2.54,
            first - 2.54 * i as f64,
            "left",
            2.54,
        ));
    }
    for (i, (n, name, ty)) in right.iter().enumerate() {
        out.push((
            *n,
            *name,
            *ty,
            w / 2.0 + 2.54,
            first - 2.54 * i as f64,
            "right",
            2.54,
        ));
    }
    let across = |list: &[(&'a str, &'a str, &'a str)],
                  y: f64,
                  side: &'static str,
                  out: &mut Vec<PinDef<'a>>| {
        let start = snap(-2.54 * (list.len() as f64 - 1.0));
        for (i, (n, name, ty)) in list.iter().enumerate() {
            out.push((*n, *name, *ty, start + 5.08 * i as f64, y, side, 2.54));
        }
    };
    across(top, up(h / 2.0) + 2.54, "up", &mut out);
    across(bottom, -up(h / 2.0) - 2.54, "down", &mut out);
    (out, up(h / 2.0) * 2.0, w)
}

/// An inductor standing up, pin 1 on top: four humps down its middle.
pub fn inductor() -> String {
    let mut humps = vec![(0.0, 2.54)];
    for i in 0..4 {
        let y = 2.54 - 1.27 * i as f64;
        humps.extend([(0.762, y - 0.254), (0.762, y - 1.016), (0.0, y - 1.27)]);
    }
    ops(vec![
        line(&humps, "none"),
        pin(&("1", "1", "passive", 0.0, 3.81, "up", 1.27)),
        pin(&("2", "2", "passive", 0.0, -3.81, "down", 1.27)),
    ])
}

/// A ferrite bead standing up, pin 1 on top: a block across its line.
pub fn ferrite() -> String {
    ops(vec![
        line(
            &[
                (-1.524, 0.508),
                (0.508, 1.524),
                (1.524, -0.508),
                (-0.508, -1.524),
                (-1.524, 0.508),
            ],
            "outline",
        ),
        line(&[(0.0, 2.54), (0.0, 1.016)], "none"),
        line(&[(0.0, -1.016), (0.0, -2.54)], "none"),
        pin(&("1", "1", "passive", 0.0, 3.81, "up", 1.27)),
        pin(&("2", "2", "passive", 0.0, -3.81, "down", 1.27)),
    ])
}

/// A test point: a ring on its one pin, below it.
pub fn test_point() -> String {
    ops(vec![
        rect(-0.762, 2.286, 0.762, 0.762, "none"),
        pin(&("1", "1", "passive", 0.0, -2.54, "down", 1.778)),
    ])
}

/// Two diodes in series standing up, both pointing up (BAV99's): pin 2
/// on top, pin 1 below, and pin 3, where they meet, to the left.
pub fn diode_pair() -> String {
    ops(vec![
        line(
            &[(-1.27, 1.27), (1.27, 1.27), (0.0, 3.302), (-1.27, 1.27)],
            "none",
        ),
        line(&[(-1.27, 3.302), (1.27, 3.302)], "none"),
        line(
            &[
                (-1.27, -3.302),
                (1.27, -3.302),
                (0.0, -1.27),
                (-1.27, -3.302),
            ],
            "none",
        ),
        line(&[(-1.27, -1.27), (1.27, -1.27)], "none"),
        line(&[(-2.54, 0.0), (0.0, 0.0)], "none"),
        pin(&("2", "~", "passive", 0.0, 6.35, "up", 3.048)),
        pin(&("1", "~", "passive", 0.0, -6.35, "down", 3.048)),
        pin(&("3", "~", "passive", -5.08, 0.0, "left", 2.54)),
    ])
}

/// An op amp's amplifier as a unit of its symbol: a triangle with `+`
/// above `-` on its left, its output on the right, and `extra` pins as
/// given (an enable on top).
pub fn opamp_unit(unit: u32, plus: &str, minus: &str, out: &str, extra: &[PinDef]) -> Vec<Value> {
    let mut list = vec![in_unit(
        line(
            &[(-5.08, 5.08), (5.08, 0.0), (-5.08, -5.08), (-5.08, 5.08)],
            "body",
        ),
        unit,
    )];
    list.push(in_unit(
        pin(&(plus, "+", "input", -7.62, 2.54, "left", 2.54)),
        unit,
    ));
    list.push(in_unit(
        pin(&(minus, "-", "input", -7.62, -2.54, "left", 2.54)),
        unit,
    ));
    list.push(in_unit(
        pin(&(out, "~", "output", 7.62, 0.0, "right", 2.54)),
        unit,
    ));
    list.extend(extra.iter().map(|p| in_unit(pin(p), unit)));
    list
}

/// A resistor array of `n` resistors, each a unit standing up: unit `k`
/// between pin `k` on top and pin `2n + 1 - k` below.
pub fn resistor_array(n: u32) -> String {
    let mut list = Vec::new();
    for k in 1..=n {
        let (top, bottom) = (k.to_string(), (2 * n + 1 - k).to_string());
        list.push(in_unit(zigzag(), k));
        list.push(in_unit(
            pin(&(&top, "~", "passive", 0.0, 3.81, "up", 1.27)),
            k,
        ));
        list.push(in_unit(
            pin(&(&bottom, "~", "passive", 0.0, -3.81, "down", 1.27)),
            k,
        ));
    }
    ops(list)
}
