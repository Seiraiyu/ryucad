//! Extrude, revolve, sweep and loft features: a sketch's closed loops
//! (or, for a sweep or a loft, a planar face's) swept or skinned into a
//! solid of their own, or joined to, cut from or intersected with others.

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
    /// Sweeps its surface profile, loops and open chains, into a sheet it
    /// founds (ryucad 18).
    pub surface: bool,
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
    /// Sweeps its surface profile, loops and open chains, into a sheet it
    /// founds (ryucad 18).
    pub surface: bool,
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

/// Refuses a surface sweep that acts on something: a surface founds the
/// sheet it sweeps, and joining sheets is sewing.
fn surface_founds(surface: bool, combine: &Combine) -> Result<()> {
    if surface && !combine.is_new() {
        return Err(Error::new(
            Code::InvalidParam,
            format!(
                "a surface sweep founds a sheet of its own, so its operation is new, not {}: sheets are joined by sewing",
                combine.operation.as_str()
            ),
        )
        .with("pointer", "/operation"));
    }
    Ok(())
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
        if self.surface {
            v["surface"] = json!(true);
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
            surface: flag(o, "surface")?,
            combine: Combine::from_fields(o)?,
        };
        surface_founds(e.surface, &e.combine)?;
        if e.surface && matches!(e.extent, Extent::UpTo { .. } | Extent::ThroughAll { .. }) {
            return Err(Error::new(
                Code::InvalidParam,
                "a surface extrude is blind or symmetric: up_to and through_all reach what it acts on, and a surface acts on nothing",
            )
            .with("pointer", "/extent/type"));
        }
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
        if self.surface {
            v["surface"] = json!(true);
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
        let r = Revolve {
            sketch: sketch_id(o)?,
            loops: loops_from_json(o.take_opt("loops"))?,
            axis,
            angle: Angle::from_json(&o.take("angle")?, env)?,
            symmetric: flag(o, "symmetric")?,
            surface: flag(o, "surface")?,
            combine: Combine::from_fields(o)?,
        };
        surface_founds(r.surface, &r.combine)?;
        Ok(r)
    }
}

/// Sweeps along a path and lofts are new in feature format 8.
pub const FORMAT: u64 = 8;

/// Surface sweeps (`surface: true` on an extrude, revolve, sweep or loft)
/// and planar faces are new in feature format 12 (ryucad 18). Format 11
/// is ryucad 16's sheet metal; see plan 389's Feature format for what
/// happens if another stage takes 12 first.
pub const SURFACE_FORMAT: u64 = 12;

/// What a sweep's profile or a loft's section is drawn from.
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
    /// Sweeps its surface profile, loops and open chains, into a sheet it
    /// founds (ryucad 18).
    pub surface: bool,
    pub combine: Combine,
}

/// The request fields a sweep takes, by their request names.
pub const SWEEP_FIELDS: [&str; 11] = [
    "sketch",
    "face",
    "loops",
    "path",
    "law",
    "direction",
    "twist",
    "scale",
    "surface",
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
        if self.surface {
            v["surface"] = json!(true);
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
        let surface = flag(o, "surface")?;
        let combine = Combine::from_fields(o)?;
        surface_founds(surface, &combine)?;
        Ok(Sweep {
            profile,
            path,
            law,
            twist,
            scale,
            surface,
            combine,
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

/// Which way a loft leaves its first section or enters its last.
#[derive(Clone, Debug, PartialEq)]
pub enum TangentDirection {
    /// The section's own normal, facing along the loft.
    Normal,
    /// An axis's direction or a plane's normal, made to face along the
    /// loft.
    Ref(GeomRef),
}

/// A loft's derivative at an end section: its direction, and its weight
/// (a positive number, 1 when none) times the straight distance between
/// the first and last sections' centroids.
#[derive(Clone, Debug, PartialEq)]
pub struct Tangent {
    pub direction: TangentDirection,
    pub weight: Option<Quantity>,
}

/// A loft: a skin through two or more planar sections, in order, round
/// to the first again when `closed`.
#[derive(Clone, Debug, PartialEq)]
pub struct Loft {
    pub sections: Vec<Section>,
    pub closed: bool,
    pub start_tangent: Option<Tangent>,
    pub end_tangent: Option<Tangent>,
    /// Skins its sections' loops and open chains into a sheet it founds
    /// (ryucad 18).
    pub surface: bool,
    pub combine: Combine,
}

/// The request fields a loft takes, by their request names.
pub const LOFT_FIELDS: [&str; 9] = [
    "sections",
    "closed",
    "start_tangent",
    "start_weight",
    "end_tangent",
    "end_weight",
    "surface",
    "operation",
    "targets",
];

impl Tangent {
    pub fn to_json(&self) -> Value {
        let mut v = json!({"direction": match &self.direction {
            TangentDirection::Normal => json!("normal"),
            TangentDirection::Ref(g) => g.to_json(),
        }});
        if let Some(q) = &self.weight {
            v["weight"] = json!(q.text());
        }
        v
    }

    /// Reads a tangent; `at` is its JSON pointer for errors.
    pub fn from_json(v: &Value, at: &str, env: &Env) -> Result<Tangent> {
        let mut o = Obj::new(v, at)?;
        let d = o.take("direction")?;
        let direction = if d == "normal" {
            TangentDirection::Normal
        } else {
            TangentDirection::Ref(GeomRef::from_json(&d, &format!("{at}/direction"))?)
        };
        let weight = match o.take_opt("weight") {
            None => None,
            Some(v) => {
                let p = format!("{at}/weight");
                let q = Quantity::from_json_as(&v, Field::Number, &|n| env.get(n))
                    .map_err(|e| e.with("pointer", &p))?;
                if q.number().is_nan() || q.number() <= 0.0 || !q.number().is_finite() {
                    return Err(Error::new(
                        Code::InvalidParam,
                        format!("a loft's tangent weight is more than nothing, not {q}"),
                    )
                    .with("pointer", p)
                    .with("value", q.text()));
                }
                Some(q)
            }
        };
        o.finish()?;
        Ok(Tangent { direction, weight })
    }

    /// The weight, 1 when none.
    pub fn weight_number(&self) -> f64 {
        self.weight.as_ref().map_or(1.0, Quantity::number)
    }
}

impl Loft {
    pub fn fields(&self) -> Value {
        let mut v = json!({
            "sections": self.sections.iter().map(Section::to_json).collect::<Vec<_>>(),
        });
        if self.closed {
            v["closed"] = json!(true);
        }
        if let Some(t) = &self.start_tangent {
            v["start_tangent"] = t.to_json();
        }
        if let Some(t) = &self.end_tangent {
            v["end_tangent"] = t.to_json();
        }
        if self.surface {
            v["surface"] = json!(true);
        }
        self.combine.write(&mut v);
        v
    }

    #[doc(hidden)]
    pub fn from_fields(o: &mut Obj, env: &Env) -> Result<Loft> {
        let v = o.take("sections")?;
        let a = v.as_array().ok_or_else(|| {
            invalid(
                "/sections",
                "sections is an array of sections, each {sketch} or {face}",
            )
        })?;
        let sections = a
            .iter()
            .enumerate()
            .map(|(i, x)| Section::from_json(x, &format!("/sections/{i}")))
            .collect::<Result<Vec<_>>>()?;
        let closed = flag(o, "closed")?;
        let mut tangent = |key: &str| {
            o.take_opt(key)
                .map(|v| Tangent::from_json(&v, &format!("/{key}"), env))
                .transpose()
        };
        let start_tangent = tangent("start_tangent")?;
        let end_tangent = tangent("end_tangent")?;
        let want = if closed { 3 } else { 2 };
        if sections.len() < want {
            return Err(Error::new(
                Code::InvalidParam,
                format!(
                    "a loft takes {want} sections or more{}, not {}",
                    if closed { " round a ring" } else { "" },
                    sections.len()
                ),
            )
            .with("pointer", "/sections"));
        }
        if closed {
            for (k, t) in [
                ("start_tangent", &start_tangent),
                ("end_tangent", &end_tangent),
            ] {
                if t.is_some() {
                    return Err(invalid(
                        &format!("/{k}"),
                        "a closed loft has no ends, so no end tangents",
                    ));
                }
            }
        }
        let surface = flag(o, "surface")?;
        let combine = Combine::from_fields(o)?;
        surface_founds(surface, &combine)?;
        Ok(Loft {
            sections,
            closed,
            start_tangent,
            end_tangent,
            surface,
            combine,
        })
    }

    /// Its references to geometry, each with its role: each face
    /// section's `sections.<i>`, and each tangent's direction
    /// (`start_tangent`, `end_tangent`).
    pub fn geometry(&self) -> Vec<(String, &GeomRef)> {
        let mut out = Vec::new();
        for (i, s) in self.sections.iter().enumerate() {
            if let Source::Face(g) = &s.source {
                out.push((format!("sections.{i}"), g));
            }
        }
        for (k, t) in [
            ("start_tangent", &self.start_tangent),
            ("end_tangent", &self.end_tangent),
        ] {
            if let Some(Tangent {
                direction: TangentDirection::Ref(g),
                ..
            }) = t
            {
                out.push((k.to_owned(), g));
            }
        }
        out
    }

    /// The sketches its sections are drawn on, in order, each once.
    pub fn sketches(&self) -> Vec<&Id> {
        let mut out: Vec<&Id> = Vec::new();
        for s in self.sections.iter().filter_map(Section::sketch) {
            if !out.contains(&s) {
                out.push(s);
            }
        }
        out
    }

    /// Its quantities, each with its field, as written.
    pub fn quantities(&self) -> Vec<(String, &Quantity)> {
        let mut out = Vec::new();
        for (k, t) in [
            ("start_tangent", &self.start_tangent),
            ("end_tangent", &self.end_tangent),
        ] {
            if let Some(q) = t.as_ref().and_then(|t| t.weight.as_ref()) {
                out.push((format!("{k}/weight"), q));
            }
        }
        out
    }
}

/// A planar face: a profile's closed loops, one plane face per lump, a
/// sheet it founds (ryucad 18). Its faces are named as an extrude's start
/// cap would be.
#[derive(Clone, Debug, PartialEq)]
pub struct Planar {
    pub profile: Section,
}

/// The fields a request may give a planar face.
pub const PLANAR_FIELDS: [&str; 3] = ["sketch", "face", "loops"];

impl Planar {
    pub fn fields(&self) -> Value {
        json!({"profile": self.profile.to_json()})
    }

    #[doc(hidden)]
    pub fn from_fields(o: &mut Obj) -> Result<Planar> {
        if o.has("operation") || o.has("targets") {
            return Err(Error::new(
                Code::InvalidParam,
                "a planar face founds a sheet of its own: it has no operation or targets",
            )
            .with("pointer", "/operation"));
        }
        Ok(Planar {
            profile: Section::from_json(&o.take("profile")?, "/profile")?,
        })
    }

    /// Its face section's reference, if it is one.
    pub fn geometry(&self) -> Vec<(String, &GeomRef)> {
        match &self.profile.source {
            Source::Face(g) => vec![("profile".to_owned(), g)],
            Source::Sketch(_) => Vec::new(),
        }
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
    fn a_surface_extrude_or_revolve_is_format_12_and_founds_a_sheet() {
        let mut v = extrude();
        v["surface"] = json!(true);
        v["format"] = json!(SURFACE_FORMAT);
        let f = read(&v).unwrap();
        let Feature::Extrude(e) = &f else { panic!() };
        assert!(e.surface);
        assert!(f.surface() && f.founds());
        assert_eq!(f.format(), SURFACE_FORMAT);
        assert_eq!(
            crate::doc::files::feature_format(&f.to_json()),
            SURFACE_FORMAT
        );
        assert_eq!(f.to_json()["surface"], true);
        assert_eq!(read(&f.to_json()).unwrap(), f);
        // Not a surface: nothing written, the format as before.
        let f = read(&extrude()).unwrap();
        assert!(f.to_json().get("surface").is_none());
        assert_eq!(f.format(), 2);
        let mut r = revolve();
        r["surface"] = json!(true);
        r["format"] = json!(SURFACE_FORMAT);
        let f = read(&r).unwrap();
        assert_eq!(f.format(), SURFACE_FORMAT);
        assert_eq!(f.to_json()["surface"], true);
        assert_eq!(read(&f.to_json()).unwrap(), f);
    }

    #[test]
    fn a_surface_sweep_founds_and_reaches_nothing() {
        let cases = [
            // Joining sheets is sewing.
            (
                json!({"operation": "join", "targets": ["b"]}),
                Code::InvalidParam,
                "/operation",
            ),
            (
                json!({"extent": {"type": "up_to", "target": "xy"}}),
                Code::InvalidParam,
                "/extent/type",
            ),
            (json!({"surface": "yes"}), Code::InvalidDocument, "/surface"),
        ];
        for (patch, code, pointer) in cases {
            let mut v = extrude();
            v["surface"] = json!(true);
            v["format"] = json!(SURFACE_FORMAT);
            for (k, x) in patch.as_object().unwrap() {
                v[k] = x.clone();
            }
            let e = read(&v).unwrap_err();
            assert_eq!(
                (e.code, e.details["pointer"].as_str()),
                (code, Some(pointer)),
                "{patch}: {e:?}"
            );
        }
        let mut v = revolve();
        v["surface"] = json!(true);
        v["format"] = json!(SURFACE_FORMAT);
        v["operation"] = json!("cut");
        v["targets"] = json!(["b"]);
        assert_eq!(read(&v).unwrap_err().details["pointer"], "/operation");
    }

    #[test]
    fn sweeps_lofts_and_planars_found_sheets_at_format_12() {
        let w = json!({"format": SURFACE_FORMAT, "kind": "ryucad.feature", "type": "sweep",
                       "profile": {"sketch": "s"}, "path": {"feature": "p1"}, "law": "minimal",
                       "surface": true});
        let f = read(&w).unwrap();
        assert_eq!((f.format(), f.to_json()), (SURFACE_FORMAT, w.clone()));
        let mut bad = w.clone();
        bad["operation"] = json!("join");
        bad["targets"] = json!(["b"]);
        assert_eq!(read(&bad).unwrap_err().details["pointer"], "/operation");
        let l = json!({"format": SURFACE_FORMAT, "kind": "ryucad.feature", "type": "loft",
                       "sections": [{"sketch": "a"}, {"sketch": "b"}], "surface": true});
        let f = read(&l).unwrap();
        assert_eq!((f.format(), f.to_json()), (SURFACE_FORMAT, l));
        let p = json!({"format": SURFACE_FORMAT, "kind": "ryucad.feature", "type": "planar",
                       "profile": {"sketch": "s", "loops": ["l1"]}});
        let f = read(&p).unwrap();
        assert_eq!(
            (f.type_name(), f.format(), f.to_json()),
            ("planar", SURFACE_FORMAT, p.clone())
        );
        assert!(f.founds());
        assert_eq!(f.profiles(), [&Id::parse("s", "feature").unwrap()]);
        assert_eq!(
            crate::doc::files::feature_acts(&p),
            crate::doc::files::Acts {
                founds: true,
                ..Default::default()
            }
        );
        let mut q = p.clone();
        q["operation"] = json!("join");
        assert_eq!(read(&q).unwrap_err().details["pointer"], "/operation");
    }

    #[test]
    fn a_sweep_or_loft_that_doesnt_load_still_says_what_it_does() {
        use crate::doc::files::{Acts, feature_acts};
        let id = |s: &str| Id::parse(s, "feature").unwrap();
        let new = json!({"type": "sweep", "profile": {"sketch": "s"}, "path": 3});
        assert_eq!(
            feature_acts(&new),
            Acts {
                founds: true,
                ..Default::default()
            }
        );
        let cut = json!({"type": "loft", "operation": "cut", "targets": ["b"], "sections": 3});
        assert_eq!(
            feature_acts(&cut),
            Acts {
                acts_on: vec![id("b")],
                ..Default::default()
            }
        );
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

    fn loft() -> Value {
        json!({"format": 8, "kind": "ryucad.feature", "type": "loft",
               "sections": [{"sketch": "s1"}, {"sketch": "s2", "loops": ["p7"]},
                            {"face": {"feature": "b", "name": "0123456789abcdef0123456789abcdef"}}],
               "start_tangent": {"direction": "normal", "weight": "depth / 5 mm"},
               "end_tangent": {"direction": "z"},
               "operation": "join", "targets": ["b"]})
    }

    #[test]
    fn a_loft_round_trips() {
        let f = read(&loft()).unwrap();
        assert_eq!(f.format(), FORMAT);
        assert_eq!(crate::doc::files::feature_format(&loft()), FORMAT);
        let text = canonical::to_string(&f.to_json());
        assert_eq!(read(&canonical::parse(&text).unwrap()).unwrap(), f);
        assert_eq!(f.to_json(), loft());
        let Feature::Loft(l) = &f else { panic!() };
        assert_eq!(l.sections.len(), 3);
        assert!(!l.closed);
        let start = l.start_tangent.as_ref().unwrap();
        assert_eq!(start.direction, TangentDirection::Normal);
        assert_eq!(start.weight_number(), 1.0);
        assert_eq!(l.end_tangent.as_ref().unwrap().weight_number(), 1.0);
        // Its sketches, its face section and tangent's references, and its
        // weight as an expression field.
        assert_eq!(
            f.profiles().iter().map(|s| s.as_str()).collect::<Vec<_>>(),
            ["s1", "s2"]
        );
        let roles: Vec<String> = f.geometry().into_iter().map(|(r, _)| r).collect();
        assert_eq!(roles, ["sections.2", "end_tangent"]);
        assert_eq!(
            f.references()
                .iter()
                .map(|(r, _)| r.as_str())
                .collect::<Vec<_>>(),
            ["sections.2"]
        );
        let fields: Vec<String> = crate::doc::files::expression_fields(&loft())
            .into_iter()
            .map(|(p, _)| p)
            .collect();
        assert_eq!(fields, ["/start_tangent/weight"]);
        // A closed ring of three sketches, no tangents: nothing more written.
        let v = json!({"format": 8, "kind": "ryucad.feature", "type": "loft", "closed": true,
                       "sections": [{"sketch": "a"}, {"sketch": "b"}, {"sketch": "a"}]});
        let f = read(&v).unwrap();
        assert_eq!(f.to_json(), v);
        assert_eq!(
            f.profiles().iter().map(|s| s.as_str()).collect::<Vec<_>>(),
            ["a", "b"],
            "each sketch once"
        );
    }

    #[test]
    fn a_lofts_fields_are_checked() {
        let cases: [(&str, Value, Code, &str); 8] = [
            ("/sections", json!({}), Code::InvalidDocument, "/sections"),
            (
                "/sections",
                json!([{"sketch": "s1"}]),
                Code::InvalidParam,
                "/sections",
            ),
            (
                "/sections",
                json!([{"sketch": "s1"}, {"face": "xy"}]),
                Code::InvalidDocument,
                "/sections/1/face",
            ),
            ("/closed", json!("yes"), Code::InvalidDocument, "/closed"),
            (
                "/start_tangent",
                json!({"direction": "normal", "weight": "0"}),
                Code::InvalidParam,
                "/start_tangent/weight",
            ),
            (
                "/start_tangent",
                json!({"direction": "normal", "weight": "2 mm"}),
                Code::UnitMismatch,
                "/start_tangent/weight",
            ),
            (
                "/end_tangent",
                json!({"direction": "sideways"}),
                Code::InvalidDocument,
                "/end_tangent/direction",
            ),
            ("/glue", json!(1), Code::InvalidDocument, "/glue"),
        ];
        for (path, value, code, pointer) in cases {
            let mut v = loft();
            v[&path[1..]] = value;
            let e = read(&v).unwrap_err();
            assert_eq!(
                (e.code, e.details["pointer"].as_str()),
                (code, Some(pointer)),
                "{path}: {e}"
            );
        }
        // A closed loft takes three sections or more, and no tangents.
        let mut v = loft();
        v["closed"] = json!(true);
        assert_eq!(read(&v).unwrap_err().details["pointer"], "/start_tangent");
        v.as_object_mut().unwrap().remove("start_tangent");
        v.as_object_mut().unwrap().remove("end_tangent");
        assert!(read(&v).is_ok());
        v["sections"] = json!([{"sketch": "s1"}, {"sketch": "s2"}]);
        let e = read(&v).unwrap_err();
        assert_eq!(
            (e.code, e.details["pointer"].as_str()),
            (Code::InvalidParam, Some("/sections"))
        );
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
