//! Motions and sweeps (design 529, plan 26.B). A motion
//! (`parts/<asm>/motions/<id>.json`, [`crate::doc::motion`]) drives an
//! assembly from start to end. A sweep steps it: each step's drives are
//! placed at its fraction `t`, and the component they reach is solved
//! from the previous step's poses, so a linkage stays on its branch.
//! With `interference` each step's model is checked against rest (what
//! collides that doesn't at rest), with `clearance` the closest moving
//! pair is found, and the first collision and the first stop are refined
//! by bisection. Nothing is written: a sweep is a read.
//!
//! Interference runs only when asked (the owner's rule): `assembly.sweep`
//! with `interference` and `check --interference`, never `motion.list`,
//! `motion.save` or plain `check`.

use crate::assembly::Model;
use crate::doc::files::{Feature, PartKind};
use crate::doc::id::Id;
use crate::doc::motion::{Driven, MotionFile};
use crate::doc::project::Part;
use crate::error::{Code, Error, Result};
use crate::rebuild::{Output, PartResult};
use crate::session::Session;
use ryu_session::job::Phase;
use ryukei::geom::Rigid;
use serde_json::{Value, json};
use std::collections::{BTreeMap, BTreeSet};

pub use ryu_session::job::Job;

/// ryusketch's tolerances, as ryumate measures stops with them.
const ANGLE_TOL: f64 = 1e-12;
const LENGTH_TOL: f64 = 1e-10;

/// What a sweep starts from: the assembly's settled solve in ryumate's
/// terms, or, for an assembly with no mates and no patterns, its
/// top-level instances as bodies where they are placed.
#[derive(Clone, Debug)]
pub struct Rest {
    /// The instance each body is, by its index.
    pub instances: Vec<Id>,
    /// The mate each of ryumate's mates is, by its index.
    pub mates: Vec<Id>,
    /// The instance pattern each of ryumate's patterns is, by its index.
    pub patterns: Vec<Id>,
    pub asm: ryumate::Assembly,
    /// Whether its mates hold (true with none).
    pub holds: bool,
}

impl Rest {
    pub fn of(result: &PartResult) -> Rest {
        if let Some(s) = result.solve.as_deref() {
            return Rest {
                instances: s.instances.clone(),
                mates: s.mates.clone(),
                patterns: s.patterns.clone(),
                asm: s.asm.clone(),
                holds: s.report.converged,
            };
        }
        let mut instances = Vec::new();
        let mut asm = ryumate::Assembly::default();
        for (id, r) in &result.features {
            if let Ok(Output::Instance(p)) = r {
                asm.bodies.insert(
                    instances.len() as u32,
                    ryumate::Body {
                        pose: crate::connector::pose(&p.pose),
                        fixed: p.fixed,
                    },
                );
                instances.push(id.clone());
            }
        }
        Rest {
            instances,
            mates: Vec::new(),
            patterns: Vec::new(),
            asm,
            holds: true,
        }
    }

    fn body(&self, id: &Id) -> Option<u32> {
        self.instances
            .iter()
            .position(|i| i == id)
            .map(|k| k as u32)
    }
}

/// A move drive's turn, resolved: about `axis` (unit) through `origin`
/// (meters), from `from` to `to` (radians).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Turn {
    pub origin: [f64; 3],
    pub axis: [f64; 3],
    pub from: f64,
    pub to: f64,
}

/// A move drive's slide, resolved: along `direction` (unit), meters.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Slide {
    pub direction: [f64; 3],
    pub from: f64,
    pub to: f64,
}

/// A drive in ryumate's IDs, radians and meters.
#[derive(Clone, Debug, PartialEq)]
pub enum Drive {
    Mate {
        mate: Id,
        /// Its index among ryumate's mates.
        index: u32,
        driven: Driven,
        from: f64,
        to: f64,
    },
    Move {
        instances: Vec<Id>,
        bodies: Vec<u32>,
        rotate: Option<Turn>,
        translate: Option<Slide>,
    },
}

/// A motion resolved: its steps, its drives, and `motion_past_limit`
/// warnings.
#[derive(Clone, Debug, PartialEq)]
pub struct Resolved {
    pub steps: usize,
    pub drives: Vec<Drive>,
    pub warnings: Vec<Value>,
}

impl Resolved {
    /// The drives as `motion.list` gives them, in radians and meters.
    pub fn to_json(&self) -> Value {
        json!(
            self.drives
                .iter()
                .map(|d| match d {
                    Drive::Mate {
                        mate,
                        driven,
                        from,
                        to,
                        ..
                    } => match driven {
                        Driven::Angle => json!({"mate": mate, "angle_rad": [from, to]}),
                        Driven::Distance => json!({"mate": mate, "distance_m": [from, to]}),
                    },
                    Drive::Move {
                        instances,
                        rotate,
                        translate,
                        ..
                    } => {
                        let mut v = json!({"instances": instances});
                        if let Some(r) = rotate {
                            v["rotate"] = json!({"origin_m": r.origin, "axis": r.axis,
                                             "angle_rad": [r.from, r.to]});
                        }
                        if let Some(t) = translate {
                            v["translate"] = json!({"direction": t.direction,
                                                "distance_m": [t.from, t.to]});
                        }
                        v
                    }
                })
                .collect::<Vec<_>>()
        )
    }
}

/// Whether a mate of `form` has `driven` to drive.
fn has(form: crate::doc::mate::Form, driven: Driven) -> bool {
    use crate::doc::mate::Form;
    matches!(
        (form, driven),
        (
            Form::Revolute | Form::Cylindrical | Form::Planar,
            Driven::Angle
        ) | (
            Form::Slider | Form::Cylindrical | Form::Planar,
            Driven::Distance
        )
    )
}

/// The stop `v` lies past, if any, among `limits`' range for `driven`.
pub(crate) fn past(limits: &ryumate::Limits, driven: Driven, v: f64) -> Option<ryumate::Stop> {
    match driven {
        Driven::Angle => limits
            .angle
            .as_ref()
            .and_then(|r| r.past(ryumate::unwrap_angle(v, r), ANGLE_TOL)),
        Driven::Distance => limits.distance.as_ref().and_then(|r| r.past(v, LENGTH_TOL)),
    }
}

pub(crate) fn stop_text(s: ryumate::Stop) -> &'static str {
    match s {
        ryumate::Stop::Min => "min",
        ryumate::Stop::Max => "max",
    }
}

/// Motion `motion` (its ID, for messages) of assembly `asm` resolved
/// against `rest`: `motion_unresolved` for a drive naming a mate or an
/// instance the assembly doesn't place, or a mate whose form has no such
/// value; `motion_past_limit` warnings for a mate drive whose ends lie
/// past that mate's own stops.
pub fn resolve(
    asm: &Id,
    part: &Part,
    rest: &Rest,
    motion: &str,
    file: &MotionFile,
) -> Result<Resolved> {
    let unresolved = |pointer: String, name: &str, why: String| {
        Error::new(
            Code::MotionUnresolved,
            format!("{asm}'s motion {motion}: {why}"),
        )
        .with("part", asm.as_str())
        .with("motion", motion)
        .with("pointer", pointer)
        .with("name", name)
    };
    let mut drives = Vec::with_capacity(file.drives.len());
    let mut warnings = Vec::new();
    for (i, d) in file.drives.iter().enumerate() {
        let at = format!("/drives/{i}");
        match d {
            crate::doc::motion::Drive::Mate {
                mate,
                driven,
                range,
            } => {
                let here = format!("{at}/mate");
                let Some(Ok(Feature::Mate(doc))) = part.features.get(mate) else {
                    return Err(unresolved(
                        here,
                        mate.as_str(),
                        format!("{asm} has no mate {mate}"),
                    ));
                };
                if !has(doc.form, *driven) {
                    return Err(unresolved(
                        here,
                        mate.as_str(),
                        format!("{mate}'s form has no free {} to drive", driven.key()),
                    ));
                }
                let Some(index) = rest.mates.iter().position(|m| m == mate) else {
                    return Err(unresolved(
                        here,
                        mate.as_str(),
                        format!("{mate} isn't solved: its connectors don't resolve"),
                    ));
                };
                let (from, to) = (range.from.si(), range.to.si());
                let limits = crate::mate::limits(doc);
                if let Some(s) = [from, to].iter().find_map(|v| past(&limits, *driven, *v)) {
                    warnings.push(json!({
                        "code": Code::MotionPastLimit,
                        "message": format!(
                            "{asm}'s motion {motion} drives {mate}'s {} past its {} stop: those steps are blocked",
                            driven.key(), stop_text(s)),
                        "part": asm.as_str(), "motion": motion, "mate": mate,
                        "limit": stop_text(s), "pointer": at,
                    }));
                }
                drives.push(Drive::Mate {
                    mate: mate.clone(),
                    index: index as u32,
                    driven: *driven,
                    from,
                    to,
                });
            }
            crate::doc::motion::Drive::Move {
                instances,
                rotate,
                translate,
            } => {
                let mut bodies = Vec::with_capacity(instances.len());
                for (j, inst) in instances.iter().enumerate() {
                    let Some(k) = rest.body(inst) else {
                        return Err(unresolved(
                            format!("{at}/instances/{j}"),
                            inst.as_str(),
                            format!("{asm} doesn't place an instance {inst}"),
                        ));
                    };
                    bodies.push(k);
                }
                drives.push(Drive::Move {
                    instances: instances.clone(),
                    bodies,
                    rotate: rotate.as_ref().map(|r| Turn {
                        origin: [
                            r.origin[0].meters(),
                            r.origin[1].meters(),
                            r.origin[2].meters(),
                        ],
                        axis: r.direction.unit(),
                        from: r.range.from.radians(),
                        to: r.range.to.radians(),
                    }),
                    translate: translate.as_ref().map(|t| Slide {
                        direction: t.direction.unit(),
                        from: t.range.from.meters(),
                        to: t.range.to.meters(),
                    }),
                });
            }
        }
    }
    Ok(Resolved {
        steps: file.steps as usize,
        drives,
        warnings,
    })
}

/// `motion_not_found` for `id` in `asm`, naming the motions it has.
pub fn not_found(session: &Session, asm: &Id, id: &str) -> Error {
    let known: Vec<String> = session
        .project()
        .part(asm)
        .map(|p| p.motions.keys().map(Id::to_string).collect())
        .unwrap_or_default();
    Error::new(Code::MotionNotFound, format!("{asm} has no motion {id}"))
        .with("part", asm.as_str())
        .with("motion", id)
        .with("motions", known)
}

/// Motion `id` of assembly `asm` as the working files have it.
pub fn file_of(session: &Session, asm: &Id, id: &str) -> Result<MotionFile> {
    let part = session.project().part(asm)?;
    if crate::server::part_kind(part) != PartKind::Assembly {
        return Err(crate::doc::motion::not_an_assembly(asm.as_str()));
    }
    match Id::parse(id, "motion")
        .ok()
        .and_then(|k| part.motions.get(&k))
    {
        Some(Ok(f)) => Ok(f.clone()),
        Some(Err(e)) => Err(e.clone()),
        None => Err(not_found(session, asm, id)),
    }
}

/// Motion `id` of assembly `asm` resolved against the assembly as the
/// working files rebuild it.
pub fn resolve_in(session: &mut Session, asm: &Id, id: &str) -> Result<Resolved> {
    let file = file_of(session, asm, id)?;
    let part = session.project().part(asm)?.clone();
    let result = session.rebuilt(asm)?.clone();
    resolve(asm, &part, &Rest::of(&result), id, &file)
}

/// One motion as `motion.list` gives it.
fn entry(session: &mut Session, part: &Id, id: &Id) -> Value {
    let file = match file_of(session, part, id.as_str()) {
        Ok(f) => f,
        Err(e) => return json!({"part": part, "motion": id, "error": e}),
    };
    let written = file.to_json();
    let mut v = json!({"part": part, "motion": id, "steps": written["steps"],
                       "drives": written["drives"]});
    match resolve_in(session, part, id.as_str()) {
        Ok(r) => {
            v["resolved"] = r.to_json();
            v["warnings"] = json!(r.warnings);
        }
        Err(e) => v["error"] = json!(e),
    }
    v
}

/// The motions of assembly `part`, or of every assembly, with each
/// listed assembly's revision.
pub fn list(session: &mut Session, part: Option<&str>) -> Result<Value> {
    let parts: Vec<Id> = match part {
        Some(p) => {
            let id = crate::server::part_id(p)?;
            let found = session.project().part(&id)?;
            if crate::server::part_kind(found) != PartKind::Assembly {
                return Err(crate::doc::motion::not_an_assembly(p));
            }
            vec![id]
        }
        None => session.project().parts.keys().cloned().collect(),
    };
    let mut motions = Vec::new();
    let mut revisions = serde_json::Map::new();
    for id in &parts {
        let p = session.project().part(id)?;
        if part.is_some() || !p.motions.is_empty() {
            revisions.insert(id.to_string(), json!(p.revision));
        }
        let ids: Vec<Id> = p.motions.keys().cloned().collect();
        for m in ids {
            motions.push(entry(session, id, &m));
        }
    }
    Ok(json!({"motions": motions, "revisions": revisions}))
}

fn path(session: &Session, part: &Id, motion: &Id) -> std::path::PathBuf {
    crate::doc::project::part_dir(session.root(), part)
        .join(crate::doc::motion::motion_file(motion))
}

/// The assembly `part`, refusing a part (`wrong_part_kind`) and a stale
/// revision.
fn assembly(session: &Session, part: &str, expected: Option<&str>) -> Result<Id> {
    let pid = crate::server::part_id(part)?;
    let p = session.project().part(&pid)?;
    if crate::server::part_kind(p) != PartKind::Assembly {
        return Err(crate::doc::motion::not_an_assembly(part));
    }
    crate::server::check_revision(p, expected)?;
    Ok(pid)
}

/// A file's refusal as a request's: `invalid_param` at the same pointer.
fn as_param(e: Error) -> Error {
    Error::new(Code::InvalidParam, e.message.clone()).with("pointer", e.details["pointer"].clone())
}

/// Saves `drives` and `steps` as `part`'s motion `motion`, replacing one
/// of that ID: read against the assembly's parameters and resolved
/// against its rebuild first, so what doesn't read or resolve is never
/// written.
pub fn save(
    session: &mut Session,
    part: &str,
    motion: &str,
    drives: &Value,
    steps: Option<u32>,
    expected: Option<&str>,
) -> Result<Value> {
    let pid = assembly(session, part, expected)?;
    let mid = crate::doc::motion::check_id(motion)?;
    let p = session.project().part(&pid)?.clone();
    let replaced = p.motions.contains_key(&mid);
    let unit = session.project().manifest.default_unit;
    let doc = json!({
        "format": crate::doc::motion::FORMAT,
        "kind": crate::doc::motion::KIND,
        "steps": steps.unwrap_or(crate::doc::motion::STEPS),
        "drives": drives,
    });
    let file = MotionFile::from_json(&doc, unit, &p.env).map_err(as_param)?;
    let result = session.rebuilt(&pid)?.clone();
    resolve(&pid, &p, &Rest::of(&result), motion, &file)?;
    session.write_json(&path(session, &pid, &mid), &file.to_json())?;
    session.reload()?;
    let mut v = entry(session, &pid, &mid);
    v["replaced"] = json!(replaced);
    v["revision"] = json!(session.project().part(&pid)?.revision);
    Ok(v)
}

/// Deletes `part`'s motion `motion`; a file that doesn't read can be
/// deleted too.
pub fn delete(
    session: &mut Session,
    part: &str,
    motion: &str,
    expected: Option<&str>,
) -> Result<Value> {
    let pid = assembly(session, part, expected)?;
    let p = session.project().part(&pid)?;
    let mid = Id::parse(motion, "motion")
        .ok()
        .filter(|m| p.motions.contains_key(m))
        .ok_or_else(|| not_found(session, &pid, motion))?;
    session.remove_file(&path(session, &pid, &mid))?;
    session.reload()?;
    let revision = session.project().part(&pid)?.revision.clone();
    Ok(json!({"part": pid, "motion": mid, "revision": revision}))
}

/// The fraction of step `k` of `n`: `k / (n - 1)`, and exactly 1 at the
/// last.
pub fn t_of(k: usize, n: usize) -> f64 {
    if k + 1 >= n {
        1.0
    } else {
        k as f64 / (n - 1) as f64
    }
}

/// `from + (to - from) t`, and exactly `to` at 1.
pub fn lerp(from: f64, to: f64, t: f64) -> f64 {
    if t == 1.0 { to } else { from + (to - from) * t }
}

/// The rotation by `angle` about unit `axis` (Rodrigues), rows as
/// [`ryumate::Pose`] holds them.
pub(crate) fn turn(axis: [f64; 3], angle: f64) -> [[f64; 3]; 3] {
    let (s, c) = (ryukei::math::sin(angle), ryukei::math::cos(angle));
    let [x, y, z] = axis;
    let k = 1.0 - c;
    [
        [c + x * x * k, x * y * k - z * s, x * z * k + y * s],
        [y * x * k + z * s, c + y * y * k, y * z * k - x * s],
        [z * x * k - y * s, z * y * k + x * s, c + z * z * k],
    ]
}

/// Where a move drive's rigid motion is at `t`, in the assembly's frame:
/// turned about its axis, then slid.
fn motion_at(rotate: Option<&Turn>, translate: Option<&Slide>, t: f64) -> ryumate::Pose {
    let mut m = ryumate::Pose::IDENTITY;
    if let Some(r) = rotate {
        let rot = turn(r.axis, lerp(r.from, r.to, t));
        let o = r.origin;
        let ro = [0, 1, 2].map(|i| rot[i][0] * o[0] + rot[i][1] * o[1] + rot[i][2] * o[2]);
        m = ryumate::Pose {
            rotation: rot,
            origin: [0, 1, 2].map(|i| o[i] - ro[i]),
        };
    }
    if let Some(s) = translate {
        let d = lerp(s.from, s.to, t);
        m.origin = [0, 1, 2].map(|i| m.origin[i] + s.direction[i] * d);
    }
    m
}

/// Sets a mate kind's `driven` value to `v`.
fn set_value(kind: &mut ryumate::Kind, driven: Driven, v: f64) {
    use ryumate::Kind;
    match (kind, driven) {
        (Kind::Revolute { angle }, Driven::Angle)
        | (Kind::Cylindrical { angle, .. }, Driven::Angle)
        | (Kind::Planar { angle, .. }, Driven::Angle) => *angle = Some(v),
        (Kind::Slider { distance }, Driven::Distance)
        | (Kind::Cylindrical { distance, .. }, Driven::Distance) => *distance = Some(v),
        (Kind::Planar { distance, .. }, Driven::Distance) => *distance = v,
        (k, d) => unreachable!("resolve checked the form: {k:?} has no {d:?}"),
    }
}

/// The body a connector is on (a copy's seed).
fn body_on(on: &ryumate::On) -> u32 {
    match *on {
        ryumate::On::Body(b) => b,
        ryumate::On::Copy { seed, .. } => seed,
    }
}

/// Places every drive at `t` in `a`: each mate drive's value set, each
/// move drive's bodies moved from rest and held. The bodies to solve
/// from (the free ends of driven mates, the moved bodies), or the first
/// mate drive commanded past its own stop.
fn place(
    a: &mut ryumate::Assembly,
    rest: &ryumate::Assembly,
    drives: &[Drive],
    t: f64,
) -> std::result::Result<Vec<u32>, (u32, ryumate::Stop)> {
    let mut roots = Vec::new();
    for d in drives {
        match d {
            Drive::Mate {
                index,
                driven,
                from,
                to,
                ..
            } => {
                let v = lerp(*from, *to, t);
                let m = a.mates.get_mut(index).expect("a resolved mate");
                if let Some(stop) = past(&m.limits, *driven, v) {
                    return Err((*index, stop));
                }
                set_value(&mut m.kind, *driven, v);
                let ends = [body_on(&m.a.on), body_on(&m.b.on)];
                let free: Vec<u32> = ends
                    .iter()
                    .copied()
                    .filter(|b| !a.bodies[b].fixed)
                    .collect();
                roots.extend(if free.is_empty() { ends.to_vec() } else { free });
            }
            Drive::Move {
                bodies,
                rotate,
                translate,
                ..
            } => {
                let m = motion_at(rotate.as_ref(), translate.as_ref(), t);
                for b in bodies {
                    let body = a.bodies.get_mut(b).expect("a resolved body");
                    body.pose = m.then_after(&rest.bodies[b].pose);
                    body.fixed = true;
                    roots.push(*b);
                }
            }
        }
    }
    roots.sort_unstable();
    roots.dedup();
    Ok(roots)
}

/// One step of a sweep.
#[derive(Clone, Debug, PartialEq)]
pub struct Step {
    pub t: f64,
    pub holds: bool,
    /// Every body whose pose differs from rest, where this step puts it
    /// (empty when it doesn't hold).
    pub moved: BTreeMap<u32, ryumate::Pose>,
    /// The values of the mates the step solved, with their stops.
    pub values: BTreeMap<u32, ryumate::MateValues>,
    /// Where it doesn't hold: the conflicting sets of mates.
    pub conflicting: Vec<Vec<u32>>,
    /// A mate drive commanded past its own stop.
    pub past: Option<(u32, ryumate::Stop)>,
    /// The solver's refusal, if it refused (a blocked step, not an error).
    pub refused: Option<String>,
}

impl Step {
    fn blocked(t: f64) -> Step {
        Step {
            t,
            holds: false,
            moved: BTreeMap::new(),
            values: BTreeMap::new(),
            conflicting: Vec::new(),
            past: None,
            refused: None,
        }
    }
}

/// Step `t` solved from `start` (the last held step's poses): the step,
/// and when it holds, the poses the next step starts from.
pub fn solve_at(
    start: &ryumate::Assembly,
    rest: &ryumate::Assembly,
    drives: &[Drive],
    t: f64,
) -> Result<(Step, Option<ryumate::Assembly>)> {
    let mut a = start.clone();
    let roots = match place(&mut a, rest, drives, t) {
        Ok(r) => r,
        Err(past) => {
            let mut s = Step::blocked(t);
            s.past = Some(past);
            return Ok((s, None));
        }
    };
    let solved = match ryumate::solve(&a.component(&roots)) {
        Ok(s) => s,
        Err(e) => {
            let mut s = Step::blocked(t);
            s.refused = Some(e.to_string());
            return Ok((s, None));
        }
    };
    if !solved.report.converged {
        let mut s = Step::blocked(t);
        s.conflicting = solved.report.conflicting.clone();
        return Ok((s, None));
    }
    // The next step starts from these poses, with the mates and holds
    // as they were.
    let mut next = start.clone();
    for (k, b) in &a.bodies {
        next.bodies.get_mut(k).expect("a body").pose = b.pose;
    }
    for (k, p) in &solved.poses {
        next.bodies.get_mut(k).expect("a body").pose = *p;
    }
    let moved = next
        .bodies
        .iter()
        .filter(|(k, b)| rest.bodies[*k].pose != b.pose)
        .map(|(k, b)| (*k, b.pose))
        .collect();
    let step = Step {
        t,
        holds: true,
        moved,
        values: solved.report.values.clone(),
        conflicting: Vec::new(),
        past: None,
        refused: None,
    };
    Ok((step, Some(next)))
}

/// Every step at `ts`, one after another, each from the last that held:
/// so the result doesn't depend on threads, and a linkage keeps its
/// branch. Cancellable between steps.
pub fn steps(
    rest: &ryumate::Assembly,
    drives: &[Drive],
    ts: &[f64],
    job: &Job,
) -> Result<Vec<Step>> {
    let mut current = rest.clone();
    let mut out = Vec::with_capacity(ts.len());
    for &t in ts {
        job.check()?;
        let (step, next) = solve_at(&current, rest, drives, t)?;
        if let Some(next) = next {
            current = next;
        }
        out.push(step);
    }
    Ok(out)
}

/// A ryumate pose as an occurrence's placement, snapped as a rebuild
/// snaps a solved pose.
fn placement(p: &ryumate::Pose) -> Result<Rigid> {
    crate::assembly::pose_rigid(&crate::connector::doc_pose(p))
}

/// The body an occurrence's path sits under: its top-level instance, or
/// a copy's seed.
fn top_body(rest: &Rest, path: &[Id]) -> Option<u32> {
    match path[0].copy_of() {
        Some(_) => path.get(1).and_then(|s| rest.body(s)),
        None => rest.body(&path[0]),
    }
}

/// The rest model with each occurrence under a body in `moved` (a
/// top-level instance, or a copy of a moved seed) placed where the step
/// puts it, and for each occurrence the body that moved it.
pub fn step_model(
    rest_model: &Model,
    rest: &Rest,
    moved: &BTreeMap<u32, ryumate::Pose>,
) -> Result<(Model, Vec<Option<u32>>)> {
    let pattern_of = |p: &Id| rest.patterns.iter().position(|x| x == p).map(|k| k as u32);
    // Each copy of a moved seed, where the step's poses put it.
    let mut full = rest.asm.clone();
    for (k, p) in moved {
        full.bodies.get_mut(k).expect("a body").pose = *p;
    }
    let mut copies: Vec<(u32, u32, [u32; 2])> = Vec::new();
    for o in &rest_model.occurrences {
        if let Some((pattern, index)) = o.path[0].copy_of()
            && let Some(seed) = o.path.get(1).and_then(|s| rest.body(s))
            && moved.contains_key(&seed)
            && let Some(k) = pattern_of(&pattern)
        {
            copies.push((k, seed, index));
        }
    }
    copies.sort_unstable();
    copies.dedup();
    let copy_at: BTreeMap<(u32, u32, [u32; 2]), ryumate::Pose> = copies
        .iter()
        .copied()
        .zip(ryumate::copy_poses(&full, &copies))
        .collect();
    let mut occurrences = Vec::with_capacity(rest_model.occurrences.len());
    let mut movers = Vec::with_capacity(rest_model.occurrences.len());
    for o in &rest_model.occurrences {
        let top = match o.path[0].copy_of() {
            Some((pattern, index)) => {
                match (
                    pattern_of(&pattern),
                    o.path.get(1).and_then(|s| rest.body(s)),
                ) {
                    (Some(k), Some(seed)) => copy_at.get(&(k, seed, index)).map(|p| (seed, *p)),
                    _ => None,
                }
            }
            None => rest
                .body(&o.path[0])
                .and_then(|k| moved.get(&k).map(|p| (k, *p))),
        };
        let Some((k, pose)) = top else {
            occurrences.push(o.clone());
            movers.push(None);
            continue;
        };
        let child = o.within.get(1).copied().unwrap_or(Rigid::IDENTITY);
        let mut n = o.clone();
        n.pose = placement(&pose)?.then_after(&child);
        n.within[0] = n.pose;
        occurrences.push(n);
        movers.push(Some(k));
    }
    Ok((Model::new(occurrences), movers))
}

/// What a sweep finds besides its solve.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Options {
    /// Intersect pairs at each step and report what collides that
    /// doesn't at rest.
    pub interference: bool,
    /// Report the closest pair whose relative pose moved, at each step.
    pub clearance: bool,
    /// Clearance's reach in meters (`None`: clearance's own default).
    pub below: Option<f64>,
    /// Give each step's moved instances' poses.
    pub poses: bool,
    /// Bisections of the first collision and the first stop (0: none).
    pub refine: u32,
}

impl Options {
    fn to_json(&self) -> Value {
        json!({"interference": self.interference, "clearance": self.clearance,
               "below": self.below.map(f64::to_bits), "poses": self.poses,
               "refine": self.refine})
    }
}

/// What a sweep runs over: the assembly as some rebuild has it.
pub struct Input<'a> {
    pub asm: &'a Id,
    pub part: &'a Part,
    pub result: &'a PartResult,
    pub model: &'a Model,
    /// The project's default unit, for distances.
    pub unit: crate::units::Unit,
}

/// The caches a sweep fills: the session's, or check's own.
pub struct Caches<'c> {
    pub interference: &'c mut crate::interference::Cache,
    pub clearance: &'c mut crate::clearance::Cache,
}

/// `assembly_unsolved` for a sweep from an assembly that doesn't hold.
fn unsolved(asm: &Id) -> Error {
    Error::new(
        Code::AssemblyUnsolved,
        format!("{asm}'s mates can't all hold: settle them before sweeping"),
    )
    .with("part", asm.as_str())
}

/// The value of each drive at `t`: a mate's by its ID, a move's as
/// `drives/<i>`; angles in degrees, lengths in `unit`.
fn values_at(drives: &[Drive], t: f64, unit: crate::units::Unit) -> Value {
    let deg = |r: f64| r * (180.0 / core::f64::consts::PI);
    let len = |m: f64| json!(format!("{} {}", m / unit.factor(), unit.as_str()));
    let mut out = serde_json::Map::new();
    for (i, d) in drives.iter().enumerate() {
        match d {
            Drive::Mate {
                mate,
                driven,
                from,
                to,
                ..
            } => {
                let v = lerp(*from, *to, t);
                let entry = match driven {
                    Driven::Angle => json!({"angle_deg": deg(v)}),
                    Driven::Distance => json!({"distance": len(v)}),
                };
                out.insert(mate.to_string(), entry);
            }
            Drive::Move {
                rotate, translate, ..
            } => {
                let mut v = json!({});
                if let Some(r) = rotate {
                    v["angle_deg"] = json!(deg(lerp(r.from, r.to, t)));
                }
                if let Some(s) = translate {
                    v["distance"] = len(lerp(s.from, s.to, t));
                }
                out.insert(format!("drives/{i}"), v);
            }
        }
    }
    Value::Object(out)
}

/// Which group each body moves with: a move drive's bodies together.
fn groups(drives: &[Drive]) -> BTreeMap<u32, u32> {
    let mut g = BTreeMap::new();
    for (i, d) in drives.iter().enumerate() {
        if let Drive::Move { bodies, .. } = d {
            for b in bodies {
                g.insert(*b, u32::MAX - i as u32);
            }
        }
    }
    g
}

/// The occurrence path of an interference or clearance name
/// `<asm>/<path>/<solid>`.
fn occurrence_of(name: &str) -> &str {
    let rest = name.split_once('/').map_or("", |x| x.1);
    rest.rsplit_once('/').map_or(rest, |x| x.0)
}

/// A pair's two names.
fn pair_key(v: &Value) -> (String, String) {
    (
        v["a"].as_str().unwrap_or_default().to_owned(),
        v["b"].as_str().unwrap_or_default().to_owned(),
    )
}

/// An interference entry as a sweep gives a collision.
fn collision(i: &Value) -> Value {
    let mut c = json!({"pair": [i["a"], i["b"]], "volume_m3": i["volume_m3"], "box": i["box"],
                       "faces_a": i["faces"]["a"], "faces_b": i["faces"]["b"]});
    if i["approximate"] == true {
        c["approximate"] = json!(true);
    }
    c
}

/// Each moved instance's pose, as a file would hold it.
fn poses_json(rest: &Rest, moved: &BTreeMap<u32, ryumate::Pose>) -> Value {
    let mut out = serde_json::Map::new();
    for (k, p) in moved {
        out.insert(
            rest.instances[*k as usize].to_string(),
            crate::connector::doc_pose(p).to_json(),
        );
    }
    Value::Object(out)
}

/// `n` halvings of `[lo, hi]`, where `hit(lo)` is false and `hit(hi)`
/// true: the last bracket.
fn bisect(
    lo: f64,
    hi: f64,
    n: u32,
    mut hit: impl FnMut(f64) -> Result<bool>,
) -> Result<(f64, f64)> {
    let (mut lo, mut hi) = (lo, hi);
    for _ in 0..n {
        let mid = lo + (hi - lo) * 0.5;
        if hit(mid)? {
            hi = mid;
        } else {
            lo = mid;
        }
    }
    Ok((lo, hi))
}

/// `rest` with `moved` laid over it: the poses a step left.
fn overlay(rest: &ryumate::Assembly, moved: &BTreeMap<u32, ryumate::Pose>) -> ryumate::Assembly {
    let mut a = rest.clone();
    for (k, p) in moved {
        a.bodies.get_mut(k).expect("a body").pose = *p;
    }
    a
}

/// Sweeps `resolved` over `input`: every step solved in turn from the
/// last that held; with `opts.interference` each held step's collisions
/// (pairs that interfere there and don't at rest) and the pairs ryukei
/// can't intersect; with `opts.clearance` each held step's closest pair
/// whose relative pose moved. `motion` is the saved motion's ID, if any.
pub fn sweep(
    input: &Input<'_>,
    motion: Option<&Id>,
    resolved: &Resolved,
    opts: &Options,
    caches: Caches<'_>,
    job: &Job,
) -> Result<Value> {
    let Caches {
        interference,
        clearance,
    } = caches;
    let rest = Rest::of(input.result);
    if !rest.holds {
        return Err(unsolved(input.asm));
    }
    let n = resolved.steps;
    let ts: Vec<f64> = (0..n).map(|k| t_of(k, n)).collect();
    let steps = steps(&rest.asm, &resolved.drives, &ts, job)?;
    let groups = groups(&resolved.drives);
    // Which group each occurrence moves with over the whole sweep (`None`
    // for what no step moves), so a moving pair is one even at a step
    // where nothing has moved yet.
    let moving: BTreeSet<u32> = steps
        .iter()
        .flat_map(|s| s.moved.keys().copied())
        .chain(groups.keys().copied())
        .collect();
    let group: BTreeMap<String, Option<u32>> = input
        .model
        .occurrences
        .iter()
        .map(|o| {
            let b = top_body(&rest, &o.path).filter(|b| moving.contains(b));
            (
                crate::assembly::path_text(&o.path),
                b.map(|b| groups.get(&b).copied().unwrap_or(b)),
            )
        })
        .collect();
    let rest_found = if opts.interference {
        Some(crate::interference::find_for(
            input.asm,
            input.model,
            &[],
            interference,
            job,
        )?)
    } else {
        None
    };
    let at_rest: BTreeSet<(String, String)> = rest_found
        .iter()
        .flat_map(|f| f["interferences"].as_array().into_iter().flatten())
        .map(pair_key)
        .collect();
    let mut out = Vec::with_capacity(n);
    let mut unchecked = Vec::new();
    let mut seen_unchecked = BTreeSet::new();
    let mut first_collision: Option<(usize, Value)> = None;
    let mut first_blocked: Option<usize> = None;
    let mut limits_reached = Vec::new();
    let mut seen_limits = BTreeSet::new();
    let stage = job.begin(Phase::Intersect, "steps", n);
    for (k, s) in steps.iter().enumerate() {
        stage.step(|| format!("step {k}"))?;
        let mut v = json!({"index": k, "t": s.t,
                           "values": values_at(&resolved.drives, s.t, input.unit),
                           "state": if s.holds { "holds" } else { "blocked" }});
        if !s.holds {
            let sets: Vec<Vec<&Id>> = s
                .conflicting
                .iter()
                .map(|set| set.iter().map(|m| &rest.mates[*m as usize]).collect())
                .collect();
            v["conflicting"] = json!(sets);
            if let Some((m, stop)) = s.past {
                v["limit"] = json!({"mate": rest.mates[m as usize], "at": stop_text(stop)});
            }
            if let Some(r) = &s.refused {
                v["refused"] = json!(r);
            }
            first_blocked.get_or_insert(k);
            out.push(v);
            continue;
        }
        for (m, vals) in &s.values {
            let stops = [
                ("angle", vals.angle.and_then(|x| x.at)),
                ("distance", vals.distance.and_then(|x| x.at)),
            ];
            for (what, at) in stops {
                if let Some(at) = at
                    && seen_limits.insert((*m, what, stop_text(at)))
                {
                    limits_reached.push(json!({"mate": rest.mates[*m as usize], "value": what,
                                               "at": stop_text(at), "index": k}));
                }
            }
        }
        if opts.poses {
            v["poses"] = poses_json(&rest, &s.moved);
        }
        if opts.interference || opts.clearance {
            let (model, _) = match step_model(input.model, &rest, &s.moved) {
                Ok(x) => x,
                Err(e) => {
                    v["state"] = json!("blocked");
                    v["refused"] = json!(e.message);
                    first_blocked.get_or_insert(k);
                    out.push(v);
                    continue;
                }
            };
            if opts.interference {
                let found =
                    crate::interference::find_for(input.asm, &model, &[], interference, job)?;
                let collisions: Vec<Value> = found["interferences"]
                    .as_array()
                    .into_iter()
                    .flatten()
                    .filter(|i| !at_rest.contains(&pair_key(i)))
                    .map(collision)
                    .collect();
                if first_collision.is_none()
                    && let Some(c) = collisions.first()
                {
                    first_collision = Some((k, c.clone()));
                }
                v["collisions"] = json!(collisions);
                for u in found["unchecked"].as_array().into_iter().flatten() {
                    if seen_unchecked.insert(pair_key(u)) {
                        let mut u = u.clone();
                        u["index"] = json!(k);
                        unchecked.push(u);
                    }
                }
            }
            if opts.clearance {
                let of = |name: &Value| {
                    group
                        .get(occurrence_of(name.as_str().unwrap_or_default()))
                        .copied()
                        .flatten()
                };
                let found = crate::clearance::find(
                    input.asm,
                    &model,
                    &[],
                    opts.below,
                    input.unit,
                    clearance,
                )?;
                v["clearance"] = found["clearances"]
                    .as_array()
                    .into_iter()
                    .flatten()
                    .find(|c| of(&c["a"]) != of(&c["b"]))
                    .map_or(Value::Null, |c| {
                        json!({"pair": [c["a"], c["b"]], "distance": c["distance"],
                               "points": c["points"]})
                    });
            }
        }
        out.push(v);
    }
    drop(stage);
    // The first collision, refined: between the last clear step and the
    // first colliding one, each probe solved from the clear side's poses
    // and only the colliding pair's first side intersected.
    if opts.refine > 0
        && let Some((k, c)) = &mut first_collision
        && *k > 0
        && steps[*k - 1].holds
    {
        let pair = (
            c["pair"][0].as_str().unwrap_or_default().to_owned(),
            c["pair"][1].as_str().unwrap_or_default().to_owned(),
        );
        let only =
            crate::interference::instance_paths(input.asm, &[occurrence_of(&pair.0).to_owned()])?;
        let mut clear = steps[*k - 1].moved.clone();
        let (lo, hi) = bisect(ts[*k - 1], ts[*k], opts.refine, |t| {
            let (s, _) = solve_at(&overlay(&rest.asm, &clear), &rest.asm, &resolved.drives, t)?;
            if !s.holds {
                return Ok(true);
            }
            let Ok((model, _)) = step_model(input.model, &rest, &s.moved) else {
                return Ok(true);
            };
            let found = crate::interference::find_for(input.asm, &model, &only, interference, job)?;
            let hit = found["interferences"]
                .as_array()
                .into_iter()
                .flatten()
                .any(|i| pair_key(i) == pair);
            if !hit {
                clear = s.moved;
            }
            Ok(hit)
        })?;
        c["contact"] = json!({"t": hi, "values": values_at(&resolved.drives, hi, input.unit),
                              "bracket": [lo, hi]});
    }
    // The first stop, likewise: where the steps stop holding.
    let mut stop = None;
    if opts.refine > 0
        && let Some(k) = first_blocked
        && k > 0
        && steps[k - 1].holds
    {
        let mut held = steps[k - 1].moved.clone();
        let (lo, hi) = bisect(ts[k - 1], ts[k], opts.refine, |t| {
            let (s, _) = solve_at(&overlay(&rest.asm, &held), &rest.asm, &resolved.drives, t)?;
            if s.holds {
                held = s.moved;
            }
            Ok(!s.holds)
        })?;
        stop = Some(
            json!({"t": hi, "values": values_at(&resolved.drives, hi, input.unit),
                           "bracket": [lo, hi]}),
        );
    }
    let at = |k: usize| json!({"index": k, "t": ts[k], "values": values_at(&resolved.drives, ts[k], input.unit)});
    let first_collision = first_collision.map(|(k, c)| {
        let mut v = at(k);
        for (key, x) in c.as_object().into_iter().flatten() {
            v[key] = x.clone();
        }
        v
    });
    let first_blocked = first_blocked.map(|k| {
        let mut v = at(k);
        for key in ["conflicting", "limit", "refused"] {
            if let Some(x) = out[k].get(key) {
                v[key] = x.clone();
            }
        }
        if let Some(s) = &stop {
            v["stop"] = s.clone();
        }
        v
    });
    let mut answer = json!({
        "motion": motion,
        "steps": out,
        "first_collision": first_collision,
        "first_blocked": first_blocked,
        "limits_reached": limits_reached,
    });
    if let Some(f) = rest_found {
        answer["at_rest"] = json!(
            f["interferences"]
                .as_array()
                .into_iter()
                .flatten()
                .map(|i| json!({"pair": [i["a"], i["b"]], "volume_m3": i["volume_m3"]}))
                .collect::<Vec<_>>()
        );
        answer["unchecked"] = json!(unchecked);
    }
    Ok(answer)
}

/// The last [`Cache::KEEP`] sweep answers, by a hash of the assembly's
/// revision, the resolved drives and steps, and the options.
#[derive(Clone, Debug, Default)]
pub struct Cache {
    answers: Vec<(String, std::sync::Arc<Value>)>,
}

impl Cache {
    pub const KEEP: usize = 32;

    pub fn get(&self, key: &str) -> Option<std::sync::Arc<Value>> {
        self.answers
            .iter()
            .find(|(k, _)| k == key)
            .map(|(_, v)| v.clone())
    }

    pub fn put(&mut self, key: String, v: std::sync::Arc<Value>) {
        self.answers.retain(|(k, _)| *k != key);
        self.answers.push((key, v));
        if self.answers.len() > Self::KEEP {
            self.answers.remove(0);
        }
    }

    pub fn len(&self) -> usize {
        self.answers.len()
    }

    pub fn is_empty(&self) -> bool {
        self.answers.is_empty()
    }
}

/// What `assembly.sweep` asks.
#[derive(Clone, Debug, Default)]
pub struct SweepAsk {
    pub part: String,
    pub motion: Option<String>,
    pub drives: Option<Value>,
    pub mate: Option<String>,
    pub from: Option<String>,
    pub to: Option<String>,
    pub steps: Option<u32>,
    pub interference: bool,
    pub clearance: bool,
    pub below: Option<String>,
    pub poses: bool,
    pub refine: Option<u32>,
}

/// Bisections a sweep may ask for: past 30 a step is narrower than a
/// double's spacing near 1.
const MAX_REFINE: u32 = 30;

fn bad(message: &str) -> Error {
    Error::new(Code::InvalidParam, message)
}

/// `assembly.sweep`: the motion asked for, swept over the working files'
/// rebuild with the session's caches; asked again, from the session's
/// last answers.
pub fn sweep_request(session: &mut Session, ask: SweepAsk) -> Result<Value> {
    let pid = assembly(session, &ask.part, None)?;
    let part = session.project().part(&pid)?.clone();
    let unit = session.project().manifest.default_unit;
    let file = match (&ask.motion, &ask.drives, &ask.mate) {
        (Some(m), None, None) if ask.steps.is_none() => file_of(session, &pid, m)?,
        (None, Some(d), None) => {
            let doc = json!({"format": crate::doc::motion::FORMAT, "kind": crate::doc::motion::KIND,
                             "steps": ask.steps.unwrap_or(crate::doc::motion::STEPS), "drives": d});
            MotionFile::from_json(&doc, unit, &part.env).map_err(as_param)?
        }
        (None, None, Some(m)) => {
            let (Some(from), Some(to)) = (&ask.from, &ask.to) else {
                return Err(bad("a mate's sweep goes --from one value --to another"));
            };
            let slider = matches!(
                Id::parse(m, "mate").ok().and_then(|k| part.features.get(&k)),
                Some(Ok(Feature::Mate(doc))) if doc.form == crate::doc::mate::Form::Slider
            );
            let key = if slider { "distance" } else { "angle" };
            let mut drive = json!({"mate": m});
            drive[key] = json!({"from": from, "to": to});
            let doc = json!({"format": crate::doc::motion::FORMAT, "kind": crate::doc::motion::KIND,
                             "steps": ask.steps.unwrap_or(crate::doc::motion::STEPS), "drives": [drive]});
            MotionFile::from_json(&doc, unit, &part.env).map_err(as_param)?
        }
        _ => {
            return Err(bad(
                "sweep a saved motion, inline drives, or one mate: exactly one of motion, drives and mate (steps goes with drives or mate)",
            ));
        }
    };
    let refine = ask.refine.unwrap_or(10);
    if refine > MAX_REFINE {
        return Err(bad(&format!("refine is 0 to {MAX_REFINE} bisections")));
    }
    let below = match &ask.below {
        None => None,
        Some(b) => {
            let q =
                crate::units::Quantity::from_json_in(&json!(b), unit, &|n: &str| part.env.get(n))
                    .map_err(|e| bad(&e.message))?;
            if q.meters() < 0.0 {
                return Err(bad("below is a length of 0 or more"));
            }
            Some(q.meters())
        }
    };
    let opts = Options {
        interference: ask.interference,
        clearance: ask.clearance,
        below,
        poses: ask.poses,
        refine,
    };
    let result = session.rebuilt(&pid)?.clone();
    let rest = Rest::of(&result);
    let motion_id = ask
        .motion
        .as_deref()
        .map(|m| Id::parse(m, "motion"))
        .transpose()?;
    let resolved = resolve(
        &pid,
        &part,
        &rest,
        ask.motion.as_deref().unwrap_or("(inline)"),
        &file,
    )?;
    let key = {
        let mut h = blake3::Hasher::new();
        h.update(part.revision.as_bytes());
        h.update(crate::doc::canonical::to_string(&resolved.to_json()).as_bytes());
        h.update(&(resolved.steps as u64).to_le_bytes());
        h.update(crate::doc::canonical::to_string(&opts.to_json()).as_bytes());
        h.update(crate::doc::canonical::to_string(&json!(motion_id)).as_bytes());
        h.finalize().to_hex().to_string()
    };
    if let Some(v) = session.sweeps.get(&key) {
        return Ok((*v).clone());
    }
    let model = session.model(&pid, &crate::doc::assembly::Rev::Working)?;
    let job = session.job.clone();
    let input = Input {
        asm: &pid,
        part: &part,
        result: &result,
        model: &model,
        unit,
    };
    let caches = Caches {
        interference: &mut session.interference,
        clearance: &mut session.clearance,
    };
    let mut v = sweep(&input, motion_id.as_ref(), &resolved, &opts, caches, &job)?;
    v["part"] = json!(pid);
    v["revision"] = json!(part.revision);
    v["warnings"] = json!(resolved.warnings);
    session.sweeps.put(key, std::sync::Arc::new(v.clone()));
    Ok(v)
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use crate::mate::tests::hinged;

    pub(crate) fn id(s: &str) -> Id {
        Id::parse(s, "part").unwrap()
    }

    /// [`hinged`] with `m1` stopped at ±6°.
    pub(crate) fn stopped() -> (tempfile::TempDir, Session) {
        let (dir, mut s) = hinged();
        let m1 = crate::doc::project::feature_path(dir.path(), &id("frame"), &id("m1"));
        let mut v = crate::doc::canonical::parse(&std::fs::read_to_string(&m1).unwrap()).unwrap();
        v["format"] = json!(crate::doc::mate::LIMITS_FORMAT);
        v["limits"] = json!({"angle": {"min": "-6 deg", "max": "6 deg"}});
        crate::doc::project::write_json(&m1, &v).unwrap();
        s.refresh().unwrap();
        (dir, s)
    }

    pub(crate) fn file(s: &Session, asm: &str, drives: Value, steps: u32) -> MotionFile {
        let p = s.project().part(&id(asm)).unwrap();
        MotionFile::from_json(
            &json!({"format": 1, "kind": "ryucad.motion", "steps": steps, "drives": drives}),
            s.project().manifest.default_unit,
            &p.env,
        )
        .unwrap()
    }

    pub(crate) fn resolved(s: &mut Session, asm: &str, f: &MotionFile) -> Result<Resolved> {
        let part = s.project().part(&id(asm)).unwrap().clone();
        let result = s.rebuilt(&id(asm)).unwrap().clone();
        resolve(&id(asm), &part, &Rest::of(&result), "m", f)
    }

    #[test]
    fn drives_resolve_to_the_solvers_ids() {
        let (_dir, mut s) = stopped();
        let f = file(
            &s,
            "frame",
            json!([{"mate": "m1", "angle": {"from": "-3 deg", "to": "3 deg"}},
                   {"instances": ["pin"], "translate":
                       {"direction": "z", "from": "0 mm", "to": "1 mm"}}]),
            7,
        );
        let r = resolved(&mut s, "frame", &f).unwrap();
        assert_eq!(r.steps, 7);
        assert!(r.warnings.is_empty(), "{:?}", r.warnings);
        let Drive::Mate {
            mate,
            driven,
            from,
            to,
            ..
        } = &r.drives[0]
        else {
            panic!()
        };
        assert_eq!((mate.as_str(), *driven), ("m1", Driven::Angle));
        assert!((from + 3f64.to_radians()).abs() < 1e-15 && (to - 3f64.to_radians()).abs() < 1e-15);
        let Drive::Move {
            bodies, translate, ..
        } = &r.drives[1]
        else {
            panic!()
        };
        assert_eq!(bodies.len(), 1);
        let t = translate.unwrap();
        assert_eq!((t.direction, t.from, t.to), ([0.0, 0.0, 1.0], 0.0, 0.001));
        assert_eq!(r.to_json()[0]["mate"], "m1");
    }

    #[test]
    fn a_range_past_a_stop_warns_and_a_missing_name_is_unresolved() {
        let (_dir, mut s) = stopped();
        let past = file(
            &s,
            "frame",
            json!([{"mate": "m1", "angle": {"from": "0 deg", "to": "10 deg"}}]),
            11,
        );
        let r = resolved(&mut s, "frame", &past).unwrap();
        assert_eq!(r.warnings.len(), 1, "{:?}", r.warnings);
        assert_eq!(r.warnings[0]["code"], "motion_past_limit");
        assert_eq!(r.warnings[0]["mate"], "m1");
        assert_eq!(r.warnings[0]["limit"], "max");
        assert_eq!(r.warnings[0]["pointer"], "/drives/0");
        for (drives, pointer, name) in [
            (
                json!([{"mate": "m9", "angle": {"from": 0, "to": 1}}]),
                "/drives/0/mate",
                "m9",
            ),
            (
                json!([{"mate": "m1", "distance": {"from": 0, "to": 1}}]),
                "/drives/0/mate",
                "m1",
            ),
            (
                json!([{"mate": "pin", "angle": {"from": 0, "to": 1}}]),
                "/drives/0/mate",
                "pin",
            ),
            (
                json!([{"instances": ["pin", "gone"], "translate":
                    {"direction": "x", "from": 0, "to": 1}}]),
                "/drives/0/instances/1",
                "gone",
            ),
        ] {
            let f = file(&s, "frame", drives, 3);
            let e = resolved(&mut s, "frame", &f).unwrap_err();
            assert_eq!(e.code, Code::MotionUnresolved, "{e:?}");
            assert_eq!(e.details["pointer"], pointer, "{e:?}");
            assert_eq!(e.details["name"], name, "{e:?}");
        }
    }

    use crate::server::tests::call;

    #[test]
    fn motions_save_list_and_delete_as_explodes_do() {
        let (dir, mut s) = stopped();
        let drives = json!([{"mate": "m1", "angle": {"from": "0 deg", "to": "10 deg"}}]);
        let v = call(
            &mut s,
            "motion.save",
            json!({"part": "frame", "motion": "swing", "drives": drives, "steps": 11}),
        )
        .unwrap();
        assert_eq!(v["replaced"], false);
        assert_eq!(v["warnings"][0]["code"], "motion_past_limit");
        assert_eq!(v["resolved"][0]["mate"], "m1");
        let on_disk: Value = serde_json::from_str(
            &std::fs::read_to_string(dir.path().join("parts/frame/motions/swing.json")).unwrap(),
        )
        .unwrap();
        assert_eq!(on_disk["steps"], 11);
        assert_eq!(on_disk["drives"], drives);
        let list = call(&mut s, "motion.list", json!({"part": "frame"})).unwrap();
        assert_eq!(list["motions"][0]["motion"], "swing");
        assert!(list["revisions"]["frame"].is_string());
        // Every assembly's when no part is named.
        assert_eq!(
            call(&mut s, "motion.list", json!({})).unwrap()["motions"],
            list["motions"]
        );
        // Refusals: what doesn't read, what doesn't resolve, a part.
        let bad = call(
            &mut s,
            "motion.save",
            json!({"part": "frame", "motion": "x", "drives": [], "steps": 11}),
        )
        .unwrap_err();
        assert_eq!(
            (bad.code, bad.details["pointer"].as_str()),
            (Code::InvalidParam, Some("/drives"))
        );
        let gone = call(
            &mut s,
            "motion.save",
            json!({"part": "frame", "motion": "x",
                   "drives": [{"mate": "m9", "angle": {"from": 0, "to": 1}}]}),
        )
        .unwrap_err();
        assert_eq!(gone.code, Code::MotionUnresolved);
        assert!(!dir.path().join("parts/frame/motions/x.json").exists());
        let part = call(
            &mut s,
            "motion.save",
            json!({"part": "plate", "motion": "x", "drives": drives}),
        )
        .unwrap_err();
        assert_eq!(part.code, Code::WrongPartKind);
        // A stale revision is refused.
        let stale = call(
            &mut s,
            "motion.delete",
            json!({"part": "frame", "motion": "swing", "expected_revision": "0"}),
        )
        .unwrap_err();
        assert_eq!(stale.code, Code::Conflict);
        let missing = call(
            &mut s,
            "motion.delete",
            json!({"part": "frame", "motion": "nope"}),
        )
        .unwrap_err();
        assert_eq!(missing.code, Code::MotionNotFound);
        assert_eq!(missing.details["motions"], json!(["swing"]));
        call(
            &mut s,
            "motion.delete",
            json!({"part": "frame", "motion": "swing"}),
        )
        .unwrap();
        assert!(!dir.path().join("parts/frame/motions/swing.json").exists());
    }

    #[test]
    fn a_parameter_rename_reaches_motions() {
        let (dir, mut s) = stopped();
        call(
            &mut s,
            "param.set",
            json!({"part": "frame", "parameters": {"swing": "3 deg"}}),
        )
        .unwrap();
        call(
            &mut s,
            "motion.save",
            json!({"part": "frame", "motion": "swing",
                   "drives": [{"mate": "m1", "angle": {"from": "-swing", "to": "swing"}}]}),
        )
        .unwrap();
        call(
            &mut s,
            "param.rename",
            json!({"part": "frame", "from": "swing", "to": "reach"}),
        )
        .unwrap();
        let v: Value = serde_json::from_str(
            &std::fs::read_to_string(dir.path().join("parts/frame/motions/swing.json")).unwrap(),
        )
        .unwrap();
        assert_eq!(
            v["drives"][0]["angle"],
            json!({"from": "-reach", "to": "reach"})
        );
        // The parameter is read, so not unused.
        let list = call(&mut s, "param.list", json!({"part": "frame"})).unwrap();
        assert_ne!(list["parameters"]["reach"]["unused"], true, "{list}");
    }

    #[test]
    fn rest_names_bodies_mates_and_patterns() {
        // `frame` has a pattern and no mates, so the rebuild settles it;
        // `pair` has neither, so its rest is its instances as placed.
        let (_dir, mut s) = crate::explode::tests::project();
        let r = s.rebuilt(&id("frame")).unwrap().clone();
        let rest = Rest::of(&r);
        assert_eq!(rest.patterns, [id("row")]);
        assert!(rest.instances.contains(&id("pin")), "{:?}", rest.instances);
        assert!(rest.holds);
        let r = s.rebuilt(&id("pair")).unwrap().clone();
        assert!(r.solve.is_none());
        let rest = Rest::of(&r);
        assert_eq!(rest.instances, [id("left"), id("right")]);
        assert!(rest.patterns.is_empty() && rest.mates.is_empty());
        assert_eq!(rest.asm.bodies.len(), 2);
        assert_eq!(rest.asm.bodies[&0].pose.origin, [-0.008, 0.0, 0.0]);
    }

    fn at(origin: [f64; 3], angle: f64) -> ryumate::Pose {
        ryumate::Pose {
            rotation: turn([0.0, 0.0, 1.0], angle),
            origin,
        }
    }

    fn on(b: u32, origin: [f64; 3]) -> ryumate::Connector {
        ryumate::Connector {
            on: ryumate::On::Body(b),
            frame: ryumate::Pose {
                origin,
                ..ryumate::Pose::IDENTITY
            },
        }
    }

    fn joint(kind: ryumate::Kind, a: ryumate::Connector, b: ryumate::Connector) -> ryumate::Mate {
        ryumate::Mate {
            kind,
            a,
            b,
            limits: ryumate::Limits::NONE,
        }
    }

    /// A crank-rocker in the xy plane, settled with its elbow up: ground
    /// (fixed); a crank of 1 turning about the origin (mate 0, driven);
    /// a coupler of 2.5 and a rocker of 2 standing on the ground at
    /// (3, 0). Ball joints at the coupler's ends keep it from being
    /// overconstrained (the coupler may spin about its own line, nothing
    /// else).
    fn four_bar() -> ryumate::Assembly {
        let mut a = ryumate::Assembly::default();
        let body = |pose, fixed| ryumate::Body { pose, fixed };
        a.bodies.insert(0, body(ryumate::Pose::IDENTITY, true));
        a.bodies.insert(1, body(ryumate::Pose::IDENTITY, false));
        a.bodies.insert(2, body(at([1.0, 0.0, 0.0], 0.8957), false));
        a.bodies.insert(3, body(at([3.0, 0.0, 0.0], 1.7913), false));
        a.mates.insert(
            0,
            joint(
                ryumate::Kind::Revolute { angle: Some(0.0) },
                on(0, [0.0; 3]),
                on(1, [0.0; 3]),
            ),
        );
        a.mates.insert(
            1,
            joint(ryumate::Kind::Ball, on(1, [1.0, 0.0, 0.0]), on(2, [0.0; 3])),
        );
        a.mates.insert(
            2,
            joint(
                ryumate::Kind::Revolute { angle: None },
                on(0, [3.0, 0.0, 0.0]),
                on(3, [0.0; 3]),
            ),
        );
        a.mates.insert(
            3,
            joint(
                ryumate::Kind::Ball,
                on(2, [2.5, 0.0, 0.0]),
                on(3, [2.0, 0.0, 0.0]),
            ),
        );
        let solved = ryumate::solve(&a).unwrap();
        assert!(solved.report.converged);
        solved.assembly(&a)
    }

    #[test]
    fn fractions_end_exactly_at_the_end() {
        assert_eq!(
            (0..3).map(|k| t_of(k, 3)).collect::<Vec<_>>(),
            [0.0, 0.5, 1.0]
        );
        assert_eq!(t_of(12, 13), 1.0);
        assert_eq!(lerp(-0.1, 0.3, 1.0), 0.3);
        assert_eq!(lerp(-0.1, 0.3, 0.0), -0.1);
    }

    #[test]
    fn a_four_bar_stays_on_its_branch_through_a_whole_turn() {
        let rest = four_bar();
        let drives = [Drive::Mate {
            mate: id("crank"),
            index: 0,
            driven: Driven::Angle,
            from: 0.0,
            to: 2.0 * core::f64::consts::PI,
        }];
        let ts: Vec<f64> = (0..37).map(|k| t_of(k, 37)).collect();
        let steps = steps(&rest, &drives, &ts, &Job::default()).unwrap();
        for (k, s) in steps.iter().enumerate() {
            assert!(s.holds, "step {k}: {s:?}");
            let rocker = s.moved.get(&3).copied().unwrap_or(rest.bodies[&3].pose);
            let tip = rocker.apply([2.0, 0.0, 0.0]);
            assert!(
                tip[1] > 0.5,
                "step {k}: the rocker's tip {tip:?} flipped below"
            );
        }
        // The same bits again.
        assert_eq!(
            steps,
            super::steps(&rest, &drives, &ts, &Job::default()).unwrap()
        );
    }

    #[test]
    fn steps_past_a_stop_are_blocked_and_the_stop_is_reached() {
        let (_dir, mut s) = stopped();
        let f = file(
            &s,
            "frame",
            json!([{"mate": "m1", "angle": {"from": "0 deg", "to": "10 deg"}}]),
            11,
        );
        let r = resolved(&mut s, "frame", &f).unwrap();
        let result = s.rebuilt(&id("frame")).unwrap().clone();
        let rest = Rest::of(&result);
        let ts: Vec<f64> = (0..11).map(|k| t_of(k, 11)).collect();
        let steps = steps(&rest.asm, &r.drives, &ts, &Job::default()).unwrap();
        let held: Vec<bool> = steps.iter().map(|s| s.holds).collect();
        assert_eq!(
            held,
            [
                true, true, true, true, true, true, true, false, false, false, false
            ]
        );
        assert_eq!(steps[7].past, Some((0, ryumate::Stop::Max)));
        let m1 = steps[6].values[&0].angle.unwrap();
        assert_eq!(m1.at, Some(ryumate::Stop::Max), "{m1:?}");
    }

    #[test]
    fn a_held_move_carries_its_group_rigidly() {
        let (_dir, mut s) = crate::explode::tests::project();
        let f = file(
            &s,
            "frame",
            json!([{"instances": ["p1", "p2"], "rotate":
                {"axis": {"origin": ["0 mm", "0 mm", "0 mm"], "direction": "z"},
                 "from": "0 deg", "to": "90 deg"}}]),
            3,
        );
        let r = resolved(&mut s, "frame", &f).unwrap();
        let result = s.rebuilt(&id("frame")).unwrap().clone();
        let rest = Rest::of(&result);
        let steps = steps(&rest.asm, &r.drives, &[0.0, 0.5, 1.0], &Job::default()).unwrap();
        assert!(steps.iter().all(|s| s.holds));
        // p1 was at (15, 15, 6) mm: a quarter turn about z puts it at
        // (-15, 15, 6) mm.
        let p1 = rest.body(&id("p1")).unwrap();
        let o = steps[2].moved[&p1].origin;
        assert!(
            (o[0] + 0.015).abs() < 1e-15 && (o[1] - 0.015).abs() < 1e-15,
            "{o:?}"
        );
        assert!(steps[0].moved.is_empty(), "{:?}", steps[0].moved);
    }

    #[test]
    fn a_step_model_moves_what_the_step_moved_and_nothing_else() {
        let (_dir, mut s) = crate::explode::tests::project();
        let pid = id("frame");
        let result = s.rebuilt(&pid).unwrap().clone();
        let rest = Rest::of(&result);
        let model = s.model(&pid, &crate::doc::assembly::Rev::Working).unwrap();
        let pin = rest.body(&id("pin")).unwrap();
        let mut lifted = rest.asm.bodies[&pin].pose;
        lifted.origin[2] += 0.01;
        let moved = BTreeMap::from([(pin, lifted)]);
        let (m, movers) = step_model(&model, &rest, &moved).unwrap();
        for ((o, n), mover) in model.occurrences.iter().zip(&m.occurrences).zip(&movers) {
            let path = crate::assembly::path_text(&o.path);
            let (a, b) = (o.pose.translation_part(), n.pose.translation_part());
            if ["pin", "row:1/pin", "row:2/pin"].contains(&path.as_str()) {
                assert_eq!(*mover, Some(pin), "{path}");
                assert!(
                    (b.z - a.z - 0.01).abs() < 1e-15 && (b.x - a.x).abs() < 1e-15,
                    "{path}"
                );
            } else {
                assert_eq!(*mover, None, "{path}");
                assert_eq!(o.pose, n.pose, "{path}: unmoved keeps its bits");
            }
        }
        assert_ne!(m.hash, model.hash);
        // Nothing moved: the rest model, hash and all.
        assert_eq!(
            step_model(&model, &rest, &BTreeMap::new()).unwrap().0.hash,
            model.hash
        );
    }

    /// Parts `arm` (a 50 × 4 × 4 mm bar from the origin along x), `block`
    /// (10 × 10 × 4 mm) and `cube` (2 mm), and assembly `swing`: the arm
    /// at the origin, the block fixed at (30, 20, 0) mm and, with `nub`, a
    /// cube fixed at (10, 1, 1) mm, sunk into the arm at rest. Turning the
    /// arm about z, its leading face first meets the block's corner
    /// (40, 20) at atan2(20, 40) - asin(4 / √2000), about 21.43°.
    pub(crate) fn swing(nub: bool) -> (tempfile::TempDir, Session) {
        let (dir, mut s) = crate::server::tests::fresh();
        let mut ok = |m: &str, v: Value| {
            call(&mut s, m, v).unwrap_or_else(|e| panic!("{m}: {e}"));
        };
        for (part, x, y, z) in [("arm", 50, 4, 4), ("block", 10, 10, 4), ("cube", 2, 2, 2)] {
            ok("part.create", json!({"part": part}));
            ok(
                "feature.add",
                json!({"part": part, "type": "box", "id": "body", "params":
                    {"x": format!("{x} mm"), "y": format!("{y} mm"), "z": format!("{z} mm")}}),
            );
        }
        ok("part.create", json!({"part": "swing", "kind": "assembly"}));
        ok(
            "feature.add",
            json!({"part": "swing", "type": "instance", "id": "arm", "of": "arm"}),
        );
        ok(
            "feature.add",
            json!({"part": "swing", "type": "instance", "id": "block", "of": "block",
                   "fixed": true, "placement": {"origin": ["30 mm", "20 mm", "0 mm"]}}),
        );
        if nub {
            ok(
                "feature.add",
                json!({"part": "swing", "type": "instance", "id": "nub", "of": "cube",
                       "fixed": true, "placement": {"origin": ["10 mm", "1 mm", "1 mm"]}}),
            );
        }
        (dir, s)
    }

    pub(crate) fn turning(to: &str) -> Value {
        json!([{"instances": ["arm"], "rotate":
            {"axis": {"origin": ["0 mm", "0 mm", "0 mm"], "direction": "z"},
             "from": "0 deg", "to": to}}])
    }

    /// A sweep of `drives` over `steps` with the session's caches.
    pub(crate) fn swept(
        s: &mut Session,
        asm: &str,
        drives: Value,
        steps: u32,
        opts: &Options,
    ) -> Value {
        let f = file(s, asm, drives, steps);
        let r = resolved(s, asm, &f).unwrap();
        let pid = id(asm);
        let part = s.project().part(&pid).unwrap().clone();
        let result = s.rebuilt(&pid).unwrap().clone();
        let model = s.model(&pid, &crate::doc::assembly::Rev::Working).unwrap();
        let unit = s.project().manifest.default_unit;
        let input = Input {
            asm: &pid,
            part: &part,
            result: &result,
            model: &model,
            unit,
        };
        let caches = Caches {
            interference: &mut s.interference,
            clearance: &mut s.clearance,
        };
        sweep(&input, None, &r, opts, caches, &Job::default()).unwrap()
    }

    fn names(pair: &Value) -> BTreeSet<String> {
        pair.as_array()
            .unwrap()
            .iter()
            .map(|x| x.as_str().unwrap().to_owned())
            .collect()
    }

    #[test]
    fn collisions_are_what_rest_does_not_have() {
        let (_dir, mut s) = swing(true);
        let opts = Options {
            interference: true,
            ..Options::default()
        };
        let v = swept(&mut s, "swing", turning("45 deg"), 46, &opts);
        // The nub sits in the arm at rest: reported once, never a collision.
        assert_eq!(v["at_rest"].as_array().unwrap().len(), 1, "{v}");
        assert!(
            names(&v["at_rest"][0]["pair"]).contains("swing/nub/body"),
            "{v}"
        );
        // Contact is at about 21.43°: step 21 is clear, step 22 collides.
        assert_eq!(
            v["steps"][21]["collisions"],
            json!([]),
            "{}",
            v["steps"][21]
        );
        let c = &v["steps"][22]["collisions"][0];
        assert_eq!(
            names(&c["pair"]),
            BTreeSet::from(["swing/arm/body".to_owned(), "swing/block/body".to_owned()])
        );
        assert!(c["volume_m3"].as_f64().unwrap() > 0.0);
        assert!(!c["faces_a"].as_array().unwrap().is_empty());
        assert_eq!(v["first_collision"]["index"], 22);
        assert_eq!(v["steps"][22]["values"]["drives/0"]["angle_deg"], 22.0);
        for st in v["steps"].as_array().unwrap() {
            for c in st["collisions"].as_array().unwrap() {
                assert!(!names(&c["pair"]).contains("swing/nub/body"), "{st}");
            }
        }
        // Asked again with what it cached: the same answer, and the
        // pairs whose relative pose didn't change were never intersected
        // again (the cache holds one verdict per distinct pose).
        let cached = s.interference.len();
        assert_eq!(swept(&mut s, "swing", turning("45 deg"), 46, &opts), v);
        assert_eq!(s.interference.len(), cached);
    }

    #[test]
    fn without_interference_nothing_is_intersected() {
        let (_dir, mut s) = swing(true);
        let v = swept(&mut s, "swing", turning("45 deg"), 46, &Options::default());
        assert!(s.interference.is_empty());
        assert!(
            v.get("at_rest").is_none() && v["first_collision"].is_null(),
            "{v}"
        );
        assert!(v["steps"][30].get("collisions").is_none());
    }

    #[test]
    fn clearance_follows_the_closest_moving_pair() {
        let (_dir, mut s) = swing(false);
        let opts = Options {
            clearance: true,
            below: Some(0.05),
            ..Options::default()
        };
        let v = swept(&mut s, "swing", turning("20 deg"), 3, &opts);
        let c = &v["steps"][0]["clearance"];
        assert_eq!(
            names(&c["pair"]),
            BTreeSet::from(["swing/arm/body".to_owned(), "swing/block/body".to_owned()]),
            "{v}"
        );
        assert!(!v["steps"][2]["clearance"].is_null());
    }

    #[test]
    fn a_cancelled_sweep_says_so() {
        let (_dir, mut s) = swing(false);
        let f = file(&s, "swing", turning("45 deg"), 46);
        let r = resolved(&mut s, "swing", &f).unwrap();
        let pid = id("swing");
        let part = s.project().part(&pid).unwrap().clone();
        let result = s.rebuilt(&pid).unwrap().clone();
        let model = s.model(&pid, &crate::doc::assembly::Rev::Working).unwrap();
        let input = Input {
            asm: &pid,
            part: &part,
            result: &result,
            model: &model,
            unit: crate::units::Unit::Mm,
        };
        let caches = Caches {
            interference: &mut s.interference,
            clearance: &mut s.clearance,
        };
        let e = sweep(
            &input,
            None,
            &r,
            &Options::default(),
            caches,
            &Job::cancelled_at(0),
        )
        .unwrap_err();
        assert_eq!(e.code, Code::Cancelled);
    }

    #[test]
    fn the_first_collision_is_refined_well_below_a_step() {
        let (_dir, mut s) = swing(false);
        let opts = Options {
            interference: true,
            refine: 10,
            ..Options::default()
        };
        let v = swept(&mut s, "swing", turning("45 deg"), 46, &opts);
        let contact = &v["first_collision"]["contact"];
        let [lo, hi] = [0, 1].map(|i| contact["bracket"][i].as_f64().unwrap());
        // The true contact, as a fraction of the 45° sweep.
        let theta = ryukei::math::atan2(20.0, 40.0) - ryukei::math::asin(4.0 / 2000f64.sqrt());
        let truth = theta / (45f64.to_radians());
        // A corner sunk less than interference's touch reach (#558: up to
        // a micrometre or two) touches, so the first collision is found
        // just past the geometric contact: the corner, √2000 mm from the
        // axis, sunk at most 2 µm.
        let reach = 2e-6 / (2000f64.sqrt() * 1e-3) / 45f64.to_radians();
        assert!(truth <= hi && lo <= truth + reach, "{lo} {truth} {hi}");
        // Ten halvings of one step (1/45).
        assert!(hi - lo <= 1.0 / 45.0 / 1024.0 + 1e-15, "{}", hi - lo);
        assert_eq!(contact["t"].as_f64().unwrap(), hi);
        let deg = contact["values"]["drives/0"]["angle_deg"].as_f64().unwrap();
        assert!(
            deg >= theta.to_degrees() && deg - theta.to_degrees() < 1.0 / 1024.0 + reach * 45.0,
            "{deg}"
        );
        // With no refining, no contact.
        let plain = swept(
            &mut s,
            "swing",
            turning("45 deg"),
            46,
            &Options {
                interference: true,
                ..Options::default()
            },
        );
        assert!(plain["first_collision"].get("contact").is_none());
    }

    #[test]
    fn the_first_stop_is_refined_too() {
        let (_dir, mut s) = stopped();
        let opts = Options {
            refine: 10,
            ..Options::default()
        };
        let v = swept(
            &mut s,
            "frame",
            json!([{"mate": "m1", "angle": {"from": "0 deg", "to": "10 deg"}}]),
            11,
            &opts,
        );
        let b = &v["first_blocked"];
        assert_eq!(b["index"], 7, "{v}");
        assert_eq!(b["limit"], json!({"mate": "m1", "at": "max"}));
        let deg = b["stop"]["values"]["m1"]["angle_deg"].as_f64().unwrap();
        assert!(deg > 6.0 && deg - 6.0 <= 1.0 / 1024.0 + 1e-12, "{deg}");
        assert_eq!(
            v["limits_reached"][0],
            json!({"mate": "m1", "value": "angle", "at": "max", "index": 6})
        );
    }

    #[test]
    fn assembly_sweep_answers_saved_and_inline_alike_and_caches() {
        let (_dir, mut s) = stopped();
        let drives = json!([{"mate": "m1", "angle": {"from": "-3 deg", "to": "3 deg"}}]);
        call(
            &mut s,
            "motion.save",
            json!({"part": "frame", "motion": "swing", "drives": drives, "steps": 7}),
        )
        .unwrap();
        let saved = call(
            &mut s,
            "assembly.sweep",
            json!({"part": "frame", "motion": "swing"}),
        )
        .unwrap();
        assert_eq!(saved["motion"], "swing");
        assert!(saved["revision"].is_string());
        assert_eq!(saved["steps"].as_array().unwrap().len(), 7);
        let inline = call(
            &mut s,
            "assembly.sweep",
            json!({"part": "frame", "drives": drives, "steps": 7}),
        )
        .unwrap();
        assert_eq!(inline["motion"], Value::Null);
        assert_eq!(inline["steps"], saved["steps"]);
        // The shorthand: a revolute's angle.
        let short = call(
            &mut s,
            "assembly.sweep",
            json!({"part": "frame", "mate": "m1", "from": "-3 deg", "to": "3 deg", "steps": 7}),
        )
        .unwrap();
        assert_eq!(short["steps"], saved["steps"]);
        // Asked again: from the session's cache.
        assert_eq!(s.sweeps.len(), 2);
        let again = call(
            &mut s,
            "assembly.sweep",
            json!({"part": "frame", "motion": "swing"}),
        )
        .unwrap();
        assert_eq!(again, saved);
        assert_eq!(s.sweeps.len(), 2);
        // Refusals.
        for (p, code) in [
            (json!({"part": "frame"}), Code::InvalidParam),
            (
                json!({"part": "frame", "motion": "swing", "drives": drives}),
                Code::InvalidParam,
            ),
            (
                json!({"part": "frame", "motion": "nope"}),
                Code::MotionNotFound,
            ),
            (
                json!({"part": "plate", "drives": drives}),
                Code::WrongPartKind,
            ),
            (
                json!({"part": "frame", "drives": drives, "refine": 31}),
                Code::InvalidParam,
            ),
            (
                json!({"part": "frame", "drives": drives, "clearance": true, "below": "-1 mm"}),
                Code::InvalidParam,
            ),
        ] {
            assert_eq!(
                call(&mut s, "assembly.sweep", p.clone()).unwrap_err().code,
                code,
                "{p}"
            );
        }
    }

    #[test]
    fn a_sweep_of_an_assembly_that_does_not_hold_is_refused() {
        // `hinged` with a second mate fastening the pin 20 mm from the
        // hinge's axis: the two can't both hold, so the solve doesn't
        // converge (a mate whose connector doesn't resolve never reaches
        // the solver, so `hinged_with_failing_mate` still settles).
        let (dir, mut s) = crate::mate::tests::hinged();
        let top = crate::mate::tests::face(&mut s, "plate", "slab", [0.0, 0.0, 1.0]);
        let foot = crate::mate::tests::face(&mut s, "bolt", "shank", [0.0, 0.0, -1.0]);
        crate::mate::tests::add_in(
            dir.path(),
            "frame",
            "m2",
            "mate",
            json!({"form": "fastened",
                   "a": {"instance": ["base"], "at": {"feature": "slab", "name": top},
                         "offset": {"origin": ["40 mm", "10 mm", "0 mm"]}},
                   "b": {"instance": ["pin"], "at": {"feature": "shank", "name": foot},
                         "offset": {"flip": true}}}),
        );
        s.refresh().unwrap();
        assert!(
            !s.rebuilt(&id("frame"))
                .unwrap()
                .solve
                .as_ref()
                .unwrap()
                .report
                .converged
        );
        let e = call(
            &mut s,
            "assembly.sweep",
            json!({"part": "frame", "mate": "m1", "from": "0 deg", "to": "1 deg"}),
        )
        .unwrap_err();
        assert_eq!(e.code, Code::AssemblyUnsolved);
    }
}
