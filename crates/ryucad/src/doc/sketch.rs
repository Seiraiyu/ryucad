//! Sketch features: points, lines, arcs and circles on a plane, the
//! constraints and dimensions between them, and their solved positions.
//!
//! Entities and constraints are keyed by ID, never held in arrays, so two
//! people's edits to different entities merge line by line. Positions are
//! solver state, plain f64 meters written in shortest round-trip form;
//! dimensions are where intent lives, as expressions.

use super::files::{Obj, Placement, invalid, number};
use super::id::Id;
use super::reference::Reference;
use crate::error::{Code, Error, Result};
use crate::expr::Env;
use crate::units::{Field, Quantity, Unit};
use serde_json::{Map, Value, json};
use std::collections::BTreeMap;

/// One of the part's origin planes.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum OriginPlane {
    Xy,
    Yz,
    Zx,
}

impl OriginPlane {
    pub const ALL: [OriginPlane; 3] = [OriginPlane::Xy, OriginPlane::Yz, OriginPlane::Zx];

    pub fn as_str(self) -> &'static str {
        match self {
            OriginPlane::Xy => "xy",
            OriginPlane::Yz => "yz",
            OriginPlane::Zx => "zx",
        }
    }

    pub fn parse(s: &str) -> Result<OriginPlane> {
        OriginPlane::ALL
            .into_iter()
            .find(|p| p.as_str() == s)
            .ok_or_else(|| {
                Error::new(
                    Code::InvalidParam,
                    format!("unknown plane {s:?}; use xy, yz or zx"),
                )
                .with("plane", s)
            })
    }

    /// The plane's normal and its `x` direction (along its first axis).
    pub fn axes(self) -> ([f64; 3], [f64; 3]) {
        match self {
            OriginPlane::Xy => ([0.0, 0.0, 1.0], [1.0, 0.0, 0.0]),
            OriginPlane::Yz => ([1.0, 0.0, 0.0], [0.0, 1.0, 0.0]),
            OriginPlane::Zx => ([0.0, 1.0, 0.0], [0.0, 0.0, 1.0]),
        }
    }
}

/// Where a sketch lies: an origin plane, an explicit frame whose `xy`
/// plane it is, or a planar face of an earlier feature.
#[derive(Clone, Debug, PartialEq)]
pub enum Plane {
    Origin(OriginPlane),
    Placement(Placement),
    Face(OnFace),
}

/// A sketch on a planar face: its normal is the face's outward normal, its
/// origin the part origin projected onto the face's plane or a vertex, its
/// `x` the part's `x` projected (the part's `y` when `x` is along the
/// normal) or a straight edge, from its start.
#[derive(Clone, Debug, PartialEq)]
pub struct OnFace {
    pub face: Reference,
    pub origin: Option<Reference>,
    pub x: Option<Reference>,
}

impl Plane {
    /// As a placement: an origin plane's frame sits at the part origin.
    /// `None` on a face, whose frame only rebuild can tell.
    pub fn placement(&self, unit: Unit) -> Option<Placement> {
        match self {
            Plane::Origin(o) => {
                let (z, x) = o.axes();
                let mut p = Placement::identity(unit);
                p.z = z;
                p.x = x;
                Some(p)
            }
            Plane::Placement(p) => Some(p.clone()),
            Plane::Face(_) => None,
        }
    }

    /// Its references, each with its role: `plane` (the face), `origin`
    /// and `x`.
    pub fn references(&self) -> Vec<(String, &Reference)> {
        let Plane::Face(on) = self else {
            return Vec::new();
        };
        let mut out = vec![("plane".to_owned(), &on.face)];
        out.extend(on.origin.iter().map(|r| ("origin".to_owned(), r)));
        out.extend(on.x.iter().map(|r| ("x".to_owned(), r)));
        out
    }
}

/// A sketch entity's geometry. Lines, arcs and circles are built on point
/// entities, which constraints may share.
#[derive(Clone, Debug, PartialEq)]
pub enum Geometry {
    /// A point at `m` (meters, in the sketch's frame).
    Point {
        m: [f64; 2],
    },
    Line {
        start: Id,
        end: Id,
    },
    /// Counterclockwise from `start` to `end` about `center`.
    Arc {
        center: Id,
        start: Id,
        end: Id,
    },
    /// A full circle of radius `r_m` meters about `center`.
    Circle {
        center: Id,
        r_m: f64,
    },
}

impl Geometry {
    pub fn type_name(&self) -> &'static str {
        match self {
            Geometry::Point { .. } => "point",
            Geometry::Line { .. } => "line",
            Geometry::Arc { .. } => "arc",
            Geometry::Circle { .. } => "circle",
        }
    }

    /// The points it is built on, by field name.
    pub fn points(&self) -> Vec<(&'static str, &Id)> {
        match self {
            Geometry::Point { .. } => vec![],
            Geometry::Line { start, end } => vec![("start", start), ("end", end)],
            Geometry::Arc { center, start, end } => {
                vec![("center", center), ("start", start), ("end", end)]
            }
            Geometry::Circle { center, .. } => vec![("center", center)],
        }
    }
}

/// An entity: its geometry, whether it is construction geometry, which
/// profiles leave out, and whether it is projected from an earlier
/// feature's vertex or edge. A projected point, and the points a projected
/// curve is built on, are fixed where the projection puts them, as is a
/// projected circle's radius.
#[derive(Clone, Debug, PartialEq)]
pub struct Entity {
    pub geometry: Geometry,
    pub construction: bool,
    pub projected: Option<Reference>,
}

/// What a constraint requires.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ConstraintType {
    Coincident,
    Horizontal,
    Vertical,
    Parallel,
    Perpendicular,
    Tangent,
    Equal,
    Symmetric,
    Midpoint,
    On,
    Concentric,
    Fix,
    Distance,
    DistanceX,
    DistanceY,
    Length,
    Angle,
    Radius,
    Diameter,
}

/// The fields that name a constraint's entities, in order.
pub const ENTITY_FIELDS: [&str; 3] = ["a", "b", "about"];

impl ConstraintType {
    pub const ALL: [ConstraintType; 19] = [
        ConstraintType::Coincident,
        ConstraintType::Horizontal,
        ConstraintType::Vertical,
        ConstraintType::Parallel,
        ConstraintType::Perpendicular,
        ConstraintType::Tangent,
        ConstraintType::Equal,
        ConstraintType::Symmetric,
        ConstraintType::Midpoint,
        ConstraintType::On,
        ConstraintType::Concentric,
        ConstraintType::Fix,
        ConstraintType::Distance,
        ConstraintType::DistanceX,
        ConstraintType::DistanceY,
        ConstraintType::Length,
        ConstraintType::Angle,
        ConstraintType::Radius,
        ConstraintType::Diameter,
    ];

    pub fn as_str(self) -> &'static str {
        use ConstraintType as C;
        match self {
            C::Coincident => "coincident",
            C::Horizontal => "horizontal",
            C::Vertical => "vertical",
            C::Parallel => "parallel",
            C::Perpendicular => "perpendicular",
            C::Tangent => "tangent",
            C::Equal => "equal",
            C::Symmetric => "symmetric",
            C::Midpoint => "midpoint",
            C::On => "on",
            C::Concentric => "concentric",
            C::Fix => "fix",
            C::Distance => "distance",
            C::DistanceX => "distance_x",
            C::DistanceY => "distance_y",
            C::Length => "length",
            C::Angle => "angle",
            C::Radius => "radius",
            C::Diameter => "diameter",
        }
    }

    pub fn parse(s: &str) -> Result<ConstraintType> {
        ConstraintType::ALL
            .into_iter()
            .find(|t| t.as_str() == s)
            .ok_or_else(|| {
                let known: Vec<&str> = ConstraintType::ALL.iter().map(|t| t.as_str()).collect();
                Error::new(
                    Code::InvalidParam,
                    format!("unknown constraint type {s:?}; known: {}", known.join(", ")),
                )
                .with("type", s)
            })
    }

    /// How many entities it names (`a`, then `b`, then `about`).
    pub fn arity(self) -> usize {
        use ConstraintType as C;
        match self {
            C::Horizontal | C::Vertical | C::Fix | C::Length | C::Radius | C::Diameter => 1,
            C::Symmetric => 3,
            _ => 2,
        }
    }

    /// For a dimension, what its value is: a length or an angle.
    pub fn value(self) -> Option<Field> {
        use ConstraintType as C;
        match self {
            C::Distance | C::DistanceX | C::DistanceY | C::Length | C::Radius | C::Diameter => {
                Some(Field::Length(Unit::M))
            }
            C::Angle => Some(Field::Angle),
            _ => None,
        }
    }
}

/// A constraint or a dimension: the entities it names and, for a
/// dimension, its value.
#[derive(Clone, Debug, PartialEq)]
pub struct Constraint {
    pub ty: ConstraintType,
    pub entities: Vec<Id>,
    pub value: Option<Quantity>,
}

/// A sketch feature.
#[derive(Clone, Debug, PartialEq)]
pub struct Sketch {
    pub plane: Plane,
    pub entities: BTreeMap<Id, Entity>,
    pub constraints: BTreeMap<Id, Constraint>,
}

/// The solver's `u32` IDs for a sketch's entities and constraints: their
/// positions in ID order.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Index {
    pub entities: Vec<Id>,
    pub constraints: Vec<Id>,
}

impl Index {
    pub fn of(s: &Sketch) -> Index {
        Index {
            entities: s.entities.keys().cloned().collect(),
            constraints: s.constraints.keys().cloned().collect(),
        }
    }

    pub fn entity(&self, id: &Id) -> Option<u32> {
        self.entities.binary_search(id).ok().map(|i| i as u32)
    }

    pub fn constraint(&self, id: &Id) -> Option<u32> {
        self.constraints.binary_search(id).ok().map(|i| i as u32)
    }
}

/// The value a position takes in memory and in the file: `-0` is `0`,
/// which is what canonical JSON writes, so a write and a read give the
/// same bits.
pub fn clean(x: f64) -> f64 {
    if x == 0.0 { 0.0 } else { x }
}

impl Sketch {
    /// An empty sketch on `plane`.
    pub fn new(plane: Plane) -> Sketch {
        Sketch {
            plane,
            entities: BTreeMap::new(),
            constraints: BTreeMap::new(),
        }
    }

    /// Sketches are new in feature format 2.
    pub fn format(&self) -> u64 {
        2
    }

    /// Its fields in a feature file, besides the header.
    pub fn fields(&self) -> Value {
        let entities: Map<String, Value> = self
            .entities
            .iter()
            .map(|(id, e)| (id.to_string(), entity_json(e)))
            .collect();
        let constraints: Map<String, Value> = self
            .constraints
            .iter()
            .map(|(id, c)| (id.to_string(), constraint_json(c)))
            .collect();
        let mut v = json!({"constraints": constraints, "entities": entities});
        match &self.plane {
            Plane::Origin(o) => v["plane"] = json!(o.as_str()),
            Plane::Placement(p) => v["placement"] = p.to_json(),
            Plane::Face(on) => {
                v["face"] = on.face.to_json();
                if let Some(r) = &on.origin {
                    v["origin"] = r.to_json();
                }
                if let Some(r) = &on.x {
                    v["x"] = r.to_json();
                }
            }
        }
        v
    }

    /// Reads a sketch's fields, its dimensions' expressions reading `env`.
    /// Bare numbers in lengths take `default`, in angles degrees.
    pub(crate) fn from_fields(o: &mut Obj, default: Unit, env: &Env) -> Result<Sketch> {
        let (origin, x) = (o.take_opt("origin"), o.take_opt("x"));
        let plane = match (
            o.take_opt("plane"),
            o.take_opt("placement"),
            o.take_opt("face"),
        ) {
            (Some(p), None, None) => {
                let s = p
                    .as_str()
                    .ok_or_else(|| invalid("/plane", "plane must be xy, yz or zx"))?;
                Plane::Origin(OriginPlane::parse(s).map_err(|e| e.with("pointer", "/plane"))?)
            }
            (None, Some(p), None) => {
                Plane::Placement(Placement::from_json(&p, default, "/placement", env)?)
            }
            (None, None, Some(f)) => Plane::Face(OnFace {
                face: Reference::from_json(&f, "/face")?,
                origin: origin
                    .as_ref()
                    .map(|v| Reference::from_json(v, "/origin"))
                    .transpose()?,
                x: x.as_ref()
                    .map(|v| Reference::from_json(v, "/x"))
                    .transpose()?,
            }),
            _ => {
                return Err(invalid(
                    "/plane",
                    "a sketch has exactly one of plane, placement and face",
                ));
            }
        };
        if !matches!(plane, Plane::Face(_)) {
            for (k, v) in [("origin", &origin), ("x", &x)] {
                if v.is_some() {
                    return Err(invalid(
                        &format!("/{k}"),
                        &format!("{k} goes with face: a vertex or edge of a sketch's face"),
                    ));
                }
            }
        }
        let entities = o.take("entities")?;
        let entities = entities
            .as_object()
            .ok_or_else(|| invalid("/entities", "entities must be an object"))?;
        let constraints = o.take("constraints")?;
        let constraints = constraints
            .as_object()
            .ok_or_else(|| invalid("/constraints", "constraints must be an object"))?;
        let mut s = Sketch::new(plane);
        for (k, v) in entities {
            let at = format!("/entities/{k}");
            let id = Id::parse(k, "entity").map_err(|e| e.with("pointer", &at))?;
            s.entities.insert(id, entity_from_json(v, &at)?);
        }
        for (k, v) in constraints {
            let at = format!("/constraints/{k}");
            let id = Id::parse(k, "constraint").map_err(|e| e.with("pointer", &at))?;
            if s.entities.contains_key(&id) {
                return Err(Error::new(
                    Code::DuplicateId,
                    format!("{id} names both an entity and a constraint"),
                )
                .with("id", &id)
                .with("pointer", &at));
            }
            s.constraints
                .insert(id, constraint_from_json(v, &at, default, env)?);
        }
        s.validate()?;
        Ok(s)
    }

    /// Its references to other features, each with its role: the plane's,
    /// then each projected entity's (`entities.<id>`), in ID order.
    pub fn references(&self) -> Vec<(String, &Reference)> {
        let mut out = self.plane.references();
        for (id, e) in &self.entities {
            if let Some(r) = &e.projected {
                out.push((format!("entities.{id}"), r));
            }
        }
        out
    }

    /// Checks that every reference names an entity of the right kind and
    /// every value is in range, through the solver's own validation.
    pub fn validate(&self) -> Result<()> {
        self.solver().map(|_| ())
    }

    /// The sketch as the solver takes it, with the IDs it uses.
    pub fn solver(&self) -> Result<(ryusketch::Sketch, Index)> {
        self.solver_fixing(&std::collections::BTreeSet::new())
    }

    /// [`Sketch::solver`], with the points and circles in `fixed` held
    /// where they are.
    pub fn solver_fixing(
        &self,
        fixed: &std::collections::BTreeSet<&Id>,
    ) -> Result<(ryusketch::Sketch, Index)> {
        let index = Index::of(self);
        let mut out = ryusketch::Sketch::default();
        let find = |id: &Id, at: String| {
            index.entity(id).ok_or_else(|| {
                Error::new(
                    Code::InvalidDocument,
                    format!("no entity {id} in the sketch"),
                )
                .with("entity", id)
                .with("pointer", at)
            })
        };
        for (i, (id, e)) in self.entities.iter().enumerate() {
            let at = |f: &str| format!("/entities/{id}/{f}");
            let construction = e.construction;
            let solver = match &e.geometry {
                Geometry::Point { m } => ryusketch::Entity::Point {
                    at: *m,
                    fixed: fixed.contains(id),
                },
                Geometry::Line { start, end } => ryusketch::Entity::Line {
                    a: find(start, at("start"))?,
                    b: find(end, at("end"))?,
                    construction,
                },
                Geometry::Arc { center, start, end } => ryusketch::Entity::Arc {
                    center: find(center, at("center"))?,
                    start: find(start, at("start"))?,
                    end: find(end, at("end"))?,
                    construction,
                },
                Geometry::Circle { center, r_m } => ryusketch::Entity::Circle {
                    center: find(center, at("center"))?,
                    r: *r_m,
                    fixed: fixed.contains(id),
                    construction,
                },
            };
            out.entities.insert(i as u32, solver);
        }
        for (i, (id, c)) in self.constraints.iter().enumerate() {
            let e: Vec<u32> = c
                .entities
                .iter()
                .zip(ENTITY_FIELDS)
                .map(|(e, f)| find(e, format!("/constraints/{id}/{f}")))
                .collect::<Result<_>>()?;
            let v = c.value.as_ref().map_or(0.0, si);
            use ConstraintType as T;
            use ryusketch::Constraint as S;
            let solver = match c.ty {
                T::Coincident => S::Coincident(e[0], e[1]),
                T::Horizontal => S::Horizontal(e[0]),
                T::Vertical => S::Vertical(e[0]),
                T::Parallel => S::Parallel(e[0], e[1]),
                T::Perpendicular => S::Perpendicular(e[0], e[1]),
                T::Tangent => S::Tangent(e[0], e[1]),
                T::Equal => S::Equal(e[0], e[1]),
                T::Symmetric => S::Symmetric(e[0], e[1], e[2]),
                T::Midpoint => S::Midpoint(e[0], e[1]),
                T::On => S::On(e[0], e[1]),
                T::Concentric => S::Concentric(e[0], e[1]),
                T::Fix => S::Fix(e[0]),
                T::Distance => S::Distance(e[0], e[1], v),
                T::DistanceX => S::DistanceX(e[0], e[1], v),
                T::DistanceY => S::DistanceY(e[0], e[1], v),
                T::Length => S::Length(e[0], v),
                T::Angle => S::Angle(e[0], e[1], v),
                T::Radius => S::Radius(e[0], v),
                T::Diameter => S::Diameter(e[0], v),
            };
            out.constraints.insert(i as u32, solver);
        }
        out.validate().map_err(|e| self.fault(&index, e))?;
        Ok((out, index))
    }

    /// A solver's validation fault, in the file's terms.
    fn fault(&self, index: &Index, e: ryusketch::SketchError) -> Error {
        match e {
            ryusketch::SketchError::BadEntity { entity, why } => {
                let id = &index.entities[entity as usize];
                Error::new(Code::InvalidDocument, format!("entity {id}: {why}"))
                    .with("entity", id)
                    .with("pointer", format!("/entities/{id}"))
            }
            ryusketch::SketchError::BadConstraint { constraint, why } => {
                let id = &index.constraints[constraint as usize];
                let value = why.contains("value") || why.contains("negative");
                let (code, at) = if value {
                    (Code::InvalidParam, format!("/constraints/{id}/value"))
                } else {
                    (Code::InvalidDocument, format!("/constraints/{id}"))
                };
                Error::new(code, format!("constraint {id}: {why}"))
                    .with("constraint", id)
                    .with("pointer", at)
            }
        }
    }

    /// Takes solved positions and radii from the solver's sketch, as clean
    /// values.
    pub fn take_positions(&mut self, solved: &ryusketch::Sketch, index: &Index) {
        for (i, id) in index.entities.iter().enumerate() {
            let e = self.entities.get_mut(id).expect("indexed");
            match (&mut e.geometry, &solved.entities[&(i as u32)]) {
                (Geometry::Point { m }, ryusketch::Entity::Point { at, .. }) => {
                    *m = at.map(clean);
                }
                (Geometry::Circle { r_m, .. }, ryusketch::Entity::Circle { r, .. }) => {
                    *r_m = clean(*r);
                }
                _ => {}
            }
        }
    }

    /// The expression strings of its dimensions, from a feature file's
    /// `constraints` object.
    pub(crate) fn expression_strings(constraints: &mut Value) -> Vec<&mut String> {
        let Value::Object(cs) = constraints else {
            return Vec::new();
        };
        cs.values_mut()
            .filter_map(|c| match c.get_mut("value") {
                Some(Value::String(s)) => Some(s),
                _ => None,
            })
            .collect()
    }
}

/// A dimension's SI value.
pub fn si(q: &Quantity) -> f64 {
    match q.dim() {
        d if d == crate::units::Dim::ANGLE => q.radians(),
        _ => q.meters(),
    }
}

fn entity_json(e: &Entity) -> Value {
    let mut v = match &e.geometry {
        Geometry::Point { m } => json!({"m": m.map(number)}),
        Geometry::Line { start, end } => json!({"start": start, "end": end}),
        Geometry::Arc { center, start, end } => {
            json!({"center": center, "start": start, "end": end})
        }
        Geometry::Circle { center, r_m } => json!({"center": center, "r_m": number(*r_m)}),
    };
    v["type"] = json!(e.geometry.type_name());
    if e.construction {
        v["construction"] = json!(true);
    }
    if let Some(r) = &e.projected {
        v["projected"] = r.to_json();
    }
    v
}

fn constraint_json(c: &Constraint) -> Value {
    let mut v = json!({"type": c.ty.as_str()});
    for (id, f) in c.entities.iter().zip(ENTITY_FIELDS) {
        v[f] = json!(id);
    }
    if let Some(q) = &c.value {
        v["value"] = json!(q.text());
    }
    v
}

fn entity_from_json(v: &Value, at: &str) -> Result<Entity> {
    let mut o = Obj::new(v, at)?;
    let ty = o.take("type")?;
    let point = |o: &mut Obj, f: &str| -> Result<Id> {
        let v = o.take(f)?;
        let s = v
            .as_str()
            .ok_or_else(|| invalid(&format!("{at}/{f}"), "expected a point's ID"))?;
        Id::parse(s, "entity").map_err(|e| e.with("pointer", format!("{at}/{f}")))
    };
    let number_at = |v: &Value, f: &str| -> Result<f64> {
        v.as_f64()
            .ok_or_else(|| invalid(&format!("{at}/{f}"), "expected a number"))
    };
    let geometry = match ty.as_str() {
        Some("point") => {
            let m = o.take("m")?;
            let a = m
                .as_array()
                .filter(|a| a.len() == 2)
                .ok_or_else(|| invalid(&format!("{at}/m"), "expected two numbers"))?;
            Geometry::Point {
                m: [
                    clean(number_at(&a[0], "m/0")?),
                    clean(number_at(&a[1], "m/1")?),
                ],
            }
        }
        Some("line") => Geometry::Line {
            start: point(&mut o, "start")?,
            end: point(&mut o, "end")?,
        },
        Some("arc") => Geometry::Arc {
            center: point(&mut o, "center")?,
            start: point(&mut o, "start")?,
            end: point(&mut o, "end")?,
        },
        Some("circle") => Geometry::Circle {
            center: point(&mut o, "center")?,
            r_m: clean(number_at(&o.take("r_m")?, "r_m")?),
        },
        _ => {
            return Err(Error::new(
                Code::InvalidParam,
                format!("unknown entity type {ty}; known: point, line, arc, circle"),
            )
            .with("pointer", format!("{at}/type")));
        }
    };
    let construction = match o.take_opt("construction") {
        None => false,
        Some(Value::Bool(b)) if !matches!(geometry, Geometry::Point { .. }) => b,
        Some(_) => {
            return Err(invalid(
                &format!("{at}/construction"),
                "construction is true or false, on a line, arc or circle",
            ));
        }
    };
    let projected = o
        .take_opt("projected")
        .map(|v| Reference::from_json(&v, &format!("{at}/projected")))
        .transpose()?;
    o.finish()?;
    Ok(Entity {
        geometry,
        construction,
        projected,
    })
}

pub(crate) fn constraint_from_json(
    v: &Value,
    at: &str,
    default: Unit,
    env: &Env,
) -> Result<Constraint> {
    let mut o = Obj::new(v, at)?;
    let ty = o.take("type")?;
    let ty = ConstraintType::parse(
        ty.as_str()
            .ok_or_else(|| invalid(&format!("{at}/type"), "type must be a string"))?,
    )
    .map_err(|e| e.with("pointer", format!("{at}/type")))?;
    let mut entities = Vec::new();
    for f in &ENTITY_FIELDS[..ty.arity()] {
        let v = o.take(f)?;
        let s = v
            .as_str()
            .ok_or_else(|| invalid(&format!("{at}/{f}"), "expected an entity's ID"))?;
        entities.push(Id::parse(s, "entity").map_err(|e| e.with("pointer", format!("{at}/{f}")))?);
    }
    let value = match ty.value() {
        Some(field) => {
            let field = match field {
                Field::Length(_) => Field::Length(default),
                f => f,
            };
            let v = o.take("value")?;
            Some(
                Quantity::from_json_as(&v, field, &|n| env.get(n))
                    .map_err(|e| e.with("pointer", format!("{at}/value")))?,
            )
        }
        None => None,
    };
    o.finish()?;
    Ok(Constraint {
        ty,
        entities,
        value,
    })
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use crate::doc::canonical;
    use crate::doc::files::Feature;

    /// A 20 × 10 rectangle with a horizontal bottom, its width a
    /// parameter, and a construction diagonal.
    pub(crate) fn rectangle() -> Value {
        json!({
            "constraints": {
                "c1": {"a": "l1", "type": "horizontal"},
                "c2": {"a": "l1", "b": "l2", "type": "perpendicular"},
                "d1": {"a": "l1", "type": "length", "value": "width"},
                "d2": {"a": "p1", "b": "p3", "type": "distance", "value": 22.360679774997898}
            },
            "entities": {
                "l1": {"end": "p2", "start": "p1", "type": "line"},
                "l2": {"end": "p3", "start": "p2", "type": "line"},
                "l3": {"end": "p4", "start": "p3", "type": "line"},
                "l4": {"end": "p1", "start": "p4", "type": "line"},
                "l5": {"construction": true, "end": "p3", "start": "p1", "type": "line"},
                "p1": {"m": [0, 0], "type": "point"},
                "p2": {"m": [0.02, 0], "type": "point"},
                "p3": {"m": [0.02, 0.01], "type": "point"},
                "p4": {"m": [0, 0.01], "type": "point"}
            },
            "format": 2,
            "kind": "ryucad.feature",
            "plane": "xy",
            "type": "sketch"
        })
    }

    fn width() -> Env {
        let p = [(
            "width".to_owned(),
            crate::expr::Expr::parse("20 mm").unwrap(),
        )]
        .into();
        Env::new(&p)
    }

    fn read(v: &Value) -> Result<Sketch> {
        match Feature::from_json(v, Unit::Mm, &width())? {
            Feature::Sketch(s) => Ok(s),
            f => panic!("not a sketch: {f:?}"),
        }
    }

    #[test]
    fn a_sketch_round_trips() {
        let s = read(&rectangle()).unwrap();
        assert_eq!(s.plane, Plane::Origin(OriginPlane::Xy));
        assert_eq!(s.entities.len(), 9);
        let d1 = &s.constraints[&Id::parse("d1", "constraint").unwrap()];
        assert_eq!(d1.value.as_ref().unwrap().meters(), 0.02);
        let d2 = &s.constraints[&Id::parse("d2", "constraint").unwrap()];
        assert_eq!(d2.value.as_ref().unwrap().text(), "22.360679774997898 mm");
        let f = Feature::Sketch(s.clone());
        let v = f.to_json();
        assert_eq!(v["format"], 2);
        assert_eq!(v["entities"]["l5"]["construction"], true);
        assert!(v["entities"]["l1"].get("construction").is_none());
        let text = canonical::to_string(&v);
        let back = read(&canonical::parse(&text).unwrap()).unwrap();
        assert_eq!(back, s);
        assert_eq!(canonical::to_string(&Feature::Sketch(back).to_json()), text);
    }

    #[test]
    fn placements_and_origin_planes() {
        let mut v = rectangle();
        v["plane"] = json!("zx");
        let s = read(&v).unwrap();
        assert_eq!(s.plane.placement(Unit::Mm).unwrap().z, [0.0, 1.0, 0.0]);
        assert_eq!(s.plane.placement(Unit::Mm).unwrap().x, [0.0, 0.0, 1.0]);
        v.as_object_mut().unwrap().remove("plane");
        v["placement"] = json!({"origin": ["0", "0", "width"], "x": [1, 0, 0], "z": [0, 0, 1]});
        let s = read(&v).unwrap();
        let Plane::Placement(p) = &s.plane else {
            panic!()
        };
        assert_eq!(p.origin[2].meters(), 0.02);
        v["plane"] = json!("xy");
        assert_eq!(read(&v).unwrap_err().details["pointer"], "/plane");
        v.as_object_mut().unwrap().remove("placement");
        v["plane"] = json!("xz");
        let e = read(&v).unwrap_err();
        assert_eq!(
            (e.code, e.details["pointer"].as_str()),
            (Code::InvalidParam, Some("/plane"))
        );
    }

    #[test]
    fn sketches_on_faces() {
        const N: &str = "0123456789abcdef0123456789abcdef";
        let mut v = rectangle();
        v.as_object_mut().unwrap().remove("plane");
        v["face"] = json!({"feature": "b", "name": N});
        v["x"] = json!({"feature": "b", "name": N});
        let s = read(&v).unwrap();
        let Plane::Face(on) = &s.plane else { panic!() };
        assert_eq!((on.face.feature.as_str(), on.origin.is_none()), ("b", true));
        assert_eq!(
            s.plane
                .references()
                .iter()
                .map(|(r, _)| r.as_str())
                .collect::<Vec<_>>(),
            ["plane", "x"]
        );
        let f = Feature::Sketch(s.clone());
        assert_eq!(f.to_json()["x"], v["x"]);
        assert_eq!(read(&f.to_json()).unwrap(), s);
        assert!(s.plane.placement(Unit::Mm).is_none());
        // origin and x go only with a face; a face goes alone.
        let mut w = rectangle();
        w["origin"] = json!({"feature": "b", "name": N});
        assert_eq!(read(&w).unwrap_err().details["pointer"], "/origin");
        v["plane"] = json!("xy");
        assert_eq!(read(&v).unwrap_err().details["pointer"], "/plane");
    }

    #[test]
    fn projected_entities() {
        const N: &str = "0123456789abcdef0123456789abcdef";
        let mut v = rectangle();
        v["entities"]["l1"]["projected"] = json!({"feature": "b", "name": N});
        v["entities"]["p3"]["projected"] = json!({"feature": "b", "name": N});
        let s = read(&v).unwrap();
        assert_eq!(
            s.references()
                .iter()
                .map(|(r, _)| r.as_str())
                .collect::<Vec<_>>(),
            ["entities.l1", "entities.p3"]
        );
        assert_eq!(read(&Feature::Sketch(s.clone()).to_json()).unwrap(), s);
        let fixed = [Id::parse("p1", "entity").unwrap()];
        let (solver, index) = s.solver_fixing(&fixed.iter().collect()).unwrap();
        let p1 = index.entity(&fixed[0]).unwrap();
        assert!(matches!(
            solver.entities[&p1],
            ryusketch::Entity::Point { fixed: true, .. }
        ));
        v["entities"]["l1"]["projected"] = json!("b");
        assert_eq!(
            read(&v).unwrap_err().details["pointer"],
            "/entities/l1/projected"
        );
    }

    #[test]
    fn faults_point_at_the_field() {
        let cases: [(&str, Value, Code, &str); 11] = [
            (
                "/entities/l1/end",
                json!("p9"),
                Code::InvalidDocument,
                "/entities/l1/end",
            ),
            (
                "/entities/l1/end",
                json!("l2"),
                Code::InvalidDocument,
                "/entities/l1",
            ),
            (
                "/entities/l1/type",
                json!("spline"),
                Code::InvalidParam,
                "/entities/l1/type",
            ),
            (
                "/entities/p1/m",
                json!([0]),
                Code::InvalidDocument,
                "/entities/p1/m",
            ),
            (
                "/entities/p1/construction",
                json!(true),
                Code::InvalidDocument,
                "/entities/p1/construction",
            ),
            (
                "/entities/Bad",
                json!({"m": [0, 0], "type": "point"}),
                Code::InvalidId,
                "/entities/Bad",
            ),
            (
                "/constraints/c1/a",
                json!("p1"),
                Code::InvalidDocument,
                "/constraints/c1",
            ),
            (
                "/constraints/c1/type",
                json!("glue"),
                Code::InvalidParam,
                "/constraints/c1/type",
            ),
            (
                "/constraints/d1/value",
                json!("-1 mm"),
                Code::InvalidParam,
                "/constraints/d1/value",
            ),
            (
                "/constraints/d1/value",
                json!("15 deg"),
                Code::UnitMismatch,
                "/constraints/d1/value",
            ),
            (
                "/constraints/p1",
                json!({"a": "l1", "type": "horizontal"}),
                Code::DuplicateId,
                "/constraints/p1",
            ),
        ];
        for (path, value, code, pointer) in cases {
            let mut v = rectangle();
            let (parent, key) = path.rsplit_once('/').unwrap();
            v.pointer_mut(parent).unwrap()[key] = value;
            let e = read(&v).unwrap_err();
            assert_eq!(e.code, code, "{path}: {e}");
            assert_eq!(e.details["pointer"], pointer, "{path}");
        }
        let mut v = rectangle();
        v["constraints"]["c1"].as_object_mut().unwrap().remove("a");
        assert_eq!(
            read(&v).unwrap_err().details["pointer"],
            "/constraints/c1/a"
        );
        v = rectangle();
        v["constraints"]["d1"]["value"] = json!("gap");
        assert_eq!(read(&v).unwrap_err().code, Code::UnknownParameter);
    }

    #[test]
    fn angles_and_negative_zero() {
        let mut v = rectangle();
        v["constraints"]["d3"] = json!({"a": "l1", "b": "l5", "type": "angle", "value": 30});
        v["entities"]["p1"]["m"] = json!([-0.0, 0]);
        let s = read(&v).unwrap();
        let d3 = &s.constraints[&Id::parse("d3", "constraint").unwrap()];
        assert_eq!(d3.value.as_ref().unwrap().text(), "30 deg");
        let Geometry::Point { m } = s.entities[&Id::parse("p1", "entity").unwrap()].geometry else {
            panic!()
        };
        assert!(m[0].is_sign_positive());
        assert_eq!(clean(-0.0).to_bits(), 0.0f64.to_bits());
    }

    #[test]
    fn dimensions_name_parameters() {
        use crate::doc::files::{feature_names, rename_in_feature};
        let mut v = rectangle();
        assert_eq!(feature_names(&v).into_iter().collect::<Vec<_>>(), ["width"]);
        assert!(rename_in_feature(&mut v, "width", "w"));
        assert_eq!(v["constraints"]["d1"]["value"], "w");
    }

    #[test]
    fn the_solver_sees_ids_in_order() {
        let s = read(&rectangle()).unwrap();
        let (solver, index) = s.solver().unwrap();
        assert_eq!(index.entities[0].as_str(), "l1");
        assert_eq!(index.entity(&Id::parse("p1", "entity").unwrap()), Some(5));
        assert_eq!(solver.entities.len(), 9);
        assert_eq!(
            solver.constraints[&2],
            ryusketch::Constraint::Length(0, 0.02)
        );
    }
}
