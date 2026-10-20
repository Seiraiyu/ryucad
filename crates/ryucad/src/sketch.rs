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
use crate::doc::sketch::{
    ConstraintType, Entity, Geometry, Index, Plane, Sketch, clean, constraint_from_json,
};
use crate::error::{Code, Error, Result};
use crate::expr::Env;
use crate::rebuild::placement;
use crate::server;
use crate::session::Session;
use crate::units::{Field, Quantity, Unit};
use ryukei::geom::Rigid;
use ryusketch::{Coord, State};
use serde_json::{Map, Value, json};

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
pub fn solve_any(id: &Id, s: &Sketch) -> Result<(Sketch, Report)> {
    let (solver, index) = s.solver()?;
    let solved = ryusketch::solve(&solver)
        .map_err(|e| Error::new(Code::Internal, format!("sketch {id}: {e}")).with("feature", id))?;
    let mut sketch = s.clone();
    sketch.take_positions(&solved.sketch, &index);
    Ok((sketch, Report::new(&solved.report, &index)))
}

/// Solves an edited sketch. What was moved is first held where it was put,
/// so it stays there if everything else can follow; the solve from there
/// gives the sketch's own report.
pub fn solve_edited(id: &Id, s: &Sketch, moved: &[Id]) -> Result<(Sketch, Report)> {
    let mut start = s.clone();
    if !moved.is_empty() {
        let (mut solver, index) = s.solver()?;
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
    solve_any(id, &start)
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

/// Solves a sketch from its stored positions. A sketch whose constraints
/// can't all be made to hold is `sketch_unsolved`, with the report.
pub fn solve(id: &Id, s: &Sketch) -> Result<Solved> {
    let (sketch, report) = solve_any(id, s)?;
    if !report.converged {
        return Err(unsolved(id, &report));
    }
    let frame = frame(&sketch.plane)?;
    let hash = hash(&sketch, &frame, &report);
    Ok(Solved {
        stale: sketch != *s,
        sketch,
        report,
        frame,
        hash,
    })
}

/// The rigid frame of a sketch's plane.
pub fn frame(plane: &Plane) -> Result<Rigid> {
    // The unit only writes a zero origin; it doesn't change the frame.
    placement(&plane.placement(Unit::M))
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

/// The smallest `<prefix><n>` the sketch doesn't use.
fn fresh(s: &Sketch, prefix: &str) -> Id {
    (1u64..)
        .map(|n| Id::parse(&format!("{prefix}{n}"), "entity").expect("a valid ID"))
        .find(|id| !s.entities.contains_key(id) && !s.constraints.contains_key(id))
        .expect("some n is free")
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

/// Applies a batch of edits to a sketch, in order.
pub fn apply(s: &mut Sketch, ops: &[Value], unit: Unit, env: &Env) -> Result<Applied> {
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
            Some("add") => added.push(add(s, o, i, unit, env)?),
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

fn add(s: &mut Sketch, o: &Map<String, Value>, i: usize, unit: Unit, env: &Env) -> Result<Id> {
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
    let point = |f: &str| -> Result<Id> {
        let v = o
            .get(f)
            .and_then(Value::as_str)
            .ok_or_else(|| op_error(i, f, format!("a {ty} needs {f}: a point's ID")))?;
        Id::parse(v, "entity").map_err(|e| e.with("pointer", format!("/ops/{i}/{f}")))
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
    let geometry = match ty {
        "point" => {
            only(o, i, &["op", "id", "type", "at"])?;
            let at = o
                .get("at")
                .ok_or_else(|| op_error(i, "at", "a point needs at: [x, y]".into()))?;
            Some(Geometry::Point {
                m: rough(at, i, unit, env)?,
            })
        }
        "line" => {
            only(o, i, &["op", "id", "type", "start", "end", "construction"])?;
            Some(Geometry::Line {
                start: point("start")?,
                end: point("end")?,
            })
        }
        "arc" => {
            only(
                o,
                i,
                &["op", "id", "type", "center", "start", "end", "construction"],
            )?;
            Some(Geometry::Arc {
                center: point("center")?,
                start: point("start")?,
                end: point("end")?,
            })
        }
        "circle" => {
            only(o, i, &["op", "id", "type", "center", "r", "construction"])?;
            let r = o
                .get("r")
                .ok_or_else(|| op_error(i, "r", "a circle needs r".into()))?;
            let r = Quantity::from_json_in(r, unit, &|n| env.get(n))
                .map_err(|e| e.with("pointer", format!("/ops/{i}/r")))?;
            Some(Geometry::Circle {
                center: point("center")?,
                r_m: clean(r.meters()),
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
                },
            );
        }
        None => {
            let mut c = o.clone();
            c.remove("op");
            c.remove("id");
            let c = constraint_from_json(&Value::Object(c), &format!("/ops/{i}"), unit, env)?;
            s.constraints.insert(id.clone(), c);
        }
    }
    Ok(id)
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
    let applied = apply(&mut s, ops, unit, &env)?;
    let (solved, report) = solve_edited(&fid, &s, &applied.moved)?;
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
    let revision = server::save(session, &pid, &[(&fid, &f)], list)?;
    Ok(
        json!({"feature": fid, "added": applied.added, "changed": changed,
              "report": report.to_json(), "revision": revision}),
    )
}

/// `sketch.solve`: the report for a sketch as it is, or with a batch of
/// edits applied, and where its entities would be. Writes nothing.
pub fn solve_request(session: &Session, part: &str, sketch: &str, ops: &[Value]) -> Result<Value> {
    let (pid, fid) = (server::part_id(part)?, server::feature_id(sketch)?);
    let unit = session.project().manifest.default_unit;
    let env = session.project().part(&pid)?.env.clone();
    let mut s = sketch_of(session, &pid, &fid)?;
    let applied = apply(&mut s, ops, unit, &env)?;
    let (solved, report) = solve_edited(&fid, &s, &applied.moved)?;
    Ok(
        json!({"feature": fid, "added": applied.added, "changed": solved != s,
              "report": report.to_json(), "entities": solved.fields()["entities"]}),
    )
}

/// `sketch.get`: the solved geometry in the sketch's frame and in model
/// space, each entity's and constraint's state, and the report. A sketch
/// that can't be solved gives its best try, with the report saying so.
pub fn get(session: &Session, part: &str, sketch: &str) -> Result<Value> {
    let (pid, fid) = (server::part_id(part)?, server::feature_id(sketch)?);
    let stored = sketch_of(session, &pid, &fid)?;
    let (solved, report) = solve_any(&fid, &stored)?;
    let frame = frame(&solved.plane)?;
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
    match &solved.plane {
        Plane::Origin(o) => v["plane"] = json!(o.as_str()),
        Plane::Placement(_) => v["placement"] = solved.fields()["placement"].clone(),
    }
    Ok(v)
}

#[cfg(test)]
mod tests {
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
    fn with_sketch() -> (tempfile::TempDir, Session) {
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
    fn rectangle_ops() -> Value {
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

    fn edit(s: &mut Session, ops: Value) -> Result<Value> {
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
