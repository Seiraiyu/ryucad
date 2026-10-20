//! Solving sketch features with `ryusketch`, and reporting what the solve
//! found in the sketch's own IDs.
//!
//! A sketch's stored positions are *current* when every constraint holds
//! at them, its dimensions evaluated as they are now: the solver then
//! returns them with the very same bits, and rebuild uses them as stored.
//! Otherwise the sketch is *stale*: rebuild uses positions solved in
//! memory from the stored ones and warns `sketch_stale`.

use crate::doc::files::Feature;
use crate::doc::id::Id;
use crate::doc::project::Part;
use crate::doc::reference::Reference;
use crate::doc::sketch::{
    ConstraintType, Entity, Geometry, Index, OnFace, Plane, Sketch, clean, constraint_from_json,
};
use crate::doc::sweep::RevolveAxis;
use crate::error::{Code, Error, Result};
use crate::expr::Env;
use crate::rebuild::{Results, placement};
use crate::server;
use crate::session::Session;
use crate::units::{Field, Quantity, Unit};
use ryukei::geom::{Point3, Rigid};
use ryusketch::{Coord, State};
use serde_json::{Map, Value, json};
use std::collections::{BTreeMap, BTreeSet};

/// What a solve found, in the sketch's IDs.
#[derive(Clone, Debug, PartialEq)]
pub struct Report {
    pub converged: bool,
    pub state: State,
    pub dof: usize,
    /// Entities that can still move, and the coordinates that can.
    pub free: Vec<(Id, Vec<Coord>)>,
    pub redundant: Vec<Vec<Id>>,
    pub conflicting: Vec<Vec<Id>>,
    /// Closed loops of profile curves: each curve, and whether it is
    /// walked from its end to its start.
    pub loops: Vec<Vec<(Id, bool)>>,
    /// Curves that join but don't close, and the points where they stop.
    pub open: Vec<(Vec<Id>, Vec<Id>)>,
}

impl Report {
    fn new(r: &ryusketch::Report, index: &Index) -> Report {
        let e = |i: &u32| index.entities[*i as usize].clone();
        let c = |set: &Vec<u32>| -> Vec<Id> {
            let mut ids: Vec<Id> = set
                .iter()
                .map(|i| index.constraints[*i as usize].clone())
                .collect();
            ids.sort();
            ids
        };
        Report {
            converged: r.converged,
            state: r.state,
            dof: r.dof,
            free: r
                .free
                .iter()
                .map(|f| (e(&f.entity), f.moves.clone()))
                .collect(),
            redundant: r.redundant.iter().map(c).collect(),
            conflicting: r.conflicting.iter().map(c).collect(),
            loops: r
                .loops
                .iter()
                .map(|l| {
                    l.segments
                        .iter()
                        .map(|s| (e(&s.entity), s.reversed))
                        .collect()
                })
                .collect(),
            open: r
                .open
                .iter()
                .map(|ch| {
                    (
                        ch.entities.iter().map(e).collect(),
                        ch.ends.iter().map(e).collect(),
                    )
                })
                .collect(),
        }
    }

    pub fn state_str(&self) -> &'static str {
        match self.state {
            State::Under => "under",
            State::Well => "well",
            State::Over => "over",
        }
    }

    pub fn to_json(&self) -> Value {
        let coord = |c: &Coord| match c {
            Coord::X => "x",
            Coord::Y => "y",
            Coord::R => "r",
        };
        json!({
            "converged": self.converged,
            "state": self.state_str(),
            "dof": self.dof,
            "free": self.free.iter().map(|(id, m)| {
                json!({"entity": id, "moves": m.iter().map(coord).collect::<Vec<_>>()})
            }).collect::<Vec<_>>(),
            "redundant": self.redundant,
            "conflicting": self.conflicting,
            "loops": self.loops.iter().map(|l| {
                json!({"entities": l.iter().map(|(id, _)| id).collect::<Vec<_>>(),
                       "reversed": l.iter().map(|(_, r)| r).collect::<Vec<_>>()})
            }).collect::<Vec<_>>(),
            "open": self.open.iter().map(|(entities, ends)| {
                json!({"entities": entities, "ends": ends})
            }).collect::<Vec<_>>(),
        })
    }
}

/// A sketch with its positions solved.
#[derive(Clone, Debug, PartialEq)]
pub struct Solved {
    /// The sketch at its solved positions.
    pub sketch: Sketch,
    pub report: Report,
    /// Whether the stored positions didn't hold, so these were solved in
    /// memory.
    pub stale: bool,
    /// The sketch's frame: its plane's placement.
    pub frame: Rigid,
    /// BLAKE3 of the solved positions, frame and report.
    pub hash: String,
}

/// Solves a sketch from its stored positions, whether or not every
/// constraint can be made to hold: the sketch at the solution, and the
/// report.
pub fn solve_any(id: &Id, s: &Sketch, ctx: &Context) -> Result<(Sketch, Report)> {
    let (sketch, solver, index) = projected(s, ctx)?;
    let solved = ryusketch::solve(&solver)
        .map_err(|e| Error::new(Code::Internal, format!("sketch {id}: {e}")).with("feature", id))?;
    let mut sketch = sketch;
    sketch.take_positions(&solved.sketch, &index);
    Ok((sketch, Report::new(&solved.report, &index)))
}

/// The sketch with what `ctx` projects put in place, and the solver's
/// sketch with those entities fixed.
fn projected(s: &Sketch, ctx: &Context) -> Result<(Sketch, ryusketch::Sketch, Index)> {
    let mut sketch = s.clone();
    for (id, p) in &ctx.projected {
        match (sketch.entities.get_mut(id).map(|e| &mut e.geometry), p) {
            (Some(Geometry::Point { m }), Projected::At(at)) => *m = *at,
            (Some(Geometry::Circle { r_m, .. }), Projected::Radius(r)) => *r_m = clean(*r),
            _ => unreachable!("projections fix points and circles of the sketch"),
        }
    }
    let fixed = ctx.projected.keys().collect();
    let (solver, index) = sketch.solver_fixing(&fixed)?;
    Ok((sketch, solver, index))
}

/// Solves an edited sketch. What was moved is first held where it was put,
/// so it stays there if everything else can follow; the solve from there
/// gives the sketch's own report.
pub fn solve_edited(id: &Id, s: &Sketch, moved: &[Id], ctx: &Context) -> Result<(Sketch, Report)> {
    let mut start = s.clone();
    if !moved.is_empty() {
        let (_, mut solver, index) = projected(s, ctx)?;
        let first = solver.constraints.len() as u32;
        for (c, m) in (first..).zip(moved) {
            let e = index.entity(m).expect("moved entities exist");
            solver.constraints.insert(c, ryusketch::Constraint::Fix(e));
        }
        if let Ok(held) = ryusketch::solve(&solver)
            && held.report.converged
        {
            start.take_positions(&held.sketch, &index);
        }
    }
    solve_any(id, &start, ctx)
}

/// `sketch_unsolved` for a sketch whose constraints can't all hold.
fn unsolved(id: &Id, report: &Report) -> Error {
    Error::new(
        Code::SketchUnsolved,
        format!("sketch {id}: its constraints can't all be made to hold"),
    )
    .with("feature", id)
    .with("report", report.to_json())
}

/// What a sketch reads from outside itself: the frame of its plane, and
/// where its projected entities go.
#[derive(Clone, Debug, PartialEq)]
pub struct Context {
    pub frame: Rigid,
    /// The points and circles projections fix, by ID.
    pub projected: BTreeMap<Id, Projected>,
}

/// What a projection fixes: a point's position, in the sketch's frame, or
/// a circle's radius.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Projected {
    At([f64; 2]),
    Radius(f64),
}

impl Context {
    /// The context of a sketch that reads no other feature: on an origin
    /// plane or a placement. `None` for one that does, whose context comes
    /// from the features before it ([`context`]).
    pub fn standalone(s: &Sketch) -> Option<Result<Context>> {
        if !s.references().is_empty() {
            return None;
        }
        // The unit only writes a zero origin; it doesn't change the frame.
        let p = s.plane.placement(Unit::M)?;
        Some(placement(&p).map(|frame| Context {
            frame,
            projected: BTreeMap::new(),
        }))
    }
}

/// The context of sketch `id` from the results of the features before it;
/// `order` is the part's list as it is, or as a write would leave it.
pub fn context(
    part: &Part,
    order: &[Id],
    id: &Id,
    s: &Sketch,
    results: &dyn Results,
) -> Result<Context> {
    if let Some(c) = Context::standalone(s) {
        return c;
    }
    let frame = match &s.plane {
        Plane::Face(on) => face_frame(part, order, id, on, results)?,
        p => placement(&p.placement(Unit::M).expect("not on a face"))?,
    };
    let mut projected = BTreeMap::new();
    for (eid, e) in &s.entities {
        if let Some(r) = &e.projected {
            let at = Projection {
                part,
                order,
                sketch: id,
                entity: eid,
                r,
                frame: &frame,
            };
            at.project(e, results, &mut projected)?;
        }
    }
    Ok(Context { frame, projected })
}

/// One projected entity of a sketch, and where it sits.
struct Projection<'a> {
    part: &'a Part,
    order: &'a [Id],
    sketch: &'a Id,
    entity: &'a Id,
    r: &'a Reference,
    frame: &'a Rigid,
}

impl Projection<'_> {
    fn role(&self) -> String {
        format!("entities.{}", self.entity)
    }

    fn fail(&self, code: Code, msg: &str) -> Error {
        let text = self.r.text(&self.part.id);
        Error::new(
            code,
            format!(
                "sketch {}: {} projects {text}, which {msg}",
                self.sketch, self.entity
            ),
        )
        .with("feature", self.sketch)
        .with("role", self.role())
        .with("reference", text)
    }

    /// A point of the model in the sketch's plane.
    fn flat(&self, p: Point3) -> [f64; 2] {
        let q = self.frame.inverse().apply_point(p);
        [clean(q.x), clean(q.y)]
    }

    /// Fixes `id` at `p`. A point fixed twice must be fixed in one place.
    fn put(&self, out: &mut BTreeMap<Id, Projected>, id: &Id, p: Projected) -> Result<()> {
        match (out.get(id), p) {
            (Some(Projected::At(a)), Projected::At(b))
                if ryukei::math::hypot(a[0] - b[0], a[1] - b[1])
                    > ryukei::tol::LINEAR_RESOLUTION =>
            {
                Err(self.fail(
                    Code::ProjectionUnsupported,
                    &format!("puts {id} somewhere another projection doesn't"),
                ))
            }
            (Some(_), _) => Ok(()),
            (None, p) => {
                out.insert(id.clone(), p);
                Ok(())
            }
        }
    }

    /// Where the entity's projection fixes its points and radius.
    fn project(
        &self,
        e: &Entity,
        results: &dyn Results,
        out: &mut BTreeMap<Id, Projected>,
    ) -> Result<()> {
        use crate::reference::{kind, resolve, wrong_kind};
        use ryukei::geom::{Curve, Vector3};
        use ryukei::topo::EntityId;
        let role = self.role();
        let (b, found) = resolve(self.part, self.order, self.sketch, &role, self.r, results)?;
        let body = &b.body;
        let wrong =
            |what: &str, want: &str| wrong_kind(self.part, self.sketch, &role, self.r, what, want);
        let want = match e.geometry {
            Geometry::Point { .. } => "a vertex",
            Geometry::Line { .. } => "a straight edge",
            Geometry::Arc { .. } => "an arc of a circle",
            Geometry::Circle { .. } => "a whole circle",
        };
        let vertex = |v| body.vertex(v).expect("a live vertex").point;
        match (&e.geometry, found) {
            (Geometry::Point { .. }, EntityId::Vertex(v)) => {
                self.put(out, self.entity, Projected::At(self.flat(vertex(v))))
            }
            (Geometry::Line { start, end }, EntityId::Edge(x)) => {
                let edge = body.edge(x).expect("a live edge");
                match body.curve(edge.curve) {
                    Some(Curve::Line(_)) => {}
                    Some(Curve::Circle(_)) => return Err(wrong("circular edge", want)),
                    _ => {
                        return Err(self.fail(
                            Code::ProjectionUnsupported,
                            "is neither straight nor a circle",
                        ));
                    }
                }
                let (a, z) = (self.flat(vertex(edge.start)), self.flat(vertex(edge.end)));
                if ryukei::math::hypot(z[0] - a[0], z[1] - a[1]) <= ryukei::tol::LINEAR_RESOLUTION {
                    return Err(self.fail(
                        Code::ProjectionUnsupported,
                        "runs along the sketch's normal",
                    ));
                }
                self.put(out, start, Projected::At(a))?;
                self.put(out, end, Projected::At(z))
            }
            (Geometry::Arc { center, .. } | Geometry::Circle { center, .. }, EntityId::Edge(x)) => {
                let edge = body.edge(x).expect("a live edge");
                let c = match body.curve(edge.curve) {
                    Some(Curve::Circle(c)) => c,
                    Some(Curve::Line(_)) => return Err(wrong("straight edge", want)),
                    _ => {
                        return Err(self.fail(
                            Code::ProjectionUnsupported,
                            "is neither straight nor a circle",
                        ));
                    }
                };
                let n = self.frame.apply_vector(Vector3::new(0.0, 0.0, 1.0));
                let axis = c.frame().z().get();
                if axis.cross(n).length() > ryukei::tol::ANGULAR_RESOLUTION {
                    return Err(self.fail(
                        Code::ProjectionUnsupported,
                        "is a circle not parallel to the sketch",
                    ));
                }
                let closed = edge.start == edge.end;
                self.put(out, center, Projected::At(self.flat(c.frame().origin())))?;
                match &e.geometry {
                    Geometry::Circle { .. } if closed => {
                        self.put(out, self.entity, Projected::Radius(c.radius()))
                    }
                    Geometry::Arc { start, end, .. } if !closed => {
                        // Counterclockwise in the sketch: along the edge
                        // when the circle turns the sketch's way.
                        let (a, z) = (vertex(edge.start), vertex(edge.end));
                        let (a, z) = if axis.dot(n) > 0.0 { (a, z) } else { (z, a) };
                        self.put(out, start, Projected::At(self.flat(a)))?;
                        self.put(out, end, Projected::At(self.flat(z)))
                    }
                    _ if closed => Err(wrong("whole circle", want)),
                    _ => Err(wrong("arc of a circle", want)),
                }
            }
            (_, found) => Err(wrong(kind(found), want)),
        }
    }
}

/// The context of sketch `id` of `part` from the part's current rebuild.
pub fn context_now(session: &mut Session, part: &Id, id: &Id, s: &Sketch) -> Result<Context> {
    if let Some(c) = Context::standalone(s) {
        return c;
    }
    let p = session.project().part(part)?.clone();
    let results = session.rebuilt(part)?;
    let c = context(&p, p.order(), id, s, results);
    c.map_err(|e| session.explain(part, e))
}

/// The frame of a sketch on a face: the face's outward normal, the part
/// origin (or a vertex) projected onto its plane, and `x` along the
/// part's `x` (or `y`, if `x` is along the normal) or a straight edge,
/// projected onto the plane.
fn face_frame(
    part: &Part,
    order: &[Id],
    id: &Id,
    on: &OnFace,
    results: &dyn Results,
) -> Result<Rigid> {
    use crate::reference::{line, plane, point, resolve};
    use ryukei::geom::{Frame, Point3, Vector3};
    let found = resolve(part, order, id, "plane", &on.face, results)?;
    let (p0, n) = plane(part, id, "plane", &on.face, found)?;
    let onto = |q: Point3| q - n * (q - p0).dot(n);
    let origin = match &on.origin {
        Some(r) => {
            let found = resolve(part, order, id, "origin", r, results)?;
            onto(point(part, id, "origin", r, found)?)
        }
        None => onto(Point3::new(0.0, 0.0, 0.0)),
    };
    let hint = match &on.x {
        Some(r) => {
            let found = resolve(part, order, id, "x", r, results)?;
            let (a, b) = line(part, id, "x", r, found)?;
            let d = b - a;
            let d = d - n * d.dot(n);
            if Frame::new(origin, n, d).is_err() {
                return Err(Error::new(
                    Code::ReferenceWrongKind,
                    format!(
                        "{id}: its x {} runs along the face's normal",
                        r.text(&part.id)
                    ),
                )
                .with("feature", id)
                .with("role", "x")
                .with("reference", r.text(&part.id)));
            }
            d
        }
        None => {
            let x = Vector3::new(1.0, 0.0, 0.0);
            if Frame::new(origin, n, x).is_ok() {
                x
            } else {
                Vector3::new(0.0, 1.0, 0.0)
            }
        }
    };
    let frame = Frame::new(origin, n, hint).map_err(|e| {
        Error::new(
            Code::InvalidPlacement,
            format!("sketch {id}: its face gives no frame"),
        )
        .with("feature", id)
        .with("kernel", format!("{e:?}"))
    })?;
    Ok(Rigid::from_frame(&frame))
}

/// Solves a sketch that reads no other feature from its stored positions.
pub fn solve(id: &Id, s: &Sketch) -> Result<Solved> {
    match Context::standalone(s) {
        Some(c) => solve_in(id, s, &c?),
        None => Err(Error::new(
            Code::Internal,
            format!("sketch {id} reads other features: solve it in its part"),
        )
        .with("feature", id)),
    }
}

/// Solves a sketch from its stored positions, in `ctx`. A sketch whose
/// constraints can't all be made to hold is `sketch_unsolved`, with the
/// report.
pub fn solve_in(id: &Id, s: &Sketch, ctx: &Context) -> Result<Solved> {
    let (sketch, report) = solve_any(id, s, ctx)?;
    if !report.converged {
        return Err(unsolved(id, &report));
    }
    let frame = ctx.frame;
    let hash = hash(&sketch, &frame, &report);
    Ok(Solved {
        stale: sketch != *s,
        sketch,
        report,
        frame,
        hash,
    })
}

fn hash(s: &Sketch, frame: &Rigid, report: &Report) -> String {
    let mut h = blake3::Hasher::new();
    h.update(crate::doc::canonical::to_string(&s.fields()).as_bytes());
    for x in frame.rotation_matrix().iter().flatten() {
        h.update(&x.to_bits().to_le_bytes());
    }
    let o = frame.apply_point(ryukei::geom::Point3::new(0.0, 0.0, 0.0));
    for x in [o.x, o.y, o.z] {
        h.update(&x.to_bits().to_le_bytes());
    }
    h.update(report.to_json().to_string().as_bytes());
    h.finalize().to_hex()[..32].to_owned()
}

/// The warning for a sketch rebuilt from positions solved in memory.
pub fn stale_warning(id: &Id) -> Value {
    json!({
        "code": "sketch_stale",
        "feature": id,
        "message": format!(
            "sketch {id}: its stored positions don't satisfy its constraints; \
             rebuilt from positions solved in memory (run `ryucad doctor` to write them)"
        ),
    })
}

/// An entity type's prefix for generated IDs: `p3`, `l2`, `a1`, `ci4`.
fn prefix(ty: &str) -> &'static str {
    match ty {
        "point" => "p",
        "line" => "l",
        "arc" => "a",
        "circle" => "ci",
        t if ConstraintType::parse(t).is_ok_and(|t| t.value().is_some()) => "d",
        _ => "c",
    }
}

/// `<prefix><n>` for `n` one past the largest the sketch uses with that
/// prefix. Never a gap: a loop is anchored on its first point, so a new
/// point must not sort before the old ones.
fn fresh(s: &Sketch, prefix: &str) -> Id {
    let n = s
        .entities
        .keys()
        .chain(s.constraints.keys())
        .filter_map(|id| id.as_str().strip_prefix(prefix)?.parse::<u64>().ok())
        .max()
        .unwrap_or(0);
    Id::parse(&format!("{prefix}{}", n + 1), "entity").expect("a valid ID")
}

fn op_error(i: usize, field: &str, msg: String) -> Error {
    let at = if field.is_empty() {
        format!("/ops/{i}")
    } else {
        format!("/ops/{i}/{field}")
    };
    Error::new(Code::InvalidParam, msg).with("pointer", at)
}

/// A rough position, two quantities in the default unit.
fn rough(v: &Value, i: usize, unit: Unit, env: &Env) -> Result<[f64; 2]> {
    let a = v
        .as_array()
        .filter(|a| a.len() == 2)
        .ok_or_else(|| op_error(i, "at", "at is two quantities: [x, y]".into()))?;
    let q = |k: usize| {
        Quantity::from_json_in(&a[k], unit, &|n| env.get(n))
            .map(|q| clean(q.meters()))
            .map_err(|e| e.with("pointer", format!("/ops/{i}/at/{k}")))
    };
    Ok([q(0)?, q(1)?])
}

/// The ID an op names, which must exist.
fn named(s: &Sketch, o: &Map<String, Value>, i: usize) -> Result<Id> {
    let id = o
        .get("id")
        .and_then(Value::as_str)
        .ok_or_else(|| op_error(i, "id", "this op names an entity or constraint: id".into()))?;
    let id = Id::parse(id, "entity").map_err(|e| e.with("pointer", format!("/ops/{i}/id")))?;
    if !s.entities.contains_key(&id) && !s.constraints.contains_key(&id) {
        return Err(
            Error::new(Code::NotFound, format!("the sketch has no {id}"))
                .with("id", &id)
                .with("pointer", format!("/ops/{i}/id")),
        );
    }
    Ok(id)
}

/// Refuses fields an op doesn't take.
fn only(o: &Map<String, Value>, i: usize, fields: &[&str]) -> Result<()> {
    match o.keys().find(|k| !fields.contains(&k.as_str())) {
        None => Ok(()),
        Some(k) => Err(op_error(i, k, format!("this op takes no {k}"))),
    }
}

/// What a batch of edits did besides changing the sketch.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Applied {
    /// The IDs `add` ops gave or were given, in order.
    pub added: Vec<Id>,
    /// What `move` ops put somewhere.
    pub moved: Vec<Id>,
}

/// Applies a batch of edits to a sketch of `part`, in order.
pub fn apply(s: &mut Sketch, part: &Id, ops: &[Value], unit: Unit, env: &Env) -> Result<Applied> {
    let mut added = Vec::new();
    let mut moved = Vec::new();
    for (i, op) in ops.iter().enumerate() {
        let o = op.as_object().ok_or_else(|| {
            op_error(
                i,
                "",
                "an op is an object such as {\"op\": \"add\", ...}".into(),
            )
        })?;
        match o.get("op").and_then(Value::as_str) {
            Some("add") => add(s, o, i, part, unit, env, &mut added)?,
            Some("remove") => {
                only(o, i, &["op", "id"])?;
                let id = named(s, o, i)?;
                s.entities.remove(&id);
                s.constraints.remove(&id);
                s.constraints.retain(|_, c| !c.entities.contains(&id));
            }
            Some("set") => {
                only(o, i, &["op", "id", "value", "construction"])?;
                let id = named(s, o, i)?;
                set(s, &id, o, i, unit, env)?;
            }
            Some("move") => {
                only(o, i, &["op", "id", "at", "r"])?;
                let id = named(s, o, i)?;
                moved.push(id.clone());
                match (
                    s.entities.get_mut(&id).map(|e| &mut e.geometry),
                    o.get("at"),
                    o.get("r"),
                ) {
                    (Some(Geometry::Point { m }), Some(at), None) => *m = rough(at, i, unit, env)?,
                    (Some(Geometry::Circle { r_m, .. }), None, Some(r)) => {
                        *r_m = Quantity::from_json_in(r, unit, &|n| env.get(n))
                            .map_err(|e| e.with("pointer", format!("/ops/{i}/r")))?
                            .meters()
                    }
                    _ => {
                        return Err(op_error(
                            i,
                            "",
                            format!("move a point with at, or a circle with r; {id} is neither"),
                        ));
                    }
                }
            }
            _ => return Err(op_error(i, "op", "op is add, remove, set or move".into())),
        }
    }
    // A curve can't outlive the points it is built on.
    for (id, e) in &s.entities {
        if let Some((_, p)) = e
            .geometry
            .points()
            .into_iter()
            .find(|(_, p)| !s.entities.contains_key(*p))
        {
            return Err(Error::new(
                Code::HasDependents,
                format!("{p} is gone but {id} is built on it; remove {id} too"),
            )
            .with("entity", p)
            .with("dependents", [id]));
        }
    }
    s.validate()?;
    moved.retain(|id| s.entities.contains_key(id));
    Ok(Applied { added, moved })
}

/// The point an op names in field `f`. A projected curve that leaves it
/// out gets a new one, which the projection will place.
fn point_of(
    s: &mut Sketch,
    o: &Map<String, Value>,
    (i, ty, f): (usize, &str, &str),
    projected: bool,
    added: &mut Vec<Id>,
) -> Result<Id> {
    match o.get(f) {
        Some(v) => {
            let v = v
                .as_str()
                .ok_or_else(|| op_error(i, f, format!("a {ty} needs {f}: a point's ID")))?;
            Id::parse(v, "entity").map_err(|e| e.with("pointer", format!("/ops/{i}/{f}")))
        }
        None if projected => {
            let id = fresh(s, "p");
            let geometry = Geometry::Point { m: [0.0, 0.0] };
            s.entities.insert(
                id.clone(),
                Entity {
                    geometry,
                    construction: false,
                    projected: None,
                },
            );
            added.push(id.clone());
            Ok(id)
        }
        None => Err(op_error(i, f, format!("a {ty} needs {f}: a point's ID"))),
    }
}

/// Adds an entity or a constraint, and records what it added.
fn add(
    s: &mut Sketch,
    o: &Map<String, Value>,
    i: usize,
    part: &Id,
    unit: Unit,
    env: &Env,
    added: &mut Vec<Id>,
) -> Result<()> {
    let ty = o
        .get("type")
        .and_then(Value::as_str)
        .ok_or_else(|| op_error(i, "type", "add needs a type".into()))?;
    let id = match o.get("id") {
        Some(v) => {
            let id = v
                .as_str()
                .ok_or_else(|| op_error(i, "id", "id is a string".into()))?;
            let id =
                Id::parse(id, "entity").map_err(|e| e.with("pointer", format!("/ops/{i}/id")))?;
            if s.entities.contains_key(&id) || s.constraints.contains_key(&id) {
                return Err(
                    Error::new(Code::DuplicateId, format!("the sketch already has {id}"))
                        .with("id", &id)
                        .with("pointer", format!("/ops/{i}/id")),
                );
            }
            id
        }
        None => fresh(s, prefix(ty)),
    };
    let construction = match o.get("construction") {
        None => false,
        Some(Value::Bool(b)) => *b,
        Some(_) => {
            return Err(op_error(
                i,
                "construction",
                "construction is true or false".into(),
            ));
        }
    };
    let projected = match o.get("projected") {
        None => None,
        Some(v) => {
            let text = v.as_str().ok_or_else(|| {
                op_error(
                    i,
                    "projected",
                    "projected is a vertex or edge: <part>/<feature>/<name>".into(),
                )
            })?;
            Some(
                Reference::parse(text, part)
                    .map_err(|e| e.with("pointer", format!("/ops/{i}/projected")))?,
            )
        }
    };
    let auto = projected.is_some();
    let mut point = |s: &mut Sketch, f: &str| point_of(s, o, (i, ty, f), auto, added);
    let geometry = match ty {
        "point" => {
            only(o, i, &["op", "id", "type", "at", "projected"])?;
            let m = match o.get("at") {
                Some(at) => rough(at, i, unit, env)?,
                None if auto => [0.0, 0.0],
                None => return Err(op_error(i, "at", "a point needs at: [x, y]".into())),
            };
            Some(Geometry::Point { m })
        }
        "line" => {
            only(
                o,
                i,
                &[
                    "op",
                    "id",
                    "type",
                    "start",
                    "end",
                    "construction",
                    "projected",
                ],
            )?;
            Some(Geometry::Line {
                start: point(s, "start")?,
                end: point(s, "end")?,
            })
        }
        "arc" => {
            only(
                o,
                i,
                &[
                    "op",
                    "id",
                    "type",
                    "center",
                    "start",
                    "end",
                    "construction",
                    "projected",
                ],
            )?;
            Some(Geometry::Arc {
                center: point(s, "center")?,
                start: point(s, "start")?,
                end: point(s, "end")?,
            })
        }
        "circle" => {
            only(
                o,
                i,
                &[
                    "op",
                    "id",
                    "type",
                    "center",
                    "r",
                    "construction",
                    "projected",
                ],
            )?;
            let r_m = match o.get("r") {
                Some(r) => clean(
                    Quantity::from_json_in(r, unit, &|n| env.get(n))
                        .map_err(|e| e.with("pointer", format!("/ops/{i}/r")))?
                        .meters(),
                ),
                None if auto => 0.0,
                None => return Err(op_error(i, "r", "a circle needs r".into())),
            };
            Some(Geometry::Circle {
                center: point(s, "center")?,
                r_m,
            })
        }
        _ => None,
    };
    match geometry {
        Some(geometry) => {
            s.entities.insert(
                id.clone(),
                Entity {
                    geometry,
                    construction,
                    projected,
                },
            );
        }
        None => {
            if auto {
                return Err(op_error(
                    i,
                    "projected",
                    "only a point, line, arc or circle is projected".into(),
                ));
            }
            let mut c = o.clone();
            c.remove("op");
            c.remove("id");
            let c = constraint_from_json(&Value::Object(c), &format!("/ops/{i}"), unit, env)?;
            s.constraints.insert(id.clone(), c);
        }
    }
    added.push(id);
    Ok(())
}

fn set(
    s: &mut Sketch,
    id: &Id,
    o: &Map<String, Value>,
    i: usize,
    unit: Unit,
    env: &Env,
) -> Result<()> {
    if let Some(v) = o.get("value") {
        let c = s
            .constraints
            .get_mut(id)
            .filter(|c| c.ty.value().is_some())
            .ok_or_else(|| op_error(i, "value", format!("{id} is not a dimension")))?;
        let field = match c.ty.value().expect("a dimension") {
            Field::Length(_) => Field::Length(unit),
            f => f,
        };
        c.value = Some(
            Quantity::from_json_as(v, field, &|n| env.get(n))
                .map_err(|e| e.with("pointer", format!("/ops/{i}/value")))?,
        );
    }
    if let Some(v) = o.get("construction") {
        let e = s
            .entities
            .get_mut(id)
            .filter(|e| !matches!(e.geometry, Geometry::Point { .. }))
            .ok_or_else(|| {
                op_error(
                    i,
                    "construction",
                    format!("{id} is not a line, arc or circle"),
                )
            })?;
        e.construction = v
            .as_bool()
            .ok_or_else(|| op_error(i, "construction", "construction is true or false".into()))?;
    }
    if o.get("value").is_none() && o.get("construction").is_none() {
        return Err(op_error(
            i,
            "",
            "set a dimension's value or a curve's construction".into(),
        ));
    }
    Ok(())
}

/// The sketch feature `id` of a part.
fn sketch_of(session: &Session, part: &Id, id: &Id) -> Result<Sketch> {
    let p = session.project().part(part)?;
    match server::feature(p, id)? {
        Feature::Sketch(s) => Ok(s.clone()),
        f => Err(Error::new(
            Code::InvalidParam,
            format!("{id} is a {}, not a sketch", f.type_name()),
        )
        .with("feature", id)),
    }
}

/// `sketch.edit`: applies a batch of edits with one solve, and writes the
/// solved positions, or refuses the whole batch.
pub fn edit(
    session: &mut Session,
    part: &str,
    sketch: &str,
    ops: &[Value],
    expected: Option<&str>,
) -> Result<Value> {
    let (pid, fid) = (server::part_id(part)?, server::feature_id(sketch)?);
    let unit = session.project().manifest.default_unit;
    let p = session.project().part(&pid)?;
    server::check_revision(p, expected)?;
    let list = server::order(p)?;
    let env = p.env.clone();
    let mut s = sketch_of(session, &pid, &fid)?;
    let applied = apply(&mut s, &pid, ops, unit, &env)?;
    // What sweeps name must stay.
    for f in server::sweeps_of(p, &fid) {
        let names = match p.features[&f].as_ref().expect("a loaded sweep") {
            Feature::Extrude(e) => e.loops.clone(),
            Feature::Revolve(r) => match &r.axis {
                RevolveAxis::Line(l) => [r.loops.clone(), vec![l.clone()]].concat(),
                RevolveAxis::Edge(_) => r.loops.clone(),
            },
            _ => unreachable!("a sweep"),
        };
        if let Some(gone) = names.iter().find(|e| !s.entities.contains_key(*e)) {
            return Err(Error::new(
                Code::HasDependents,
                format!("{f} names {gone}; change {f} first"),
            )
            .with("entity", gone)
            .with("features", [&f]));
        }
    }
    let ctx = context_now(session, &pid, &fid, &s)?;
    let (solved, report) = solve_edited(&fid, &s, &applied.moved, &ctx)?;
    if !report.converged {
        return Err(unsolved(&fid, &report));
    }
    if report.state == State::Over {
        return Err(Error::new(
            Code::SketchOverconstrained,
            format!("sketch {fid}: some constraints are redundant or in conflict"),
        )
        .with("feature", &fid)
        .with("report", report.to_json()));
    }
    // Whether the solve moved anything.
    let changed = solved != s;
    let f = Feature::Sketch(solved);
    let before = current(session, &pid)?;
    server::save(session, &pid, &[(&fid, &f)], list)?;
    let sketches = keep_current(session, &pid, &before)?;
    let revision = &session.project().part(&pid)?.revision;
    let mut v = json!({"feature": fid, "added": applied.added, "changed": changed,
                       "report": report.to_json(), "revision": revision});
    if !sketches.is_empty() {
        v["sketches"] = json!(sketches);
    }
    Ok(v)
}

/// `sketch.solve`: the report for a sketch as it is, or with a batch of
/// edits applied, and where its entities would be. Writes nothing.
pub fn solve_request(
    session: &mut Session,
    part: &str,
    sketch: &str,
    ops: &[Value],
) -> Result<Value> {
    let (pid, fid) = (server::part_id(part)?, server::feature_id(sketch)?);
    let unit = session.project().manifest.default_unit;
    let env = session.project().part(&pid)?.env.clone();
    let mut s = sketch_of(session, &pid, &fid)?;
    let applied = apply(&mut s, &pid, ops, unit, &env)?;
    let ctx = context_now(session, &pid, &fid, &s)?;
    let (solved, report) = solve_edited(&fid, &s, &applied.moved, &ctx)?;
    Ok(
        json!({"feature": fid, "added": applied.added, "changed": solved != s,
              "report": report.to_json(), "entities": solved.fields()["entities"]}),
    )
}

/// `sketch.get`: the solved geometry in the sketch's frame and in model
/// space, each entity's and constraint's state, and the report. A sketch
/// that can't be solved gives its best try, with the report saying so.
pub fn get(session: &mut Session, part: &str, sketch: &str) -> Result<Value> {
    let (pid, fid) = (server::part_id(part)?, server::feature_id(sketch)?);
    let stored = sketch_of(session, &pid, &fid)?;
    let ctx = context_now(session, &pid, &fid, &stored)?;
    let (solved, report) = solve_any(&fid, &stored, &ctx)?;
    let frame = ctx.frame;
    let at = |m: [f64; 2]| {
        let p = frame.apply_point(ryukei::geom::Point3::new(m[0], m[1], 0.0));
        [p.x, p.y, p.z]
    };
    let axis = |x: f64, y: f64, z: f64| {
        let v = frame.apply_vector(ryukei::geom::Vector3::new(x, y, z));
        [v.x, v.y, v.z]
    };
    // What can move, and what takes part in a redundant or conflicting set.
    let free: Vec<&Id> = report.free.iter().map(|(id, _)| id).collect();
    let in_set = |sets: &[Vec<Id>], id: &Id| sets.iter().any(|s| s.contains(id));
    let mut over: Vec<&Id> = Vec::new();
    for (cid, c) in &solved.constraints {
        if in_set(&report.redundant, cid) || in_set(&report.conflicting, cid) {
            over.extend(&c.entities);
        }
    }
    let mut entities = solved.fields()["entities"].clone();
    for (id, e) in &solved.entities {
        let moves = free.contains(&id) || e.geometry.points().iter().any(|(_, p)| free.contains(p));
        let state = if over.contains(&id) {
            if report.converged { "over" } else { "failing" }
        } else if moves {
            "under"
        } else {
            "well"
        };
        let v = &mut entities[id.as_str()];
        v["state"] = json!(state);
        match &e.geometry {
            Geometry::Point { m } => v["at"] = json!(at(*m)),
            Geometry::Arc { center, start, .. } => {
                let m = |p: &Id| match solved.entities[p].geometry {
                    Geometry::Point { m } => m,
                    _ => unreachable!("validated"),
                };
                let (c, s) = (m(center), m(start));
                v["r_m"] = json!(ryukei::math::hypot(s[0] - c[0], s[1] - c[1]));
            }
            _ => {}
        }
    }
    let mut constraints = serde_json::Map::new();
    for (id, c) in &solved.constraints {
        let mut v = json!({"type": c.ty.as_str()});
        for (e, f) in c.entities.iter().zip(crate::doc::sketch::ENTITY_FIELDS) {
            v[f] = json!(e);
        }
        if let Some(q) = &c.value {
            v["value"] = q.report();
        }
        v["state"] = json!(if in_set(&report.conflicting, id) {
            "conflicting"
        } else if in_set(&report.redundant, id) {
            "redundant"
        } else {
            "ok"
        });
        constraints.insert(id.to_string(), v);
    }
    let o = frame.apply_point(ryukei::geom::Point3::new(0.0, 0.0, 0.0));
    let mut v = json!({
        "feature": fid,
        "frame": {"origin": [o.x, o.y, o.z], "x": axis(1.0, 0.0, 0.0),
                  "y": axis(0.0, 1.0, 0.0), "z": axis(0.0, 0.0, 1.0)},
        "stale": solved != stored,
        "report": report.to_json(),
        "entities": entities,
        "constraints": constraints,
        "revision": session.project().part(&pid)?.revision,
    });
    let mut fields = solved.fields();
    server::reference_texts(&pid, &solved, &mut fields);
    for k in ["plane", "placement", "face", "origin", "x"] {
        if let Some(x) = fields.get(k) {
            v[k] = x.clone();
        }
    }
    for (id, e) in &solved.entities {
        if e.projected.is_some() {
            v["entities"][id.as_str()]["projected"] =
                fields["entities"][id.as_str()]["projected"].clone();
        }
    }
    Ok(v)
}

/// Points in model space that show where a sketch entity is: a point, a
/// curve's points, and a circle's four quadrant points.
pub fn entity_points(solved: &Solved, id: &Id) -> Vec<[f64; 3]> {
    let s = &solved.sketch;
    let m = |p: &Id| match s.entities.get(p).map(|e| &e.geometry) {
        Some(Geometry::Point { m }) => *m,
        _ => unreachable!("validated"),
    };
    let flat: Vec<[f64; 2]> = match s.entities.get(id).map(|e| &e.geometry) {
        None => Vec::new(),
        Some(Geometry::Point { m }) => vec![*m],
        Some(Geometry::Circle { center, r_m }) => {
            let c = m(center);
            vec![
                [c[0] + r_m, c[1]],
                [c[0], c[1] + r_m],
                [c[0] - r_m, c[1]],
                [c[0], c[1] - r_m],
            ]
        }
        Some(g) => g.points().into_iter().map(|(_, p)| m(p)).collect(),
    };
    flat.into_iter()
        .map(|[x, y]| {
            let p = solved
                .frame
                .apply_point(ryukei::geom::Point3::new(x, y, 0.0));
            [p.x, p.y, p.z]
        })
        .collect()
}

/// The sketches of `part` whose stored positions are current: they
/// rebuild as stored. Of these, what a write leaves stale it writes back
/// solved ([`keep_current`]).
pub fn current(session: &mut Session, part: &Id) -> Result<BTreeSet<Id>> {
    let p = session.project().part(part)?;
    if !p
        .features
        .values()
        .any(|f| matches!(f, Ok(Feature::Sketch(_))))
    {
        return Ok(BTreeSet::new());
    }
    let r = session.rebuilt(part)?;
    Ok(r.features
        .iter()
        .filter_map(|(id, o)| {
            let s = o.as_ref().ok()?.sketch()?;
            (!s.stale).then(|| id.clone())
        })
        .collect())
}

/// After a write to `part`: writes the sketches of `before`, current until
/// then, that the write left stale, with their solved positions, in the
/// same change. One that now fails is left as it was. Each is reported:
/// `{"feature", "ok"}`, with its error when it failed.
pub fn keep_current(session: &mut Session, part: &Id, before: &BTreeSet<Id>) -> Result<Vec<Value>> {
    if before.is_empty() {
        return Ok(Vec::new());
    }
    let r = session.rebuilt(part)?;
    let mut out = Vec::new();
    let mut writes = Vec::new();
    for (id, o) in r.features.iter().filter(|(id, _)| before.contains(id)) {
        match o.as_ref().map(|o| o.sketch()) {
            Ok(Some(s)) if s.stale => {
                writes.push((id.clone(), Feature::Sketch(s.sketch.clone())));
                out.push(json!({"feature": id, "ok": true}));
            }
            Ok(_) => {}
            Err(e) => out.push(json!({"feature": id, "ok": false, "error": e})),
        }
    }
    if !writes.is_empty() {
        let list = server::order(session.project().part(part)?)?;
        let refs: Vec<(&Id, &Feature)> = writes.iter().map(|(id, f)| (id, f)).collect();
        server::save(session, part, &refs, list)?;
    }
    Ok(out)
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use crate::doc::files::Feature;
    use crate::expr::Env;

    fn id(s: &str) -> Id {
        Id::parse(s, "feature").unwrap()
    }

    pub(crate) fn rectangle() -> Sketch {
        let v = crate::doc::sketch::tests::rectangle();
        let p = [(
            "width".to_owned(),
            crate::expr::Expr::parse("20 mm").unwrap(),
        )]
        .into();
        match Feature::from_json(&v, Unit::Mm, &Env::new(&p)).unwrap() {
            Feature::Sketch(s) => s,
            _ => unreachable!(),
        }
    }

    #[test]
    fn current_positions_are_used_as_stored() {
        let s = rectangle();
        let solved = solve(&id("s"), &s).unwrap();
        assert!(!solved.stale);
        assert_eq!(solved.sketch, s);
        assert_eq!(solved.report.state, State::Under);
        assert_eq!(solved.report.loops.len(), 1);
        assert_eq!(solved.report.loops[0][0].0.as_str(), "l1");
        let v = solved.report.to_json();
        assert_eq!(v["state"], "under");
        assert_eq!(v["loops"][0]["entities"], json!(["l1", "l2", "l3", "l4"]));
        assert_eq!(solve(&id("s"), &s).unwrap().hash, solved.hash);
    }

    #[test]
    fn stale_positions_are_solved_in_memory() {
        let mut s = rectangle();
        let d1 = Id::parse("d1", "constraint").unwrap();
        s.constraints.get_mut(&d1).unwrap().value =
            Some(crate::units::Quantity::parse("15 mm", Unit::Mm).unwrap());
        let solved = solve(&id("s"), &s).unwrap();
        assert!(solved.stale);
        let at = |name: &str| {
            let e = &solved.sketch.entities[&Id::parse(name, "entity").unwrap()];
            let crate::doc::sketch::Geometry::Point { m } = e.geometry else {
                panic!()
            };
            m
        };
        let (p1, p2) = (at("p1"), at("p2"));
        let width = ryukei::math::hypot(p2[0] - p1[0], p2[1] - p1[1]);
        assert!((width - 0.015).abs() < 1e-10, "{width}");
        // Solved positions are current: solving again changes nothing.
        let again = solve(&id("s"), &solved.sketch).unwrap();
        assert!(!again.stale);
        assert_eq!(again.hash, solved.hash);
    }

    #[test]
    fn conflicts_are_unsolved() {
        let mut s = rectangle();
        let d2 = Id::parse("d2", "constraint").unwrap();
        // The diagonal can't be shorter than the width.
        s.constraints.get_mut(&d2).unwrap().value =
            Some(crate::units::Quantity::parse("5 mm", Unit::Mm).unwrap());
        let e = solve(&id("s"), &s).unwrap_err();
        assert_eq!(e.code, Code::SketchUnsolved);
        assert_eq!(e.details["report"]["converged"], false);
    }

    use crate::server::tests::{call, fresh};

    /// Part `p` with an empty sketch `s` on xy.
    pub(crate) fn with_sketch() -> (tempfile::TempDir, Session) {
        let (dir, mut s) = fresh();
        call(&mut s, "part.create", json!({"part": "p"})).unwrap();
        call(
            &mut s,
            "feature.add",
            json!({"part": "p", "type": "sketch", "id": "s"}),
        )
        .unwrap();
        (dir, s)
    }

    /// Ops drawing a rough 20 × 10 rectangle, held square and sized.
    pub(crate) fn rectangle_ops() -> Value {
        json!([
            {"op": "add", "type": "point", "at": [0, 0]},
            {"op": "add", "type": "point", "at": [19, 1]},
            {"op": "add", "type": "point", "at": [21, 9]},
            {"op": "add", "type": "point", "at": [1, 11]},
            {"op": "add", "type": "line", "start": "p1", "end": "p2"},
            {"op": "add", "type": "line", "start": "p2", "end": "p3"},
            {"op": "add", "type": "line", "start": "p3", "end": "p4"},
            {"op": "add", "type": "line", "start": "p4", "end": "p1"},
            {"op": "add", "type": "horizontal", "a": "l1"},
            {"op": "add", "type": "horizontal", "a": "l3"},
            {"op": "add", "type": "vertical", "a": "l2"},
            {"op": "add", "type": "vertical", "a": "l4"},
            {"op": "add", "type": "fix", "a": "p1"},
            {"op": "add", "id": "width", "type": "length", "a": "l1", "value": "20 mm"},
            {"op": "add", "type": "distance", "a": "p1", "b": "l3", "value": 10}
        ])
    }

    /// A point's position in a `feature.show`, which must be within the
    /// solver's tolerance of `want`.
    fn near(show: &Value, point: &str, want: [f64; 2]) {
        let m = &show["entities"][point]["m"];
        for k in 0..2 {
            let x = m[k].as_f64().unwrap();
            assert!(
                (x - want[k]).abs() < ryusketch::SOLVE_TOL,
                "{point}: {m} != {want:?}"
            );
        }
    }

    pub(crate) fn edit(s: &mut Session, ops: Value) -> Result<Value> {
        call(
            s,
            "sketch.edit",
            json!({"part": "p", "sketch": "s", "ops": ops}),
        )
    }

    #[test]
    fn a_batch_is_applied_and_solved_at_once() {
        let (_dir, mut s) = with_sketch();
        let r = edit(&mut s, rectangle_ops()).unwrap();
        assert_eq!(
            r["added"],
            json!([
                "p1", "p2", "p3", "p4", "l1", "l2", "l3", "l4", "c1", "c2", "c3", "c4", "c5",
                "width", "d1"
            ])
        );
        assert_eq!(r["changed"], true);
        assert_eq!(r["report"]["state"], "well");
        assert_eq!(
            r["report"]["loops"][0]["entities"],
            json!(["l1", "l2", "l3", "l4"])
        );
        let show = call(&mut s, "feature.show", json!({"part": "p", "id": "s"})).unwrap();
        near(&show, "p3", [0.02, 0.01]);
        assert_eq!(
            show["constraints"]["d1"]["value"],
            json!({"value": "10 mm", "m": 0.01})
        );
        // What was written is current: it rebuilds as stored.
        let rb = call(&mut s, "rebuild", json!({})).unwrap();
        assert_eq!(rb["parts"][0]["features"][0]["sketch"]["stale"], false);
        // A dimension's value, and a curve made construction geometry.
        let r = edit(
            &mut s,
            json!([{"op": "set", "id": "width", "value": "30 mm"},
                   {"op": "set", "id": "l4", "construction": true}]),
        )
        .unwrap();
        assert_eq!(r["report"]["loops"], json!([]));
        let show = call(&mut s, "feature.show", json!({"part": "p", "id": "s"})).unwrap();
        near(&show, "p2", [0.03, 0.0]);
        assert_eq!(show["entities"]["l4"]["construction"], true);
        // Removing a constraint frees what it held.
        let r = edit(&mut s, json!([{"op": "remove", "id": "d1"}])).unwrap();
        assert_eq!(
            (r["report"]["state"].as_str(), r["report"]["dof"].as_u64()),
            (Some("under"), Some(1))
        );
        assert_eq!(r["changed"], false);
        // A free point moves where it is put, as far as it can.
        let r = edit(&mut s, json!([{"op": "move", "id": "p3", "at": [30, 25]}])).unwrap();
        assert_eq!(r["changed"], true);
        let show = call(&mut s, "feature.show", json!({"part": "p", "id": "s"})).unwrap();
        near(&show, "p4", [0.0, 0.025]);
    }

    #[test]
    fn refused_batches_write_nothing() {
        let (dir, mut s) = with_sketch();
        edit(&mut s, rectangle_ops()).unwrap();
        let path = dir.path().join("parts/p/features/s.json");
        let before = std::fs::read_to_string(&path).unwrap();
        for (ops, code, pointer) in [
            // Already held: redundant.
            (
                json!([{"op": "add", "type": "parallel", "a": "l1", "b": "l3"}]),
                Code::SketchOverconstrained,
                None,
            ),
            // Can't hold with the rest.
            (
                json!([{"op": "add", "type": "length", "a": "l2", "value": "11 mm"}]),
                Code::SketchUnsolved,
                None,
            ),
            (
                json!([{"op": "remove", "id": "p1"}]),
                Code::HasDependents,
                None,
            ),
            (
                json!([{"op": "remove", "id": "ghost"}]),
                Code::NotFound,
                Some("/ops/0/id"),
            ),
            (
                json!([{"op": "add", "id": "l1", "type": "point", "at": [0, 0]}]),
                Code::DuplicateId,
                Some("/ops/0/id"),
            ),
            (
                json!([{"op": "add", "type": "point", "at": [0]}]),
                Code::InvalidParam,
                Some("/ops/0/at"),
            ),
            (
                json!([{"op": "add", "type": "point", "at": [0, 0], "r": 1}]),
                Code::InvalidParam,
                Some("/ops/0/r"),
            ),
            (
                json!([{"op": "add", "type": "glue", "a": "l1"}]),
                Code::InvalidParam,
                Some("/ops/0/type"),
            ),
            (
                json!([{"op": "add", "type": "horizontal", "a": "p1"}]),
                Code::InvalidDocument,
                None,
            ),
            (
                json!([{"op": "set", "id": "l1", "value": "1 mm"}]),
                Code::InvalidParam,
                Some("/ops/0/value"),
            ),
            (
                json!([{"op": "set", "id": "width", "value": "1 deg"}]),
                Code::UnitMismatch,
                Some("/ops/0/value"),
            ),
            (
                json!([{"op": "move", "id": "l1", "at": [0, 0]}]),
                Code::InvalidParam,
                Some("/ops/0"),
            ),
            (
                json!([{"op": "explode"}]),
                Code::InvalidParam,
                Some("/ops/0/op"),
            ),
        ] {
            let e = edit(&mut s, ops.clone()).unwrap_err();
            assert_eq!(e.code, code, "{ops}: {e}");
            if let Some(p) = pointer {
                assert_eq!(e.details["pointer"], p, "{ops}");
            }
            assert_eq!(std::fs::read_to_string(&path).unwrap(), before, "{ops}");
        }
        // The report says what is wrong.
        let e = edit(
            &mut s,
            json!([{"op": "add", "type": "parallel", "a": "l1", "b": "l3"}]),
        )
        .unwrap_err();
        assert_eq!(
            e.details["report"]["redundant"][0],
            json!(["c1", "c2", "c6"])
        );
        let e = call(
            &mut s,
            "sketch.edit",
            json!({"part": "p", "sketch": "s", "ops": [], "expected_revision": "old"}),
        )
        .unwrap_err();
        assert_eq!(e.code, Code::Conflict);
        call(
            &mut s,
            "feature.add",
            json!({"part": "p", "type": "sphere", "id": "b", "params": {"radius": 1}}),
        )
        .unwrap();
        let e = call(
            &mut s,
            "sketch.edit",
            json!({"part": "p", "sketch": "b", "ops": []}),
        )
        .unwrap_err();
        assert_eq!(e.code, Code::InvalidParam);
    }

    #[test]
    fn solve_proposes_without_writing() {
        let (dir, mut s) = with_sketch();
        edit(&mut s, rectangle_ops()).unwrap();
        let path = dir.path().join("parts/p/features/s.json");
        let before = std::fs::read_to_string(&path).unwrap();
        let r = call(&mut s, "sketch.solve", json!({"part": "p", "sketch": "s"})).unwrap();
        assert_eq!(
            (r["report"]["state"].as_str(), r["changed"].as_bool()),
            (Some("well"), Some(false))
        );
        let r = call(
            &mut s,
            "sketch.solve",
            json!({"part": "p", "sketch": "s",
                   "ops": [{"op": "set", "id": "width", "value": "40 mm"}]}),
        )
        .unwrap();
        near(&r, "p2", [0.04, 0.0]);
        // Even an edit that would be refused gets its report.
        let r = call(
            &mut s,
            "sketch.solve",
            json!({"part": "p", "sketch": "s",
                   "ops": [{"op": "add", "type": "length", "a": "l2", "value": "11 mm"}]}),
        )
        .unwrap();
        assert_eq!(r["report"]["converged"], false);
        assert_eq!(std::fs::read_to_string(&path).unwrap(), before);
    }

    #[test]
    fn an_edit_is_one_change_and_undoes_as_one() {
        use crate::live::Live;
        use crate::server::tests::call_live;
        let (_dir, mut s) = with_sketch();
        let mut live = Live::new();
        call_live(
            &mut s,
            &mut live,
            None,
            "sketch.edit",
            json!({"part": "p", "sketch": "s", "ops": rectangle_ops()}),
        )
        .unwrap();
        let log = call_live(&mut s, &mut live, None, "activity.log", json!({})).unwrap();
        let last = log["entries"].as_array().unwrap().last().unwrap().clone();
        assert_eq!(last["method"], "sketch.edit");
        call_live(&mut s, &mut live, None, "undo", json!({})).unwrap();
        let show = call(&mut s, "feature.show", json!({"part": "p", "id": "s"})).unwrap();
        assert_eq!(show["entities"], json!({}));
    }

    /// The name of the face of `body` (an `inspect` body) whose outward
    /// normal is `n`.
    pub(crate) fn face_along(body: &Value, n: [f64; 3]) -> String {
        body["faces"]
            .as_array()
            .unwrap()
            .iter()
            .find(|f| {
                let z = &f["surface"]["frame"]["z"];
                let sign = if f["reversed"] == true { -1.0 } else { 1.0 };
                f["surface"]["type"] == "plane"
                    && (0..3).all(|k| (z[k].as_f64().unwrap() * sign - n[k]).abs() < 1e-12)
            })
            .map(|f| f["name"].as_str().unwrap().to_owned())
            .expect("a face along n")
    }

    /// The name of the vertex of `body` at `p`, and of the edge from `a`
    /// to `b`.
    pub(crate) fn vertex_at(body: &Value, p: [f64; 3]) -> String {
        body["vertices"]
            .as_array()
            .unwrap()
            .iter()
            .find(|v| (0..3).all(|k| (v["point"][k].as_f64().unwrap() - p[k]).abs() < 1e-9))
            .map(|v| v["name"].as_str().unwrap().to_owned())
            .expect("a vertex at p")
    }

    pub(crate) fn edge_between(body: &Value, a: &str, b: &str) -> String {
        body["edges"]
            .as_array()
            .unwrap()
            .iter()
            .find(|e| (e["start"] == a && e["end"] == b) || (e["start"] == b && e["end"] == a))
            .map(|e| e["name"].as_str().unwrap().to_owned())
            .expect("an edge from a to b")
    }

    /// Part `p` with a 20 × 10 × 5 mm box `b`, and its body in `inspect`.
    pub(crate) fn with_box() -> (tempfile::TempDir, Session, Value) {
        let (dir, mut s) = fresh();
        call(&mut s, "part.create", json!({"part": "p"})).unwrap();
        call(
            &mut s,
            "feature.add",
            json!({"part": "p", "type": "box", "id": "b", "params": {"x": 20, "y": 10, "z": 5}}),
        )
        .unwrap();
        let body = call(&mut s, "inspect", json!({"part": "p", "feature": "b"})).unwrap()["bodies"]
            [0]
        .clone();
        (dir, s, body)
    }

    fn close(v: &Value, want: [f64; 3]) {
        for k in 0..3 {
            assert!(
                (v[k].as_f64().unwrap() - want[k]).abs() < 1e-15,
                "{v} != {want:?}"
            );
        }
    }

    #[test]
    fn a_sketch_sits_on_a_face() {
        let (_dir, mut s, body) = with_box();
        let top = format!("p/b/{}", face_along(&body, [0.0, 0.0, 1.0]));
        call(
            &mut s,
            "feature.add",
            json!({"part": "p", "type": "sketch", "id": "s", "face": top}),
        )
        .unwrap();
        let g = call(&mut s, "sketch.get", json!({"part": "p", "sketch": "s"})).unwrap();
        close(&g["frame"]["origin"], [0.0, 0.0, 0.005]);
        close(&g["frame"]["z"], [0.0, 0.0, 1.0]);
        close(&g["frame"]["x"], [1.0, 0.0, 0.0]);
        assert_eq!(g["face"], top);
        let show = call(&mut s, "feature.show", json!({"part": "p", "id": "s"})).unwrap();
        assert_eq!(show["face"], top);
        // A corner for its origin, and the edge along y from it for x.
        let corner = vertex_at(&body, [0.02, 0.0, 0.005]);
        let far = vertex_at(&body, [0.02, 0.01, 0.005]);
        let edge = edge_between(&body, &corner, &far);
        call(
            &mut s,
            "feature.edit",
            json!({"part": "p", "id": "s", "face_origin": format!("p/b/{corner}"),
                   "face_x": format!("p/b/{edge}")}),
        )
        .unwrap();
        let g = call(&mut s, "sketch.get", json!({"part": "p", "sketch": "s"})).unwrap();
        close(&g["frame"]["origin"], [0.02, 0.0, 0.005]);
        let x = g["frame"]["x"][1].as_f64().unwrap();
        assert!(x.abs() == 1.0, "{}", g["frame"]);
        // Back to the defaults.
        call(
            &mut s,
            "feature.edit",
            json!({"part": "p", "id": "s", "face_origin": "", "face_x": ""}),
        )
        .unwrap();
        let show = call(&mut s, "feature.show", json!({"part": "p", "id": "s"})).unwrap();
        assert!(show.get("origin").is_none() && show.get("x").is_none());
    }

    #[test]
    fn a_body_on_a_face_follows_it() {
        let (_dir, mut s, body) = with_box();
        let top = format!("p/b/{}", face_along(&body, [0.0, 0.0, 1.0]));
        call(
            &mut s,
            "feature.add",
            json!({"part": "p", "type": "sketch", "id": "s", "face": top}),
        )
        .unwrap();
        edit(&mut s, rectangle_ops()).unwrap();
        call(
            &mut s,
            "feature.add",
            json!({"part": "p", "type": "extrude", "id": "e", "sketch": "s",
                   "extent": {"length": 3}}),
        )
        .unwrap();
        let bounds = |s: &mut Session| {
            call(s, "inspect", json!({"part": "p", "feature": "e"})).unwrap()["bodies"][0]["bounds"]
                .clone()
        };
        let z = |b: &Value, k: &str| b[k][2].as_f64().unwrap();
        let b = bounds(&mut s);
        assert!(
            (z(&b, "min") - 0.005).abs() < 1e-12 && (z(&b, "max") - 0.008).abs() < 1e-12,
            "{b}"
        );
        // The box grows; the sketch, still current, rides on its top.
        call(
            &mut s,
            "feature.edit",
            json!({"part": "p", "id": "b", "params": {"z": 7}}),
        )
        .unwrap();
        let rb = call(&mut s, "rebuild", json!({})).unwrap();
        assert_eq!(rb["parts"][0]["features"][1]["sketch"]["stale"], false);
        let b = bounds(&mut s);
        assert!((z(&b, "min") - 0.007).abs() < 1e-12, "{b}");
        // What a sketch sits on can't be deleted, or moved after it.
        let e = call(&mut s, "feature.delete", json!({"part": "p", "id": "b"})).unwrap_err();
        assert_eq!(e.code, Code::HasDependents);
        let e = call(
            &mut s,
            "feature.move",
            json!({"part": "p", "id": "b", "after": "s"}),
        )
        .unwrap_err();
        assert_eq!(e.code, Code::ReferenceForward);
    }

    #[test]
    fn faces_that_wont_do() {
        let (dir, mut s, body) = with_box();
        call(
            &mut s,
            "feature.add",
            json!({"part": "p", "type": "cylinder", "id": "c",
                   "params": {"radius": 2, "height": 4}}),
        )
        .unwrap();
        let cyl = call(&mut s, "inspect", json!({"part": "p", "feature": "c"})).unwrap()["bodies"]
            [0]
        .clone();
        let side = cyl["faces"]
            .as_array()
            .unwrap()
            .iter()
            .find(|f| f["surface"]["type"] == "cylinder")
            .unwrap()["name"]
            .as_str()
            .unwrap()
            .to_owned();
        let top = face_along(&body, [0.0, 0.0, 1.0]);
        let corner = vertex_at(&body, [0.0, 0.0, 0.005]);
        let ghost = "0123456789abcdef0123456789abcdef";
        for (fields, code) in [
            (
                json!({"face": format!("p/c/{side}")}),
                Code::ReferenceWrongKind,
            ),
            (
                json!({"face": format!("p/b/{corner}")}),
                Code::ReferenceWrongKind,
            ),
            (
                json!({"face": format!("p/b/{top}"), "face_origin": format!("p/b/{top}")}),
                Code::ReferenceWrongKind,
            ),
            (
                json!({"face": format!("p/b/{ghost}")}),
                Code::ReferenceUnresolved,
            ),
            (json!({"face": format!("p/z/{top}")}), Code::NotFound),
            (json!({"face": format!("q/b/{top}")}), Code::InvalidParam),
            (
                json!({"face": format!("p/b/{top}"), "plane": "xy"}),
                Code::Usage,
            ),
            (json!({"face_x": format!("p/b/{corner}")}), Code::Usage),
        ] {
            let mut f = fields.clone();
            f["part"] = json!("p");
            f["type"] = json!("sketch");
            let e = call(&mut s, "feature.add", f).unwrap_err();
            assert_eq!(e.code, code, "{fields}: {e}");
        }
        // Before what it sits on.
        let e = call(
            &mut s,
            "feature.add",
            json!({"part": "p", "type": "sketch", "face": format!("p/b/{top}"), "first": true}),
        )
        .unwrap_err();
        assert_eq!(e.code, Code::ReferenceForward);
        // A name that goes away behind ryucad's back fails at rebuild.
        call(
            &mut s,
            "feature.add",
            json!({"part": "p", "type": "sketch", "id": "s", "face": format!("p/b/{top}")}),
        )
        .unwrap();
        let path = dir.path().join("parts/p/features/s.json");
        let text = std::fs::read_to_string(&path).unwrap().replace(&top, ghost);
        std::fs::write(&path, text).unwrap();
        let e = call(&mut s, "rebuild", json!({})).unwrap_err();
        let f = &e.details["parts"][0]["features"][2];
        assert_eq!(f["error"]["code"], "reference_unresolved", "{f}");
        assert_eq!(f["error"]["details"]["role"], "plane");
        assert_eq!(f["error"]["details"]["reference"], format!("p/b/{ghost}"));
    }

    /// Where a point of a `sketch.get` sits in the sketch's frame.
    fn m(g: &Value, p: &str) -> [f64; 2] {
        let m = &g["entities"][p]["m"];
        [m[0].as_f64().unwrap(), m[1].as_f64().unwrap()]
    }

    fn near2(a: [f64; 2], b: [f64; 2]) -> bool {
        (a[0] - b[0]).abs() < 1e-15 && (a[1] - b[1]).abs() < 1e-15
    }

    #[test]
    fn projections_put_entities_where_edges_are() {
        let (_dir, mut s, body) = with_box();
        call(
            &mut s,
            "feature.add",
            json!({"part": "p", "type": "cylinder", "id": "c",
                   "placement": {"origin": [40, 0, 0]}, "params": {"radius": 2, "height": 4}}),
        )
        .unwrap();
        let cyl = call(&mut s, "inspect", json!({"part": "p", "feature": "c"})).unwrap()["bodies"]
            [0]
        .clone();
        let rim = cyl["edges"]
            .as_array()
            .unwrap()
            .iter()
            .find(|e| e["curve"]["type"] == "circle" && e["curve"]["frame"]["origin"][2] == 0.004)
            .unwrap()["name"]
            .as_str()
            .unwrap()
            .to_owned();
        call(
            &mut s,
            "feature.add",
            json!({"part": "p", "type": "sketch", "id": "s"}),
        )
        .unwrap();
        // The top's front edge and far corner, dropped onto xy.
        let (a, b) = (
            vertex_at(&body, [0.0, 0.0, 0.005]),
            vertex_at(&body, [0.02, 0.0, 0.005]),
        );
        let front = edge_between(&body, &a, &b);
        let far = vertex_at(&body, [0.02, 0.01, 0.005]);
        let r = edit(
            &mut s,
            json!([
                {"op": "add", "id": "l1", "type": "line", "projected": format!("p/b/{front}")},
                {"op": "add", "id": "corner", "type": "point", "projected": format!("p/b/{far}")},
                {"op": "add", "id": "ci1", "type": "circle", "projected": format!("p/c/{rim}")}
            ]),
        )
        .unwrap();
        assert_eq!(r["added"], json!(["p1", "p2", "l1", "corner", "p3", "ci1"]));
        // Nothing projected can move.
        assert_eq!(
            (r["report"]["state"].as_str(), r["report"]["dof"].as_u64()),
            (Some("well"), Some(0))
        );
        let g = call(&mut s, "sketch.get", json!({"part": "p", "sketch": "s"})).unwrap();
        let ends = [m(&g, "p1"), m(&g, "p2")];
        assert!(ends.iter().any(|e| near2(*e, [0.0, 0.0])), "{ends:?}");
        assert!(ends.iter().any(|e| near2(*e, [0.02, 0.0])), "{ends:?}");
        assert!(near2(m(&g, "corner"), [0.02, 0.01]));
        assert!(near2(m(&g, "p3"), [0.04, 0.0]));
        assert_eq!(g["entities"]["ci1"]["r_m"], 0.002);
        assert_eq!(g["entities"]["l1"]["projected"], format!("p/b/{front}"));
        let show = call(&mut s, "feature.show", json!({"part": "p", "id": "s"})).unwrap();
        assert_eq!(
            show["entities"]["corner"]["projected"],
            format!("p/b/{far}")
        );
        // Fixed where projected: a dimension can't move them.
        let e = edit(
            &mut s,
            json!([{"op": "add", "type": "length", "a": "l1", "value": "30 mm"}]),
        )
        .unwrap_err();
        assert_eq!(e.code, Code::SketchUnsolved);
        // The box grows: the projection follows, and the sketch is written
        // solved in the same change.
        let r = call(
            &mut s,
            "feature.edit",
            json!({"part": "p", "id": "b", "params": {"x": 25}}),
        )
        .unwrap();
        assert_eq!(r["sketches"], json!([{"feature": "s", "ok": true}]));
        let g = call(&mut s, "sketch.get", json!({"part": "p", "sketch": "s"})).unwrap();
        assert!(
            near2(m(&g, "corner"), [0.025, 0.01]),
            "{}",
            g["entities"]["corner"]
        );
        assert_eq!(g["stale"], false);
        assert_eq!(r["revision"], g["revision"]);
    }

    #[test]
    fn writes_keep_projecting_sketches_current() {
        use crate::live::Live;
        use crate::server::tests::call_live;
        let (dir, mut s) = with_sketch();
        let mut live = Live::new();
        edit(&mut s, rectangle_ops()).unwrap();
        call(
            &mut s,
            "feature.add",
            json!({"part": "p", "type": "extrude", "id": "e", "sketch": "s",
                   "extent": {"length": 5}}),
        )
        .unwrap();
        let body = call(&mut s, "inspect", json!({"part": "p", "feature": "e"})).unwrap()["bodies"]
            [0]
        .clone();
        let far = vertex_at(&body, [0.02, 0.01, 0.005]);
        call(
            &mut s,
            "feature.add",
            json!({"part": "p", "type": "sketch", "id": "t", "plane": "zx"}),
        )
        .unwrap();
        call(
            &mut s,
            "sketch.edit",
            json!({"part": "p", "sketch": "t", "ops": [
                {"op": "add", "id": "k", "type": "point", "projected": format!("p/e/{far}")}
            ]}),
        )
        .unwrap();
        // Widening s moves e's corner, so t is written too, in one change.
        let r = call_live(
            &mut s,
            &mut live,
            None,
            "sketch.edit",
            json!({"part": "p", "sketch": "s",
                   "ops": [{"op": "set", "id": "width", "value": "30 mm"}]}),
        )
        .unwrap();
        assert_eq!(r["sketches"], json!([{"feature": "t", "ok": true}]));
        let rb = call(&mut s, "rebuild", json!({})).unwrap();
        assert!(rb.get("warnings").is_none(), "{rb}");
        let log = call_live(&mut s, &mut live, None, "activity.log", json!({})).unwrap();
        let last = log["entries"].as_array().unwrap().last().unwrap().clone();
        let files: Vec<&str> = last["files"]
            .as_array()
            .unwrap()
            .iter()
            .map(|f| f["path"].as_str().unwrap())
            .collect();
        assert_eq!(
            files,
            ["parts/p/features/s.json", "parts/p/features/t.json"]
        );
        // Undo restores both.
        let t = dir.path().join("parts/p/features/t.json");
        call_live(&mut s, &mut live, None, "undo", json!({})).unwrap();
        let g = call(&mut s, "sketch.get", json!({"part": "p", "sketch": "t"})).unwrap();
        assert_eq!(g["stale"], false);
        let k = &g["entities"]["k"]["m"];
        // On zx, the part's x is the sketch's y: the corner is back at 20 mm.
        assert!((k[1].as_f64().unwrap() - 0.02).abs() < 1e-9, "{k}");
        assert!(std::fs::read_to_string(&t).unwrap().contains("projected"));
        // Behind ryucad's back, nothing is written: rebuild warns instead.
        let sp = dir.path().join("parts/p/features/s.json");
        let text = std::fs::read_to_string(&sp)
            .unwrap()
            .replace("\"value\": \"20 mm\"", "\"value\": \"25 mm\"");
        std::fs::write(&sp, text).unwrap();
        let before = std::fs::read_to_string(&t).unwrap();
        let rb = call(&mut s, "rebuild", json!({})).unwrap();
        assert_eq!(rb["warnings"].as_array().unwrap().len(), 2, "{rb}");
        assert_eq!(std::fs::read_to_string(&t).unwrap(), before);
    }

    #[test]
    fn an_arc_projects_counterclockwise() {
        let (_dir, mut s) = with_sketch();
        // A half disc above the x axis, extruded.
        edit(
            &mut s,
            json!([
                {"op": "add", "type": "point", "at": [0, 0]},
                {"op": "add", "type": "point", "at": [20, 0]},
                {"op": "add", "type": "point", "at": [10, 0]},
                {"op": "add", "type": "line", "start": "p1", "end": "p2"},
                {"op": "add", "type": "arc", "center": "p3", "start": "p2", "end": "p1"}
            ]),
        )
        .unwrap();
        call(
            &mut s,
            "feature.add",
            json!({"part": "p", "type": "extrude", "id": "e", "sketch": "s",
                   "extent": {"length": 5}}),
        )
        .unwrap();
        let body = call(&mut s, "inspect", json!({"part": "p", "feature": "e"})).unwrap()["bodies"]
            [0]
        .clone();
        let top = body["edges"]
            .as_array()
            .unwrap()
            .iter()
            .find(|e| e["curve"]["type"] == "circle" && e["curve"]["frame"]["origin"][2] == 0.005)
            .unwrap()["name"]
            .as_str()
            .unwrap()
            .to_owned();
        call(
            &mut s,
            "feature.add",
            json!({"part": "p", "type": "sketch", "id": "t"}),
        )
        .unwrap();
        call(
            &mut s,
            "sketch.edit",
            json!({"part": "p", "sketch": "t", "ops": [
                {"op": "add", "type": "arc", "projected": format!("p/e/{top}")}
            ]}),
        )
        .unwrap();
        let g = call(&mut s, "sketch.get", json!({"part": "p", "sketch": "t"})).unwrap();
        let a = &g["entities"]["a1"];
        let at = |f: &str| m(&g, a[f].as_str().unwrap());
        assert!(near2(at("center"), [0.01, 0.0]), "{g}");
        assert!(near2(at("start"), [0.02, 0.0]), "{g}");
        assert!(near2(at("end"), [0.0, 0.0]), "{g}");
    }

    #[test]
    fn projections_that_wont_do() {
        let (_dir, mut s, body) = with_box();
        call(
            &mut s,
            "feature.add",
            json!({"part": "p", "type": "cylinder", "id": "c",
                   "params": {"radius": 2, "height": 4}}),
        )
        .unwrap();
        let cyl = call(&mut s, "inspect", json!({"part": "p", "feature": "c"})).unwrap()["bodies"]
            [0]
        .clone();
        let rim = cyl["edges"]
            .as_array()
            .unwrap()
            .iter()
            .find(|e| e["curve"]["type"] == "circle")
            .unwrap()["name"]
            .as_str()
            .unwrap()
            .to_owned();
        call(
            &mut s,
            "feature.add",
            json!({"part": "p", "type": "sketch", "id": "s", "plane": "zx"}),
        )
        .unwrap();
        let (a, b, up) = (
            vertex_at(&body, [0.0, 0.0, 0.0]),
            vertex_at(&body, [0.02, 0.0, 0.0]),
            vertex_at(&body, [0.0, 0.0, 0.005]),
        );
        let bottom = edge_between(&body, &a, &b);
        let rising = edge_between(&body, &a, &up);
        let top = face_along(&body, [0.0, 0.0, 1.0]);
        let rect = |ty: &str, r: String| json!([{"op": "add", "type": ty, "projected": r}]);
        for (ops, code) in [
            (
                rect("circle", format!("p/c/{rim}")),
                Code::ProjectionUnsupported,
            ),
            (rect("line", format!("p/c/{rim}")), Code::ReferenceWrongKind),
            (
                rect("circle", format!("p/b/{bottom}")),
                Code::ReferenceWrongKind,
            ),
            (
                rect("point", format!("p/b/{bottom}")),
                Code::ReferenceWrongKind,
            ),
            (rect("line", format!("p/b/{top}")), Code::ReferenceWrongKind),
            (
                rect("line", format!("p/s/{bottom}")),
                Code::ReferenceForward,
            ),
            (
                json!([{"op": "add", "type": "horizontal", "a": "l1", "projected": format!("p/b/{a}")}]),
                Code::InvalidParam,
            ),
        ] {
            let e = edit(&mut s, ops.clone()).unwrap_err();
            assert_eq!(e.code, code, "{ops}: {e}");
        }
        // On xy, the rising edge projects to a point.
        call(
            &mut s,
            "feature.edit",
            json!({"part": "p", "id": "s", "plane": "xy"}),
        )
        .unwrap();
        let e = edit(&mut s, rect("line", format!("p/b/{rising}"))).unwrap_err();
        assert_eq!(e.code, Code::ProjectionUnsupported, "{e}");
        assert_eq!(e.details["role"], "entities.l1");
    }

    #[test]
    fn get_gives_geometry_in_both_frames_and_states() {
        let (_dir, mut s) = with_sketch();
        call(
            &mut s,
            "feature.edit",
            json!({"part": "p", "id": "s", "plane": "zx"}),
        )
        .unwrap();
        let mut ops = rectangle_ops();
        ops.as_array_mut().unwrap().pop();
        edit(&mut s, ops).unwrap();
        let g = call(&mut s, "sketch.get", json!({"part": "p", "sketch": "s"})).unwrap();
        assert_eq!(g["plane"], "zx");
        assert_eq!(g["frame"]["z"], json!([0.0, 1.0, 0.0]));
        assert_eq!(g["stale"], false);
        // p2 is 20 mm along the sketch's x, which is the part's z.
        let at = &g["entities"]["p2"]["at"];
        for (k, want) in [0.0, 0.0, 0.02].into_iter().enumerate() {
            assert!(
                (at[k].as_f64().unwrap() - want).abs() < ryusketch::SOLVE_TOL,
                "{at}"
            );
        }
        assert_eq!(g["entities"]["p1"]["state"], "well");
        assert_eq!(g["entities"]["p3"]["state"], "under");
        assert_eq!(g["entities"]["l2"]["state"], "under");
        assert_eq!(g["entities"]["l1"]["state"], "well");
        assert_eq!(
            g["constraints"]["width"]["value"],
            json!({"value": "20 mm", "m": 0.02})
        );
        assert_eq!(g["constraints"]["width"]["state"], "ok");
        assert_eq!(g["report"]["dof"], 1);
    }
}
