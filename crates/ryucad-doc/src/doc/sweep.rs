//! Extrude, revolve and sweep features: a sketch's closed loops (or, for
//! a sweep, a planar face's) swept into a solid of their own, or joined
//! to, cut from or intersected with others.

use super::files::{Obj, invalid};
use super::geometry::GeomRef;
use super::id::Id;
use super::solid::{Combine, Operation};
use crate::error::{Code, Error, Result};
use crate::expr::Env;
use crate::units::{Field, Quantity, Unit};
use serde_json::{Value, json};

/// How far an extrude goes along the sketch's normal.
#[derive(Clone, Debug, PartialEq)]
pub enum Extent {
    /// From the sketch's plane to this far along the direction.
    Blind(Quantity),
    /// This long in all, half each side of the sketch's plane.
    Symmetric(Quantity),
    /// From the sketch's plane to a plane parallel to it (a planar face, a
    /// datum plane), or to a point (a vertex, a datum point), measured
    /// along the direction, then `offset` further (a length, of either
    /// sign; zero when left out).
    UpTo {
        target: GeomRef,
        offset: Option<Quantity>,
    },
    /// From the sketch's plane past everything of the solids it cuts or
    /// intersects (`both`: each way), however far they reach.
    ThroughAll { both: bool },
}

/// An extrude: the sketch's loops along its normal.
#[derive(Clone, Debug, PartialEq)]
pub struct Extrude {
    pub sketch: Id,
    /// An entity on each chosen loop; empty means every closed loop.
    pub loops: Vec<Id>,
    pub extent: Extent,
    /// Against the sketch's normal.
    pub reverse: bool,
    pub combine: Combine,
}

/// How far a revolve turns.
#[derive(Clone, Debug, PartialEq)]
pub enum Angle {
    Full,
    Of(Quantity),
}

/// What a revolve turns about, directed: the turn is counterclockwise
/// about it.
#[derive(Clone, Debug, PartialEq)]
pub enum RevolveAxis {
    /// A line of the sketch, from its start to its end.
    Line(Id),
    /// Any axis in the sketch's plane, as an object: a straight edge from
    /// its start to its end, a datum axis, a line of another sketch.
    Ref(GeomRef),
}

/// A revolve: the sketch's loops about an axis.
#[derive(Clone, Debug, PartialEq)]
pub struct Revolve {
    pub sketch: Id,
    pub loops: Vec<Id>,
    pub axis: RevolveAxis,
    pub angle: Angle,
    /// Half the angle each side of the sketch's plane.
    pub symmetric: bool,
    pub combine: Combine,
}

fn invalid_extent(at: &str, msg: String) -> Error {
    Error::new(Code::InvalidExtent, msg).with("pointer", at)
}

impl Extent {
    /// A blind or symmetric extent's length.
    pub fn length(&self) -> Option<&Quantity> {
        match self {
            Extent::Blind(q) | Extent::Symmetric(q) => Some(q),
            Extent::UpTo { .. } | Extent::ThroughAll { .. } => None,
        }
    }

    pub fn type_name(&self) -> &'static str {
        match self {
            Extent::Blind(_) => "blind",
            Extent::Symmetric(_) => "symmetric",
            Extent::UpTo { .. } => "up_to",
            Extent::ThroughAll { .. } => "through_all",
        }
    }

    pub fn to_json(&self) -> Value {
        let mut v = json!({"type": self.type_name()});
        match self {
            Extent::Blind(q) | Extent::Symmetric(q) => v["length"] = json!(q.text()),
            Extent::UpTo { target, offset } => {
                v["target"] = target.to_json();
                if let Some(q) = offset {
                    v["offset"] = json!(q.text());
                }
            }
            Extent::ThroughAll { both } => {
                if *both {
                    v["both"] = json!(true);
                }
            }
        }
        v
    }

    /// Reads an extent, its quantities reading `env`: a blind or symmetric
    /// length greater than zero, an up-to target and offset, or through
    /// all.
    pub fn from_json(v: &Value, default: Unit, env: &Env) -> Result<Extent> {
        let mut o = Obj::new(v, "/extent")?;
        let ty = o.take("type")?;
        let quantity = |v: &Value, at: &str| {
            Quantity::from_json_in(v, default, &|n| env.get(n)).map_err(|e| e.with("pointer", at))
        };
        let extent = match ty.as_str() {
            Some(t @ ("blind" | "symmetric")) => {
                let q = quantity(&o.take("length")?, "/extent/length")?;
                if q.meters() <= 0.0 {
                    return Err(invalid_extent(
                        "/extent/length",
                        format!("an extrude's length must be greater than zero, not {q}"),
                    ));
                }
                if t == "blind" {
                    Extent::Blind(q)
                } else {
                    Extent::Symmetric(q)
                }
            }
            Some("up_to") => Extent::UpTo {
                target: GeomRef::from_json(&o.take("target")?, "/extent/target")?,
                offset: o
                    .take_opt("offset")
                    .map(|v| quantity(&v, "/extent/offset"))
                    .transpose()?,
            },
            Some("through_all") => Extent::ThroughAll {
                both: match o.take_opt("both") {
                    None => false,
                    Some(Value::Bool(b)) => b,
                    Some(_) => return Err(invalid("/extent/both", "both is true or false")),
                },
            },
            _ => {
                return Err(invalid_extent(
                    "/extent/type",
                    format!("an extent's type is blind, symmetric, up_to or through_all, not {ty}"),
                ));
            }
        };
        o.finish()?;
        Ok(extent)
    }
}

impl Angle {
    pub fn to_json(&self) -> Value {
        match self {
            Angle::Full => json!("full"),
            Angle::Of(q) => json!(q.text()),
        }
    }

    /// Reads `"full"` or an angle in `(0, 360 deg]`.
    pub fn from_json(v: &Value, env: &Env) -> Result<Angle> {
        if v == "full" {
            return Ok(Angle::Full);
        }
        let q = Quantity::from_json_as(v, Field::Angle, &|n| env.get(n))
            .map_err(|e| e.with("pointer", "/angle"))?;
        let a = q.radians();
        if a <= 0.0 || a > core::f64::consts::TAU + ryukei::tol::ANGULAR_RESOLUTION {
            return Err(invalid_extent(
                "/angle",
                format!("a revolve's angle is in (0, 360 deg], or \"full\"; not {q}"),
            ));
        }
        Ok(Angle::Of(q))
    }
}

/// Reads the `sketch` field.
fn sketch_id(o: &mut Obj) -> Result<Id> {
    let v = o.take("sketch")?;
    let s = v
        .as_str()
        .ok_or_else(|| invalid("/sketch", "sketch is a sketch feature's ID"))?;
    Id::parse(s, "feature").map_err(|e| e.with("pointer", "/sketch"))
}

/// Reads the optional `loops` field: entity IDs.
pub fn loops_from_json(v: Option<Value>) -> Result<Vec<Id>> {
    let Some(v) = v else {
        return Ok(Vec::new());
    };
    let a = v
        .as_array()
        .ok_or_else(|| invalid("/loops", "loops is an array of entity IDs"))?;
    a.iter()
        .enumerate()
        .map(|(i, x)| {
            let at = format!("/loops/{i}");
            let s = x
                .as_str()
                .ok_or_else(|| invalid(&at, "expected an entity's ID"))?;
            Id::parse(s, "entity").map_err(|e| e.with("pointer", &at))
        })
        .collect()
}

/// Reads an optional `true`/`false` field, false when left out.
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

/// Writes the fields every sweep has.
fn common(v: &mut Value, sketch: &Id, loops: &[Id]) {
    v["sketch"] = json!(sketch);
    if !loops.is_empty() {
        v["loops"] = json!(loops);
    }
}

impl Extrude {
    pub fn fields(&self) -> Value {
        let mut v = json!({"extent": self.extent.to_json()});
        common(&mut v, &self.sketch, &self.loops);
        if self.reverse {
            v["reverse"] = json!(true);
        }
        self.combine.write(&mut v);
        v
    }

    #[doc(hidden)]
    pub fn from_fields(o: &mut Obj, default: Unit, env: &Env) -> Result<Extrude> {
        let e = Extrude {
            sketch: sketch_id(o)?,
            loops: loops_from_json(o.take_opt("loops"))?,
            extent: Extent::from_json(&o.take("extent")?, default, env)?,
            reverse: flag(o, "reverse")?,
            combine: Combine::from_fields(o)?,
        };
        // Through all reaches past what it cuts or intersects; a new solid
        // or a join has nothing to reach past.
        let reaches = matches!(e.combine.operation, Operation::Cut | Operation::Intersect);
        if matches!(e.extent, Extent::ThroughAll { .. }) && !reaches {
            return Err(invalid_extent(
                "/extent/type",
                format!(
                    "through_all goes with a cut or an intersect, not {}",
                    e.combine.operation.as_str()
                ),
            ));
        }
        Ok(e)
    }
}

impl Revolve {
    pub fn fields(&self) -> Value {
        let axis = match &self.axis {
            RevolveAxis::Line(l) => json!(l),
            RevolveAxis::Ref(r) => r.to_json(),
        };
        let mut v = json!({"axis": axis, "angle": self.angle.to_json()});
        common(&mut v, &self.sketch, &self.loops);
        if self.symmetric {
            v["symmetric"] = json!(true);
        }
        self.combine.write(&mut v);
        v
    }

    #[doc(hidden)]
    pub fn from_fields(o: &mut Obj, env: &Env) -> Result<Revolve> {
        let axis = match o.take("axis")? {
            Value::String(l) => {
                RevolveAxis::Line(Id::parse(&l, "entity").map_err(|e| e.with("pointer", "/axis"))?)
            }
            v @ Value::Object(_) => RevolveAxis::Ref(GeomRef::from_json(&v, "/axis")?),
            _ => {
                return Err(invalid(
                    "/axis",
                    "axis is a line of the sketch, or an object: a straight edge {feature, name}, a datum axis {feature} or another sketch's line {feature, entity}",
                ));
            }
        };
        Ok(Revolve {
            sketch: sketch_id(o)?,
            loops: loops_from_json(o.take_opt("loops"))?,
            axis,
            angle: Angle::from_json(&o.take("angle")?, env)?,
            symmetric: flag(o, "symmetric")?,
            combine: Combine::from_fields(o)?,
        })
    }
}

/// Sweeps along a path are new in feature format 8.
pub const FORMAT: u64 = 8;

/// What a sweep's profile (or, in phase 14.B, a loft's section) is drawn
/// from.
#[derive(Clone, Debug, PartialEq)]
pub enum Source {
    /// A sketch listed before the feature: its closed loops, as an
    /// extrude takes them.
    Sketch(Id),
    /// A planar face of a solid, `{feature, name}`: its loops, outer and
    /// holes. Always a [`GeomRef::Entity`].
    Face(GeomRef),
}

/// A profile: a sketch's closed loops or a planar face's, every one or
/// those `loops` names (a sketch's entity on each, or a face's edge or
/// vertex on each by its name).
#[derive(Clone, Debug, PartialEq)]
pub struct Section {
    pub source: Source,
    /// Empty means every loop.
    pub loops: Vec<Id>,
}

/// How a sweep carries its profile along its path.
#[derive(Clone, Debug, PartialEq)]
pub enum Law {
    /// Rotation-minimizing: the profile turns only as the path does.
    Minimal,
    /// The profile's frame keeps `direction` (an axis's direction, or a
    /// plane's normal) in the profile's plane: a thread or a spring up a
    /// helix about its axis.
    Binormal(GeomRef),
}

/// A sweep: a profile carried along a path, turning by `twist` and
/// growing to `scale` times its size from the path's start to its end.
#[derive(Clone, Debug, PartialEq)]
pub struct Sweep {
    pub profile: Section,
    /// Any curve: a path or helix `{feature}`, an edge `{feature, name}`,
    /// or a sketch's line, arc or circle `{feature, entity}`.
    pub path: GeomRef,
    pub law: Law,
    /// An angle; none is no twist.
    pub twist: Option<Quantity>,
    /// A positive number, the end's size over the start's; none is 1.
    pub scale: Option<Quantity>,
    pub combine: Combine,
}

/// The request fields a sweep takes, by their request names.
pub const SWEEP_FIELDS: [&str; 10] = [
    "sketch",
    "face",
    "loops",
    "path",
    "law",
    "direction",
    "twist",
    "scale",
    "operation",
    "targets",
];

impl Law {
    pub fn as_str(&self) -> &'static str {
        match self {
            Law::Minimal => "minimal",
            Law::Binormal(_) => "binormal",
        }
    }
}

impl Section {
    /// The sketch it reads, if it is a sketch's.
    pub fn sketch(&self) -> Option<&Id> {
        match &self.source {
            Source::Sketch(s) => Some(s),
            Source::Face(_) => None,
        }
    }

    /// Its fields as a file holds them: `{sketch}` or `{face}`, and
    /// `loops` when it names any.
    pub fn to_json(&self) -> Value {
        let mut v = match &self.source {
            Source::Sketch(s) => json!({"sketch": s}),
            Source::Face(g) => json!({"face": g.to_json()}),
        };
        if !self.loops.is_empty() {
            v["loops"] = json!(self.loops);
        }
        v
    }

    /// Reads a section; `at` is its JSON pointer for errors.
    pub fn from_json(v: &Value, at: &str) -> Result<Section> {
        let mut o = Obj::new(v, at)?;
        let source = match (o.take_opt("sketch"), o.take_opt("face")) {
            (Some(s), None) => {
                let s = s.as_str().ok_or_else(|| {
                    invalid(&format!("{at}/sketch"), "sketch is a sketch feature's ID")
                })?;
                Source::Sketch(
                    Id::parse(s, "feature")
                        .map_err(|e| e.with("pointer", format!("{at}/sketch")))?,
                )
            }
            (None, Some(f)) => {
                let g = GeomRef::from_json(&f, &format!("{at}/face"))?;
                if !matches!(g, GeomRef::Entity(_)) {
                    return Err(invalid(
                        &format!("{at}/face"),
                        "a face is a solid's face {feature, name}",
                    ));
                }
                Source::Face(g)
            }
            _ => {
                return Err(invalid(
                    at,
                    "a profile is a sketch's loops {sketch} or a planar face's {face}, one or the other",
                ));
            }
        };
        let loops = match o.take_opt("loops") {
            None => Vec::new(),
            Some(v) => {
                let a = v.as_array().ok_or_else(|| {
                    invalid(
                        &format!("{at}/loops"),
                        "loops is an array of a sketch's entities, or of a face's edges or vertices by name",
                    )
                })?;
                a.iter()
                    .enumerate()
                    .map(|(i, x)| {
                        let p = format!("{at}/loops/{i}");
                        let s = x
                            .as_str()
                            .ok_or_else(|| invalid(&p, "expected an entity's ID or name"))?;
                        Id::parse(s, "entity").map_err(|e| e.with("pointer", &p))
                    })
                    .collect::<Result<Vec<Id>>>()?
            }
        };
        o.finish()?;
        Ok(Section { source, loops })
    }
}

impl Sweep {
    pub fn fields(&self) -> Value {
        let mut v = json!({
            "profile": self.profile.to_json(),
            "path": self.path.to_json(),
            "law": self.law.as_str(),
        });
        if let Law::Binormal(d) = &self.law {
            v["direction"] = d.to_json();
        }
        if let Some(q) = &self.twist {
            v["twist"] = json!(q.text());
        }
        if let Some(q) = &self.scale {
            v["scale"] = json!(q.text());
        }
        self.combine.write(&mut v);
        v
    }

    #[doc(hidden)]
    pub fn from_fields(o: &mut Obj, env: &Env) -> Result<Sweep> {
        let lookup = |n: &str| env.get(n);
        let profile = Section::from_json(&o.take("profile")?, "/profile")?;
        let path = super::geometry::take(o, "path")?;
        if !matches!(
            path,
            GeomRef::Entity(_) | GeomRef::Sketch { .. } | GeomRef::Feature(_)
        ) {
            return Err(invalid(
                "/path",
                "a sweep's path is a curve: a path or helix {feature}, an edge {feature, name}, or a sketch's line, arc or circle {feature, entity}",
            ));
        }
        let law = match o.take("law")?.as_str() {
            Some("minimal") => Law::Minimal,
            Some("binormal") => Law::Binormal(super::geometry::take(o, "direction")?),
            _ => {
                return Err(invalid(
                    "/law",
                    "a sweep's law is minimal, or binormal with a direction",
                ));
            }
        };
        if matches!(law, Law::Minimal) && o.take_opt("direction").is_some() {
            return Err(invalid(
                "/direction",
                "only a binormal law takes a direction",
            ));
        }
        let twist = o
            .take_opt("twist")
            .map(|v| {
                Quantity::from_json_as(&v, Field::Angle, &lookup)
                    .map_err(|e| e.with("pointer", "/twist"))
            })
            .transpose()?;
        let scale = match o.take_opt("scale") {
            None => None,
            Some(v) => {
                let q = Quantity::from_json_as(&v, Field::Number, &lookup)
                    .map_err(|e| e.with("pointer", "/scale"))?;
                if q.number().is_nan() || q.number() <= 0.0 {
                    return Err(Error::new(
                        Code::InvalidParam,
                        format!("a sweep's scale is more than nothing, not {q}"),
                    )
                    .with("pointer", "/scale")
                    .with("value", q.text()));
                }
                Some(q)
            }
        };
        Ok(Sweep {
            profile,
            path,
            law,
            twist,
            scale,
            combine: Combine::from_fields(o)?,
        })
    }

    /// Its references to geometry, each with its role: a face profile's
    /// `profile`, the `path` and a binormal law's `direction`.
    pub fn geometry(&self) -> Vec<(String, &GeomRef)> {
        let mut out = Vec::new();
        if let Source::Face(g) = &self.profile.source {
            out.push(("profile".to_owned(), g));
        }
        out.push(("path".to_owned(), &self.path));
        if let Law::Binormal(d) = &self.law {
            out.push(("direction".to_owned(), d));
        }
        out
    }

    /// Its quantities, each with its field, as written.
    pub fn quantities(&self) -> Vec<(String, &Quantity)> {
        let mut out = Vec::new();
        if let Some(q) = &self.twist {
            out.push(("twist".to_owned(), q));
        }
        if let Some(q) = &self.scale {
            out.push(("scale".to_owned(), q));
        }
        out
    }

    /// The twist in radians, 0 when none.
    pub fn twist_radians(&self) -> f64 {
        self.twist.as_ref().map_or(0.0, Quantity::radians)
    }

    /// The scale, 1 when none.
    pub fn scale_number(&self) -> f64 {
        self.scale.as_ref().map_or(1.0, Quantity::number)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::doc::canonical;
    use crate::doc::files::Feature;

    fn read(v: &Value) -> Result<Feature> {
        let p = [(
            "depth".to_owned(),
            crate::expr::Expr::parse("5 mm").unwrap(),
        )]
        .into();
        Feature::from_json(v, Unit::Mm, &Env::new(&p))
    }

    fn extrude() -> Value {
        json!({"extent": {"length": "depth * 2", "type": "symmetric"}, "format": 2,
               "kind": "ryucad.feature", "sketch": "s", "type": "extrude"})
    }

    fn revolve() -> Value {
        json!({"angle": 90, "axis": "l5", "format": 2, "kind": "ryucad.feature",
               "loops": ["l1"], "sketch": "s", "symmetric": true, "type": "revolve"})
    }

    #[test]
    fn sweeps_round_trip() {
        for v in [extrude(), revolve()] {
            let f = read(&v).unwrap();
            let text = canonical::to_string(&f.to_json());
            assert_eq!(read(&canonical::parse(&text).unwrap()).unwrap(), f);
        }
        let Feature::Extrude(e) = read(&extrude()).unwrap() else {
            panic!()
        };
        assert_eq!(
            e.extent,
            Extent::Symmetric(
                Quantity::eval("depth * 2", Unit::Mm, &|_| {
                    Ok(crate::expr::Val::new(0.005, crate::units::Dim::LENGTH))
                })
                .unwrap()
            )
        );
        assert!(e.loops.is_empty() && !e.reverse);
        let Feature::Revolve(r) = read(&revolve()).unwrap() else {
            panic!()
        };
        assert_eq!(r.angle.to_json(), "90 deg");
        assert!(r.symmetric);
        let mut v = revolve();
        v["angle"] = json!("full");
        let Feature::Revolve(r) = read(&v).unwrap() else {
            panic!()
        };
        assert_eq!(r.angle, Angle::Full);
    }

    #[test]
    fn up_to_a_target() {
        const N: &str = "0123456789abcdef0123456789abcdef";
        let mut v = extrude();
        v["extent"] = json!({"type": "up_to", "target": {"feature": "b", "name": N},
                             "offset": "-depth"});
        let f = read(&v).unwrap();
        let Feature::Extrude(e) = &f else { panic!() };
        let Extent::UpTo { target, offset } = &e.extent else {
            panic!()
        };
        assert_eq!(target.feature().unwrap().as_str(), "b");
        assert_eq!(offset.as_ref().unwrap().meters(), -0.005);
        assert!(e.extent.length().is_none());
        assert_eq!(
            f.references()
                .iter()
                .map(|(r, _)| r.as_str())
                .collect::<Vec<_>>(),
            ["target"]
        );
        let text = canonical::to_string(&f.to_json());
        assert_eq!(read(&canonical::parse(&text).unwrap()).unwrap(), f);
        v["extent"]["length"] = json!(3);
        assert_eq!(read(&v).unwrap_err().details["pointer"], "/extent/length");
    }

    #[test]
    fn through_all() {
        let mut v = extrude();
        v["extent"] = json!({"type": "through_all"});
        v["operation"] = json!("cut");
        v["targets"] = json!(["b"]);
        let f = read(&v).unwrap();
        let Feature::Extrude(e) = &f else { panic!() };
        assert_eq!(e.extent, Extent::ThroughAll { both: false });
        assert!(e.extent.length().is_none());
        assert_eq!(f.format(), 3);
        assert_eq!(read(&f.to_json()).unwrap(), f);
        v["extent"]["both"] = json!(true);
        let f = read(&v).unwrap();
        assert_eq!(
            f.to_json()["extent"],
            json!({"type": "through_all", "both": true})
        );
        v["operation"] = json!("intersect");
        assert!(read(&v).is_ok());
        // Nothing to reach past.
        for op in ["join", "new"] {
            v["operation"] = json!(op);
            if op == "new" {
                v.as_object_mut().unwrap().remove("targets");
            }
            let e = read(&v).unwrap_err();
            assert_eq!(
                (e.code, e.details["pointer"].as_str()),
                (Code::InvalidExtent, Some("/extent/type")),
                "{op}"
            );
        }
        let mut v = extrude();
        v["extent"] = json!({"type": "through_all", "both": "yes"});
        v["operation"] = json!("cut");
        v["targets"] = json!(["b"]);
        assert_eq!(read(&v).unwrap_err().details["pointer"], "/extent/both");
    }

    #[test]
    fn an_axis_may_be_an_edge() {
        const N: &str = "0123456789abcdef0123456789abcdef";
        let mut v = revolve();
        v["axis"] = json!({"feature": "b", "name": N});
        let f = read(&v).unwrap();
        let Feature::Revolve(r) = &f else { panic!() };
        assert!(matches!(&r.axis, RevolveAxis::Ref(e) if e.feature().unwrap().as_str() == "b"));
        assert_eq!(
            f.references()
                .iter()
                .map(|(r, _)| r.as_str())
                .collect::<Vec<_>>(),
            ["axis"]
        );
        assert_eq!(read(&f.to_json()).unwrap(), f);
        v["axis"] = json!(3);
        assert_eq!(read(&v).unwrap_err().details["pointer"], "/axis");
    }

    fn sweep() -> Value {
        json!({"format": 8, "kind": "ryucad.feature", "type": "sweep",
               "profile": {"sketch": "s", "loops": ["l1"]}, "path": {"feature": "h1"},
               "law": "binormal", "direction": "z", "twist": "90 deg", "scale": "0.5",
               "operation": "cut", "targets": ["b"]})
    }

    #[test]
    fn a_sweep_round_trips() {
        const N: &str = "0123456789abcdef0123456789abcdef";
        let f = read(&sweep()).unwrap();
        assert_eq!(f.format(), FORMAT);
        let text = canonical::to_string(&f.to_json());
        assert_eq!(read(&canonical::parse(&text).unwrap()).unwrap(), f);
        let Feature::Sweep(w) = &f else { panic!() };
        assert_eq!(w.law.as_str(), "binormal");
        assert!((w.twist_radians() - core::f64::consts::FRAC_PI_2).abs() < 1e-15);
        assert_eq!(w.scale_number(), 0.5);
        assert_eq!(
            f.profiles().iter().map(|s| s.as_str()).collect::<Vec<_>>(),
            ["s"]
        );
        // What it reads: its sketch, path and direction (an origin axis
        // reads nothing), and its targets.
        assert_eq!(
            crate::doc::files::feature_format(&f.to_json()),
            FORMAT,
            "judged from the JSON alone"
        );
        let roles: Vec<String> = f.geometry().into_iter().map(|(r, _)| r).collect();
        assert_eq!(roles, ["path", "direction"]);
        // A face's loops, minimal, no twist or scale: nothing more written.
        let v = json!({"format": 8, "kind": "ryucad.feature", "type": "sweep",
                       "profile": {"face": {"feature": "b", "name": N}},
                       "path": {"feature": "s", "entity": "l2"}, "law": "minimal"});
        let f = read(&v).unwrap();
        assert_eq!(f.to_json(), v);
        assert!(f.profiles().is_empty());
        let roles: Vec<String> = f.geometry().into_iter().map(|(r, _)| r).collect();
        assert_eq!(roles, ["profile", "path"]);
        assert_eq!(
            f.references()
                .iter()
                .map(|(r, _)| r.as_str())
                .collect::<Vec<_>>(),
            ["profile"]
        );
        // Its twist and scale are expression fields.
        let fields: Vec<String> = crate::doc::files::expression_fields(&sweep())
            .into_iter()
            .map(|(p, _)| p)
            .collect();
        assert_eq!(fields, ["/scale", "/twist"]);
    }

    #[test]
    fn a_sweeps_fields_are_checked() {
        let cases: [(&str, Value, Code, &str); 9] = [
            ("/profile", json!({}), Code::InvalidDocument, "/profile"),
            (
                "/profile",
                json!({"sketch": "s", "face": {"feature": "b", "name": "0123456789abcdef0123456789abcdef"}}),
                Code::InvalidDocument,
                "/profile",
            ),
            (
                "/profile",
                json!({"face": "xy"}),
                Code::InvalidDocument,
                "/profile/face",
            ),
            ("/path", json!("xy"), Code::InvalidDocument, "/path"),
            ("/law", json!("frenet"), Code::InvalidDocument, "/law"),
            ("/twist", json!("3 mm"), Code::UnitMismatch, "/twist"),
            ("/scale", json!("0"), Code::InvalidParam, "/scale"),
            ("/scale", json!("2 mm"), Code::UnitMismatch, "/scale"),
            ("/glue", json!(1), Code::InvalidDocument, "/glue"),
        ];
        for (path, value, code, pointer) in cases {
            let mut v = sweep();
            v[&path[1..]] = value;
            let e = read(&v).unwrap_err();
            assert_eq!(
                (e.code, e.details["pointer"].as_str()),
                (code, Some(pointer)),
                "{path}: {e}"
            );
        }
        // A minimal law takes no direction; binormal needs one.
        let mut v = sweep();
        v["law"] = json!("minimal");
        assert_eq!(read(&v).unwrap_err().details["pointer"], "/direction");
        v["law"] = json!("binormal");
        v.as_object_mut().unwrap().remove("direction");
        assert_eq!(read(&v).unwrap_err().code, Code::InvalidDocument);
    }

    #[test]
    fn extents_are_checked() {
        let cases: [(&str, Value, Code, &str); 8] = [
            (
                "/extent/length",
                json!("-1 mm"),
                Code::InvalidExtent,
                "/extent/length",
            ),
            (
                "/extent/length",
                json!("1 deg"),
                Code::UnitMismatch,
                "/extent/length",
            ),
            (
                "/extent/type",
                json!("up_to"),
                Code::InvalidDocument,
                "/extent/target",
            ),
            (
                "/extent/type",
                json!("through"),
                Code::InvalidExtent,
                "/extent/type",
            ),
            ("/sketch", json!("S"), Code::InvalidId, "/sketch"),
            (
                "/loops",
                json!(["l1", 2]),
                Code::InvalidDocument,
                "/loops/1",
            ),
            ("/reverse", json!("yes"), Code::InvalidDocument, "/reverse"),
            (
                "/extent/glue",
                json!(1),
                Code::InvalidDocument,
                "/extent/glue",
            ),
        ];
        for (path, value, code, pointer) in cases {
            let mut v = extrude();
            let (parent, key) = path.rsplit_once('/').unwrap();
            v.pointer_mut(if parent.is_empty() { "" } else { parent })
                .unwrap()[key] = value;
            let e = read(&v).unwrap_err();
            assert_eq!(
                (e.code, e.details["pointer"].as_str()),
                (code, Some(pointer)),
                "{path}: {e}"
            );
        }
        for (angle, code) in [
            (json!(0), Code::InvalidExtent),
            (json!("361 deg"), Code::InvalidExtent),
            (json!("1 mm"), Code::UnitMismatch),
            (json!("half"), Code::UnknownParameter),
        ] {
            let mut v = revolve();
            v["angle"] = angle.clone();
            assert_eq!(read(&v).unwrap_err().code, code, "{angle}");
        }
        assert_eq!(
            read(&json!({"format": 1, "kind": "ryucad.feature", "type": "extrude"}))
                .unwrap_err()
                .code,
            Code::InvalidDocument
        );
    }
}
