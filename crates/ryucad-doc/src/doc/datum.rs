//! Datum features: planes, axes and points built from the model, which
//! found no solid, act on none, and are referred to by feature ID alone
//! (`{"feature": "d1"}`). Each says how it is built in `by`, and takes
//! that construction's fields: references to geometry
//! ([`super::geometry::GeomRef`]) and lengths and angles as expressions.

use super::files::{Obj, invalid};
use super::geometry::{self, GeomRef};
use crate::error::{Code, Error, Result};
use crate::expr::Env;
use crate::units::{Field, Quantity, Unit};
use serde_json::{Value, json};

/// Datums are new in feature format 7.
pub const FORMAT: u64 = 7;

/// A datum plane, axis or point.
#[derive(Clone, Debug, PartialEq)]
pub enum Datum {
    Plane(DatumPlane),
    Axis(DatumAxis),
    Point(DatumPoint),
}

/// A datum plane: how it is built, and what a sketch on it sees.
#[derive(Clone, Debug, PartialEq)]
pub struct DatumPlane {
    pub by: PlaneBy,
    /// A point projected onto the plane for its frame's origin, in place
    /// of the construction's.
    pub origin: Option<GeomRef>,
    /// An axis projected onto the plane for its frame's `x`.
    pub x: Option<GeomRef>,
    /// The normal turned over.
    pub flip: bool,
}

/// How a datum plane is built.
#[derive(Clone, Debug, PartialEq)]
pub enum PlaneBy {
    /// `from` moved `distance` along its normal.
    Offset { from: GeomRef, distance: Quantity },
    /// Through `about`, its normal `from`'s turned by `angle` about it.
    Angle {
        from: GeomRef,
        about: GeomRef,
        angle: Quantity,
    },
    /// Through three points, its normal by the right-hand rule.
    ThreePoints { points: [GeomRef; 3] },
    /// `from` moved to pass through `through`.
    Parallel { from: GeomRef, through: GeomRef },
    /// Through an axis and a point.
    AxisPoint { axis: GeomRef, point: GeomRef },
    /// Through `at`, normal to the curve there, the way it runs.
    NormalTo { curve: GeomRef, at: CurveAt },
    /// Halfway between two planes, or between two that meet.
    Midplane { between: [GeomRef; 2] },
}

/// Where along a curve: an end of it, or a point's foot on it.
#[derive(Clone, Debug, PartialEq)]
pub enum CurveAt {
    Start,
    End,
    Point(GeomRef),
}

/// A datum axis.
#[derive(Clone, Debug, PartialEq)]
pub struct DatumAxis {
    pub by: AxisBy,
    /// The direction turned round.
    pub flip: bool,
}

/// How a datum axis is built.
#[derive(Clone, Debug, PartialEq)]
pub enum AxisBy {
    /// A copy of any axis.
    Of { axis: GeomRef },
    /// From the first point to the second.
    TwoPoints { points: [GeomRef; 2] },
    /// Where two planes meet.
    Planes { between: [GeomRef; 2] },
    /// Through a point, along a plane's normal.
    Normal { plane: GeomRef, point: GeomRef },
}

/// A datum point.
#[derive(Clone, Debug, PartialEq)]
pub struct DatumPoint {
    pub by: PointBy,
}

/// An end of a curve, where a distance along it starts.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum End {
    Start,
    End,
}

/// How far along a curve.
#[derive(Clone, Debug, PartialEq)]
pub enum Along {
    Distance(Quantity),
    Fraction(Quantity),
}

/// How a datum point is built.
#[derive(Clone, Debug, PartialEq)]
pub enum PointBy {
    /// Any point.
    Of { point: GeomRef },
    /// At coordinates in the part's frame, or in a plane's.
    Coordinates {
        at: [Quantity; 3],
        plane: Option<GeomRef>,
    },
    /// That far along a curve from one of its ends.
    Along {
        curve: GeomRef,
        from: End,
        how: Along,
    },
    /// Where two things meet: an axis or a curve and a plane, or two axes.
    Intersection { of: [GeomRef; 2] },
    /// A point's foot on a plane or an axis.
    Projection { point: GeomRef, onto: GeomRef },
    /// Halfway between two points.
    Midpoint { points: [GeomRef; 2] },
}

/// Each construction's name and the fields it takes, by kind: what
/// `feature.edit` keeps when `by` changes.
pub const PLANE_BY: [(&str, &[&str]); 7] = [
    ("offset", &["from", "distance"]),
    ("angle", &["from", "about", "angle"]),
    ("three_points", &["points"]),
    ("parallel", &["from", "through"]),
    ("axis_point", &["axis", "point"]),
    ("normal_to", &["curve", "at"]),
    ("midplane", &["between"]),
];

pub const AXIS_BY: [(&str, &[&str]); 4] = [
    ("of", &["axis"]),
    ("two_points", &["points"]),
    ("planes", &["between"]),
    ("normal", &["plane", "point"]),
];

pub const POINT_BY: [(&str, &[&str]); 6] = [
    ("of", &["point"]),
    ("coordinates", &["at", "in"]),
    ("along", &["curve", "from", "distance", "fraction"]),
    ("intersection", &["of"]),
    ("projection", &["point", "onto"]),
    ("midpoint", &["points"]),
];

/// The feature types of datums.
pub const TYPES: [&str; 3] = ["plane", "axis", "point"];

/// The constructions of a datum type, with their fields.
pub fn constructions(ty: &str) -> &'static [(&'static str, &'static [&'static str])] {
    match ty {
        "plane" => &PLANE_BY,
        "axis" => &AXIS_BY,
        "point" => &POINT_BY,
        _ => &[],
    }
}

fn refs<const N: usize>(rs: &[GeomRef; N]) -> Value {
    json!(rs.iter().map(GeomRef::to_json).collect::<Vec<_>>())
}

fn take_arr<const N: usize>(o: &mut Obj, key: &str) -> Result<[GeomRef; N]> {
    let v = geometry::take_n(o, key, N)?;
    Ok(v.try_into().expect("take_n gives N"))
}

fn length(o: &mut Obj, key: &str, default: Unit, env: &Env) -> Result<Quantity> {
    Quantity::from_json_in(&o.take(key)?, default, &|n| env.get(n))
        .map_err(|e| e.with("pointer", format!("/{key}")))
}

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

fn by(o: &mut Obj, ty: &str) -> Result<String> {
    let v = o.take("by")?;
    let names: Vec<&str> = constructions(ty).iter().map(|(n, _)| *n).collect();
    match v.as_str() {
        Some(s) if names.contains(&s) => Ok(s.to_owned()),
        _ => Err(Error::new(
            Code::InvalidDocument,
            format!("a {ty}'s by is one of {}, not {v}", names.join(", ")),
        )
        .with("pointer", "/by")
        .with("known", names)),
    }
}

impl CurveAt {
    fn to_json(&self) -> Value {
        match self {
            CurveAt::Start => json!("start"),
            CurveAt::End => json!("end"),
            CurveAt::Point(g) => g.to_json(),
        }
    }

    fn from_json(v: &Value) -> Result<CurveAt> {
        match v.as_str() {
            Some("start") => Ok(CurveAt::Start),
            Some("end") => Ok(CurveAt::End),
            _ => Ok(CurveAt::Point(GeomRef::from_json(v, "/at")?)),
        }
    }
}

impl End {
    pub fn as_str(self) -> &'static str {
        match self {
            End::Start => "start",
            End::End => "end",
        }
    }

    pub fn parse(s: &str) -> Option<End> {
        match s {
            "start" => Some(End::Start),
            "end" => Some(End::End),
            _ => None,
        }
    }
}

impl Datum {
    /// The feature type: `plane`, `axis` or `point`.
    pub fn type_name(&self) -> &'static str {
        match self {
            Datum::Plane(_) => "plane",
            Datum::Axis(_) => "axis",
            Datum::Point(_) => "point",
        }
    }

    /// Its construction's name.
    pub fn by(&self) -> &'static str {
        match self {
            Datum::Plane(p) => match p.by {
                PlaneBy::Offset { .. } => "offset",
                PlaneBy::Angle { .. } => "angle",
                PlaneBy::ThreePoints { .. } => "three_points",
                PlaneBy::Parallel { .. } => "parallel",
                PlaneBy::AxisPoint { .. } => "axis_point",
                PlaneBy::NormalTo { .. } => "normal_to",
                PlaneBy::Midplane { .. } => "midplane",
            },
            Datum::Axis(a) => match a.by {
                AxisBy::Of { .. } => "of",
                AxisBy::TwoPoints { .. } => "two_points",
                AxisBy::Planes { .. } => "planes",
                AxisBy::Normal { .. } => "normal",
            },
            Datum::Point(p) => match p.by {
                PointBy::Of { .. } => "of",
                PointBy::Coordinates { .. } => "coordinates",
                PointBy::Along { .. } => "along",
                PointBy::Intersection { .. } => "intersection",
                PointBy::Projection { .. } => "projection",
                PointBy::Midpoint { .. } => "midpoint",
            },
        }
    }

    /// Its fields in a feature file, besides the header.
    pub fn fields(&self) -> Value {
        let mut v = json!({"by": self.by()});
        match self {
            Datum::Plane(p) => {
                match &p.by {
                    PlaneBy::Offset { from, distance } => {
                        v["from"] = from.to_json();
                        v["distance"] = json!(distance.text());
                    }
                    PlaneBy::Angle { from, about, angle } => {
                        v["from"] = from.to_json();
                        v["about"] = about.to_json();
                        v["angle"] = json!(angle.text());
                    }
                    PlaneBy::ThreePoints { points } => v["points"] = refs(points),
                    PlaneBy::Parallel { from, through } => {
                        v["from"] = from.to_json();
                        v["through"] = through.to_json();
                    }
                    PlaneBy::AxisPoint { axis, point } => {
                        v["axis"] = axis.to_json();
                        v["point"] = point.to_json();
                    }
                    PlaneBy::NormalTo { curve, at } => {
                        v["curve"] = curve.to_json();
                        v["at"] = at.to_json();
                    }
                    PlaneBy::Midplane { between } => v["between"] = refs(between),
                }
                if let Some(o) = &p.origin {
                    v["origin"] = o.to_json();
                }
                if let Some(x) = &p.x {
                    v["x"] = x.to_json();
                }
                if p.flip {
                    v["flip"] = json!(true);
                }
            }
            Datum::Axis(a) => {
                match &a.by {
                    AxisBy::Of { axis } => v["axis"] = axis.to_json(),
                    AxisBy::TwoPoints { points } => v["points"] = refs(points),
                    AxisBy::Planes { between } => v["between"] = refs(between),
                    AxisBy::Normal { plane, point } => {
                        v["plane"] = plane.to_json();
                        v["point"] = point.to_json();
                    }
                }
                if a.flip {
                    v["flip"] = json!(true);
                }
            }
            Datum::Point(p) => match &p.by {
                PointBy::Of { point } => v["point"] = point.to_json(),
                PointBy::Coordinates { at, plane } => {
                    v["at"] = json!(at.iter().map(Quantity::text).collect::<Vec<_>>());
                    if let Some(g) = plane {
                        v["in"] = g.to_json();
                    }
                }
                PointBy::Along { curve, from, how } => {
                    v["curve"] = curve.to_json();
                    v["from"] = json!(from.as_str());
                    match how {
                        Along::Distance(q) => v["distance"] = json!(q.text()),
                        Along::Fraction(q) => v["fraction"] = json!(q.text()),
                    }
                }
                PointBy::Intersection { of } => v["of"] = refs(of),
                PointBy::Projection { point, onto } => {
                    v["point"] = point.to_json();
                    v["onto"] = onto.to_json();
                }
                PointBy::Midpoint { points } => v["points"] = refs(points),
            },
        }
        v
    }

    /// Reads a datum of type `ty`, its quantities reading `env`; bare
    /// lengths take `default`, bare angles degrees.
    pub fn from_fields(ty: &str, o: &mut Obj, default: Unit, env: &Env) -> Result<Datum> {
        let by = by(o, ty)?;
        let get = geometry::take;
        Ok(match ty {
            "plane" => {
                let by = match by.as_str() {
                    "offset" => PlaneBy::Offset {
                        from: get(o, "from")?,
                        distance: length(o, "distance", default, env)?,
                    },
                    "angle" => PlaneBy::Angle {
                        from: get(o, "from")?,
                        about: get(o, "about")?,
                        angle: Quantity::from_json_as(&o.take("angle")?, Field::Angle, &|n| {
                            env.get(n)
                        })
                        .map_err(|e| e.with("pointer", "/angle"))?,
                    },
                    "three_points" => PlaneBy::ThreePoints {
                        points: take_arr(o, "points")?,
                    },
                    "parallel" => PlaneBy::Parallel {
                        from: get(o, "from")?,
                        through: get(o, "through")?,
                    },
                    "axis_point" => PlaneBy::AxisPoint {
                        axis: get(o, "axis")?,
                        point: get(o, "point")?,
                    },
                    "normal_to" => PlaneBy::NormalTo {
                        curve: get(o, "curve")?,
                        at: CurveAt::from_json(&o.take("at")?)?,
                    },
                    _ => PlaneBy::Midplane {
                        between: take_arr(o, "between")?,
                    },
                };
                Datum::Plane(DatumPlane {
                    by,
                    origin: geometry::opt(o, "origin")?,
                    x: geometry::opt(o, "x")?,
                    flip: flag(o, "flip")?,
                })
            }
            "axis" => {
                let by = match by.as_str() {
                    "of" => AxisBy::Of {
                        axis: get(o, "axis")?,
                    },
                    "two_points" => AxisBy::TwoPoints {
                        points: take_arr(o, "points")?,
                    },
                    "planes" => AxisBy::Planes {
                        between: take_arr(o, "between")?,
                    },
                    _ => AxisBy::Normal {
                        plane: get(o, "plane")?,
                        point: get(o, "point")?,
                    },
                };
                Datum::Axis(DatumAxis {
                    by,
                    flip: flag(o, "flip")?,
                })
            }
            _ => {
                let by = match by.as_str() {
                    "of" => PointBy::Of {
                        point: get(o, "point")?,
                    },
                    "coordinates" => {
                        let v = o.take("at")?;
                        let a = v.as_array().filter(|a| a.len() == 3).ok_or_else(|| {
                            invalid("/at", "a point's coordinates are three lengths")
                        })?;
                        let q = |i: usize| {
                            Quantity::from_json_in(&a[i], default, &|n| env.get(n))
                                .map_err(|e| e.with("pointer", format!("/at/{i}")))
                        };
                        PointBy::Coordinates {
                            at: [q(0)?, q(1)?, q(2)?],
                            plane: geometry::opt(o, "in")?,
                        }
                    }
                    "along" => {
                        let curve = get(o, "curve")?;
                        let from = o.take("from")?;
                        let from = from.as_str().and_then(End::parse).ok_or_else(|| {
                            invalid("/from", "a distance along a curve is from its start or end")
                        })?;
                        let how = match (o.take_opt("distance"), o.take_opt("fraction")) {
                            (Some(d), None) => {
                                let q = Quantity::from_json_in(&d, default, &|n| env.get(n))
                                    .map_err(|e| e.with("pointer", "/distance"))?;
                                if q.meters() < 0.0 {
                                    return Err(Error::new(
                                        Code::InvalidParam,
                                        format!("a distance along a curve isn't negative, not {q}"),
                                    )
                                    .with("pointer", "/distance")
                                    .with("value", q.text()));
                                }
                                Along::Distance(q)
                            }
                            (None, Some(f)) => {
                                let q = Quantity::from_json_as(&f, Field::Number, &|n| env.get(n))
                                    .map_err(|e| e.with("pointer", "/fraction"))?;
                                if !(0.0..=1.0).contains(&q.number()) {
                                    return Err(Error::new(
                                        Code::InvalidParam,
                                        format!("a fraction of a curve is from 0 to 1, not {q}"),
                                    )
                                    .with("pointer", "/fraction")
                                    .with("value", q.text()));
                                }
                                Along::Fraction(q)
                            }
                            _ => {
                                return Err(invalid(
                                    "/distance",
                                    "a point along a curve has a distance or a fraction, not both",
                                ));
                            }
                        };
                        PointBy::Along { curve, from, how }
                    }
                    "intersection" => PointBy::Intersection {
                        of: take_arr(o, "of")?,
                    },
                    "projection" => PointBy::Projection {
                        point: get(o, "point")?,
                        onto: get(o, "onto")?,
                    },
                    _ => PointBy::Midpoint {
                        points: take_arr(o, "points")?,
                    },
                };
                Datum::Point(DatumPoint { by })
            }
        })
    }

    /// Its references to geometry, each with its role (its field, and an
    /// index in a list: `points.1`), in a fixed order.
    pub fn geometry(&self) -> Vec<(String, &GeomRef)> {
        fn one<'a>(k: &str, g: &'a GeomRef) -> (String, &'a GeomRef) {
            (k.to_owned(), g)
        }
        fn many<'a>(k: &str, gs: &'a [GeomRef]) -> Vec<(String, &'a GeomRef)> {
            gs.iter()
                .enumerate()
                .map(|(i, g)| (format!("{k}.{i}"), g))
                .collect()
        }
        let mut out: Vec<(String, &GeomRef)> = Vec::new();
        match self {
            Datum::Plane(p) => {
                match &p.by {
                    PlaneBy::Offset { from, .. } => out.push(one("from", from)),
                    PlaneBy::Angle { from, about, .. } => {
                        out.extend([one("from", from), one("about", about)]);
                    }
                    PlaneBy::ThreePoints { points } => out.extend(many("points", points)),
                    PlaneBy::Parallel { from, through } => {
                        out.extend([one("from", from), one("through", through)]);
                    }
                    PlaneBy::AxisPoint { axis, point } => {
                        out.extend([one("axis", axis), one("point", point)]);
                    }
                    PlaneBy::NormalTo { curve, at } => {
                        out.push(one("curve", curve));
                        if let CurveAt::Point(g) = at {
                            out.push(one("at", g));
                        }
                    }
                    PlaneBy::Midplane { between } => out.extend(many("between", between)),
                }
                out.extend(p.origin.iter().map(|g| one("origin", g)));
                out.extend(p.x.iter().map(|g| one("x", g)));
            }
            Datum::Axis(a) => match &a.by {
                AxisBy::Of { axis } => out.push(one("axis", axis)),
                AxisBy::TwoPoints { points } => out.extend(many("points", points)),
                AxisBy::Planes { between } => out.extend(many("between", between)),
                AxisBy::Normal { plane, point } => {
                    out.extend([one("plane", plane), one("point", point)]);
                }
            },
            Datum::Point(p) => match &p.by {
                PointBy::Of { point } => out.push(one("point", point)),
                PointBy::Coordinates { plane, .. } => {
                    out.extend(plane.iter().map(|g| one("in", g)))
                }
                PointBy::Along { curve, .. } => out.push(one("curve", curve)),
                PointBy::Intersection { of } => out.extend(many("of", of)),
                PointBy::Projection { point, onto } => {
                    out.extend([one("point", point), one("onto", onto)]);
                }
                PointBy::Midpoint { points } => out.extend(many("points", points)),
            },
        }
        out
    }

    /// Its quantities, each with its field, as written.
    pub fn quantities(&self) -> Vec<(String, &Quantity)> {
        match self {
            Datum::Plane(p) => match &p.by {
                PlaneBy::Offset { distance, .. } => vec![("distance".into(), distance)],
                PlaneBy::Angle { angle, .. } => vec![("angle".into(), angle)],
                _ => Vec::new(),
            },
            Datum::Axis(_) => Vec::new(),
            Datum::Point(p) => match &p.by {
                PointBy::Coordinates { at, .. } => at
                    .iter()
                    .enumerate()
                    .map(|(i, q)| (format!("at.{i}"), q))
                    .collect(),
                PointBy::Along { how, .. } => match how {
                    Along::Distance(q) => vec![("distance".into(), q)],
                    Along::Fraction(q) => vec![("fraction".into(), q)],
                },
                _ => Vec::new(),
            },
        }
    }

    /// Whether its plane's normal, or its axis's direction, is turned.
    pub fn flip(&self) -> bool {
        match self {
            Datum::Plane(p) => p.flip,
            Datum::Axis(a) => a.flip,
            Datum::Point(_) => false,
        }
    }
}

/// The expression strings of a datum's JSON beyond those every feature's
/// share by name (`distance`, `angle`): a fraction and coordinates.
pub(crate) fn expression_strings<'a>(k: &str, v: &'a mut Value) -> Vec<&'a mut String> {
    match (k, v) {
        ("fraction", Value::String(s)) => vec![s],
        ("at", Value::Array(a)) => a
            .iter_mut()
            .filter_map(|x| match x {
                Value::String(s) => Some(s),
                _ => None,
            })
            .collect(),
        _ => Vec::new(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::doc::files::Feature;
    use crate::doc::id::Id;

    const NAME: &str = "0123456789abcdef0123456789abcdef";

    fn read(v: Value) -> Result<Feature> {
        Feature::from_json(&v, Unit::Mm, &Env::default())
    }

    fn file(ty: &str, fields: Value) -> Value {
        let mut v = fields;
        v["format"] = json!(7);
        v["kind"] = json!("ryucad.feature");
        v["type"] = json!(ty);
        v
    }

    #[test]
    fn every_construction_round_trips() {
        let face = json!({"feature": "b", "name": NAME});
        let all = [
            (
                "plane",
                json!({"by": "offset", "from": face, "distance": "30 mm"}),
            ),
            (
                "plane",
                json!({"by": "angle", "from": {"feature": "d1"}, "about": face, "angle": "15 deg",
                       "flip": true}),
            ),
            (
                "plane",
                json!({"by": "three_points",
                       "points": ["origin", {"feature": "s", "entity": "p3"}, {"feature": "p1"}],
                       "origin": {"feature": "p1"}, "x": "y"}),
            ),
            (
                "plane",
                json!({"by": "parallel", "from": "xy", "through": face}),
            ),
            (
                "plane",
                json!({"by": "axis_point", "axis": "z", "point": {"feature": "p1"}}),
            ),
            (
                "plane",
                json!({"by": "normal_to", "curve": face, "at": "end"}),
            ),
            (
                "plane",
                json!({"by": "normal_to", "curve": face, "at": {"feature": "p1"}}),
            ),
            ("plane", json!({"by": "midplane", "between": [face, "yz"]})),
            ("axis", json!({"by": "of", "axis": face})),
            (
                "axis",
                json!({"by": "two_points", "points": ["origin", face], "flip": true}),
            ),
            (
                "axis",
                json!({"by": "planes", "between": ["xy", {"feature": "d1"}]}),
            ),
            (
                "axis",
                json!({"by": "normal", "plane": "xy", "point": face}),
            ),
            ("point", json!({"by": "of", "point": face})),
            (
                "point",
                json!({"by": "coordinates", "at": ["1 mm", "2 mm", "w"]}),
            ),
            (
                "point",
                json!({"by": "coordinates", "at": ["1 mm", "2 mm", "0 mm"], "in": {"feature": "d1"}}),
            ),
            (
                "point",
                json!({"by": "along", "curve": face, "from": "start", "fraction": "0.25"}),
            ),
            (
                "point",
                json!({"by": "along", "curve": face, "from": "end", "distance": "4 mm"}),
            ),
            (
                "point",
                json!({"by": "intersection", "of": ["z", {"feature": "d1"}]}),
            ),
            (
                "point",
                json!({"by": "projection", "point": face, "onto": "x"}),
            ),
            (
                "point",
                json!({"by": "midpoint", "points": [face, "origin"]}),
            ),
        ];
        let env = {
            let mut p = std::collections::BTreeMap::new();
            p.insert("w".to_owned(), crate::expr::Expr::parse("3 mm").unwrap());
            Env::new(&p)
        };
        for (ty, fields) in all {
            let v = file(ty, fields);
            let f = Feature::from_json(&v, Unit::Mm, &env).unwrap_or_else(|e| panic!("{v}: {e}"));
            assert_eq!(f.type_name(), ty);
            assert_eq!(f.to_json(), v, "{v}");
            assert_eq!(f.format(), 7);
            assert_eq!(crate::doc::files::feature_format(&v), 7);
        }
    }

    #[test]
    fn what_a_datum_names() {
        let v = file(
            "plane",
            json!({"by": "three_points",
                   "points": ["origin", {"feature": "s", "entity": "p3"}, {"feature": "p1"}],
                   "x": {"feature": "b", "name": NAME}}),
        );
        let f = read(v).unwrap();
        let Feature::Datum(d) = &f else { panic!() };
        let roles: Vec<String> = d.geometry().into_iter().map(|(r, _)| r).collect();
        assert_eq!(roles, ["points.0", "points.1", "points.2", "x"]);
        // The face's solid, the sketch and the point it reads.
        let id = |s: &str| Id::parse(s, "feature").unwrap();
        assert_eq!(f.geometry_reads(), [&id("s"), &id("p1"), &id("b")]);
        assert_eq!(f.references()[0].0, "x");
    }

    #[test]
    fn datums_that_wont_read() {
        let face = json!({"feature": "b", "name": NAME});
        for (ty, fields, pointer) in [
            ("plane", json!({"by": "sideways", "from": "xy"}), "/by"),
            ("plane", json!({"by": "offset", "from": "xy"}), "/distance"),
            (
                "plane",
                json!({"by": "offset", "from": "xz", "distance": 1}),
                "/from",
            ),
            (
                "plane",
                json!({"by": "three_points", "points": ["origin", face]}),
                "/points",
            ),
            (
                "plane",
                json!({"by": "offset", "from": "xy", "distance": 1, "flip": 1}),
                "/flip",
            ),
            ("axis", json!({"by": "offset", "axis": "x"}), "/by"),
            (
                "axis",
                json!({"by": "of", "axis": "x", "distance": 1}),
                "/distance",
            ),
            ("point", json!({"by": "coordinates", "at": [1, 2]}), "/at"),
            (
                "point",
                json!({"by": "along", "curve": face, "from": "middle", "fraction": 0}),
                "/from",
            ),
            (
                "point",
                json!({"by": "along", "curve": face, "from": "start"}),
                "/distance",
            ),
            (
                "point",
                json!({"by": "along", "curve": face, "from": "start", "fraction": "1.5"}),
                "/fraction",
            ),
            (
                "point",
                json!({"by": "along", "curve": face, "from": "start", "distance": "-1 mm"}),
                "/distance",
            ),
        ] {
            let e = read(file(ty, fields.clone())).unwrap_err();
            assert_eq!(e.details["pointer"], pointer, "{fields}: {e}");
        }
        let e = read(file(
            "point",
            json!({"by": "along", "curve": face, "from": "start", "fraction": 2}),
        ))
        .unwrap_err();
        assert_eq!(e.code, Code::InvalidParam);
    }
}
