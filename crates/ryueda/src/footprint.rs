//! Footprints: `library/footprints/<id>.json` (kind `ryueda.footprint`).
//! The whole footprint as KiCad has it, in nanometres and millidegrees
//! from its origin, y up: pads, graphics on layers, keepouts, the
//! reference and value fields, and where its 3D models sit. Layers are
//! ryueda's abstract names, mapped to a board's stackup in stage 7.
//! Footprints come from import (plan 0.D); nothing here authors them.

use crate::coord::METRE;
use crate::error::Code;
use crate::fault::Fault;
use crate::id::Id;
use crate::symbol::{Point, cross};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

pub const KIND: &str = "ryueda.footprint";
pub const FORMAT: u64 = 1;

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Footprint {
    pub name: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub description: String,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub tags: Vec<String>,
    #[serde(default)]
    pub mount: Mount,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub flags: Vec<Flag>,
    #[serde(flatten)]
    pub margins: Margins,
    /// By generated ID: pad numbers repeat (several `GND` pads, a thermal
    /// pad's array), so they can't key them.
    #[serde(default)]
    pub pads: BTreeMap<Id, Pad>,
    #[serde(default)]
    pub graphics: BTreeMap<Id, Graphic>,
    #[serde(default)]
    pub keepouts: BTreeMap<Id, Keepout>,
    #[serde(default)]
    pub fields: Fields,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub models: Vec<Placed>,
}

/// How the footprint is mounted, as a placement file reads it.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Mount {
    Smd,
    Through,
    #[default]
    Unspecified,
}

/// KiCad's footprint attributes besides how it is mounted.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Flag {
    BoardOnly,
    ExcludeFromPos,
    ExcludeFromBom,
    AllowMissingCourtyard,
    AllowSoldermaskBridges,
    Dnp,
}

/// Mask, paste and clearance settings: a footprint's apply to its pads
/// that have none of their own.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Margins {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub mask_margin_nm: Option<i64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub paste_margin_nm: Option<i64>,
    /// Of the pad's size, in millionths (negative shrinks).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub paste_ratio_ppm: Option<i64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub clearance_nm: Option<i64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub zone_connect: Option<ZoneConnect>,
}

/// How copper pours meet a pad.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ZoneConnect {
    None,
    Thermal,
    Solid,
    /// Thermal reliefs on through-hole pads, solid on the rest.
    ThermalThrough,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Pad {
    /// The pad's number, which library components map pins to; none for a
    /// mechanical pad.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub number: Option<String>,
    pub kind: PadKind,
    pub shape: PadShape,
    pub at: Point,
    #[serde(default)]
    pub rotation_mdeg: i64,
    /// Width and height before rotation.
    pub size: [i64; 2],
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub drill: Option<Drill>,
    pub layers: Vec<String>,
    #[serde(flatten)]
    pub margins: Margins,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub thermal: Option<Thermal>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub property: Option<PadProperty>,
    /// The length of the bond inside the package, for length matching.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub die_length_nm: Option<i64>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PadKind {
    Smd,
    /// Plated through.
    Through,
    /// An edge connector's finger: copper, no paste.
    Connector,
    /// A non-plated hole.
    Npth,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PadProperty {
    Bga,
    FiducialGlobal,
    FiducialLocal,
    Testpoint,
    Heatsink,
    Castellated,
    Mechanical,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Corner {
    TopLeft,
    TopRight,
    BottomLeft,
    BottomRight,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Anchor {
    Circle,
    Rect,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case", deny_unknown_fields)]
pub enum PadShape {
    Circle,
    Rect,
    Oval,
    /// A rectangle whose sides lean by `delta` (KiCad's rect_delta).
    Trapezoid {
        delta: [i64; 2],
    },
    /// Corners rounded by a radius of `ratio_ppm` millionths of the
    /// smaller side.
    Roundrect {
        ratio_ppm: i64,
    },
    /// `corners` cut by `ratio_ppm` millionths of the smaller side, the
    /// others rounded by `round_ratio_ppm`.
    Chamfered {
        ratio_ppm: i64,
        corners: Vec<Corner>,
        #[serde(default)]
        round_ratio_ppm: i64,
    },
    /// An anchor pad (its size) and primitives drawn in the pad's frame.
    Custom {
        anchor: Anchor,
        primitives: Vec<Shape>,
    },
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "shape", rename_all = "snake_case", deny_unknown_fields)]
pub enum Drill {
    Round {
        diameter_nm: i64,
        #[serde(default)]
        offset: Point,
    },
    Slot {
        size: [i64; 2],
        #[serde(default)]
        offset: Point,
    },
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Thermal {
    pub spoke_nm: i64,
    pub gap_nm: i64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub angle_mdeg: Option<i64>,
}

/// A drawn shape: on a layer as a graphic, or in a custom pad's frame.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case", deny_unknown_fields)]
pub enum Shape {
    Line {
        start: Point,
        end: Point,
        width_nm: i64,
    },
    Rect {
        start: Point,
        end: Point,
        width_nm: i64,
        #[serde(default)]
        fill: bool,
    },
    /// Through three points, start to end by way of `mid`.
    Arc {
        start: Point,
        mid: Point,
        end: Point,
        width_nm: i64,
    },
    Circle {
        center: Point,
        radius_nm: i64,
        width_nm: i64,
        #[serde(default)]
        fill: bool,
    },
    Polygon {
        points: Vec<Point>,
        width_nm: i64,
        #[serde(default)]
        fill: bool,
    },
    Bezier {
        points: [Point; 4],
        width_nm: i64,
    },
    Text {
        text: String,
        at: Point,
        #[serde(default)]
        rotation_mdeg: i64,
        /// Character width and height.
        size_nm: [i64; 2],
        #[serde(default)]
        thickness_nm: i64,
        #[serde(default, skip_serializing_if = "std::ops::Not::not")]
        hidden: bool,
    },
}

impl Shape {
    /// Every point it is drawn through.
    pub fn points(&self) -> Vec<Point> {
        match self {
            Shape::Line { start, end, .. } | Shape::Rect { start, end, .. } => vec![*start, *end],
            Shape::Arc {
                start, mid, end, ..
            } => vec![*start, *mid, *end],
            Shape::Circle { center, .. } => vec![*center],
            Shape::Polygon { points, .. } => points.clone(),
            Shape::Bezier { points, .. } => points.to_vec(),
            Shape::Text { at, .. } => vec![*at],
        }
    }

    /// Its sizes: widths, radii, text sizes.
    fn sizes(&self) -> Vec<i64> {
        match self {
            Shape::Line { width_nm, .. }
            | Shape::Rect { width_nm, .. }
            | Shape::Arc { width_nm, .. }
            | Shape::Polygon { width_nm, .. }
            | Shape::Bezier { width_nm, .. } => vec![*width_nm],
            Shape::Circle {
                radius_nm,
                width_nm,
                ..
            } => vec![*radius_nm, *width_nm],
            Shape::Text {
                size_nm,
                thickness_nm,
                ..
            } => vec![size_nm[0], size_nm[1], *thickness_nm],
        }
    }
}

/// A shape on a layer.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Graphic {
    pub layer: String,
    #[serde(flatten)]
    pub shape: Shape,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Keepout {
    pub polygon: Vec<Point>,
    pub keeps: Vec<Keep>,
    pub layers: Vec<String>,
}

/// What a keepout keeps out.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Keep {
    Copper,
    Vias,
    Tracks,
    Pads,
    Pours,
}

/// Where the reference designator and the value are written: their
/// places, not their font (the style's).
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Fields {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reference: Option<Field>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub value: Option<Field>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Field {
    pub layer: String,
    pub at: Point,
    #[serde(default)]
    pub rotation_mdeg: i64,
    pub size_nm: [i64; 2],
    #[serde(default)]
    pub thickness_nm: i64,
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub hidden: bool,
}

/// A 3D model where the footprint puts it.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Placed {
    pub model: Model,
    #[serde(default)]
    pub offset_nm: [i64; 3],
    #[serde(default)]
    pub rotation_mdeg: [i64; 3],
    /// In millionths along x, y and z.
    #[serde(default = "unit_scale")]
    pub scale_ppm: [i64; 3],
}

fn unit_scale() -> [i64; 3] {
    [1_000_000; 3]
}

/// What a 3D model is. Stage 9 adds ryucad parts beside STEP files.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", deny_unknown_fields)]
pub enum Model {
    Step(StepModel),
}

/// A STEP file committed under `library/models/`, by its bytes.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct StepModel {
    pub path: String,
    pub blake3: String,
}

/// Whether ryueda knows `layer`: `copper.top`, `copper.bottom`,
/// `copper.inner` (every inner layer), `copper.all`, the top and bottom
/// of `mask`, `paste`, `silk`, `fab`, `courtyard` and `adhesive`, and
/// `user.1` to `user.45`.
pub fn layer_known(layer: &str) -> bool {
    const SIDED: [&str; 6] = ["mask", "paste", "silk", "fab", "courtyard", "adhesive"];
    if let Some(n) = layer.strip_prefix("user.") {
        // Written as the number is: `user.01` isn't `user.1`.
        return !n.starts_with('0')
            && n.bytes().all(|b| b.is_ascii_digit())
            && n.parse::<u32>().is_ok_and(|n| (1..=45).contains(&n));
    }
    match layer.split_once('.') {
        Some(("copper", s)) => ["top", "bottom", "inner", "all"].contains(&s),
        Some((l, s)) => SIDED.contains(&l) && ["top", "bottom"].contains(&s),
        None => false,
    }
}

/// Whether segments `a b` and `c d` cross or touch, exactly.
fn segments_meet(a: Point, b: Point, c: Point, d: Point) -> bool {
    let (d1, d2) = (cross(c, d, a).signum(), cross(c, d, b).signum());
    let (d3, d4) = (cross(a, b, c).signum(), cross(a, b, d).signum());
    if d1 * d2 < 0 && d3 * d4 < 0 {
        return true;
    }
    let on = |p: Point, q: Point, r: Point| {
        cross(p, q, r) == 0
            && r[0] >= p[0].min(q[0])
            && r[0] <= p[0].max(q[0])
            && r[1] >= p[1].min(q[1])
            && r[1] <= p[1].max(q[1])
    };
    on(c, d, a) || on(c, d, b) || on(a, b, c) || on(a, b, d)
}

/// Whether a closed polygon is simple: at least three points, and no two
/// sides that aren't neighbours meet.
pub fn simple(points: &[Point]) -> bool {
    let n = points.len();
    if n < 3 {
        return false;
    }
    for i in 0..n {
        for j in i + 1..n {
            let neighbours = j == i + 1 || (i == 0 && j == n - 1);
            if neighbours {
                continue;
            }
            let (a, b) = (points[i], points[(i + 1) % n]);
            let (c, d) = (points[j], points[(j + 1) % n]);
            if segments_meet(a, b, c, d) {
                return false;
            }
        }
    }
    true
}

/// Why a shape can't be drawn, if it can't.
fn undrawable(s: &Shape) -> Option<&'static str> {
    if s.sizes().iter().any(|v| *v < 0) {
        return Some("a width or size is negative");
    }
    match s {
        Shape::Rect { start, end, .. } if start[0] == end[0] || start[1] == end[1] => {
            Some("a rectangle's corners differ in x and in y")
        }
        Shape::Arc {
            start, mid, end, ..
        } if cross(*start, *mid, *end) == 0 => Some("an arc's three points can't be collinear"),
        Shape::Circle { radius_nm, .. } if *radius_nm <= 0 => Some("a circle's radius is positive"),
        Shape::Polygon { points, .. } if points.len() < 3 => {
            Some("a polygon has at least three points")
        }
        Shape::Text { text, size_nm, .. }
            if text.is_empty() || size_nm[0] <= 0 || size_nm[1] <= 0 =>
        {
            Some("a text has characters and a positive size")
        }
        _ => None,
    }
}

/// Why a pad can't be made, if it can't.
fn unmakeable(p: &Pad) -> Option<String> {
    let [w, h] = p.size;
    if w <= 0 || h <= 0 {
        return Some("its size isn't positive".into());
    }
    match &p.shape {
        PadShape::Roundrect { ratio_ppm } if !(0..=500_000).contains(ratio_ppm) => {
            return Some("a corner ratio is from 0 to half (500000 ppm)".into());
        }
        PadShape::Chamfered {
            ratio_ppm,
            round_ratio_ppm,
            ..
        } if !(0..=500_000).contains(ratio_ppm) || !(0..=500_000).contains(round_ratio_ppm) => {
            return Some("a chamfer or corner ratio is from 0 to half (500000 ppm)".into());
        }
        PadShape::Trapezoid { delta } if delta[0].abs() >= h || delta[1].abs() >= w => {
            return Some("a trapezoid's lean is smaller than its sides".into());
        }
        PadShape::Custom { primitives, .. } => {
            for s in primitives {
                if let Some(why) = undrawable(s) {
                    return Some(format!("a primitive: {why}"));
                }
                if let Shape::Polygon { points, .. } = s
                    && !simple(points)
                {
                    return Some("a primitive polygon crosses itself".into());
                }
            }
        }
        _ => {}
    }
    let holed = matches!(p.kind, PadKind::Through | PadKind::Npth);
    match (&p.drill, holed) {
        (None, true) => return Some("a through or non-plated pad has a drill".into()),
        (Some(_), false) => return Some("an SMD or connector pad has no drill".into()),
        (Some(d), true) => {
            let (dw, dh, off) = match d {
                Drill::Round {
                    diameter_nm,
                    offset,
                } => (*diameter_nm, *diameter_nm, offset),
                Drill::Slot { size, offset } => (size[0], size[1], offset),
            };
            if dw <= 0 || dh <= 0 {
                return Some("its drill's size isn't positive".into());
            }
            // In the pad's own frame, so its rotation doesn't matter.
            let fits = |o: i64, d: i64, s: i64| 2 * o.abs() + d <= s;
            if !(fits(off[0], dw, w) && fits(off[1], dh, h)) {
                return Some("its drill reaches outside the pad".into());
            }
        }
        (None, false) => {}
    }
    if p.layers.is_empty() {
        return Some("it is on no layer".into());
    }
    None
}

impl Footprint {
    /// What the footprint's checks find, each with its JSON pointer: pads
    /// that can't be made, graphics that can't be drawn, layers ryueda
    /// doesn't know, anything past a metre, and (a warning) no courtyard.
    pub fn check(&self) -> Vec<Fault> {
        let mut out = Vec::new();
        let far = |v: i64| v.unsigned_abs() > METRE as u64;
        let far_points = |ps: &[Point]| ps.iter().any(|p| far(p[0]) || far(p[1]));
        let layers = |list: &[String], ptr: &str, out: &mut Vec<Fault>| {
            for (i, l) in list.iter().enumerate() {
                if !layer_known(l) {
                    out.push(
                        Fault::error(
                            Code::LibraryLayerUnknown,
                            format!("layer {l:?} isn't one ryueda knows"),
                        )
                        .with("pointer", format!("{ptr}/{i}"))
                        .with("layer", l),
                    );
                }
            }
        };
        for (id, p) in &self.pads {
            let ptr = format!("/pads/{id}");
            if let Some(why) = unmakeable(p) {
                out.push(
                    Fault::error(Code::LibraryPadInvalid, format!("pad {id}: {why}"))
                        .with("pointer", &ptr),
                );
            }
            layers(&p.layers, &format!("{ptr}/layers"), &mut out);
            let mut points = vec![p.at];
            if let PadShape::Custom { primitives, .. } = &p.shape {
                points.extend(primitives.iter().flat_map(Shape::points));
            }
            if far_points(&points) || far(p.size[0]) || far(p.size[1]) {
                out.push(
                    Fault::error(
                        Code::LibraryOutOfRange,
                        format!("pad {id} reaches past a metre from the origin"),
                    )
                    .with("pointer", &ptr),
                );
            }
        }
        for (id, g) in &self.graphics {
            let ptr = format!("/graphics/{id}");
            if let Some(why) = undrawable(&g.shape) {
                out.push(
                    Fault::error(Code::LibraryGraphicInvalid, format!("{id}: {why}"))
                        .with("pointer", &ptr),
                );
            }
            layers(std::slice::from_ref(&g.layer), &ptr, &mut out);
            if far_points(&g.shape.points()) || g.shape.sizes().into_iter().any(far) {
                out.push(
                    Fault::error(
                        Code::LibraryOutOfRange,
                        format!("{id} reaches past a metre from the origin"),
                    )
                    .with("pointer", &ptr),
                );
            }
        }
        for (id, k) in &self.keepouts {
            let ptr = format!("/keepouts/{id}");
            if !simple(&k.polygon) || k.keeps.is_empty() || k.layers.is_empty() {
                out.push(
                    Fault::error(
                        Code::LibraryGraphicInvalid,
                        format!(
                            "keepout {id}: a simple polygon, keeping something out, on a layer"
                        ),
                    )
                    .with("pointer", &ptr),
                );
            }
            layers(&k.layers, &format!("{ptr}/layers"), &mut out);
            if far_points(&k.polygon) {
                out.push(
                    Fault::error(
                        Code::LibraryOutOfRange,
                        format!("keepout {id} reaches past a metre from the origin"),
                    )
                    .with("pointer", &ptr),
                );
            }
        }
        for (name, f) in [
            ("reference", &self.fields.reference),
            ("value", &self.fields.value),
        ] {
            let Some(f) = f else { continue };
            let ptr = format!("/fields/{name}");
            layers(std::slice::from_ref(&f.layer), &ptr, &mut out);
            if f.size_nm[0] <= 0 || f.size_nm[1] <= 0 || f.thickness_nm < 0 {
                out.push(
                    Fault::error(
                        Code::LibraryGraphicInvalid,
                        format!("the {name} field's size isn't positive"),
                    )
                    .with("pointer", &ptr),
                );
            }
            if far_points(&[f.at]) {
                out.push(
                    Fault::error(
                        Code::LibraryOutOfRange,
                        format!("the {name} field is past a metre from the origin"),
                    )
                    .with("pointer", &ptr),
                );
            }
        }
        let courtyard = self
            .graphics
            .values()
            .any(|g| g.layer.starts_with("courtyard."));
        if !courtyard && !self.flags.contains(&Flag::AllowMissingCourtyard) {
            out.push(
                Fault::warning(
                    Code::LibraryCourtyardMissing,
                    "the footprint has no courtyard",
                )
                .with("pointer", "/graphics"),
            );
        }
        out
    }

    /// The pad numbers, each once, in order.
    pub fn numbers(&self) -> std::collections::BTreeSet<&str> {
        self.pads
            .values()
            .filter_map(|p| p.number.as_deref())
            .collect()
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use crate::doc;
    use serde_json::{Value, json};

    /// An 0402 resistor's footprint, as KiCad draws it (in nanometres).
    pub fn r0402() -> Footprint {
        let v = json!({
            "kind": KIND, "format": 1, "name": "R_0402_1005Metric", "mount": "smd",
            "pads": {
                "p1": {"number": "1", "kind": "smd", "shape": {"type": "roundrect", "ratio_ppm": 250_000},
                       "at": [-510_000, 0], "size": [540_000, 640_000], "layers": ["copper.top", "mask.top", "paste.top"]},
                "p2": {"number": "2", "kind": "smd", "shape": {"type": "roundrect", "ratio_ppm": 250_000},
                       "at": [510_000, 0], "size": [540_000, 640_000], "layers": ["copper.top", "mask.top", "paste.top"]}
            },
            "graphics": {
                "g1": {"layer": "courtyard.top", "type": "rect", "start": [-930_000, 470_000], "end": [930_000, -470_000], "width_nm": 50_000},
                "g2": {"layer": "fab.top", "type": "rect", "start": [-500_000, 250_000], "end": [500_000, -250_000], "width_nm": 100_000}
            },
            "fields": {
                "reference": {"layer": "silk.top", "at": [0, 1_170_000], "size_nm": [1_000_000, 1_000_000], "thickness_nm": 150_000},
                "value": {"layer": "fab.top", "at": [0, -1_170_000], "size_nm": [1_000_000, 1_000_000], "thickness_nm": 150_000}
            },
            "models": [{"model": {"step": {"path": "library/models/r_0402_1005metric.step", "blake3": "0".repeat(64)}}}]
        });
        doc::from_value(v, "library/footprints/r_0402_1005metric.json", KIND, FORMAT).unwrap()
    }

    #[test]
    fn footprints_read_and_write_canonically() {
        let f = r0402();
        assert!(f.check().is_empty(), "{:?}", f.check());
        let text = doc::to_string(&f, KIND, FORMAT);
        let back: Footprint = doc::read(text.as_bytes(), "f.json", KIND, FORMAT).unwrap();
        assert_eq!(back, f);
        assert_eq!(f.models[0].scale_ppm, [1_000_000; 3]);
        let v: Value = serde_json::from_str(&text).unwrap();
        assert_eq!(v["pads"]["p1"]["rotation_mdeg"], json!(0));
        assert_eq!(v["graphics"]["g1"]["layer"], json!("courtyard.top"));
        assert!(v["pads"]["p1"].get("drill").is_none());
        assert_eq!(f.numbers().into_iter().collect::<Vec<_>>(), ["1", "2"]);
    }

    #[test]
    fn unknown_fields_are_refused_even_beside_a_layer() {
        let mut v = doc::to_value(&r0402(), KIND, FORMAT);
        v["graphics"]["g1"]["colour"] = json!("red");
        assert_eq!(
            doc::from_value::<Footprint>(v, "f.json", KIND, FORMAT)
                .unwrap_err()
                .code,
            Code::InvalidDocument
        );
        let mut v = doc::to_value(&r0402(), KIND, FORMAT);
        v["pads"]["p1"]["net"] = json!("GND");
        assert_eq!(
            doc::from_value::<Footprint>(v, "f.json", KIND, FORMAT)
                .unwrap_err()
                .code,
            Code::InvalidDocument
        );
    }

    fn codes(f: &Footprint) -> Vec<(Code, String)> {
        f.check()
            .into_iter()
            .map(|x| {
                (
                    x.error.code,
                    x.error.details["pointer"].as_str().unwrap().to_owned(),
                )
            })
            .collect()
    }

    fn pad(v: Value) -> Pad {
        serde_json::from_value(v).unwrap()
    }

    #[test]
    fn checks_find_pads_layers_and_courtyards() {
        let mut f = r0402();
        let id = |s: &str| Id::parse(s, "pad").unwrap();
        f.pads.insert(id("p3"), pad(json!({"number": "3", "kind": "through", "shape": {"type": "circle"},
            "at": [0, 0], "size": [1_000_000, 1_000_000], "drill": {"shape": "round", "diameter_nm": 600_000, "offset": [300_000, 0]},
            "layers": ["copper.all", "mask.both"]})));
        f.pads.insert(id("p4"), pad(json!({"kind": "smd", "shape": {"type": "rect"}, "at": [0, 0], "size": [0, 1], "layers": ["copper.top"]})));
        f.pads.insert(id("p5"), pad(json!({"kind": "npth", "shape": {"type": "circle"}, "at": [0, 0], "size": [1, 1], "layers": ["copper.all"]})));
        f.pads.insert(id("p6"), pad(json!({"number": "6", "kind": "smd", "shape": {"type": "custom", "anchor": "circle",
            "primitives": [{"type": "polygon", "points": [[0, 0], [10, 10], [10, 0], [0, 10]], "width_nm": 0, "fill": true}]},
            "at": [0, 0], "size": [5, 5], "layers": ["copper.top"]})));
        f.pads.insert(id("p7"), pad(json!({"number": "7", "kind": "through", "shape": {"type": "oval"},
            "at": [0, 0], "size": [2_000_000, 1_000_000], "drill": {"shape": "slot", "size": [1_200_000, 600_000]},
            "layers": ["copper.all", "mask.top", "mask.bottom"]})));
        f.graphics.retain(|_, g| g.layer != "courtyard.top");
        f.graphics.insert(id("g3"), serde_json::from_value(json!({"layer": "user.46", "type": "line", "start": [0, 0], "end": [1, 1], "width_nm": -1})).unwrap());
        assert_eq!(
            codes(&f),
            vec![
                (Code::LibraryPadInvalid, "/pads/p3".to_owned()),
                (Code::LibraryLayerUnknown, "/pads/p3/layers/1".to_owned()),
                (Code::LibraryPadInvalid, "/pads/p4".to_owned()),
                (Code::LibraryPadInvalid, "/pads/p5".to_owned()),
                (Code::LibraryPadInvalid, "/pads/p6".to_owned()),
                (Code::LibraryGraphicInvalid, "/graphics/g3".to_owned()),
                (Code::LibraryLayerUnknown, "/graphics/g3/0".to_owned()),
                (Code::LibraryCourtyardMissing, "/graphics".to_owned()),
            ]
        );
        assert!(!f.check().last().unwrap().is_error());
        f.flags.push(Flag::AllowMissingCourtyard);
        assert_ne!(codes(&f).last().unwrap().0, Code::LibraryCourtyardMissing);
    }

    #[test]
    fn layers_are_ryuedas() {
        for l in [
            "copper.top",
            "copper.inner",
            "copper.all",
            "mask.bottom",
            "courtyard.top",
            "adhesive.bottom",
            "user.1",
            "user.45",
        ] {
            assert!(layer_known(l), "{l}");
        }
        for l in [
            "copper",
            "F.Cu",
            "mask.all",
            "silk.inner",
            "user.0",
            "user.46",
            "user.01",
            "user.x",
        ] {
            assert!(!layer_known(l), "{l}");
        }
    }

    #[test]
    fn polygons_are_simple_or_not() {
        assert!(simple(&[[0, 0], [10, 0], [10, 10], [0, 10]]));
        assert!(!simple(&[[0, 0], [10, 10], [10, 0], [0, 10]]));
        assert!(!simple(&[[0, 0], [10, 0]]));
        // A side touching a vertex that isn't its neighbour.
        assert!(!simple(&[[0, 0], [10, 0], [5, 0], [5, 5]]));
    }
}
