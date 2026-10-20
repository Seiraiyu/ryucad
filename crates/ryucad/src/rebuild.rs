//! Features to ryukei bodies and solved sketches, in the order of the
//! part's dependency graph ([`crate::graph`]): every feature of one depth
//! in parallel, each reading the results of the features before it.
//! Results are collected in feature order.

use crate::doc::files::{Feature, Placement, Primitive, PrimitiveType};
use crate::doc::id::Id;
use crate::doc::project::Part;
use crate::doc::reference::Reference;
use crate::doc::sketch::Geometry;
use crate::doc::sweep::{Angle, Extent, RevolveAxis};
use crate::error::{Code, Error, Result};
use crate::profile::Keys;
use crate::sketch::Solved;
use core::f64::consts::TAU;
use rayon::prelude::*;
use ryukei::SweepError;
use ryukei::geom::{Axis, Frame, Point3, Rigid, Vector3};
use ryukei::name::{Journal, OpKey};
use ryukei::topo::Body;
use ryukei::{primitive, sweep};
use std::collections::BTreeMap;
use std::sync::Arc;

/// A feature's rebuilt body.
#[derive(Clone, Debug)]
pub struct Built {
    pub body: Body,
    pub journal: Journal,
    /// Where a sweep's entities came from in its sketch, by name (see
    /// [`crate::profile::origins`]); empty for a primitive.
    pub origins: Arc<BTreeMap<String, serde_json::Value>>,
    /// BLAKE3 of the body's dump: equal hashes mean bit-identical bodies.
    pub hash: String,
}

/// What a feature rebuilds to: a body, or a solved sketch.
// A part holds a handful of these; boxing the body would only add a
// pointer to every mesh and inspect.
#[allow(clippy::large_enum_variant)]
#[derive(Clone, Debug)]
pub enum Output {
    Body(Built),
    Sketch(Arc<Solved>),
}

impl Output {
    /// The body, if the feature makes one.
    pub fn body(&self) -> Option<&Built> {
        match self {
            Output::Body(b) => Some(b),
            Output::Sketch(_) => None,
        }
    }

    /// The solved sketch, if the feature is one.
    pub fn sketch(&self) -> Option<&Solved> {
        match self {
            Output::Sketch(s) => Some(s),
            Output::Body(_) => None,
        }
    }

    /// The body's or the sketch's hash: equal hashes, equal results.
    pub fn hash(&self) -> &str {
        match self {
            Output::Body(b) => &b.hash,
            Output::Sketch(s) => &s.hash,
        }
    }
}

/// The results of features built so far, by ID: what a feature reads its
/// inputs from, mid-rebuild or from a finished one.
pub trait Results: Sync {
    fn result(&self, id: &Id) -> Option<&Result<Output>>;
}

impl Results for BTreeMap<Id, Result<Output>> {
    fn result(&self, id: &Id) -> Option<&Result<Output>> {
        self.get(id)
    }
}

impl Results for PartResult {
    fn result(&self, id: &Id) -> Option<&Result<Output>> {
        self.features.iter().find(|(f, _)| f == id).map(|(_, r)| r)
    }
}

/// A part's rebuild: each feature's output or error, in feature order.
#[derive(Clone, Debug)]
pub struct PartResult {
    pub features: Vec<(Id, Result<Output>)>,
    /// Combines the features' hashes (or error codes) in order.
    pub hash: String,
}

impl PartResult {
    pub fn failed(&self) -> usize {
        self.features.iter().filter(|(_, r)| r.is_err()).count()
    }

    /// The feature's output, if it rebuilt.
    pub fn output(&self, id: &Id) -> Option<&Output> {
        self.features
            .iter()
            .find(|(f, _)| f == id)
            .and_then(|(_, r)| r.as_ref().ok())
    }

    /// `sketch_stale` for each sketch rebuilt from positions solved in
    /// memory.
    pub fn warnings(&self) -> Vec<serde_json::Value> {
        self.features
            .iter()
            .filter_map(|(id, r)| {
                let s = r.as_ref().ok()?.sketch()?;
                s.stale.then(|| crate::sketch::stale_warning(id))
            })
            .collect()
    }
}

/// The ryukei key for a feature: names depend only on the part and feature
/// IDs, which never change, so editing parameters keeps every name.
pub fn op_key(part: &Id, feature: &Id) -> OpKey {
    let mut bytes = b"ryucad.feature\0".to_vec();
    bytes.extend_from_slice(part.as_str().as_bytes());
    bytes.push(0);
    bytes.extend_from_slice(feature.as_str().as_bytes());
    OpKey::from_bytes(&bytes)
}

/// The rigid placement of a feature's local frame. `x` is made
/// perpendicular to `z`; axes with no direction, or parallel ones, are
/// `invalid_placement`.
pub fn placement(p: &Placement) -> Result<Rigid> {
    let [ox, oy, oz] = p.origin.each_ref().map(|q| q.meters());
    let v = |a: [f64; 3]| Vector3::new(a[0], a[1], a[2]);
    let frame = Frame::new(Point3::new(ox, oy, oz), v(p.z), v(p.x)).map_err(|e| {
        let why = match e {
            ryukei::Error::Degenerate => "z and x must be non-zero and not parallel",
            ryukei::Error::OutsideSizeBox => "the origin is outside the ±500 m size box",
            _ => "the axes must be finite",
        };
        Error::new(Code::InvalidPlacement, format!("bad placement: {why}"))
            .with("kernel", format!("{e:?}"))
    })?;
    Ok(Rigid::from_frame(&frame))
}

/// Builds one feature and checks the result.
pub fn build_feature(part: &Id, id: &Id, f: &Feature) -> Result<Built> {
    match f {
        Feature::Primitive(p) => build_primitive(part, id, p),
        Feature::Sketch(_) => Err(
            Error::new(Code::Internal, format!("sketch {id} makes no body")).with("feature", id),
        ),
        Feature::Extrude(_) | Feature::Revolve(_) => Err(Error::new(
            Code::Internal,
            format!("{} {id} is built from its sketch", f.type_name()),
        )
        .with("feature", id)),
    }
}

/// The solved sketch a sweep reads: listed before it, a sketch, and
/// rebuilt.
fn sketch_input<'a>(
    part: &Part,
    id: &Id,
    sketch: &Id,
    before: &'a dyn Results,
) -> Result<&'a crate::sketch::Solved> {
    let out = crate::graph::input(part, id, sketch, before)?;
    out.sketch().ok_or_else(|| {
        let ty = part.features[sketch]
            .as_ref()
            .map_or("feature", Feature::type_name);
        Error::new(
            Code::ReferenceWrongKind,
            format!("{id} sweeps {sketch}, which is a {ty}, not a sketch"),
        )
        .with("feature", id)
        .with("sketch", sketch)
    })
}

/// An extrude or a revolve, from its sketch's result.
fn build_sweep(part: &Part, id: &Id, f: &Feature, before: &dyn Results) -> Result<Built> {
    let ty = f.type_name();
    let sketch = f.swept().expect("a sweep");
    let solved = sketch_input(part, id, sketch, before)?;
    let (loops, axis) = match f {
        Feature::Extrude(e) => (&e.loops, None),
        Feature::Revolve(r) => match &r.axis {
            RevolveAxis::Line(l) => (&r.loops, Some(l)),
            RevolveAxis::Edge(_) => (&r.loops, None),
        },
        _ => unreachable!("a sweep"),
    };
    let (valid, keys) = crate::profile::profile(&part.id, sketch, solved, loops)
        .map_err(|e| e.with("feature", id).with("sketch", sketch))?;
    let key = op_key(&part.id, id);
    let made = match f {
        Feature::Extrude(e) => {
            let n = valid.frame().z().get();
            let dir = if e.reverse { -n } else { n };
            let (start, end) = match &e.extent {
                Extent::Blind(q) => (0.0, q.meters()),
                Extent::Symmetric(q) => (-q.meters() / 2.0, q.meters() / 2.0),
                Extent::UpTo { target, offset } => {
                    let to = UpTo {
                        target,
                        offset: offset.as_ref(),
                        from: valid.frame().origin(),
                        dir,
                    };
                    (0.0, to.length(part, id, before)?)
                }
            };
            sweep::extrude(key, &valid, dir, start, end)
        }
        Feature::Revolve(r) => {
            let axis = match &r.axis {
                RevolveAxis::Line(l) => sweep_axis(solved, id, l)?,
                RevolveAxis::Edge(e) => edge_axis(part, id, e, solved, before)?,
            };
            let (start, end) = match &r.angle {
                Angle::Full => (0.0, TAU),
                Angle::Of(q) if r.symmetric => (-q.radians() / 2.0, q.radians() / 2.0),
                Angle::Of(q) => (0.0, q.radians()),
            };
            sweep::revolve(key, &valid, axis, start, end)
        }
        _ => unreachable!("a sweep"),
    };
    let made = made.map_err(|e| sweep_error(id, ty, e, &keys, axis))?;
    let origins = crate::profile::origins(key, sketch, &valid, &keys);
    finish(id, ty, made, origins)
}

/// An up-to extent, from a point of the sketch's plane along `dir`.
struct UpTo<'a> {
    target: &'a Reference,
    offset: Option<&'a crate::units::Quantity>,
    from: Point3,
    dir: Vector3,
}

impl UpTo<'_> {
    /// How far the extrude reaches: to the target's plane (a planar face
    /// parallel to the sketch) or its point (a vertex), then the offset.
    fn length(&self, part: &Part, id: &Id, before: &dyn Results) -> Result<f64> {
        use crate::reference::{kind, plane, resolve, wrong_kind};
        use ryukei::topo::EntityId;
        let (t, text) = (self.target, self.target.text(&part.id));
        let invalid = |msg: String| {
            Error::new(
                Code::InvalidExtent,
                format!("extrude {id}: its target {text} {msg}"),
            )
            .with("feature", id)
            .with("role", "target")
            .with("reference", &text)
        };
        let found = resolve(part, part.order(), id, "target", t, before)?;
        let at = match found.1 {
            EntityId::Vertex(v) => found.0.body.vertex(v).expect("a live vertex").point,
            EntityId::Face(_) => {
                let (p, n) = plane(part, id, "target", t, found)?;
                if n.cross(self.dir).length() > ryukei::tol::ANGULAR_RESOLUTION {
                    return Err(invalid("isn't parallel to the sketch".into()));
                }
                p
            }
            e => {
                return Err(wrong_kind(
                    part,
                    id,
                    "target",
                    t,
                    kind(e),
                    "a planar face or a vertex",
                ));
            }
        };
        let offset = self.offset.map_or(0.0, crate::units::Quantity::meters);
        let l = (at - self.from).dot(self.dir) + offset;
        if l <= ryukei::tol::LINEAR_RESOLUTION {
            return Err(invalid(format!(
                "is no further along than the sketch's plane ({l} m)"
            ))
            .with("length", l));
        }
        Ok(l)
    }
}

/// A revolve's axis: a line of its sketch, from its start to its end.
fn sweep_axis(solved: &crate::sketch::Solved, id: &Id, axis: &Id) -> Result<Axis> {
    let s = &solved.sketch;
    let at = |p: &Id| match s.entities[p].geometry {
        Geometry::Point { m } => solved.frame.apply_point(Point3::new(m[0], m[1], 0.0)),
        _ => unreachable!("validated: lines join points"),
    };
    match s.entities.get(axis).map(|e| &e.geometry) {
        Some(Geometry::Line { start, end }) => {
            let (a, b) = (at(start), at(end));
            Axis::new(a, b - a).map_err(|e| {
                Error::new(
                    Code::InvalidExtent,
                    format!("revolve {id}: its axis {axis} has no length"),
                )
                .with("feature", id)
                .with("kernel", format!("{e:?}"))
            })
        }
        Some(g) => Err(Error::new(
            Code::ReferenceWrongKind,
            format!(
                "revolve {id}: its axis {axis} is a {}, not a line",
                g.type_name()
            ),
        )
        .with("feature", id)
        .with("axis", axis)),
        None => Err(Error::new(
            Code::NotFound,
            format!("revolve {id}: its sketch has no {axis}"),
        )
        .with("feature", id)
        .with("axis", axis)),
    }
}

/// A revolve's axis from a straight edge lying in the sketch's plane,
/// from its start to its end, its ends put exactly in the plane.
fn edge_axis(
    part: &Part,
    id: &Id,
    e: &Reference,
    solved: &crate::sketch::Solved,
    before: &dyn Results,
) -> Result<Axis> {
    let found = crate::reference::resolve(part, part.order(), id, "axis", e, before)?;
    let (a, b) = crate::reference::line(part, id, "axis", e, found)?;
    let inv = solved.frame.inverse();
    let (a, b) = (inv.apply_point(a), inv.apply_point(b));
    let text = e.text(&part.id);
    let invalid = |msg: &str| {
        Error::new(
            Code::InvalidExtent,
            format!("revolve {id}: its axis {text} {msg}"),
        )
        .with("feature", id)
        .with("role", "axis")
        .with("reference", &text)
    };
    if a.z.abs().max(b.z.abs()) > ryukei::tol::LINEAR_RESOLUTION {
        return Err(invalid("doesn't lie in the sketch's plane"));
    }
    let flat = |p: Point3| solved.frame.apply_point(Point3::new(p.x, p.y, 0.0));
    let (a, b) = (flat(a), flat(b));
    Axis::new(a, b - a).map_err(|k| invalid("has no length").with("kernel", format!("{k:?}")))
}

/// The kernel's refusal of a sweep, in the sketch's terms.
fn sweep_error(id: &Id, ty: &str, e: ryukei::Error, keys: &Keys, axis: Option<&Id>) -> Error {
    let kernel = format!("{e:?}");
    let (code, msg, entities) = match e {
        ryukei::Error::InvalidSweep(
            SweepError::CrossesAxis { segment }
            | SweepError::TouchesAxisInside { segment }
            | SweepError::PinchedOnAxis { segment },
        ) => {
            let seg = keys.segments[&segment.0].clone();
            let mut entities = vec![seg.clone()];
            entities.extend(axis.cloned());
            let msg = format!("{seg} crosses or touches the axis");
            (Code::ProfileInvalid, msg, entities)
        }
        ryukei::Error::InvalidSweep(SweepError::ShortSpan | SweepError::BadAngle) => (
            Code::InvalidExtent,
            "the sweep is too short".to_owned(),
            Vec::new(),
        ),
        e => (Code::RebuildFailed, e.to_string(), Vec::new()),
    };
    let mut err = Error::new(code, format!("{ty} {id}: {msg}"))
        .with("feature", id)
        .with("kernel", kernel);
    if !entities.is_empty() {
        err = err.with("entities", entities);
    }
    err
}

fn build_primitive(part: &Id, id: &Id, f: &Primitive) -> Result<Built> {
    let key = op_key(part, id);
    let at = placement(&f.placement)?;
    let m = |name| f.meters(name);
    let made = match f.ty {
        PrimitiveType::Box => primitive::block(key, m("x"), m("y"), m("z"), &at),
        PrimitiveType::Cylinder => primitive::cylinder(key, m("radius"), m("height"), &at),
        PrimitiveType::Cone if m("top_radius") > 0.0 => {
            primitive::frustum(key, m("radius"), m("top_radius"), m("height"), &at)
        }
        PrimitiveType::Cone => primitive::cone(key, m("radius"), m("height"), &at),
        PrimitiveType::Sphere => primitive::sphere(key, m("radius"), &at),
        PrimitiveType::Torus => primitive::torus(key, m("major_radius"), m("minor_radius"), &at),
    };
    let made = made.map_err(|e| {
        Error::new(Code::RebuildFailed, format!("{} {id}: {e}", f.ty.as_str()))
            .with("feature", id)
            .with("kernel", format!("{e:?}"))
    })?;
    finish(id, f.ty.as_str(), made, BTreeMap::new())
}

/// Checks a body the kernel built and hashes it. A broken body is never
/// reported as a success, in release builds too.
fn finish(
    id: &Id,
    ty: &str,
    (body, journal): (Body, Journal),
    origins: BTreeMap<String, serde_json::Value>,
) -> Result<Built> {
    let faults = ryukei::check::check(&body);
    if !faults.is_empty() {
        return Err(Error::new(
            Code::Internal,
            format!("{ty} {id}: the kernel built an invalid body"),
        )
        .with("feature", id)
        .with(
            "faults",
            faults.iter().map(|x| format!("{x:?}")).collect::<Vec<_>>(),
        ));
    }
    let hash = blake3::hash(body.dump().as_bytes()).to_hex()[..32].to_owned();
    Ok(Built {
        body,
        journal,
        origins: Arc::new(origins),
        hash,
    })
}

/// Rebuilds one feature: solves a sketch, sweeps an extrude or revolve's
/// sketch from `before` (the results of the features it may read), builds
/// anything else.
pub fn rebuild_feature(part: &Part, id: &Id, f: &Feature, before: &dyn Results) -> Result<Output> {
    match f {
        Feature::Sketch(s) => crate::sketch::context(part, part.order(), id, s, before)
            .and_then(|c| crate::sketch::solve_in(id, s, &c))
            .map(|s| Output::Sketch(Arc::new(s))),
        Feature::Extrude(_) | Feature::Revolve(_) => {
            build_sweep(part, id, f, before).map(Output::Body)
        }
        f => build_feature(&part.id, id, f).map(Output::Body),
    }
}

/// Rebuilds every listed feature of `part`: each depth of its graph in
/// turn, the features of a depth in parallel.
pub fn rebuild_part(part: &Part) -> PartResult {
    let depths = crate::graph::depths(part);
    let mut built: BTreeMap<Id, Result<Output>> = BTreeMap::new();
    for depth in 0.. {
        let now: Vec<&Id> = part
            .order()
            .iter()
            .filter(|id| depths[*id] == depth)
            .collect();
        if now.is_empty() {
            break;
        }
        let results: Vec<(Id, Result<Output>)> = now
            .par_iter()
            .map(|id| {
                let r = match &part.features[*id] {
                    Ok(f) => rebuild_feature(part, id, f, &built),
                    Err(e) => Err(e.clone()),
                };
                ((*id).clone(), r)
            })
            .collect();
        built.extend(results);
    }
    let features: Vec<(Id, Result<Output>)> = part
        .order()
        .iter()
        .map(|id| (id.clone(), built[id].clone()))
        .collect();
    let mut h = blake3::Hasher::new();
    for (id, r) in &features {
        let piece = match r {
            Ok(b) => format!("{id} ok {}\n", b.hash()),
            Err(e) => format!("{id} err {}\n", serde_json::json!(e.code)),
        };
        h.update(piece.as_bytes());
    }
    PartResult {
        features,
        hash: h.finalize().to_hex()[..32].to_owned(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::units::{Quantity, Unit};

    fn id(s: &str) -> Id {
        Id::parse(s, "feature").unwrap()
    }

    fn feature(ty: PrimitiveType, params: &[(&str, &str)]) -> Feature {
        Feature::Primitive(Primitive {
            ty,
            params: params
                .iter()
                .map(|(k, v)| (k.to_string(), Quantity::parse(v, Unit::Mm).unwrap()))
                .collect(),
            placement: Placement::identity(Unit::Mm),
        })
    }

    #[test]
    fn every_type_builds_a_valid_body() {
        for f in [
            feature(PrimitiveType::Box, &[("x", "10"), ("y", "20"), ("z", "30")]),
            feature(
                PrimitiveType::Cylinder,
                &[("radius", "5"), ("height", "10")],
            ),
            feature(PrimitiveType::Cone, &[("radius", "5"), ("height", "10")]),
            feature(
                PrimitiveType::Cone,
                &[("radius", "5"), ("height", "10"), ("top_radius", "2")],
            ),
            feature(PrimitiveType::Sphere, &[("radius", "5")]),
            feature(
                PrimitiveType::Torus,
                &[("major_radius", "10"), ("minor_radius", "2")],
            ),
        ] {
            let b = build_feature(&id("p"), &id("f"), &f).unwrap();
            assert!(ryukei::check::check(&b.body).is_empty());
            assert_eq!(b.hash.len(), 32);
        }
    }

    #[test]
    fn placement_is_exact_for_axis_aligned_frames() {
        let mut p = Placement::identity(Unit::Mm);
        p.origin[2] = Quantity::parse("10 mm", Unit::Mm).unwrap();
        p.z = [0.0, 0.0, 2.0];
        p.x = [3.0, 0.0, 5.0];
        let r = placement(&p).unwrap();
        assert_eq!(
            r.apply_point(Point3::new(1.0, 0.0, 0.0)),
            Point3::new(1.0, 0.0, 0.01)
        );
        assert_eq!(
            r.apply_vector(Vector3::new(0.0, 1.0, 0.0)),
            Vector3::new(0.0, 1.0, 0.0)
        );
    }

    #[test]
    fn bad_placements() {
        for (z, x) in [
            ([0.0, 0.0, 0.0], [1.0, 0.0, 0.0]),
            ([0.0, 0.0, 1.0], [0.0, 0.0, -3.0]),
        ] {
            let mut p = Placement::identity(Unit::Mm);
            p.z = z;
            p.x = x;
            assert_eq!(placement(&p).unwrap_err().code, Code::InvalidPlacement);
        }
        let mut p = Placement::identity(Unit::Mm);
        p.origin[0] = Quantity::parse("600 m", Unit::Mm).unwrap();
        let e = placement(&p).unwrap_err();
        assert_eq!(
            (e.code, e.details["kernel"].as_str()),
            (Code::InvalidPlacement, Some("OutsideSizeBox"))
        );
    }

    #[test]
    fn huge_axes_are_directions() {
        // Axes so long their squares overflow. Found by fuzzing.
        let mut p = Placement::identity(Unit::Mm);
        p.z = [1.1e170, 0.0, 1.0];
        p.x = [0.0, 1.0, 0.0];
        let r = placement(&p).unwrap();
        let z = r.apply_vector(Vector3::new(0.0, 0.0, 1.0));
        assert!((z - Vector3::new(1.0, 0.0, 0.0)).length() < 1e-15, "{z:?}");
    }

    #[test]
    fn kernel_refusals_are_rebuild_failures() {
        // Equal radii make no cone; a torus whose tube reaches the axis is degenerate.
        for f in [
            feature(
                PrimitiveType::Cone,
                &[("radius", "5"), ("height", "10"), ("top_radius", "5")],
            ),
            feature(
                PrimitiveType::Torus,
                &[("major_radius", "2"), ("minor_radius", "2")],
            ),
            feature(
                PrimitiveType::Box,
                &[("x", "2000 m"), ("y", "1"), ("z", "1")],
            ),
        ] {
            let e = build_feature(&id("p"), &id("f"), &f).unwrap_err();
            assert_eq!(e.code, Code::RebuildFailed, "{e}");
            assert_eq!(e.details["feature"], "f");
        }
    }

    #[test]
    fn keys_depend_on_both_ids() {
        assert_eq!(
            op_key(&id("p"), &id("f")),
            OpKey::from_bytes(b"ryucad.feature\0p\0f")
        );
        assert_ne!(op_key(&id("p"), &id("f")), op_key(&id("q"), &id("f")));
        assert_ne!(op_key(&id("p"), &id("f")), op_key(&id("p"), &id("g")));
    }
}
