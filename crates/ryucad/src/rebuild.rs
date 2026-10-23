//! Features to ryukei bodies and solved sketches, in the order of the
//! part's dependency graph ([`crate::graph`]): every feature of one depth
//! in parallel, each reading the results of the features before it.
//! Results are collected in feature order. A body-making feature founds a
//! solid, or joins its body to, cuts it from or intersects it with each
//! solid it targets, as its view of that solid ([`crate::solid`]) left it.

use crate::doc::files::{Feature, Placement, Primitive, PrimitiveType};
use crate::doc::id::Id;
use crate::doc::project::Part;
use crate::doc::reference::Reference;
use crate::doc::sketch::Geometry;
use crate::doc::solid::Operation;
use crate::doc::sweep::{Angle, Extent, RevolveAxis};
use crate::error::{Code, Error, Result};
use crate::profile::Keys;
use crate::sketch::Solved;
use crate::solid::Solids;
use core::f64::consts::TAU;
use rayon::prelude::*;
use ryukei::SweepError;
use ryukei::boolean::{BooleanOp, Tool};
use ryukei::geom::{Axis, Frame, Point3, Rigid, Vector3};
use ryukei::name::{Event, Journal, Name, OpKey, ToolKey};
use ryukei::topo::{Body, EntityId};
use ryukei::{BooleanError, primitive, sweep};
use serde_json::{Value, json};
use std::collections::{BTreeMap, BTreeSet};
use std::sync::Arc;

/// A solid as a feature leaves it.
#[derive(Clone, Debug)]
pub struct Built {
    pub body: Body,
    pub journal: Journal,
    /// Where each entity came from, by name: the feature that made it,
    /// and for a sweep's the sketch entities (see
    /// [`crate::profile::origins`]).
    pub origins: Arc<BTreeMap<String, Value>>,
    /// BLAKE3 of the body's dump: equal hashes mean bit-identical bodies.
    pub hash: String,
}

/// What a body-making feature leaves: the solid it founds, or each solid
/// it acts on, by founder in the order it names them.
#[derive(Clone, Debug)]
pub struct Made {
    pub solids: Vec<(Id, Arc<Built>)>,
    /// The body's hash for one solid, else a hash of each solid's founder
    /// and hash: equal hashes, equal results.
    pub hash: String,
}

impl Made {
    pub fn new(solids: Vec<(Id, Arc<Built>)>) -> Made {
        let hash = match solids.as_slice() {
            [(_, b)] => b.hash.clone(),
            _ => {
                let mut h = blake3::Hasher::new();
                for (id, b) in &solids {
                    h.update(format!("{id} {}\n", b.hash).as_bytes());
                }
                h.finalize().to_hex()[..32].to_owned()
            }
        };
        Made { solids, hash }
    }
}

/// What a feature rebuilds to: solids, or a solved sketch. Shared, so a
/// cached result costs nothing to reuse.
#[derive(Clone, Debug)]
pub enum Output {
    Solids(Arc<Made>),
    Sketch(Arc<Solved>),
}

impl Output {
    /// The solids a body-making feature founds or acts on, as it leaves
    /// them; none for a sketch.
    pub fn solids(&self) -> &[(Id, Arc<Built>)] {
        match self {
            Output::Solids(m) => &m.solids,
            Output::Sketch(_) => &[],
        }
    }

    /// The solid founded by `founder`, as this feature leaves it.
    pub fn solid(&self, founder: &Id) -> Option<&Built> {
        self.solids()
            .iter()
            .find(|(s, _)| s == founder)
            .map(|(_, b)| b.as_ref())
    }

    /// The body of a feature that founds or acts on exactly one solid.
    pub fn body(&self) -> Option<&Built> {
        match self.solids() {
            [(_, b)] => Some(b),
            _ => None,
        }
    }

    /// The solved sketch, if the feature is one.
    pub fn sketch(&self) -> Option<&Solved> {
        match self {
            Output::Sketch(s) => Some(s),
            Output::Solids(_) => None,
        }
    }

    /// The solids' or the sketch's hash: equal hashes, equal results.
    pub fn hash(&self) -> &str {
        match self {
            Output::Solids(m) => &m.hash,
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
    /// The features this rebuild built, in feature order; the others came
    /// from the cache.
    pub built: Vec<Id>,
    /// The joins, cuts and intersects whose own bodies this rebuild built,
    /// in feature order; the others' came from the cache.
    pub tools: Vec<Id>,
}

/// What a rebuild keeps for the next: each feature's result by the hash
/// of the inputs it was built from, and the body each join, cut or
/// intersect applied by the hash of that body's own inputs, so a change
/// to what it acts on alone builds it again from the body it applies.
#[derive(Clone, Debug, Default)]
pub struct Cache {
    pub features: BTreeMap<Id, (blake3::Hash, Result<Output>)>,
    pub tools: BTreeMap<Id, (blake3::Hash, Arc<Built>)>,
}

impl Cache {
    pub fn new() -> Cache {
        Cache::default()
    }
}

/// A join, cut or intersect's own body: the hash of what it is built
/// from, and the body as a cache holds it, if it does.
type ToolInput<'a> = (blake3::Hash, Option<&'a (blake3::Hash, Arc<Built>)>);

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

/// The ryukei key for the body a feature joins to, cuts from or
/// intersects with its targets: like [`op_key`], from IDs only.
pub fn tool_key(part: &Id, feature: &Id) -> ToolKey {
    let mut bytes = b"ryucad.tool\0".to_vec();
    bytes.extend_from_slice(part.as_str().as_bytes());
    bytes.push(0);
    bytes.extend_from_slice(feature.as_str().as_bytes());
    ToolKey(OpKey::from_bytes(&bytes).0)
}

/// The ryukei key for a solid a `boolean` feature uses as a tool, by the
/// solid's founder.
pub fn solid_key(part: &Id, feature: &Id, solid: &Id) -> ToolKey {
    let mut bytes = b"ryucad.tool\0".to_vec();
    for id in [part, feature, solid] {
        bytes.extend_from_slice(id.as_str().as_bytes());
        bytes.push(0);
    }
    bytes.pop();
    ToolKey(OpKey::from_bytes(&bytes).0)
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

/// Builds a primitive's own body, whatever its operation, and checks it.
pub fn build_feature(part: &Id, id: &Id, f: &Feature) -> Result<Built> {
    match f {
        Feature::Primitive(p) => build_primitive(part, id, p),
        Feature::Sketch(_) | Feature::Boolean(_) => Err(Error::new(
            Code::Internal,
            format!("{} {id} makes no body of its own", f.type_name()),
        )
        .with("feature", id)),
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

/// An extrude or a revolve, from its sketch's result; `targets` are the
/// solids it acts on, which a through-all extrude reaches past.
fn build_sweep(
    part: &Part,
    id: &Id,
    f: &Feature,
    targets: &[&Built],
    before: &dyn Results,
) -> Result<Built> {
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
                Extent::ThroughAll { both } => {
                    through_all(id, valid.frame().origin(), dir, *both, targets)?
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
    let mut origins = crate::profile::origins(key, sketch, &valid, &keys);
    for o in origins.values_mut() {
        o["feature"] = json!(id);
    }
    finish(id, ty, made, origins)
}

/// How far a through-all extrude goes from `from` along `dir`: a margin
/// past every point of its targets' boxes, both ways if `both`. Fails as
/// `invalid_extent` when all of them lie behind the sketch's plane.
fn through_all(
    id: &Id,
    from: Point3,
    dir: Vector3,
    both: bool,
    targets: &[&Built],
) -> Result<(f64, f64)> {
    let (mut lo, mut hi) = (f64::INFINITY, f64::NEG_INFINITY);
    for b in targets {
        for f in b.body.faces() {
            let bx = ryukei::intersect::face_box(&b.body, f);
            let (a, z) = (bx.min(), bx.max());
            for (x, y, w) in [
                (a.x, a.y, a.z),
                (z.x, a.y, a.z),
                (a.x, z.y, a.z),
                (z.x, z.y, a.z),
                (a.x, a.y, z.z),
                (z.x, a.y, z.z),
                (a.x, z.y, z.z),
                (z.x, z.y, z.z),
            ] {
                let d = (Point3::new(x, y, w) - from).dot(dir);
                lo = lo.min(d);
                hi = hi.max(d);
            }
        }
    }
    // A hundredth of the targets' depth along the direction, so no end
    // face lies on theirs.
    let margin = (hi - lo).max(1e-6) * 0.01;
    if both {
        return Ok((lo.min(0.0) - margin, hi.max(0.0) + margin));
    }
    if hi <= ryukei::tol::LINEAR_RESOLUTION {
        return Err(Error::new(
            Code::InvalidExtent,
            format!(
                "extrude {id}: everything it cuts lies behind the sketch's plane; reverse it, or go through all both ways"
            ),
        )
        .with("feature", id)
        .with("pointer", "/extent"));
    }
    Ok((0.0, hi + margin))
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
    let origins = names(&made.0)
        .into_iter()
        .map(|n| (n.to_string(), json!({"feature": id})))
        .collect();
    finish(id, f.ty.as_str(), made, origins)
}

/// The names of a body's faces, edges and vertices.
fn names(b: &Body) -> BTreeSet<Name> {
    let faces = b.faces().map(EntityId::Face);
    let edges = b.edges().map(EntityId::Edge);
    let vertices = b.vertices().map(EntityId::Vertex);
    faces
        .chain(edges)
        .chain(vertices)
        .map(|e| b.name_of(e).expect("a live entity"))
        .collect()
}

/// The solid founded by `t` as feature `id` sees it: as its view, the
/// last feature of its chain listed before `id`, left it.
pub fn target<'a>(
    part: &Part,
    solids: &Solids,
    id: &Id,
    t: &Id,
    before: &'a dyn Results,
) -> Result<&'a Built> {
    if let Some(by) = solids.consumed_by(t, id) {
        return Err(consumed(id, t, by));
    }
    let Some(view) = solids.view(t, id) else {
        // Not listed before `id` at all, or listed but no solid.
        if let Some(Ok(f)) = part.features.get(id) {
            crate::graph::check_order(part, part.order(), id, f)?;
        }
        return Err(not_a_solid(part, solids, id, "targets", t));
    };
    let out = crate::graph::input(part, id, view, before)?;
    Ok(out.solid(t).expect("a solid's chain holds it"))
}

/// `body_consumed`: feature `id` acts on solid `t`, which the boolean
/// `by` before it consumed.
pub fn consumed(id: &Id, t: &Id, by: &Id) -> Error {
    Error::new(
        Code::BodyConsumed,
        format!("{id} acts on {t}, which the boolean {by} consumed: act on its target instead"),
    )
    .with("feature", id)
    .with("solid", t)
    .with("consumed_by", by)
}

/// `reference_wrong_kind` for `t`, named in `owner`'s `role` where a
/// solid goes, which founds none: with `solids`, the solids it acts on,
/// for the agent to name instead.
pub fn not_a_solid(part: &Part, solids: &Solids, owner: &Id, role: &str, t: &Id) -> Error {
    let ty = part
        .features
        .get(t)
        .and_then(|f| f.as_ref().ok())
        .map_or("feature", Feature::type_name);
    let of = solids.solids_of(t);
    let hint = if of.is_empty() {
        String::new()
    } else {
        let list: Vec<&str> = of.iter().map(|s| s.as_str()).collect();
        format!(
            "; name its solid by the feature that founded it: {}",
            list.join(", ")
        )
    };
    let mut e = Error::new(
        Code::ReferenceWrongKind,
        format!("{owner}: its {role} {t} is a {ty} that founds no solid{hint}"),
    )
    .with("feature", owner)
    .with("role", role)
    .with("names", t);
    if !of.is_empty() {
        e = e.with("solids", of);
    }
    e
}

/// What a body-making feature leaves: its own body as a new solid, or
/// its body applied to each solid it targets.
fn build_body(
    part: &Part,
    solids: &Solids,
    id: &Id,
    f: &Feature,
    before: &dyn Results,
    tool: Option<ToolInput>,
) -> (Result<Made>, Option<Arc<Built>>) {
    let c = f.combine().expect("a body-making feature");
    // Its targets first: a cut of a solid that failed is
    // `dependency_failed`, whatever its own body would be.
    let targets: Result<Vec<(&Id, &Built)>> = c
        .targets
        .iter()
        .map(|t| Ok((t, target(part, solids, id, t, before)?)))
        .collect();
    let targets = match targets {
        Ok(t) => t,
        Err(e) => return (Err(e), None),
    };
    let bodies: Vec<&Built> = targets.iter().map(|(_, b)| *b).collect();
    let own = match tool {
        Some((h, Some((c, b)))) if *c == h => Ok(Arc::clone(b)),
        _ => match f {
            Feature::Primitive(p) => build_primitive(&part.id, id, p),
            _ => build_sweep(part, id, f, &bodies, before),
        }
        .map(Arc::new),
    };
    let own = match own {
        Ok(own) => own,
        Err(e) => return (Err(e), None),
    };
    if c.is_new() {
        return (Ok(Made::new(vec![(id.clone(), own)])), None);
    }
    let ty = f.type_name();
    let kop = match c.operation {
        Operation::Join => BooleanOp::Unite,
        Operation::Cut => BooleanOp::Subtract,
        Operation::Intersect => BooleanOp::Intersect,
        Operation::New => unreachable!("a new solid acts on nothing"),
    };
    let tool = [ToolBody {
        key: tool_key(&part.id, id),
        built: own.as_ref(),
        name: id,
    }];
    // The body is kept however applying it goes: moving what it misses
    // needs it again.
    let made: Result<Vec<(Id, Arc<Built>)>> = targets
        .par_iter()
        .map(|(t, b)| {
            let r = apply(part, id, ty, (kop, c.operation.as_str()), t, b, &tool)?;
            Ok(((*t).clone(), Arc::new(r)))
        })
        .collect();
    (made.map(Made::new), Some(own))
}

/// A body a feature applies to a solid: its key, the body, and what to
/// call it in errors (the feature whose own body it is, or the solid a
/// `boolean` uses as a tool).
struct ToolBody<'a> {
    key: ToolKey,
    built: &'a Built,
    name: &'a Id,
}

/// Unites solid `t` with `tools`, subtracts them from it or intersects
/// it with them, for feature `id` (`what` names the operation in
/// messages). Fails as `operation_no_effect` when a tool leaves nothing
/// of itself and nothing of the target changes, `body_emptied` when
/// nothing is left.
fn apply(
    part: &Part,
    id: &Id,
    ty: &str,
    (kop, what): (BooleanOp, &str),
    t: &Id,
    target: &Built,
    tools: &[ToolBody],
) -> Result<Built> {
    let placed: Vec<Tool> = tools
        .iter()
        .map(|x| Tool {
            key: x.key,
            body: &x.built.body,
            place: Rigid::IDENTITY,
        })
        .collect();
    let (body, journal) =
        ryukei::boolean::boolean(op_key(&part.id, id), kop, &target.body, &placed)
            .map_err(|e| boolean_error(part, id, ty, t, target, tools, e))?;
    let fail = |code, msg: String| {
        Error::new(code, format!("{ty} {id}: {msg}"))
            .with("feature", id)
            .with("target", t)
            .with("operation", what)
    };
    if body.faces().next().is_none() {
        return Err(fail(
            Code::BodyEmptied,
            format!("the {what} leaves nothing of {t}"),
        ));
    }
    let kept = names(&body);
    // A tool counts if any of it is left, or any of the target went: a
    // join inside the target, a cut or an intersect that misses nothing,
    // changes nothing.
    let target_faces: BTreeSet<Name> = target
        .body
        .faces()
        .map(|f| target.body.face(f).expect("live").name)
        .collect();
    let changed = journal.events().iter().any(|e| match e {
        Event::Deleted(n) | Event::Split { parent: n, .. } => target_faces.contains(n),
        _ => false,
    });
    for x in tools {
        let left = journal.events().iter().any(
            |e| matches!(e, Event::Placed { tool, to, .. } if *tool == x.key && kept.contains(to)),
        );
        if !left && !changed {
            return Err(fail(
                Code::OperationNoEffect,
                format!("the {what} with {} changes nothing of {t}", x.name),
            )
            .with("tool", x.name));
        }
    }
    // Origins: the target's, each tool's under their placed names, and a
    // split entity's for each of its pieces.
    let mut from: BTreeMap<Name, Value> = BTreeMap::new();
    for (n, o) in target.origins.iter() {
        if let Some(n) = crate::doc::reference::parse_name(n) {
            from.insert(n, o.clone());
        }
    }
    for e in journal.events() {
        if let Event::Placed {
            tool,
            from: own,
            to,
        } = e
            && let Some(x) = tools.iter().find(|x| x.key == *tool)
            && let Some(o) = x.built.origins.get(&own.to_string())
        {
            from.insert(*to, o.clone());
        }
    }
    for e in journal.events() {
        if let Event::Split { parent, children } = e
            && let Some(o) = from.get(parent).cloned()
        {
            for c in children {
                from.entry(*c).or_insert_with(|| o.clone());
            }
        }
    }
    let origins = from
        .into_iter()
        .filter(|(n, _)| kept.contains(n))
        .map(|(n, o)| (n.to_string(), o))
        .collect();
    finish(id, ty, (body, journal), origins)
}

/// The kernel's refusal of a join, cut, intersect or boolean:
/// `boolean_failed`, with the faces it names as the target's references
/// or the tools' entities with their origins.
fn boolean_error(
    part: &Part,
    id: &Id,
    ty: &str,
    t: &Id,
    target: &Built,
    tools: &[ToolBody],
    e: ryukei::Error,
) -> Error {
    let face = |n: &Name| -> Value {
        if target.body.by_name(*n).is_some() {
            return json!({"reference": format!("{}/{t}/{n}", part.id)});
        }
        let Some(x) = tools.iter().find(|x| x.built.body.by_name(*n).is_some()) else {
            return json!({"name": n.to_string()});
        };
        let mut v = json!({"tool": x.name, "name": n.to_string()});
        if let Some(o) = x.built.origins.get(&n.to_string()) {
            v["origin"] = o.clone();
        }
        v
    };
    let (msg, faces, at) = match &e {
        ryukei::Error::InvalidBoolean(BooleanError::Unsupported { a, b }) => (
            "two faces meet that ryukei can't intersect yet".to_owned(),
            vec![face(a), face(b)],
            None,
        ),
        ryukei::Error::InvalidBoolean(BooleanError::Uncertain { a, b }) => (
            "where two faces meet stays uncertain".to_owned(),
            vec![face(a), face(b)],
            None,
        ),
        ryukei::Error::InvalidBoolean(BooleanError::NonManifold { at, faces }) => (
            "the result would touch itself along an edge or at a point".to_owned(),
            faces.iter().map(face).collect(),
            Some([at.x, at.y, at.z]),
        ),
        e => (e.to_string(), Vec::new(), None),
    };
    let mut err = Error::new(Code::BooleanFailed, format!("{ty} {id}: {msg}"))
        .with("feature", id)
        .with("target", t)
        .with("kernel", format!("{e:?}"));
    if !faces.is_empty() {
        err = err.with("faces", faces);
    }
    if let Some(at) = at {
        err = err.with("at", at);
    }
    err
}

/// A `boolean` feature: its target as it sees it, with its tools as it
/// sees them, each placed under a key from its founder.
fn build_boolean(
    part: &Part,
    solids: &Solids,
    id: &Id,
    b: &crate::doc::solid::Boolean,
    before: &dyn Results,
) -> Result<Made> {
    let target = self::target(part, solids, id, &b.target, before)?;
    let tools: Vec<(&Id, &Built)> = b
        .tools
        .iter()
        .map(|t| Ok((t, self::target(part, solids, id, t, before)?)))
        .collect::<Result<_>>()?;
    let keys: Vec<ToolKey> = b.tools.iter().map(|t| solid_key(&part.id, id, t)).collect();
    let tools: Vec<ToolBody> = tools
        .iter()
        .zip(keys)
        .map(|((name, built), key)| ToolBody { key, built, name })
        .collect();
    let kop = match b.op {
        crate::doc::solid::BooleanOp::Unite => BooleanOp::Unite,
        crate::doc::solid::BooleanOp::Subtract => BooleanOp::Subtract,
        crate::doc::solid::BooleanOp::Intersect => BooleanOp::Intersect,
    };
    let built = apply(
        part,
        id,
        "boolean",
        (kop, b.op.as_str()),
        &b.target,
        target,
        &tools,
    )?;
    Ok(Made::new(vec![(b.target.clone(), Arc::new(built))]))
}

/// Checks a body the kernel built and hashes it. A broken body is never
/// reported as a success, in release builds too.
fn finish(
    id: &Id,
    ty: &str,
    (body, journal): (Body, Journal),
    origins: BTreeMap<String, Value>,
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

/// Rebuilds one feature from `before` (the results of the features it
/// may read): solves a sketch; builds a body-making feature's own body,
/// sweeping its sketch, and founds a solid with it or applies it to the
/// solids it targets.
pub fn rebuild_feature(
    part: &Part,
    solids: &Solids,
    id: &Id,
    f: &Feature,
    before: &dyn Results,
) -> Result<Output> {
    rebuild_with(part, solids, id, f, before, None).0
}

/// [`rebuild_feature`], taking a join, cut or intersect's own body from
/// `tool` when its hash matches; also returns that body, to cache.
fn rebuild_with(
    part: &Part,
    solids: &Solids,
    id: &Id,
    f: &Feature,
    before: &dyn Results,
    tool: Option<ToolInput>,
) -> (Result<Output>, Option<Arc<Built>>) {
    let solid = |m: Made| Output::Solids(Arc::new(m));
    match f {
        Feature::Sketch(s) => {
            let r = crate::sketch::context(part, part.order(), id, s, before)
                .and_then(|c| crate::sketch::solve_in(id, s, &c))
                .map(|s| Output::Sketch(Arc::new(s)));
            (r, None)
        }
        Feature::Boolean(b) => (build_boolean(part, solids, id, b, before).map(solid), None),
        f => {
            let (r, own) = build_body(part, solids, id, f, before, tool);
            (r.map(solid), own)
        }
    }
}

/// The hash of everything a feature's result depends on: ryucad's and
/// ryukei's versions, the feature as written (canonical, every unit
/// explicit), the value of each parameter it reads, and the place in the
/// list and result of each feature it reads (a solid as its view). Equal
/// hashes, equal results.
fn input_hash(
    part: &Part,
    solids: &crate::solid::Solids,
    id: &Id,
    f: &Feature,
    before: &BTreeMap<Id, Result<Output>>,
) -> blake3::Hash {
    let reads = crate::graph::reads(solids, id, f);
    hash_of(part, id, &f.to_json(), &reads, before)
}

/// The hash of everything a join, cut or intersect's own body depends
/// on: [`input_hash`]'s, but for the feature as written less its
/// operation and targets, and for what it reads less its targets, unless
/// it goes through all of them.
fn tool_hash(
    part: &Part,
    solids: &crate::solid::Solids,
    id: &Id,
    f: &Feature,
    before: &BTreeMap<Id, Result<Output>>,
) -> blake3::Hash {
    let mut v = f.to_json();
    if let Some(o) = v.as_object_mut() {
        o.remove("operation");
        o.remove("targets");
    }
    let through = matches!(
        f,
        Feature::Extrude(e) if matches!(e.extent, Extent::ThroughAll { .. })
    );
    let named = f
        .swept()
        .into_iter()
        .chain(f.references().into_iter().map(|(_, r)| &r.feature))
        .chain(if through { f.targets() } else { Vec::new() });
    let mut reads: Vec<&Id> = Vec::new();
    for input in named {
        let read = solids.view(input, id).unwrap_or(input);
        if !reads.contains(&read) {
            reads.push(read);
        }
    }
    hash_of(part, id, &v, &reads, before)
}

/// The hash of a feature's versions, JSON, parameters and reads.
fn hash_of(
    part: &Part,
    id: &Id,
    json: &Value,
    reads: &[&Id],
    before: &BTreeMap<Id, Result<Output>>,
) -> blake3::Hash {
    let mut h = blake3::Hasher::new();
    // Length-prefixed, so no two sets of pieces hash alike.
    let mut piece = |b: &[u8]| {
        h.update(&(b.len() as u64).to_le_bytes());
        h.update(b);
    };
    let v = crate::proto::VersionInfo::current();
    piece(v.ryucad.as_bytes());
    piece(v.ryukei.as_bytes());
    piece(crate::doc::canonical::to_string(json).as_bytes());
    for name in part.uses.get(id).into_iter().flatten() {
        piece(name.as_bytes());
        match part.env.get(name) {
            Ok(v) => {
                piece(&v.x.to_bits().to_le_bytes());
                piece(format!("{:?}", v.dim).as_bytes());
            }
            Err(e) => piece(
                serde_json::to_string(&e)
                    .expect("errors serialize")
                    .as_bytes(),
            ),
        }
    }
    let order = part.order();
    let at = |x: &Id| order.iter().position(|o| o == x);
    for input in reads {
        piece(input.as_str().as_bytes());
        let before_me = matches!((at(input), at(id)), (Some(i), Some(me)) if i < me);
        piece(&[u8::from(before_me)]);
        match before.get(input) {
            Some(Ok(o)) => piece(o.hash().as_bytes()),
            Some(Err(e)) => piece(
                serde_json::to_string(&e.code)
                    .expect("codes serialize")
                    .as_bytes(),
            ),
            None => piece(b"unbuilt"),
        }
    }
    h.finalize()
}

/// One feature's turn in a rebuild: the hash of its inputs (none for a
/// file that doesn't load), its result, whether this rebuild built it,
/// and a join, cut or intersect's own body with its hash and whether this
/// rebuild built that.
struct Step {
    id: Id,
    hash: Option<blake3::Hash>,
    result: Result<Output>,
    new: bool,
    tool: Option<(blake3::Hash, Arc<Built>, bool)>,
}

impl Step {
    fn failed(id: Id, e: Error) -> Step {
        Step {
            id,
            hash: None,
            result: Err(e),
            new: false,
            tool: None,
        }
    }
}

/// Rebuilds every listed feature of `part`, from nothing.
pub fn rebuild_part(part: &Part) -> PartResult {
    rebuild_part_with(part, &Cache::new()).0
}

/// Rebuilds every listed feature of `part`, each depth of its graph in
/// turn and the features of a depth in parallel, reusing from `cache`
/// every result whose inputs haven't changed. Returns the rebuild and the
/// part's cache as it now stands. The result is the same, bit for bit,
/// whatever the cache holds.
pub fn rebuild_part_with(part: &Part, cache: &Cache) -> (PartResult, Cache) {
    let depths = crate::graph::depths(part);
    let solids = crate::solid::Solids::of(part);
    let mut built: BTreeMap<Id, Result<Output>> = BTreeMap::new();
    let mut fresh = Cache::new();
    let mut made = Vec::new();
    let mut tools_made = Vec::new();
    for depth in 0.. {
        let now: Vec<&Id> = part
            .order()
            .iter()
            .filter(|id| depths[*id] == depth)
            .collect();
        if now.is_empty() {
            break;
        }
        let results: Vec<Step> = now
            .par_iter()
            .map(|id| {
                let id = (*id).clone();
                let f = match &part.features[&id] {
                    Ok(f) => f,
                    Err(e) => return Step::failed(id, e.clone()),
                };
                let h = input_hash(part, &solids, &id, f, &built);
                let applies = f.combine().is_some_and(|c| !c.is_new());
                let kept = cache.tools.get(&id);
                let (result, new, tool) = match cache.features.get(&id) {
                    Some((c, r)) if *c == h => {
                        let tool = kept.map(|(th, b)| (*th, Arc::clone(b), false));
                        (r.clone(), false, tool)
                    }
                    _ if applies => {
                        let th = tool_hash(part, &solids, &id, f, &built);
                        let (r, own) =
                            rebuild_with(part, &solids, &id, f, &built, Some((th, kept)));
                        let fresh = !kept.is_some_and(|(c, _)| *c == th);
                        (r, true, own.map(|b| (th, b, fresh)))
                    }
                    _ => (rebuild_feature(part, &solids, &id, f, &built), true, None),
                };
                Step {
                    id,
                    hash: Some(h),
                    result,
                    new,
                    tool,
                }
            })
            .collect();
        for step in results {
            let id = step.id;
            if step.new {
                made.push(id.clone());
            }
            if let Some((th, b, built)) = step.tool {
                if built {
                    tools_made.push(id.clone());
                }
                fresh.tools.insert(id.clone(), (th, b));
            }
            if let Some(h) = step.hash {
                fresh.features.insert(id.clone(), (h, step.result.clone()));
            }
            built.insert(id, step.result);
        }
    }
    let features: Vec<(Id, Result<Output>)> = part
        .order()
        .iter()
        .map(|id| (id.clone(), built[id].clone()))
        .collect();
    let built: Vec<Id> = part
        .order()
        .iter()
        .filter(|id| made.contains(id))
        .cloned()
        .collect();
    let mut h = blake3::Hasher::new();
    for (id, r) in &features {
        let piece = match r {
            Ok(b) => format!("{id} ok {}\n", b.hash()),
            Err(e) => format!("{id} err {}\n", serde_json::json!(e.code)),
        };
        h.update(piece.as_bytes());
    }
    let tools = part
        .order()
        .iter()
        .filter(|id| tools_made.contains(id))
        .cloned()
        .collect();
    let result = PartResult {
        features,
        hash: h.finalize().to_hex()[..32].to_owned(),
        built,
        tools,
    };
    (result, fresh)
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
            combine: crate::doc::solid::Combine::NEW,
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

    /// The volume a planar solid encloses, from its mesh.
    fn volume(b: &Body) -> f64 {
        let m = ryukei::mesh::tessellate(b, &ryukei::mesh::MeshParams::new(1e-5, 0.5)).unwrap();
        ryukei_testkit::mesh::volume_and_area(&m).0
    }

    /// Solid `s` as feature `f` left it.
    fn state<'a>(r: &'a PartResult, f: &str, s: &str) -> &'a Built {
        r.output(&id(f)).unwrap().solid(&id(s)).unwrap()
    }

    fn close(a: f64, b: f64) {
        assert!((a - b).abs() < 1e-18, "{a} != {b}");
    }

    #[test]
    fn join_cut_and_intersect() {
        use crate::doc::solid::Operation::*;
        use crate::solid::tests::{boxed, part};
        // Unit (1 mm) boxes along x: a at 0, b at 0.5, overlapping it.
        let p = part(vec![
            ("a", boxed("0", New, &[])),
            ("b", boxed("0.5", New, &[])),
            ("j", boxed("0.5", Join, &["a"])),
            ("c", boxed("1", Cut, &["a"])),
            ("both", boxed("0.75", Cut, &["a", "b"])),
            ("i", boxed("0.25", Intersect, &["a"])),
        ]);
        let r = rebuild_part(&p);
        assert_eq!(r.failed(), 0, "{:?}", r.features);
        close(volume(&state(&r, "j", "a").body), 1.5e-9);
        close(volume(&state(&r, "c", "a").body), 1e-9);
        close(volume(&state(&r, "both", "a").body), 0.75e-9);
        close(volume(&state(&r, "both", "b").body), 0.25e-9);
        close(volume(&state(&r, "i", "a").body), 0.5e-9);
        // A feature acting on several solids leaves each; its hash is
        // theirs together.
        let both = r.output(&id("both")).unwrap();
        assert!(both.body().is_none());
        assert_eq!(both.solids().len(), 2);
        assert_eq!(both.hash().len(), 32);
        // One solid's hash is its body's, so files from before solids
        // hash as they did.
        assert_eq!(r.output(&id("a")).unwrap().hash(), state(&r, "a", "a").hash);
        // Every entity says which feature made it.
        let after = state(&r, "both", "b");
        let features: BTreeSet<&str> = after
            .origins
            .values()
            .map(|o| o["feature"].as_str().unwrap())
            .collect();
        assert_eq!(features, BTreeSet::from(["b", "both"]));
        // The same, bit for bit, from a cache or from nothing.
        let (again, cache) = rebuild_part_with(&p, &Cache::new());
        assert_eq!(again.hash, r.hash);
        let (cached, _) = rebuild_part_with(&p, &cache);
        assert!(cached.built.is_empty());
        assert_eq!(cached.hash, r.hash);
    }

    #[test]
    fn a_change_to_a_target_alone_keeps_the_tool() {
        use crate::doc::solid::Operation::*;
        use crate::solid::tests::{boxed, part};
        let at = |a: &str, c: &str| {
            part(vec![
                ("a", boxed(a, New, &[])),
                ("c", boxed(c, Cut, &["a"])),
            ])
        };
        let (first, cache) = rebuild_part_with(&at("0", "0.5"), &Cache::new());
        assert_eq!(first.tools, [id("c")]);
        // The target moves: the cut is built again, from the same body.
        let p = at("0.2", "0.5");
        let (moved, cache) = rebuild_part_with(&p, &cache);
        assert_eq!(moved.built, [id("a"), id("c")]);
        assert!(moved.tools.is_empty());
        assert_eq!(moved.hash, rebuild_part(&p).hash);
        // The cut moves: its body is built again.
        let (cut, cache) = rebuild_part_with(&at("0.2", "0.6"), &cache);
        assert_eq!(cut.tools, [id("c")]);
        // A cut that misses keeps its body, for when the target comes to
        // it.
        let (miss, cache) = rebuild_part_with(&at("0.2", "5"), &cache);
        assert_eq!(miss.failed(), 1);
        assert_eq!(miss.tools, [id("c")]);
        let p = at("4.8", "5");
        let (hit, _) = rebuild_part_with(&p, &cache);
        assert_eq!(hit.failed(), 0);
        assert!(hit.tools.is_empty());
        assert_eq!(hit.hash, rebuild_part(&p).hash);
    }

    #[test]
    fn operations_that_do_nothing_or_everything() {
        use crate::doc::solid::Operation::*;
        use crate::solid::tests::{boxed, part};
        // Each on a solid of its own: a failure fails the rest of its
        // solid's chain.
        let p = part(vec![
            ("a", boxed("0", New, &[])),
            ("b", boxed("0", New, &[])),
            ("c", boxed("0", New, &[])),
            ("d", boxed("0", New, &[])),
            ("miss", boxed("5", Cut, &["a"])),
            ("inside", boxed("0", Join, &["b"])),
            ("all", boxed("0", Cut, &["c"])),
            ("apart", boxed("5", Intersect, &["d"])),
            ("then", boxed("0.5", Cut, &["a"])),
        ]);
        let r = rebuild_part(&p);
        let err = |f: &str| {
            let (_, r) = r.features.iter().find(|(i, _)| *i == id(f)).unwrap();
            r.as_ref().unwrap_err().clone()
        };
        for (f, target, want) in [
            ("miss", "a", Code::OperationNoEffect),
            ("inside", "b", Code::OperationNoEffect),
            ("all", "c", Code::BodyEmptied),
            ("apart", "d", Code::BodyEmptied),
        ] {
            let e = err(f);
            assert_eq!(e.code, want, "{f}: {e}");
            assert_eq!(e.details["target"], target, "{f}");
        }
        let e = err("then");
        assert_eq!(
            (e.code, e.details["dependency"].as_str()),
            (Code::DependencyFailed, Some("miss"))
        );
    }

    #[test]
    fn booleans_combine_whole_solids() {
        use crate::doc::solid::BooleanOp::{Intersect, Subtract, Unite};
        use crate::doc::solid::Operation::*;
        use crate::solid::tests::{boolean, boxed, part};
        let p = part(vec![
            ("a", boxed("0", New, &[])),
            ("t", boxed("0.5", New, &[])),
            ("u", boxed("0.2", New, &[])),
            ("cut", boolean(Subtract, "a", &["t", "u"])),
            ("far", boxed("5", New, &[])),
            ("near", boxed("3", New, &[])),
            ("both", boolean(Unite, "far", &["near"])),
            ("x", boxed("5.5", New, &[])),
            ("y", boxed("9", New, &[])),
            ("common", boolean(Intersect, "x", &["far"])),
            ("miss", boolean(Subtract, "y", &["x"])),
            ("late", boxed("0.5", Join, &["t"])),
            ("more", boxed("0.1", Join, &["a"])),
        ]);
        let r = rebuild_part(&p);
        let err = |f: &str| {
            let (_, r) = r.features.iter().find(|(i, _)| *i == id(f)).unwrap();
            r.as_ref().unwrap_err().clone()
        };
        close(volume(&state(&r, "cut", "a").body), 0.2e-9);
        // Apart, they stay apart: two lumps of one solid.
        close(volume(&state(&r, "both", "far").body), 2e-9);
        close(volume(&state(&r, "common", "x").body), 0.5e-9);
        // The tools' entities say where they came from: what is left of
        // the cut is u's face at 0.2 mm; all of t's lay inside u.
        let o = &state(&r, "cut", "a").origins;
        assert!(o.values().any(|o| o["feature"] == "u"));
        assert!(!o.values().any(|o| o["feature"] == "t"));
        let e = err("miss");
        assert_eq!(e.code, Code::OperationNoEffect, "{e}");
        assert_eq!(e.details["tool"], "x");
        // Nothing acts on a consumed solid.
        let e = err("late");
        assert_eq!(e.code, Code::BodyConsumed, "{e}");
        assert_eq!(e.details["consumed_by"], "cut");
        // [0, 0.2] mm and [0.1, 1.1] mm.
        close(volume(&state(&r, "more", "a").body), 1.1e-9);
        // Nor does anything refer to it: it is the target's now.
        let own = &state(&r, "t", "t").body;
        let name = own.face(own.faces().next().unwrap()).unwrap().name;
        let face = crate::doc::reference::Reference {
            feature: id("t"),
            name,
        };
        let e =
            crate::reference::resolve(&p, p.order(), &id("more"), "plane", &face, &r).unwrap_err();
        assert_eq!(e.code, Code::BodyConsumed, "{e}");
        // What the model shows: every tool is consumed, x by the boolean
        // that failed with it too.
        let solids = crate::solid::Solids::of(&p);
        let tips: Vec<(&Id, &Id)> = solids.tips().collect();
        assert_eq!(tips, [(&id("a"), &id("more")), (&id("y"), &id("miss"))]);
    }

    #[test]
    fn kernel_refusals_name_their_faces() {
        use crate::doc::solid::Operation::*;
        use crate::solid::tests::{boxed, part};
        // A box touching a only along its vertical edge at x = y = 1 mm.
        let mut corner = boxed("1", Join, &["a"]);
        if let Feature::Primitive(p) = &mut corner {
            p.placement.origin[1] = Quantity::parse("1", Unit::Mm).unwrap();
        }
        let p = part(vec![("a", boxed("0", New, &[])), ("corner", corner)]);
        let r = rebuild_part(&p);
        let (_, e) = &r.features[1];
        let e = e.as_ref().unwrap_err();
        assert_eq!(e.code, Code::BooleanFailed, "{e}");
        assert!(
            e.details["kernel"]
                .as_str()
                .unwrap()
                .starts_with("InvalidBoolean(NonManifold")
        );
        let faces = e.details["faces"].as_array().unwrap();
        // The target's faces as references, the tool's by the feature
        // that made them.
        assert!(faces.iter().any(|f| {
            f["reference"]
                .as_str()
                .is_some_and(|r| r.starts_with("p/a/"))
        }));
        assert!(
            faces
                .iter()
                .any(|f| f["tool"] == "corner" && f["origin"]["feature"] == "corner")
        );
        assert_eq!(e.details["at"].as_array().unwrap().len(), 3);
    }

    #[test]
    fn targets_that_wont_do() {
        use crate::doc::solid::Operation::*;
        use crate::solid::tests::{boxed, part};
        let mut huge = boxed("0", New, &[]);
        if let Feature::Primitive(p) = &mut huge {
            p.params
                .insert("x".into(), Quantity::parse("2000 m", Unit::Mm).unwrap());
        }
        let p = part(vec![
            ("a", boxed("0", New, &[])),
            ("j", boxed("0.5", Join, &["a"])),
            ("of_j", boxed("0.5", Cut, &["j"])),
            ("early", boxed("0.5", Cut, &["late"])),
            ("late", boxed("0", New, &[])),
            ("huge", huge),
            ("of_huge", boxed("0", Cut, &["huge"])),
            ("after", boxed("0.2", Cut, &["a"])),
        ]);
        let r = rebuild_part(&p);
        let err = |f: &str| {
            let (_, r) = r.features.iter().find(|(i, _)| *i == id(f)).unwrap();
            r.as_ref().unwrap_err().clone()
        };
        let e = err("of_j");
        assert_eq!(e.code, Code::ReferenceWrongKind, "{e}");
        assert_eq!(e.details["solids"], json!(["a"]));
        assert_eq!(err("early").code, Code::ReferenceForward);
        let e = err("of_huge");
        assert_eq!(
            (e.code, e.details["dependency"].as_str()),
            (Code::DependencyFailed, Some("huge"))
        );
        // The rest of a solid's chain still builds past a feature that
        // fails on another solid.
        assert!(r.output(&id("after")).is_some());
    }

    #[test]
    fn references_read_the_solid_as_the_reader_sees_it() {
        use crate::doc::reference::Reference;
        use crate::doc::solid::Operation::*;
        use crate::solid::tests::{boxed, part};
        let p = part(vec![
            ("a", boxed("0", New, &[])),
            ("j", boxed("0.5", Join, &["a"])),
        ]);
        let r = rebuild_part(&p);
        // a's face at x = 1 mm, which the join swallows.
        let own = &state(&r, "a", "a").body;
        let right = own
            .faces()
            .find(|f| {
                let b = ryukei::intersect::face_box(own, *f);
                b.min().x > 0.0009 && b.max().x < 0.0011
            })
            .unwrap();
        let name = own.face(right).unwrap().name;
        let face = Reference {
            feature: id("a"),
            name,
        };
        // Read from between a and j it is there; from after j it is gone.
        let between = [id("a"), id("o"), id("j")];
        let after = [id("a"), id("j"), id("o")];
        let found = crate::reference::resolve(&p, &between, &id("o"), "plane", &face, &r).unwrap();
        assert_eq!(found.1, EntityId::Face(right));
        let e = crate::reference::resolve(&p, &after, &id("o"), "plane", &face, &r).unwrap_err();
        assert_eq!(e.code, Code::ReferenceUnresolved, "{e}");
        assert_eq!(e.details["dependency"], "a");
        // A feature that acts on a solid isn't one.
        let at_j = Reference {
            feature: id("j"),
            name,
        };
        let e = crate::reference::resolve(&p, &after, &id("o"), "plane", &at_j, &r).unwrap_err();
        assert_eq!(e.code, Code::ReferenceWrongKind, "{e}");
        assert_eq!(e.details["solids"], json!(["a"]));
    }

    /// Part `p`: box `b` (20 × 10 × 5 mm), a sketch `s` at height `z`
    /// holding a circle of radius 2 mm about (10, 5) mm, and an extrude
    /// `e` of it cutting `b` through all, `both` ways or not, reversed or
    /// not.
    fn hole(z: &str, both: bool, reverse: bool) -> (tempfile::TempDir, crate::session::Session) {
        use crate::server::tests::call;
        let (dir, mut s, _) = crate::sketch::tests::with_box();
        let placement = json!({"origin": [0, 0, z]});
        call(
            &mut s,
            "feature.add",
            json!({"part": "p", "type": "sketch", "id": "s", "placement": placement}),
        )
        .unwrap();
        let ops = json!([
            {"op": "add", "type": "point", "at": [10, 5]},
            {"op": "add", "type": "fix", "a": "p1"},
            {"op": "add", "type": "circle", "center": "p1", "r": 2},
            {"op": "add", "type": "radius", "a": "ci1", "value": 2},
        ]);
        call(
            &mut s,
            "sketch.edit",
            json!({"part": "p", "sketch": "s", "ops": ops}),
        )
        .unwrap();
        call(
            &mut s,
            "feature.add",
            json!({"part": "p", "type": "extrude", "id": "e", "sketch": "s",
                   "operation": "cut", "reverse": reverse,
                   "extent": {"type": "through_all", "both": both}}),
        )
        .unwrap();
        (dir, s)
    }

    #[test]
    fn through_all_reaches_past_its_targets() {
        use core::f64::consts::PI;
        let full = 1e-6 - PI * 4e-6 * 0.005;
        let half = 1e-6 - PI * 4e-6 * 0.0025;
        for (z, both, reverse, want) in [
            // From the bottom up, all the way through.
            ("0", false, false, Ok(full)),
            // From half way up, up only, or both ways.
            ("2.5 mm", false, false, Ok(half)),
            ("2.5 mm", true, false, Ok(full)),
            ("2.5 mm", false, true, Ok(half)),
            // From below the box, both ways reaching it.
            ("-3 mm", true, false, Ok(full)),
            // From the bottom down: nothing to cut.
            ("0", false, true, Err(Code::InvalidExtent)),
        ] {
            let (_dir, mut s) = hole(z, both, reverse);
            let r = s.rebuilt(&id("p")).unwrap();
            let (_, out) = r.features.iter().find(|(f, _)| *f == id("e")).unwrap();
            match (out, want) {
                (Ok(o), Ok(v)) => {
                    // The mesh's polygons fall short of the hole's circle
                    // by a few parts in ten thousand.
                    let got = volume(&o.solid(&id("b")).unwrap().body);
                    assert!(
                        (got - v).abs() < 1e-3 * v,
                        "{z} {both} {reverse}: {got} != {v}"
                    );
                }
                (Err(e), Err(code)) => assert_eq!(e.code, code, "{z} {both} {reverse}: {e}"),
                (o, w) => panic!("{z} {both} {reverse}: {o:?} != {w:?}"),
            }
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
