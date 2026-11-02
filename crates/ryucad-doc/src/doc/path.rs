//! Path and helix features: 3D curves named from the model, the
//! references phase 14's sweeps take. A path chains curves (edges, a
//! sketch's lines, arcs and circles, helices and other paths) end to end;
//! a helix climbs an axis. Neither founds a solid or acts on one, and each
//! is referred to by feature ID alone (`{"feature": "p1"}`).

use super::files::{Obj, invalid};
use super::geometry::{self, GeomRef};
use crate::error::{Code, Error, Result};
use crate::expr::Env;
use crate::units::{Field, Quantity, Unit};
use serde_json::{Value, json};

/// Paths and helices are new in feature format 7, with datums.
pub const FORMAT: u64 = 7;

/// Turns a helix may have, at most.
pub const MAX_TURNS: f64 = 10_000.0;

/// A path or a helix.
#[derive(Clone, Debug, PartialEq)]
pub enum PathFeature {
    Path(Path),
    Helix(Box<Helix>),
}

/// A chain of curves, in order. Each segment runs whichever way meets its
/// neighbour (the first as the second sets it; one alone as its curve
/// runs), and `reverse` turns the whole.
#[derive(Clone, Debug, PartialEq)]
pub struct Path {
    pub segments: Vec<PathSegment>,
    pub reverse: bool,
}

/// One segment: an edge (`{feature, name}`, grown to its tangent chain
/// with `propagate`), a sketch's line, arc or circle
/// (`{feature, entity}`), or a helix or another path (`{feature}`), its
/// segments in place.
#[derive(Clone, Debug, PartialEq)]
pub struct PathSegment {
    pub curve: GeomRef,
    pub propagate: bool,
}

/// Which way a helix turns as it climbs: counterclockwise about its axis
/// seen from where it climbs to (the right-hand rule), or clockwise.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Hand {
    Right,
    Left,
}

/// Where a helix starts: at a point (its distance from the axis the
/// radius, its foot the base, its direction from the axis the starting
/// angle), or at a radius from the axis's own point, the starting angle
/// along the default rule's `x`.
#[derive(Clone, Debug, PartialEq)]
pub enum Start {
    Point(GeomRef),
    Radius(Quantity),
}

/// How far a helix goes: two of its pitch, turns and height, the third
/// following from them.
#[derive(Clone, Debug, PartialEq)]
pub enum Size {
    PitchTurns { pitch: Quantity, turns: Quantity },
    PitchHeight { pitch: Quantity, height: Quantity },
    TurnsHeight { turns: Quantity, height: Quantity },
}

/// A helix about an axis.
#[derive(Clone, Debug, PartialEq)]
pub struct Helix {
    pub axis: GeomRef,
    pub start: Start,
    pub size: Size,
    pub hand: Hand,
    /// How far it leans out from the axis as it climbs: positive widens.
    /// `None` is none.
    pub taper: Option<Quantity>,
}

/// The feature types.
pub const TYPES: [&str; 2] = ["path", "helix"];

/// The request fields each type takes, by their request names.
pub fn request_fields(ty: &str) -> &'static [&'static str] {
    match ty {
        "path" => &["segments", "reverse"],
        _ => &[
            "axis", "radius", "start", "pitch", "turns", "height", "hand", "taper",
        ],
    }
}

impl Hand {
    pub fn as_str(self) -> &'static str {
        match self {
            Hand::Right => "right",
            Hand::Left => "left",
        }
    }

    pub fn parse(s: &str) -> Option<Hand> {
        match s {
            "right" => Some(Hand::Right),
            "left" => Some(Hand::Left),
            _ => None,
        }
    }
}

impl Size {
    /// Its pitch, turns and height as written, each where it is.
    pub fn parts(&self) -> [Option<&Quantity>; 3] {
        match self {
            Size::PitchTurns { pitch, turns } => [Some(pitch), Some(turns), None],
            Size::PitchHeight { pitch, height } => [Some(pitch), None, Some(height)],
            Size::TurnsHeight { turns, height } => [None, Some(turns), Some(height)],
        }
    }

    /// Its pitch, turns and height in meters and turns, the third worked
    /// out from the two written.
    pub fn values(&self) -> (f64, f64, f64) {
        match self {
            Size::PitchTurns { pitch, turns } => {
                let (p, n) = (pitch.meters(), turns.number());
                (p, n, p * n)
            }
            Size::PitchHeight { pitch, height } => {
                let (p, h) = (pitch.meters(), height.meters());
                (p, h / p, h)
            }
            Size::TurnsHeight { turns, height } => {
                let (n, h) = (turns.number(), height.meters());
                (h / n, n, h)
            }
        }
    }
}

const SIZE_KEYS: [&str; 3] = ["pitch", "turns", "height"];

fn flag(o: &mut Obj, key: &str) -> Result<bool> {
    match o.take_opt(key) {
        None => Ok(false),
        Some(Value::Bool(b)) => Ok(b),
        Some(_) => Err(invalid(
            &format!("/{key}"),
            &format!("{key} is true or false"),
        )),
    }
}

fn param(at: &str, msg: String, value: &Quantity) -> Error {
    Error::new(Code::HelixInvalid, msg)
        .with("pointer", at)
        .with("value", value.text())
}

impl PathFeature {
    /// The feature type: `path` or `helix`.
    pub fn type_name(&self) -> &'static str {
        match self {
            PathFeature::Path(_) => "path",
            PathFeature::Helix(_) => "helix",
        }
    }

    /// Its fields in a feature file, besides the header.
    pub fn fields(&self) -> Value {
        match self {
            PathFeature::Path(p) => {
                let segments: Vec<Value> = p
                    .segments
                    .iter()
                    .map(|s| {
                        let mut v = s.curve.to_json();
                        if s.propagate {
                            v["propagate"] = json!(true);
                        }
                        v
                    })
                    .collect();
                let mut v = json!({"segments": segments});
                if p.reverse {
                    v["reverse"] = json!(true);
                }
                v
            }
            PathFeature::Helix(h) => {
                let mut v = json!({"axis": h.axis.to_json()});
                match &h.start {
                    Start::Point(g) => v["start"] = g.to_json(),
                    Start::Radius(q) => v["radius"] = json!(q.text()),
                }
                for (k, q) in SIZE_KEYS.iter().zip(h.size.parts()) {
                    if let Some(q) = q {
                        v[*k] = json!(q.text());
                    }
                }
                if h.hand == Hand::Left {
                    v["hand"] = json!("left");
                }
                if let Some(t) = &h.taper {
                    v["taper"] = json!(t.text());
                }
                v
            }
        }
    }

    /// Reads a path or helix (`ty`), its quantities reading `env`; bare
    /// lengths take `default`, bare angles degrees.
    pub fn from_fields(ty: &str, o: &mut Obj, default: Unit, env: &Env) -> Result<PathFeature> {
        let lookup = |n: &str| env.get(n);
        if ty == "path" {
            let v = o.take("segments")?;
            let items = v
                .as_array()
                .filter(|a| !a.is_empty())
                .ok_or_else(|| invalid("/segments", "a path's segments are a list of curves"))?;
            let mut segments = Vec::with_capacity(items.len());
            for (i, item) in items.iter().enumerate() {
                let at = format!("/segments/{i}");
                let mut item = item.clone();
                let propagate = match item.as_object_mut().and_then(|m| m.remove("propagate")) {
                    None => false,
                    Some(Value::Bool(b)) => b,
                    Some(_) => {
                        return Err(invalid(
                            &format!("{at}/propagate"),
                            "propagate is true or false",
                        ));
                    }
                };
                let curve = GeomRef::from_json(&item, &at)?;
                match &curve {
                    GeomRef::Entity(_) | GeomRef::Sketch { .. } | GeomRef::Feature(_) => {}
                    _ => {
                        return Err(invalid(
                            &at,
                            "a path's segment is an edge {feature, name}, a sketch's curve {feature, entity}, or a path or helix {feature}",
                        ));
                    }
                }
                if propagate && !matches!(curve, GeomRef::Entity(_)) {
                    return Err(invalid(
                        &format!("{at}/propagate"),
                        "only an edge propagates to its tangent chain",
                    ));
                }
                segments.push(PathSegment { curve, propagate });
            }
            return Ok(PathFeature::Path(Path {
                segments,
                reverse: flag(o, "reverse")?,
            }));
        }
        let axis = geometry::take(o, "axis")?;
        let start = match (o.take_opt("start"), o.take_opt("radius")) {
            (Some(s), None) => Start::Point(GeomRef::from_json(&s, "/start")?),
            (None, Some(r)) => {
                let q = Quantity::from_json_in(&r, default, &lookup)
                    .map_err(|e| e.with("pointer", "/radius"))?;
                if q.meters() <= 0.0 {
                    return Err(param(
                        "/radius",
                        format!("a helix's radius is more than nothing, not {q}"),
                        &q,
                    ));
                }
                Start::Radius(q)
            }
            _ => {
                return Err(invalid(
                    "/radius",
                    "a helix starts at a radius or at a start point, one or the other",
                ));
            }
        };
        let mut got: Vec<(&str, Quantity)> = Vec::new();
        for k in SIZE_KEYS {
            if let Some(v) = o.take_opt(k) {
                let field = if k == "turns" {
                    Field::Number
                } else {
                    Field::Length(default)
                };
                let q = Quantity::from_json_as(&v, field, &lookup)
                    .map_err(|e| e.with("pointer", format!("/{k}")))?;
                let value = if k == "turns" { q.number() } else { q.meters() };
                if value <= 0.0 {
                    return Err(param(
                        &format!("/{k}"),
                        format!("a helix's {k} is more than nothing, not {q}"),
                        &q,
                    ));
                }
                got.push((k, q));
            }
        }
        let size = match <[(&str, Quantity); 2]>::try_from(got) {
            Ok([("pitch", pitch), ("turns", turns)]) => Size::PitchTurns { pitch, turns },
            Ok([("pitch", pitch), ("height", height)]) => Size::PitchHeight { pitch, height },
            Ok([(_, turns), (_, height)]) => Size::TurnsHeight { turns, height },
            Err(_) => {
                return Err(invalid(
                    "/pitch",
                    "a helix takes exactly two of pitch, turns and height",
                ));
            }
        };
        let (_, turns, _) = size.values();
        if turns > MAX_TURNS {
            return Err(Error::new(
                Code::HelixInvalid,
                format!("a helix turns at most {MAX_TURNS} times, not {turns}"),
            )
            .with("pointer", "/turns")
            .with("turns", turns));
        }
        let hand = match o.take_opt("hand") {
            None => Hand::Right,
            Some(v) => v
                .as_str()
                .and_then(Hand::parse)
                .ok_or_else(|| invalid("/hand", "a helix's hand is right or left"))?,
        };
        let taper = match o.take_opt("taper") {
            None => None,
            Some(v) => {
                let q = Quantity::from_json_as(&v, Field::Angle, &lookup)
                    .map_err(|e| e.with("pointer", "/taper"))?;
                if q.radians().abs() >= core::f64::consts::FRAC_PI_2 {
                    return Err(param(
                        "/taper",
                        format!("a helix's taper is less than a right angle either way, not {q}"),
                        &q,
                    ));
                }
                Some(q)
            }
        };
        Ok(PathFeature::Helix(Box::new(Helix {
            axis,
            start,
            size,
            hand,
            taper,
        })))
    }

    /// Its references to geometry, each with its role: a path's
    /// `segments.<i>`, a helix's `axis` and `start`.
    pub fn geometry(&self) -> Vec<(String, &GeomRef)> {
        match self {
            PathFeature::Path(p) => p
                .segments
                .iter()
                .enumerate()
                .map(|(i, s)| (format!("segments.{i}"), &s.curve))
                .collect(),
            PathFeature::Helix(h) => {
                let mut out = vec![("axis".to_owned(), &h.axis)];
                if let Start::Point(g) = &h.start {
                    out.push(("start".to_owned(), g));
                }
                out
            }
        }
    }

    /// Its quantities, each with its field, as written.
    pub fn quantities(&self) -> Vec<(String, &Quantity)> {
        match self {
            PathFeature::Path(_) => Vec::new(),
            PathFeature::Helix(h) => {
                let mut out = Vec::new();
                if let Start::Radius(q) = &h.start {
                    out.push(("radius".to_owned(), q));
                }
                for (k, q) in SIZE_KEYS.iter().zip(h.size.parts()) {
                    if let Some(q) = q {
                        out.push(((*k).to_owned(), q));
                    }
                }
                if let Some(t) = &h.taper {
                    out.push(("taper".to_owned(), t));
                }
                out
            }
        }
    }
}

/// The expression strings of a helix's JSON: its radius, pitch, turns,
/// height and taper.
pub(crate) fn expression_strings<'a>(k: &str, v: &'a mut Value) -> Vec<&'a mut String> {
    match (k, v) {
        ("radius" | "pitch" | "turns" | "height" | "taper", Value::String(s)) => vec![s],
        _ => Vec::new(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::doc::files::Feature;
    use crate::doc::id::Id;

    const NAME: &str = "0123456789abcdef0123456789abcdef";

    fn file(ty: &str, fields: Value) -> Value {
        let mut v = fields;
        v["format"] = json!(7);
        v["kind"] = json!("ryucad.feature");
        v["type"] = json!(ty);
        v
    }

    fn env() -> Env {
        let mut p = std::collections::BTreeMap::new();
        p.insert("w".to_owned(), crate::expr::Expr::parse("2 mm").unwrap());
        Env::new(&p)
    }

    #[test]
    fn paths_and_helices_round_trip() {
        let edge = json!({"feature": "b", "name": NAME});
        let grown = json!({"feature": "b", "name": NAME, "propagate": true});
        let all = [
            (
                "path",
                json!({"segments": [grown, {"feature": "s", "entity": "l1"}]}),
            ),
            (
                "path",
                json!({"segments": [{"feature": "h1"}, edge.clone()], "reverse": true}),
            ),
            (
                "helix",
                json!({"axis": "z", "radius": "8 mm", "pitch": "2 mm", "turns": "10"}),
            ),
            (
                "helix",
                json!({"axis": edge, "start": {"feature": "s", "entity": "p1"},
                       "pitch": "w", "height": "30 mm", "hand": "left", "taper": "5 deg"}),
            ),
            (
                "helix",
                json!({"axis": {"feature": "a1"}, "radius": "3 mm", "turns": "2.5",
                       "height": "10 mm"}),
            ),
        ];
        for (ty, fields) in all {
            let v = file(ty, fields);
            let f = Feature::from_json(&v, Unit::Mm, &env()).unwrap_or_else(|e| panic!("{v}: {e}"));
            assert_eq!(f.type_name(), ty);
            assert_eq!(f.to_json(), v, "{v}");
            assert_eq!(f.format(), 7);
            assert_eq!(crate::doc::files::feature_format(&v), 7);
        }
    }

    #[test]
    fn what_paths_and_helices_name() {
        let v = file(
            "path",
            json!({"segments": [{"feature": "s", "entity": "a1"}, {"feature": "b", "name": NAME},
                                {"feature": "h1"}]}),
        );
        let f = Feature::from_json(&v, Unit::Mm, &env()).unwrap();
        let roles: Vec<String> = f.geometry().into_iter().map(|(r, _)| r).collect();
        assert_eq!(roles, ["segments.0", "segments.1", "segments.2"]);
        let id = |s: &str| Id::parse(s, "feature").unwrap();
        assert_eq!(f.geometry_reads(), [&id("s"), &id("b"), &id("h1")]);
        let v = file(
            "helix",
            json!({"axis": {"feature": "a1"}, "start": {"feature": "p1"},
                   "turns": "3", "height": "w"}),
        );
        let f = Feature::from_json(&v, Unit::Mm, &env()).unwrap();
        assert_eq!(f.geometry_reads(), [&id("a1"), &id("p1")]);
        let Feature::Path(p) = &f else { panic!() };
        let PathFeature::Helix(h) = p.as_ref() else {
            panic!()
        };
        let (pitch, turns, height) = h.size.values();
        assert_eq!((turns, height), (3.0, 0.002));
        assert!((pitch - 0.002 / 3.0).abs() < 1e-18);
        // Every quantity is an expression an equations table can edit.
        let fields = crate::doc::files::expression_fields(&v);
        let pointers: Vec<&str> = fields.iter().map(|(p, _)| p.as_str()).collect();
        assert_eq!(pointers, ["/height", "/turns"]);
    }

    #[test]
    fn paths_and_helices_that_wont_read() {
        let edge = json!({"feature": "b", "name": NAME});
        for (ty, fields, pointer, code) in [
            (
                "path",
                json!({"segments": []}),
                "/segments",
                Code::InvalidDocument,
            ),
            (
                "path",
                json!({"segments": ["xy"]}),
                "/segments/0",
                Code::InvalidDocument,
            ),
            (
                "path",
                json!({"segments": [{"feature": "s", "entity": "l1", "propagate": true}]}),
                "/segments/0/propagate",
                Code::InvalidDocument,
            ),
            (
                "path",
                json!({"segments": [edge.clone()], "reverse": 1}),
                "/reverse",
                Code::InvalidDocument,
            ),
            (
                "helix",
                json!({"axis": "z", "pitch": "2 mm", "turns": 3}),
                "/radius",
                Code::InvalidDocument,
            ),
            (
                "helix",
                json!({"axis": "z", "radius": 1, "start": "origin", "pitch": 2, "turns": 3}),
                "/radius",
                Code::InvalidDocument,
            ),
            (
                "helix",
                json!({"axis": "z", "radius": 1, "pitch": 2}),
                "/pitch",
                Code::InvalidDocument,
            ),
            (
                "helix",
                json!({"axis": "z", "radius": 1, "pitch": 2, "turns": 3, "height": 6}),
                "/pitch",
                Code::InvalidDocument,
            ),
            (
                "helix",
                json!({"axis": "z", "radius": "0 mm", "pitch": 2, "turns": 3}),
                "/radius",
                Code::HelixInvalid,
            ),
            (
                "helix",
                json!({"axis": "z", "radius": 1, "pitch": "-2 mm", "turns": 3}),
                "/pitch",
                Code::HelixInvalid,
            ),
            (
                "helix",
                json!({"axis": "z", "radius": 1, "pitch": 1, "turns": "10001"}),
                "/turns",
                Code::HelixInvalid,
            ),
            (
                "helix",
                json!({"axis": "z", "radius": 1, "pitch": 1, "turns": 2, "taper": "90 deg"}),
                "/taper",
                Code::HelixInvalid,
            ),
            (
                "helix",
                json!({"axis": "z", "radius": 1, "pitch": 1, "turns": 2, "hand": "up"}),
                "/hand",
                Code::InvalidDocument,
            ),
        ] {
            let e = Feature::from_json(&file(ty, fields.clone()), Unit::Mm, &env()).unwrap_err();
            assert_eq!(e.details["pointer"], pointer, "{fields}: {e}");
            assert_eq!(e.code, code, "{fields}: {e}");
        }
    }
}
