//! The corpus's symbols, our own drawings from datasheets, as `symbol
//! create` ops: passives drawn as KiCad's library draws them, and ICs as
//! boxes with their pins round them, two grid steps (2.54 mm) apart.

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

/// A resistor standing up, pin 1 on top.
pub fn resistor() -> String {
    ops(vec![
        rect(-1.016, 2.54, 1.016, -2.54, "none"),
        pin(&("1", "~", "passive", 0.0, 3.81, "up", 1.27)),
        pin(&("2", "~", "passive", 0.0, -3.81, "down", 1.27)),
    ])
}

/// A capacitor standing up, pin 1 on top.
pub fn capacitor() -> String {
    ops(vec![
        line(&[(-2.032, 0.508), (2.032, 0.508)], "none"),
        line(&[(-2.032, -0.508), (2.032, -0.508)], "none"),
        pin(&("1", "~", "passive", 0.0, 2.54, "up", 2.032)),
        pin(&("2", "~", "passive", 0.0, -2.54, "down", 2.032)),
    ])
}

/// A polarized capacitor: a plate and a curve, + on top.
pub fn capacitor_polarized() -> String {
    ops(vec![
        rect(-2.032, 0.762, 2.032, 0.508, "none"),
        line(&[(-2.032, -0.508), (0.0, -0.762), (2.032, -0.508)], "none"),
        line(&[(-1.524, 1.778), (-0.508, 1.778)], "none"),
        line(&[(-1.016, 2.286), (-1.016, 1.27)], "none"),
        pin(&("1", "~", "passive", 0.0, 2.54, "up", 1.778)),
        pin(&("2", "~", "passive", 0.0, -2.54, "down", 1.778)),
    ])
}

/// An LED pointing down: anode (1) on top, cathode (2) below.
pub fn led() -> String {
    ops(vec![
        line(
            &[(-1.27, 1.27), (1.27, 1.27), (0.0, -1.27), (-1.27, 1.27)],
            "none",
        ),
        line(&[(-1.27, -1.27), (1.27, -1.27)], "none"),
        line(&[(1.778, 0.254), (2.794, -0.762)], "none"),
        line(&[(2.286, 1.016), (3.302, 0.0)], "none"),
        pin(&("1", "A", "passive", 0.0, 3.81, "up", 2.54)),
        pin(&("2", "K", "passive", 0.0, -3.81, "down", 2.54)),
    ])
}

/// A diode pointing down: anode (1) on top.
pub fn diode() -> String {
    ops(vec![
        line(
            &[(-1.27, 1.27), (1.27, 1.27), (0.0, -1.27), (-1.27, 1.27)],
            "none",
        ),
        line(&[(-1.27, -1.27), (1.27, -1.27)], "none"),
        pin(&("1", "A", "passive", 0.0, 3.81, "up", 2.54)),
        pin(&("2", "K", "passive", 0.0, -3.81, "down", 2.54)),
    ])
}

/// A crystal lying down, with its case's two ground pins below.
pub fn crystal_gnd() -> String {
    ops(vec![
        rect(-0.762, 1.524, 0.762, -1.524, "none"),
        line(&[(-1.524, 1.27), (-1.524, -1.27)], "none"),
        line(&[(1.524, 1.27), (1.524, -1.27)], "none"),
        pin(&("1", "1", "passive", -3.81, 0.0, "left", 2.286)),
        pin(&("3", "3", "passive", 3.81, 0.0, "right", 2.286)),
        pin(&("2", "GND", "passive", -1.27, -3.81, "down", 1.27)),
        pin(&("4", "GND", "passive", 1.27, -3.81, "down", 1.27)),
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

/// A unit's pins as ICs draw them: `left` down the left side and `right`
/// down the right, two steps apart from the top, `top` across the top and
/// `bottom` across the foot; `(number, name, type)`, and the box they
/// need, in millimetres.
pub fn sides<'a>(
    left: &[(&'a str, &'a str, &'a str)],
    right: &[(&'a str, &'a str, &'a str)],
    top: &[(&'a str, &'a str, &'a str)],
    bottom: &[(&'a str, &'a str, &'a str)],
    w: f64,
) -> (Vec<PinDef<'a>>, f64) {
    let rows = left.len().max(right.len()).max(1) as f64;
    let h = 2.54 * (rows + 1.0)
        + if top.is_empty() { 0.0 } else { 2.54 }
        + if bottom.is_empty() { 0.0 } else { 2.54 };
    let first = h / 2.0 - 2.54 - if top.is_empty() { 0.0 } else { 2.54 };
    let snap = |v: f64| (v / 2.54).round() * 2.54;
    let first = snap(first);
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
    across(top, snap(h / 2.0) + 2.54, "up", &mut out);
    across(bottom, -snap(h / 2.0) - 2.54, "down", &mut out);
    (out, snap(h / 2.0) * 2.0)
}
