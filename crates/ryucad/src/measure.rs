//! `measure`: an entity's properties, or the distance and angle between
//! two, from ryukei's [`ryukei::measure`]; and what [`crate::mass`] and
//! [`crate::clearance`] share: references resolved to placed bodies, and
//! values written in meters and the project's default unit.
//!
//! A reference is any entity reference (`<part>/<solid>/<name>`,
//! `<asm>/<path>/…/<solid>/<name>`), a part's solid as a whole
//! (`<part>/<solid>`), or an occurrence as a whole (`<asm>/<path>`, every
//! solid it places). An occurrence's entity is placed by its pose, so
//! distances across occurrences are in the assembly's frame.

use crate::doc::files::PartKind;
use crate::doc::id::Id;
use crate::entity::{EntityRef, Target};
use crate::error::{Code, Error, Result};
use crate::rebuild::Built;
use crate::session::Session;
use crate::units::Unit;
use ryukei::geom::{Point3, Rigid, UnitVector3};
use ryukei::measure::{
    self as kernel, EdgeShape, Entity, FaceShape, Properties, Value as Measured,
};
use ryukei::name::Name;
use ryukei::topo::EntityId;
use serde_json::{Value, json};
use std::sync::Arc;

/// One body a reference reaches: its solid, the entity of it, and where
/// the reference's model puts it.
#[derive(Clone, Debug)]
pub struct Located {
    /// The part whose solid it is: whose material it takes.
    pub part: Id,
    pub solid: Id,
    pub built: Arc<Built>,
    pub entity: Entity,
    pub pose: Rigid,
    /// `face`, `edge`, `vertex`, `solid` or `occurrence`.
    pub kind: &'static str,
}

/// The entity of `body` named by 32 hex digits.
fn named(body: &ryukei::topo::Body, name: &str) -> Option<Entity> {
    let n = Name::from_u128(u128::from_str_radix(name, 16).ok()?);
    Some(match body.by_name(n)? {
        EntityId::Face(f) => Entity::Face(f),
        EntityId::Edge(e) => Entity::Edge(e),
        EntityId::Vertex(v) => Entity::Vertex(v),
        _ => return None,
    })
}

/// The bodies `reference` reaches in the session's current rebuild; its
/// faults are [`EntityRef::resolve`]'s (`not_found` with how far it got).
pub fn locate(session: &mut Session, reference: &str) -> Result<Vec<Located>> {
    let r = EntityRef::parse(reference)?;
    let assembly = crate::server::part_kind(session.project().part(&r.part)?) == PartKind::Assembly;
    let whole_solid = !assembly && (r.is_occurrence() || !r.path.is_empty());
    if !whole_solid {
        match r.resolve(session)? {
            Target::Sketch(_) => {
                return Err(Error::new(
                    Code::InvalidParam,
                    format!("{reference} is a sketch entity: measure a face, edge or vertex, a solid or an occurrence"),
                )
                .with("entity", reference));
            }
            Target::Body(_) | Target::Occurrence => {}
        }
    }
    let r = if assembly { r.in_assembly() } else { r };
    let model = session.model(&r.part, &crate::doc::assembly::Rev::Working)?;
    let mut out = Vec::new();
    for o in &model.occurrences {
        let take = match (assembly, r.is_occurrence()) {
            (true, true) => o.path.starts_with(&r.path),
            (true, false) => o.path == r.path,
            (false, _) => o.path.is_empty(),
        };
        if !take {
            continue;
        }
        for s in &o.solids {
            let (entity, kind) = if r.is_occurrence() && assembly {
                (Entity::Body, "occurrence")
            } else if whole_solid {
                if s.founder != r.feature {
                    continue;
                }
                (Entity::Body, "solid")
            } else {
                if s.founder != r.feature {
                    continue;
                }
                let Some(e) = named(&s.built.body, &r.name) else {
                    continue;
                };
                let kind = match e {
                    Entity::Face(_) => "face",
                    Entity::Edge(_) => "edge",
                    _ => "vertex",
                };
                (e, kind)
            };
            out.push(Located {
                part: o.part.clone(),
                solid: s.founder.clone(),
                built: s.built.clone(),
                entity,
                pose: o.pose,
                kind,
            });
        }
    }
    if out.is_empty() {
        return Err(
            Error::new(Code::NotFound, format!("{reference} reaches no solid"))
                .with("entity", reference),
        );
    }
    Ok(out)
}

/// `measure_failed` for a kernel refusal, naming the references.
pub fn failed(e: ryukei::Error, entities: &[&str]) -> Error {
    let reason = match &e {
        ryukei::Error::InvalidMeasure(m) => match m {
            ryukei::MeasureError::NotAnEntity => "not_an_entity",
            ryukei::MeasureError::Unsupported => "unsupported",
            ryukei::MeasureError::Uncertain => "uncertain",
        },
        _ => "unsupported",
    };
    Error::new(
        Code::MeasureFailed,
        format!("can't measure {}: {e}", entities.join(" and ")),
    )
    .with("reason", reason)
    .with("kernel", e.to_string())
    .with("entities", entities)
}

/// A number as output writes it in the default unit: ten significant
/// digits, shortest.
pub fn number(x: f64) -> String {
    let r: f64 = format!("{x:.9e}").parse().unwrap_or(x);
    if r == 0.0 {
        "0".to_owned()
    } else if r.abs() < 1e-6 || r.abs() >= 1e15 {
        format!("{r:e}")
    } else {
        format!("{r}")
    }
}

/// What a measured value is: its SI key and how the default unit writes
/// it (`mm`, `mm^2`, `mm^3`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind {
    Length,
    Area,
    Volume,
    Angle,
    Mass,
    Inertia,
}

impl Kind {
    fn key(self) -> &'static str {
        match self {
            Kind::Length => "m",
            Kind::Area => "m2",
            Kind::Volume => "m3",
            Kind::Angle => "rad",
            Kind::Mass => "kg",
            Kind::Inertia => "kg_m2",
        }
    }

    /// The factor from SI to the unit `value` is written in, and its
    /// name.
    fn shown(self, unit: Unit) -> (f64, String) {
        let f = unit.factor();
        let u = unit.as_str();
        match self {
            Kind::Length => (f, u.to_owned()),
            Kind::Area => (f * f, format!("{u}^2")),
            Kind::Volume => (f * f * f, format!("{u}^3")),
            Kind::Angle => (crate::units::DEG, "deg".to_owned()),
            Kind::Mass => (1.0, "kg".to_owned()),
            Kind::Inertia => (f * f, format!("kg {u}^2")),
        }
    }
}

/// A measured value: `{"value": "12.5 mm", "m": 0.0125, "exact": true}`,
/// or with `"bound_m": …` (in the same SI unit) instead of `exact`.
pub fn value(v: Measured, kind: Kind, unit: Unit) -> Value {
    let (f, name) = kind.shown(unit);
    let mut out = json!({"value": format!("{} {name}", number(v.value() / f))});
    out[kind.key()] = json!(v.value());
    match v {
        Measured::Exact(_) => out["exact"] = json!(true),
        Measured::Bounded { bound, .. } => out[format!("bound_{}", kind.key())] = json!(bound),
    }
    out
}

/// A point: `{"m": [x, y, z], "value": "(1 mm, 2 mm, 3 mm)"}`.
pub fn point(p: Point3, unit: Unit) -> Value {
    let f = unit.factor();
    let u = unit.as_str();
    // Rounding's dust beside the point's size is written as 0.
    let size = p.x.abs().max(p.y.abs()).max(p.z.abs());
    let c = |x: f64| number(if x.abs() <= 1e-12 * size { 0.0 } else { x / f });
    json!({
        "m": [p.x, p.y, p.z],
        "value": format!("({} {u}, {} {u}, {} {u})", c(p.x), c(p.y), c(p.z)),
    })
}

fn direction(d: UnitVector3) -> Value {
    let d = d.get();
    json!([d.x, d.y, d.z])
}

/// An entity's properties, placed by `pose`.
fn properties(l: &Located, unit: Unit) -> Result<Value> {
    let body = &l.built.body;
    let p = kernel::properties(body, l.entity).map_err(|e| failed(e, &[]))?;
    let pose = &l.pose;
    let at = |q: Point3| point(pose.apply_point(q), unit);
    let dir = |d: UnitVector3| direction(pose.apply_unit(d));
    let len = |x: f64| value(Measured::Exact(x), Kind::Length, unit);
    let ang = |x: f64| value(Measured::Exact(x), Kind::Angle, unit);
    Ok(match p {
        Properties::Vertex { point } => json!({"point": at(point)}),
        Properties::Edge {
            length,
            start,
            end,
            shape,
        } => {
            let shape = match shape {
                EdgeShape::Line { direction } => {
                    json!({"type": "line", "direction": dir(direction)})
                }
                EdgeShape::Arc {
                    centre,
                    axis,
                    radius,
                    angle,
                } => json!({"type": "arc", "centre": at(centre), "axis": dir(axis),
                            "radius": len(radius), "angle": ang(angle)}),
                EdgeShape::Ellipse {
                    centre,
                    axis,
                    major_axis,
                    major,
                    minor,
                    angle,
                } => json!({"type": "ellipse", "centre": at(centre), "axis": dir(axis),
                            "major_axis": dir(major_axis), "major": len(major),
                            "minor": len(minor), "angle": ang(angle)}),
                _ => json!({"type": "spline"}),
            };
            json!({"length": value(length, Kind::Length, unit), "start": at(start),
                   "end": at(end), "shape": shape})
        }
        Properties::Face { area, shape } => {
            let shape = match shape {
                FaceShape::Plane { origin, normal } => {
                    json!({"type": "plane", "origin": at(origin), "normal": dir(normal)})
                }
                FaceShape::Cylinder {
                    origin,
                    axis,
                    radius,
                } => json!({"type": "cylinder", "origin": at(origin), "axis": dir(axis),
                            "radius": len(radius)}),
                FaceShape::Cone {
                    apex,
                    axis,
                    half_angle,
                } => json!({"type": "cone", "apex": at(apex), "axis": dir(axis),
                            "half_angle": ang(half_angle)}),
                FaceShape::Sphere { centre, radius } => {
                    json!({"type": "sphere", "centre": at(centre), "radius": len(radius)})
                }
                FaceShape::Torus {
                    centre,
                    axis,
                    major,
                    minor,
                } => json!({"type": "torus", "centre": at(centre), "axis": dir(axis),
                            "major": len(major), "minor": len(minor)}),
                _ => json!({"type": "other"}),
            };
            json!({"area": value(area, Kind::Area, unit), "shape": shape})
        }
        Properties::Body { volume, area } => json!({
            "volume": value(volume, Kind::Volume, unit),
            "area": value(area, Kind::Area, unit),
        }),
    })
}

/// The sum of bounded values: values add, bounds add.
fn sum(vs: impl IntoIterator<Item = Measured>) -> Measured {
    let (mut v, mut b, mut exact) = (0.0, 0.0, true);
    for x in vs {
        v += x.value();
        b += x.bound();
        exact &= matches!(x, Measured::Exact(_));
    }
    if exact {
        Measured::Exact(v)
    } else {
        Measured::Bounded { value: v, bound: b }
    }
}

/// The distance between two references, each reaching one or more
/// bodies: the least over every pair, its bound reaching down to the
/// least lower end.
fn distance(a: &[Located], b: &[Located], refs: &[&str]) -> Result<kernel::Distance> {
    let mut best: Option<kernel::Distance> = None;
    let mut low = f64::INFINITY;
    for x in a {
        for y in b {
            let d = kernel::distance(
                (&x.built.body, x.entity, &x.pose),
                (&y.built.body, y.entity, &y.pose),
                &kernel::DistanceOptions::default(),
            )
            .map_err(|e| failed(e, refs))?;
            low = low.min(d.distance.value() - d.distance.bound());
            if best.is_none_or(|b| d.distance.value() < b.distance.value()) {
                best = Some(d);
            }
        }
    }
    let mut d = best.expect("each side reaches a body");
    if a.len() * b.len() > 1 && d.distance.bound() > 0.0 {
        let v = d.distance.value();
        d.distance = Measured::Bounded {
            value: v,
            bound: v - low.max(0.0),
        };
    }
    Ok(d)
}

/// `measure` itself.
pub fn run(session: &mut Session, entities: &[String], rev: Option<&str>) -> Result<Value> {
    if entities.is_empty() || entities.len() > 2 {
        return Err(Error::new(
            Code::InvalidParam,
            format!("measure takes one entity or two, not {}", entities.len()),
        ));
    }
    let (commit, s) = session.at(rev)?;
    let unit = s.project().manifest.default_unit;
    let refs: Vec<&str> = entities.iter().map(String::as_str).collect();
    let located: Vec<Vec<Located>> = refs.iter().map(|r| locate(s, r)).collect::<Result<_>>()?;
    let mut v = match located.as_slice() {
        [one] => {
            let kind = one[0].kind;
            let mut v = if one.len() == 1 {
                properties(&one[0], unit).map_err(|e| e.with("entities", &refs))?
            } else {
                // Several solids: their volumes and areas together.
                let mut vols = Vec::new();
                let mut areas = Vec::new();
                for l in one {
                    match kernel::properties(&l.built.body, Entity::Body)
                        .map_err(|e| failed(e, &refs))?
                    {
                        Properties::Body { volume, area } => {
                            vols.push(volume);
                            areas.push(area);
                        }
                        _ => unreachable!("a body's properties"),
                    }
                }
                json!({"volume": value(sum(vols), Kind::Volume, unit),
                       "area": value(sum(areas), Kind::Area, unit)})
            };
            v["entity"] = json!(refs[0]);
            v["kind"] = json!(kind);
            v["solids"] = json!(one.len());
            v
        }
        [a, b] => {
            let d = distance(a, b, &refs)?;
            let mut v = json!({
                "entities": refs,
                "kinds": [a[0].kind, b[0].kind],
                "distance": value(d.distance, Kind::Length, unit),
                "points": [point(d.points[0], unit), point(d.points[1], unit)],
            });
            if let ([x], [y]) = (a.as_slice(), b.as_slice())
                && let Ok(an) = kernel::angle(
                    (&x.built.body, x.entity, &x.pose),
                    (&y.built.body, y.entity, &y.pose),
                )
            {
                v["angle"] = value(an.angle, Kind::Angle, unit);
                v["supplement"] = value(an.supplement, Kind::Angle, unit);
            }
            v
        }
        _ => unreachable!("one or two"),
    };
    if let Some(c) = commit {
        v["commit"] = json!(c);
    }
    v["unit"] = json!(unit.as_str());
    Ok(v)
}

#[cfg(test)]
mod tests {
    use super::number;
    use crate::assembly::tests::{place_in, project};
    use crate::error::Code;
    use crate::server::tests::{call, fresh};
    use serde_json::{Value, json};

    /// A part `p` with a box `b` 1 × 2 × 3 mm, and its faces' names.
    fn boxed() -> (tempfile::TempDir, crate::session::Session, Value) {
        let (dir, mut s) = fresh();
        call(&mut s, "part.create", json!({"part": "p"})).unwrap();
        call(
            &mut s,
            "feature.add",
            json!({"part": "p", "type": "box", "id": "b", "params": {"x": 1, "y": 2, "z": 3}}),
        )
        .unwrap();
        let body = call(&mut s, "inspect", json!({"part": "p"})).unwrap()["bodies"][0].clone();
        (dir, s, body)
    }

    fn measure(s: &mut crate::session::Session, refs: &[String]) -> Value {
        call(s, "measure", json!({"entities": refs})).unwrap()
    }

    #[test]
    fn a_boxs_entities_and_the_distance_between_its_faces() {
        let (_dir, mut s, body) = boxed();
        let faces: Vec<String> = body["faces"]
            .as_array()
            .unwrap()
            .iter()
            .map(|f| format!("p/b/{}", f["name"].as_str().unwrap()))
            .collect();
        let mut areas: Vec<f64> = faces
            .iter()
            .map(|f| {
                let v = measure(&mut s, std::slice::from_ref(f));
                assert_eq!(
                    (&v["kind"], &v["shape"]["type"]),
                    (&json!("face"), &json!("plane"))
                );
                assert!(v["area"]["bound_m2"].as_f64().unwrap() < 1e-15, "{v}");
                (v["area"]["m2"].as_f64().unwrap() * 1e6 * 1e6).round() / 1e6
            })
            .collect();
        areas.sort_by(f64::total_cmp);
        assert_eq!(areas, [2.0, 2.0, 3.0, 3.0, 6.0, 6.0]);
        // The two 2 × 3 faces face each other a millimetre apart.
        let big: Vec<&String> = faces
            .iter()
            .filter(|f| {
                let v = measure(&mut s, std::slice::from_ref(f));
                (v["area"]["m2"].as_f64().unwrap() - 6e-6).abs() < 1e-12
            })
            .collect();
        let v = measure(&mut s, &[big[0].clone(), big[1].clone()]);
        assert_eq!(v["distance"]["m"], json!(0.001), "{v}");
        assert_eq!(v["distance"]["exact"], true);
        assert_eq!(v["distance"]["value"], "1 mm");
        assert_eq!(v["angle"]["value"], "180 deg");
        let edge = format!("p/b/{}", body["edges"][0]["name"].as_str().unwrap());
        let v = measure(&mut s, &[edge]);
        assert_eq!(
            (&v["kind"], &v["shape"]["type"]),
            (&json!("edge"), &json!("line"))
        );
        assert_eq!(v["length"]["exact"], true);
        let vertex = format!("p/b/{}", body["vertices"][0]["name"].as_str().unwrap());
        let v = measure(&mut s, &[vertex]);
        assert!(v["point"]["m"].is_array(), "{v}");
        let v = measure(&mut s, &["p/b".to_owned()]);
        assert_eq!(v["kind"], "solid");
        assert!(
            (v["volume"]["m3"].as_f64().unwrap() - 6e-9).abs() < 1e-20,
            "{v}"
        );
        assert_eq!(v["volume"]["value"], "6 mm^3");
    }

    #[test]
    fn occurrences_are_measured_where_their_assembly_puts_them() {
        let (dir, mut s) = project();
        place_in(
            dir.path(),
            "frame",
            "p1",
            "plate",
            json!("working"),
            [0.0, 0.0, 0.0],
        );
        place_in(
            dir.path(),
            "frame",
            "b1",
            "bolt",
            json!("working"),
            [0.2, 0.0, 0.0],
        );
        s.refresh().unwrap();
        let v = measure(&mut s, &["frame/p1".to_owned(), "frame/b1".to_owned()]);
        // The bolt's axis is 100 mm past the plate's end, its radius 3 mm.
        let d = v["distance"]["m"].as_f64().unwrap();
        assert!((d - 0.097).abs() < 1e-9, "{v}");
        assert!(
            v["distance"]["bound_m"].as_f64().unwrap() < 1e-4 * 0.25,
            "{v}"
        );
        assert_eq!(v["kinds"], json!(["occurrence", "occurrence"]));
        assert!(v.get("angle").is_none());
    }

    #[test]
    fn numbers_are_written_to_ten_digits() {
        assert_eq!(number(0.1 + 0.2), "0.3");
        assert_eq!(number(-0.0), "0");
        assert_eq!(number(12.5), "12.5");
        assert_eq!(number(5.551115123125783e-16), "5.551115123e-16");
        assert_eq!(number(2.0e20), "2e20");
    }

    #[test]
    fn refusals() {
        let (_dir, mut s, body) = boxed();
        let e = call(&mut s, "measure", json!({"entities": []})).unwrap_err();
        assert_eq!(e.code, Code::InvalidParam);
        let e = call(&mut s, "measure", json!({"entities": ["p/c"]})).unwrap_err();
        assert_eq!(e.code, Code::NotFound);
        let vertex = format!("p/b/{}", body["vertices"][0]["name"].as_str().unwrap());
        let face = format!("p/b/{}", body["faces"][0]["name"].as_str().unwrap());
        // A vertex and a face: a distance, but no angle.
        let v = call(&mut s, "measure", json!({"entities": [vertex, face]})).unwrap();
        assert!(
            v.get("angle").is_none() && v["distance"]["exact"] == true,
            "{v}"
        );
        let e = call(
            &mut s,
            "measure",
            json!({"entities": ["p/b"], "rev": "HEAD"}),
        )
        .unwrap_err();
        assert_eq!(e.code, Code::NotAGitRepo);
    }
}
