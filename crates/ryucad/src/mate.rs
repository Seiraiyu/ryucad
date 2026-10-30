//! Solving an assembly's mates. Once its instances are placed at their
//! stored poses, the assembly's mates and instance patterns are settled
//! together: every connector is resolved in the models its instances
//! place ([`crate::connector`]), the instances become rigid bodies for
//! `ryumate` (fixed ones fixed), pattern copies become its copies, and
//! the solve gives each instance its pose.
//!
//! Poses that already hold come back with the same bits and the assembly
//! is rebuilt from them as stored. Otherwise it is *stale*: rebuilt from
//! poses solved in memory, with an `assembly_stale` warning, until a write
//! (or `doctor`'s repair) stores them. A component whose mates can't all
//! hold keeps its stored poses, and its mates fail with
//! `assembly_unsolved`, naming the conflicting mates.
//!
//! A subassembly was settled in its own rebuild, so it enters as one
//! rigid body whatever its own mates leave free inside it.

use crate::assembly::{Placed, placed_at, pose_rigid};
use crate::connector::{Found, Scope, doc_pose, pose, resolve};
use crate::doc::files::Feature;
use crate::doc::id::Id;
use crate::doc::mate::{Direction, Form, InstancePattern, Mate, PatternForm};
use crate::doc::project::Part;
use crate::error::{Code, Error, Result};
use crate::rebuild::Output;
use core::f64::consts::TAU;
use serde_json::{Value, json};
use std::collections::BTreeMap;
use std::sync::Arc;

/// What a mate rebuilds to: its state in the solve, and both connectors'
/// frames in the assembly as solved.
#[derive(Clone, Debug)]
pub struct Mated {
    pub state: ryumate::MateState,
    pub a: ryumate::Pose,
    pub b: ryumate::Pose,
    pub hash: String,
}

/// One copy an instance pattern places: its index, the seed it copies,
/// and the seed's part placed where the copy is.
#[derive(Clone, Debug)]
pub struct Copy {
    pub index: [u32; 2],
    pub seed: Id,
    pub placed: Arc<Placed>,
}

/// What an instance pattern rebuilds to: its copies, in order (by copy,
/// then seed), and a hash of them.
#[derive(Clone, Debug)]
pub struct Copies {
    pub copies: Vec<Copy>,
    pub hash: String,
}

impl Mated {
    /// How a rebuild report gives it: its state and both connectors'
    /// frames in the assembly, as poses.
    pub fn to_json(&self) -> Value {
        json!({
            "state": mate_state(self.state),
            "frames": {"a": doc_pose(&self.a).to_json(), "b": doc_pose(&self.b).to_json()},
        })
    }
}

impl Copies {
    /// How a rebuild report gives them, for pattern `id`: each copy's
    /// path step, seed and pose.
    pub fn to_json(&self, id: &Id) -> Value {
        json!(
            self.copies
                .iter()
                .map(|c| json!({
                    "copy": Id::copy(id, c.index),
                    "seed": c.seed,
                    "pose": crate::assembly::rigid_pose(&c.placed.pose).to_json(),
                }))
                .collect::<Vec<_>>()
        )
    }
}

/// What an assembly's solve found: ryumate's report, with bodies and mates
/// by the instances' and mates' IDs, and whether the stored poses held.
#[derive(Clone, Debug)]
pub struct Solve {
    pub report: ryumate::Report,
    /// The instance each body is, by its index.
    pub instances: Vec<Id>,
    /// The mate each of ryumate's mates is, by its index.
    pub mates: Vec<Id>,
    /// The instances solved off their stored poses.
    pub moved: Vec<Id>,
    /// What was solved, with each body where the rebuild placed it: what
    /// a preview of a drag solves again, in part ([`component_request`]).
    pub asm: ryumate::Assembly,
}

impl Solve {
    /// Whether the assembly was rebuilt from poses solved in memory.
    pub fn stale(&self) -> bool {
        !self.moved.is_empty()
    }

    pub fn state_str(&self) -> &'static str {
        match self.report.state {
            ryumate::State::Under => "under",
            ryumate::State::Well => "well",
            ryumate::State::Over => "over",
        }
    }

    fn mate_ids(&self, sets: &[Vec<u32>]) -> Vec<Vec<&Id>> {
        sets.iter()
            .map(|s| s.iter().map(|m| &self.mates[*m as usize]).collect())
            .collect()
    }

    /// How a rebuild report gives it: whether it held and was stale, its
    /// state and freedom, each instance's state, freedom and free motions,
    /// each mate's state, and the redundant and conflicting sets.
    pub fn to_json(&self) -> Value {
        let r = &self.report;
        let instances: serde_json::Map<String, Value> = r
            .bodies
            .iter()
            .map(|(b, rep)| (self.instances[*b as usize].to_string(), body_json(rep)))
            .collect();
        let mates: serde_json::Map<String, Value> = r
            .mates
            .iter()
            .map(|(m, s)| (self.mates[*m as usize].to_string(), json!(mate_state(*s))))
            .collect();
        json!({
            "converged": r.converged,
            "stale": self.stale(),
            "moved": self.moved,
            "state": self.state_str(),
            "dof": r.dof,
            "instances": instances,
            "mates": mates,
            "redundant": self.mate_ids(&r.redundant),
            "conflicting": self.mate_ids(&r.conflicting),
        })
    }

    /// The report of instance `id`, if it took part.
    pub fn instance(&self, id: &Id) -> Option<&ryumate::BodyReport> {
        let k = self.instances.iter().position(|i| i == id)?;
        self.report.bodies.get(&(k as u32))
    }
}

/// An instance's state and freedom, as reports give it.
pub fn body_json(rep: &ryumate::BodyReport) -> Value {
    let state = match rep.state {
        ryumate::BodyState::Well => "well",
        ryumate::BodyState::Under => "under",
        ryumate::BodyState::Over => "over",
        ryumate::BodyState::Failing => "failing",
    };
    let f = &rep.freedom;
    json!({
        "state": state,
        "dof": f.dof,
        "free": {
            "translate": f.translations,
            "rotate": f.rotations.iter()
                .map(|a| json!({"axis": a.direction, "through_m": a.through}))
                .collect::<Vec<_>>(),
        },
    })
}

pub fn mate_state(s: ryumate::MateState) -> &'static str {
    match s {
        ryumate::MateState::Well => "well",
        ryumate::MateState::Redundant => "redundant",
        ryumate::MateState::Conflicting => "conflicting",
        ryumate::MateState::Failing => "failing",
    }
}

/// `assembly_stale`: assembly `part` was rebuilt from poses solved in
/// memory.
pub fn stale_warning(part: &Id, moved: &[Id]) -> Value {
    json!({
        "code": "assembly_stale",
        "part": part,
        "instances": moved,
        "message": format!(
            "assembly {part}: its stored poses don't satisfy its mates; rebuilt from poses \
             solved in memory (run `ryucad doctor` to write them)"
        ),
    })
}

fn hash_pose(h: &mut blake3::Hasher, p: &ryumate::Pose) {
    for x in p.rotation.iter().flatten().chain(&p.origin) {
        h.update(&x.to_bits().to_le_bytes());
    }
}

/// The solver's kind for a mate.
fn kind(m: &Mate) -> ryumate::Kind {
    let angle = m.angle.as_ref().map(|q| q.radians());
    let distance = m.distance.as_ref().map(|q| q.meters());
    match m.form {
        Form::Fastened => ryumate::Kind::Fastened,
        Form::Revolute => ryumate::Kind::Revolute { angle },
        Form::Slider => ryumate::Kind::Slider { distance },
        Form::Cylindrical => ryumate::Kind::Cylindrical { angle, distance },
        Form::Planar => ryumate::Kind::Planar {
            angle,
            distance: distance.unwrap_or(0.0),
        },
        Form::Ball => ryumate::Kind::Ball,
    }
}

/// A pattern as far as it resolved: the solver's pattern (seeds by body
/// index), and the seeds by ID.
struct Resolved {
    pattern: ryumate::Pattern,
    seeds: Vec<Id>,
    copies: Vec<[u32; 2]>,
}

/// The assembly's instances and patterns as connectors are resolved in
/// them.
struct In<'a> {
    features: &'a BTreeMap<Id, Result<Output>>,
    patterns: &'a BTreeMap<Id, Result<Resolved>>,
}

impl Scope for In<'_> {
    fn instance(&self, id: &Id) -> Option<core::result::Result<&Placed, &Error>> {
        match self.features.get(id)? {
            Ok(o) => o.instance().map(Ok),
            Err(e) => Some(Err(e)),
        }
    }

    fn places(&self, pattern: &Id, seed: &Id, index: [u32; 2]) -> Option<Result<bool>> {
        Some(match self.patterns.get(pattern)? {
            Ok(r) => Ok(r.seeds.contains(seed) && r.copies.contains(&index)),
            Err(e) => Err(e.clone()),
        })
    }
}

/// The solver's direction for one of a pattern's, with its role.
fn direction(
    owner: &Id,
    role: &str,
    d: &Direction,
    scope: &In,
    body: &dyn Fn(&Id) -> u32,
) -> Result<ryumate::Direction> {
    match d {
        Direction::Origin(a) => Ok(ryumate::Direction::World(a.vector())),
        Direction::Connector(c) => {
            let f = resolve(owner, role, c, scope)?;
            if f.copy.is_some() {
                return Err(Error::new(
                    Code::InvalidParam,
                    format!("{owner}: its {role} is on a pattern's copy; put it on an instance"),
                )
                .with("feature", owner)
                .with("role", role));
            }
            frame_ok(owner, role, &f.frame)?;
            Ok(ryumate::Direction::Connector(ryumate::Connector {
                on: ryumate::On::Body(body(&f.instance)),
                frame: f.frame,
            }))
        }
    }
}

/// Resolves pattern `id`: its seeds, which must be instances listed
/// before it that rebuilt, and its directions or axis.
fn pattern(
    part: &Part,
    id: &Id,
    p: &InstancePattern,
    scope: &In,
    body: &dyn Fn(&Id) -> u32,
) -> Result<Resolved> {
    for seed in &p.instances {
        let o = crate::graph::input(part, id, seed, scope.features)?;
        if o.instance().is_none() {
            return Err(Error::new(
                Code::ReferenceWrongKind,
                format!("{id} copies {seed}, which isn't an instance"),
            )
            .with("feature", id)
            .with("seed", seed));
        }
    }
    pattern_values(id, p)?;
    let spacing = |w: &crate::doc::mate::Way| {
        let s = w.spacing.meters();
        if w.reverse { -s } else { s }
    };
    let motion = match &p.form {
        PatternForm::Linear { first, second } => ryumate::Motion::Linear {
            direction: direction(id, "direction", &first.direction, scope, body)?,
            spacing: spacing(first),
            second: match second {
                Some(w) => Some((
                    direction(id, "direction2", &w.direction, scope, body)?,
                    spacing(w),
                )),
                None => None,
            },
        },
        PatternForm::Circular { axis, count, angle } => {
            let n = count.number();
            let a = angle.radians();
            let whole = a >= TAU - ryukei::tol::ANGULAR_RESOLUTION;
            let step = match (whole, n > 1.0) {
                (true, _) => TAU / n,
                (false, true) => a / (n - 1.0),
                (false, false) => 0.0,
            };
            ryumate::Motion::Circular {
                axis: direction(id, "axis", axis, scope, body)?,
                step,
            }
        }
    };
    Ok(Resolved {
        pattern: ryumate::Pattern {
            seeds: p.instances.iter().map(body).collect(),
            motion,
        },
        seeds: p.instances.clone(),
        copies: p.copies(),
    })
}

/// The solver's connector for a resolved one.
fn on(f: &Found, body: &dyn Fn(&Id) -> u32, pattern: &dyn Fn(&Id) -> u32) -> ryumate::Connector {
    let on = match &f.copy {
        None => ryumate::On::Body(body(&f.instance)),
        Some((p, index)) => ryumate::On::Copy {
            pattern: pattern(p),
            seed: body(&f.instance),
            index: *index,
        },
    };
    ryumate::Connector { on, frame: f.frame }
}

/// An assembly's rebuild, as connectors are resolved in it when a write
/// checks them.
struct Rebuilt<'a>(&'a crate::rebuild::PartResult);

impl Scope for Rebuilt<'_> {
    fn instance(&self, id: &Id) -> Option<core::result::Result<&Placed, &Error>> {
        match self
            .0
            .features
            .iter()
            .find(|(f, _)| f == id)
            .map(|(_, r)| r)?
        {
            Ok(o) => o.instance().map(Ok),
            Err(e) => Some(Err(e)),
        }
    }

    fn places(&self, pattern: &Id, seed: &Id, index: [u32; 2]) -> Option<Result<bool>> {
        match self
            .0
            .features
            .iter()
            .find(|(f, _)| f == pattern)
            .map(|(_, r)| r)?
        {
            Ok(o) => {
                let c = o.copies()?;
                Some(Ok(c
                    .copies
                    .iter()
                    .any(|c| c.index == index && c.seed == *seed)))
            }
            Err(e) => Some(Err(e.clone())),
        }
    }
}

/// Refuses a connector `role` of feature `id` whose frame `ryumate` can't
/// take: its origin, in its instance's coordinates (offsets and the
/// occurrences a path goes through included), past the size box.
fn frame_ok(id: &Id, role: &str, frame: &ryumate::Pose) -> Result<()> {
    let half = ryukei::tol::SIZE_BOX_HALF;
    if frame.origin.iter().all(|x| x.abs() <= half) {
        return Ok(());
    }
    Err(Error::new(
        Code::InvalidParam,
        format!("{id}: its {role} lies outside the size box (±{half} m) of its instance"),
    )
    .with("feature", id)
    .with("param", role)
    .with("origin_m", frame.origin.to_vec()))
}

/// Refuses length `param` of feature `id`, `v` meters, that the solver
/// can't drive: not finite or past [`ryumate::MAX_LENGTH`].
fn length_ok(id: &Id, param: &str, v: f64) -> Result<()> {
    if v.is_finite() && v.abs() <= ryumate::MAX_LENGTH {
        return Ok(());
    }
    Err(Error::new(
        Code::InvalidParam,
        format!(
            "{id}: its {param} must be finite and at most {} m",
            ryumate::MAX_LENGTH
        ),
    )
    .with("feature", id)
    .with("param", param))
}

/// Refuses angle `param` of feature `id` that isn't finite.
fn angle_ok(id: &Id, param: &str, a: f64) -> Result<()> {
    if a.is_finite() {
        return Ok(());
    }
    Err(Error::new(
        Code::InvalidParam,
        format!("{id}: its {param} isn't finite"),
    )
    .with("feature", id)
    .with("param", param))
}

/// Refuses mate `id`'s values the solver can't take, as `ryumate`'s
/// validation would: its distance and angle.
fn mate_values(id: &Id, m: &Mate) -> Result<()> {
    if let Some(d) = &m.distance {
        length_ok(id, "distance", d.meters())?;
    }
    if let Some(a) = &m.angle {
        angle_ok(id, "angle", a.radians())?;
    }
    Ok(())
}

/// Refuses instance pattern `id`'s values the solver can't take: its
/// spacings and angle.
fn pattern_values(id: &Id, p: &InstancePattern) -> Result<()> {
    match &p.form {
        PatternForm::Linear { first, second } => {
            length_ok(id, "spacing", first.spacing.meters())?;
            if let Some(w) = second {
                length_ok(id, "spacing2", w.spacing.meters())?;
            }
        }
        PatternForm::Circular { angle, .. } => angle_ok(id, "angle", angle.radians())?,
    }
    Ok(())
}

/// The error for feature `id` when the solve would put `what` (an
/// instance, or a copy of a pattern's seed) where it can't be placed,
/// which `e` says: outside the size box.
fn outside(id: &Id, what: String, e: &Error) -> Error {
    Error::new(
        Code::InvalidParam,
        format!("{id}: the solve would put {what} where it can't be placed: {e}"),
    )
    .with("feature", id)
}

/// Refuses mate or instance pattern `f`, which a write would make feature
/// `id` of assembly `part` with the list `order`, unless what it names
/// comes before it and its connectors resolve in the assembly as it
/// rebuilds now; an instance pattern's seeds must be instances. What the
/// solve makes of it only rebuild tells.
pub fn check(
    session: &mut crate::session::Session,
    part: &Id,
    order: &[Id],
    id: &Id,
    f: &Feature,
) -> Result<()> {
    let p = session.project().part(part)?.clone();
    crate::graph::check_order(&p, order, id, f)?;
    if let Some(x) = f.instance_pattern() {
        for seed in &x.instances {
            if !matches!(p.features.get(seed), Some(Ok(Feature::Instance(_)))) {
                return Err(Error::new(
                    Code::ReferenceWrongKind,
                    format!("{id} copies {seed}, which isn't an instance"),
                )
                .with("feature", id)
                .with("seed", seed));
            }
        }
    }
    if let Some(m) = f.mate() {
        mate_values(id, m)?;
    }
    if let Some(x) = f.instance_pattern() {
        pattern_values(id, x)?;
    }
    let r = session.rebuilt(part)?.clone();
    let scope = Rebuilt(&r);
    let mut on = Vec::new();
    for (role, c) in f.connectors() {
        let found = resolve(id, role, c, &scope)?;
        frame_ok(id, role, &found.frame)?;
        on.push((found.instance.clone(), found.copy.clone()));
        if f.instance_pattern().is_some() && found.copy.is_some() {
            return Err(Error::new(
                Code::InvalidParam,
                format!("{id}: its {role} is on a pattern's copy; put it on an instance"),
            )
            .with("feature", id)
            .with("role", role));
        }
    }
    if f.mate().is_some() && on[0] == on[1] {
        return Err(Error::new(
            Code::InvalidParam,
            format!(
                "{id}: both its connectors are on {}; a mate joins two instances",
                on[0].0
            ),
        )
        .with("feature", id)
        .with("param", "b"));
    }
    Ok(())
}

/// Settles assembly `part`'s mates and instance patterns over the
/// instances already placed in `features` (each listed feature's result
/// by ID), in place: instances get their solved poses, mates and patterns
/// their results. Returns the solve, if the assembly has anything to
/// solve.
pub fn settle(part: &Part, features: &mut BTreeMap<Id, Result<Output>>) -> Option<Arc<Solve>> {
    let order = part.order();
    let listed = |pick: fn(&Feature) -> bool| -> Vec<Id> {
        order
            .iter()
            .filter(|id| {
                part.features
                    .get(*id)
                    .and_then(|f| f.as_ref().ok())
                    .is_some_and(pick)
            })
            .cloned()
            .collect()
    };
    let instances: Vec<Id> = listed(|f| f.instance().is_some())
        .into_iter()
        .filter(|id| matches!(features.get(id), Some(Ok(o)) if o.instance().is_some()))
        .collect();
    let mate_ids = listed(|f| f.mate().is_some());
    let pattern_ids = listed(|f| f.instance_pattern().is_some());
    if mate_ids.is_empty() && pattern_ids.is_empty() {
        return None;
    }
    let body_of = |id: &Id| -> u32 {
        instances
            .iter()
            .position(|i| i == id)
            .expect("a resolved connector is on an instance that placed") as u32
    };
    let pattern_of = |id: &Id| -> u32 {
        pattern_ids
            .iter()
            .position(|i| i == id)
            .expect("a resolved copy is of a listed pattern") as u32
    };
    let feature = |id: &Id| part.features[id].as_ref().expect("listed as loaded");
    // Patterns first: mates may name their copies.
    let mut patterns: BTreeMap<Id, Result<Resolved>> = BTreeMap::new();
    for id in &pattern_ids {
        let p = feature(id).instance_pattern().expect("a pattern");
        let scope = In {
            features,
            patterns: &patterns,
        };
        let r = pattern(part, id, p, &scope, &body_of);
        patterns.insert(id.clone(), r);
    }
    let mut asm = ryumate::Assembly::default();
    for (k, id) in instances.iter().enumerate() {
        let p = features[id]
            .as_ref()
            .ok()
            .and_then(|o| o.instance())
            .expect("placed");
        asm.bodies.insert(
            k as u32,
            ryumate::Body {
                pose: pose(&p.pose),
                fixed: p.fixed,
            },
        );
    }
    for (id, r) in &patterns {
        if let Ok(r) = r {
            asm.patterns.insert(pattern_of(id), r.pattern.clone());
        }
    }
    // Mates whose connectors resolve, by index, with what they resolved
    // to; the rest fail now.
    let mut mated: BTreeMap<Id, Result<(u32, [Found; 2])>> = BTreeMap::new();
    let mut solved_mates: Vec<Id> = Vec::new();
    {
        let scope = In {
            features,
            patterns: &patterns,
        };
        for id in &mate_ids {
            let m = feature(id).mate().expect("a mate");
            let r = (|| -> Result<(u32, [Found; 2])> {
                // What it names comes before it; what it is, resolving says.
                crate::graph::check_order(part, order, id, feature(id))?;
                mate_values(id, m)?;
                let a = resolve(id, "a", &m.a, &scope)?;
                let b = resolve(id, "b", &m.b, &scope)?;
                frame_ok(id, "a", &a.frame)?;
                frame_ok(id, "b", &b.frame)?;
                let k = solved_mates.len() as u32;
                Ok((k, [a, b]))
            })();
            if let Ok((_, [a, b])) = &r {
                asm.mates.insert(
                    solved_mates.len() as u32,
                    ryumate::Mate {
                        kind: kind(m),
                        a: on(a, &body_of, &pattern_of),
                        b: on(b, &body_of, &pattern_of),
                    },
                );
                solved_mates.push(id.clone());
            }
            mated.insert(id.clone(), r);
        }
    }
    let mut solved = match ryumate::solve(&asm) {
        Ok(s) => s,
        Err(e) => {
            // Every value a file or a path sets was checked above, feature
            // by feature, so a refusal is ryucad's fault. What the solve
            // would have settled fails with it; the rest keeps its own
            // error.
            let refused = |id: &Id| {
                Error::new(
                    Code::Internal,
                    format!("{id}: the solver refused the assembly: {e}"),
                )
                .with("feature", id)
            };
            for (id, r) in mated {
                let out: Result<Output> = r.and_then(|_| Err(refused(&id)));
                features.insert(id, out);
            }
            for (id, r) in patterns {
                let out: Result<Output> = r.and_then(|_| Err(refused(&id)));
                features.insert(id, out);
            }
            return None;
        }
    };
    // Instances: solved poses, but where their mates fail, as stored. One
    // the solve would put where it can't be placed stays as stored and
    // fails, with its mates.
    let mut moved = Vec::new();
    let mut unplaced: BTreeMap<u32, Error> = BTreeMap::new();
    let mut at = asm.clone();
    for (k, id) in instances.iter().enumerate() {
        let k = k as u32;
        let failing = solved.report.bodies[&k].state == ryumate::BodyState::Failing;
        let new = solved.poses[&k];
        if failing || new == asm.bodies[&k].pose {
            continue;
        }
        let rigid = match pose_rigid(&doc_pose(&new)) {
            Ok(r) => r,
            Err(e) => {
                solved.report.bodies.get_mut(&k).expect("a body").state =
                    ryumate::BodyState::Failing;
                solved.report.converged = false;
                unplaced.insert(k, e);
                continue;
            }
        };
        let old = features[id]
            .as_ref()
            .ok()
            .and_then(|o| o.instance())
            .expect("placed");
        let p = placed_at(old, rigid);
        // The solver's own frames from here: the pose as a file would
        // hold it.
        at.bodies.get_mut(&k).expect("a body").pose = pose(&p.pose);
        features.insert(id.clone(), Ok(Output::Instance(Arc::new(p))));
        moved.push(id.clone());
    }
    let report = &solved.report;
    // Every copy a pattern places or a mate names, where the bodies now
    // are, in one call.
    let mut wanted: Vec<(u32, u32, [u32; 2])> = Vec::new();
    for (id, r) in &patterns {
        if let Ok(r) = r {
            let k = pattern_of(id);
            for index in &r.copies {
                for seed in &r.seeds {
                    wanted.push((k, body_of(seed), *index));
                }
            }
        }
    }
    for m in asm.mates.values() {
        for c in [&m.a, &m.b] {
            if let ryumate::On::Copy {
                pattern,
                seed,
                index,
            } = c.on
            {
                wanted.push((pattern, seed, index));
            }
        }
    }
    wanted.sort_unstable();
    wanted.dedup();
    let copy_at: BTreeMap<(u32, u32, [u32; 2]), ryumate::Pose> = wanted
        .iter()
        .copied()
        .zip(ryumate::copy_poses(&at, &wanted))
        .collect();
    // What a mate's connector is on, by name, if the solve put it where it
    // can't be placed.
    let unplaceable = |c: &ryumate::Connector| -> Option<(String, Error)> {
        match c.on {
            ryumate::On::Body(b) => unplaced
                .get(&b)
                .map(|e| (instances[b as usize].to_string(), e.clone())),
            ryumate::On::Copy {
                pattern,
                seed,
                index,
            } => {
                if let Some(e) = unplaced.get(&seed) {
                    return Some((instances[seed as usize].to_string(), e.clone()));
                }
                pose_rigid(&doc_pose(&copy_at[&(pattern, seed, index)]))
                    .err()
                    .map(|e| {
                        (
                            format!(
                                "copy {index:?} of {} in {}",
                                instances[seed as usize], pattern_ids[pattern as usize]
                            ),
                            e,
                        )
                    })
            }
        }
    };
    // Mates: their state, and their frames where the bodies now are.
    let frame_at = |c: &ryumate::Connector| -> ryumate::Pose {
        match c.on {
            ryumate::On::Body(b) => at.bodies[&b].pose.then_after(&c.frame),
            ryumate::On::Copy {
                pattern,
                seed,
                index,
            } => copy_at[&(pattern, seed, index)].then_after(&c.frame),
        }
    };
    let conflicting = |m: u32| -> Vec<Vec<&Id>> {
        report
            .conflicting
            .iter()
            .filter(|s| s.contains(&m))
            .map(|s| s.iter().map(|x| &solved_mates[*x as usize]).collect())
            .collect()
    };
    for (id, r) in mated {
        let out = r.map(|(k, _)| {
            let state = report.mates[&k];
            let m = &asm.mates[&k];
            let off = unplaceable(&m.a).or_else(|| unplaceable(&m.b));
            (k, state, off, frame_at(&m.a), frame_at(&m.b))
        });
        let out = match out {
            Err(e) => Err(e),
            Ok((_, _, Some((what, e)), _, _)) => {
                Err(outside(&id, what.clone(), &e).with("unplaced", what))
            }
            Ok((k, ryumate::MateState::Conflicting | ryumate::MateState::Failing, _, _, _)) => {
                let sets = conflicting(k);
                let msg = if sets.is_empty() {
                    format!("{id}: the solver couldn't make it hold with the mates beside it")
                } else {
                    format!("{id} can't hold together with the mates it conflicts with")
                };
                Err(Error::new(Code::AssemblyUnsolved, msg)
                    .with("feature", &id)
                    .with("part", &part.id)
                    .with("conflicting", sets))
            }
            Ok((_, state, None, a, b)) => {
                let mut h = blake3::Hasher::new();
                h.update(mate_state(state).as_bytes());
                hash_pose(&mut h, &a);
                hash_pose(&mut h, &b);
                Ok(Output::Mate(Arc::new(Mated {
                    state,
                    a,
                    b,
                    hash: h.finalize().to_hex()[..32].to_owned(),
                })))
            }
        };
        features.insert(id, out);
    }
    // Patterns: their copies where the seeds now are; a copy that can't be
    // placed there fails its pattern, naming it.
    for (id, r) in patterns {
        let out = r.and_then(|r| {
            let k = pattern_of(&id);
            let mut copies = Vec::new();
            let mut h = blake3::Hasher::new();
            for index in &r.copies {
                for seed in &r.seeds {
                    let p = copy_at[&(k, body_of(seed), *index)];
                    let seed_placed = features[seed]
                        .as_ref()
                        .ok()
                        .and_then(|o| o.instance())
                        .expect("a seed placed");
                    let rigid = pose_rigid(&doc_pose(&p)).map_err(|e| {
                        outside(&id, format!("copy {index:?} of {seed}"), &e)
                            .with("copy", index)
                            .with("seed", seed)
                    })?;
                    let placed = placed_at(seed_placed, rigid);
                    h.update(format!("{index:?} {seed} {}\n", placed.hash).as_bytes());
                    copies.push(Copy {
                        index: *index,
                        seed: seed.clone(),
                        placed: Arc::new(placed),
                    });
                }
            }
            Ok(Output::Copies(Arc::new(Copies {
                copies,
                hash: h.finalize().to_hex()[..32].to_owned(),
            })))
        });
        features.insert(id, out);
    }
    Some(Arc::new(Solve {
        report: solved.report,
        instances,
        mates: solved_mates,
        moved,
        asm: at,
    }))
}

/// An assembly solved after moves: its rebuild as proposed, whether the
/// moved instances could be held where they were put, and the moves.
pub struct Moved {
    pub result: crate::rebuild::PartResult,
    pub held: bool,
    pub moves: Vec<(Id, crate::doc::assembly::Pose)>,
}

impl Moved {
    /// Whether the mates hold.
    pub fn converged(&self) -> bool {
        self.result
            .solve
            .as_ref()
            .is_none_or(|s| s.report.converged)
    }

    /// The pose each instance of `p` (the assembly as it is) takes: a
    /// moved one held where it was put, the rest where the solve put them.
    pub fn poses(&self, p: &Part) -> BTreeMap<Id, crate::doc::assembly::Pose> {
        let mut out = BTreeMap::new();
        for (id, r) in &self.result.features {
            let Some(placed) = r.as_ref().ok().and_then(|o| o.instance()) else {
                continue;
            };
            let held = self
                .held
                .then(|| self.moves.iter().find(|(m, _)| m == id))
                .flatten();
            // An instance placed where its file says keeps the file's pose
            // to the bit: read again, a pose needn't come back the same.
            let stored = match p.features.get(id) {
                Some(Ok(Feature::Instance(i))) => Some(i.pose),
                _ => None,
            };
            let pose = match (held, stored) {
                (Some((_, pose)), _) => *pose,
                (None, Some(s)) if pose_rigid(&s).is_ok_and(|r| r == placed.pose) => s,
                (None, _) => doc_pose(&pose(&placed.pose)),
            };
            if stored.is_some() {
                out.insert(id.clone(), pose);
            }
        }
        out
    }

    /// The instances of `p` whose files change, with their new features:
    /// each moved one, and each the solve moved, as stored (`fixed` as
    /// it was).
    pub fn writes(&self, p: &Part) -> Vec<(Id, Feature)> {
        let poses = self.poses(p);
        p.order()
            .iter()
            .filter_map(|id| {
                let Some(Ok(Feature::Instance(i))) = p.features.get(id) else {
                    return None;
                };
                let moved = self.moves.iter().any(|(m, _)| m == id);
                let pose = *poses.get(id)?;
                (moved || pose != i.pose).then(|| {
                    let mut i = i.clone();
                    i.pose = pose;
                    (id.clone(), Feature::Instance(i))
                })
            })
            .collect()
    }
}

/// Assembly `part` solved with `moves` applied: first with each moved
/// instance held where it was put (fixed), as `sketch.edit` holds what it
/// moves, then, if its mates can't hold that way, with them free from
/// there. Writes nothing. A move that leaves mates unable to hold that
/// held before is `assembly_overconstrained`.
pub fn after_moves(
    session: &mut crate::session::Session,
    part: &Id,
    moves: &[(Id, crate::doc::assembly::Pose)],
) -> Result<Moved> {
    let p = session.project().part(part)?.clone();
    let before = session
        .rebuilt(part)?
        .solve
        .as_ref()
        .is_none_or(|s| s.report.converged);
    let proposed = |held: bool| -> Result<Part> {
        let mut q = p.clone();
        for (id, pose) in moves {
            let Some(Ok(Feature::Instance(i))) = q.features.get_mut(id) else {
                return Err(Error::new(
                    Code::ReferenceWrongKind,
                    format!("{id} isn't an instance of {part}"),
                )
                .with("feature", id));
            };
            i.pose = *pose;
            i.fixed |= held;
        }
        Ok(q)
    };
    let held = Moved {
        result: session.rebuilt_as(proposed(true)?)?,
        held: true,
        moves: moves.to_vec(),
    };
    if held.converged() {
        return Ok(held);
    }
    let free = Moved {
        result: session.rebuilt_as(proposed(false)?)?,
        held: false,
        moves: moves.to_vec(),
    };
    if !free.converged() && before {
        let conflicting = free
            .result
            .solve
            .as_ref()
            .map(|s| json!(s.mate_ids(&s.report.conflicting)))
            .unwrap_or(json!([]));
        return Err(Error::new(
            Code::AssemblyOverconstrained,
            format!("moved that way, {part}'s mates can't all hold"),
        )
        .with("part", part)
        .with("conflicting", conflicting));
    }
    Ok(free)
}

/// The assemblies a write to `part` may leave stale: `part` itself if it
/// is one, then every assembly that places it at `working`, at any depth,
/// nearest first.
fn reached(project: &crate::doc::project::Project, part: &Id) -> Vec<Id> {
    let assembly = |id: &Id| {
        project
            .parts
            .get(id)
            .is_some_and(|p| crate::server::part_kind(p) == crate::doc::files::PartKind::Assembly)
    };
    let mut out: Vec<Id> = Vec::new();
    if assembly(part) {
        out.push(part.clone());
    }
    let mut todo = vec![part.clone()];
    while let Some(child) = todo.pop() {
        for (id, p) in &project.parts {
            let places = p.features.values().any(|f| {
                matches!(f, Ok(Feature::Instance(i))
                    if i.part == child && i.rev == crate::doc::assembly::Rev::Working)
            });
            if places && !out.contains(id) {
                out.push(id.clone());
                todo.push(id.clone());
            }
        }
    }
    out
}

/// Of the assemblies a write to `part` may reach, those whose stored
/// poses hold now (or that have nothing to solve): the ones
/// [`keep_current`] keeps so.
pub fn current(session: &mut crate::session::Session, part: &Id) -> Result<Vec<Id>> {
    let mut out = Vec::new();
    for a in reached(session.project(), part) {
        // With nothing to solve, its poses are as stored.
        let held = session
            .rebuilt(&a)?
            .solve
            .as_ref()
            .is_none_or(|s| s.report.converged && !s.stale());
        if held {
            out.push(a);
        }
    }
    Ok(out)
}

/// After a write: writes the solved poses of each assembly of `before`,
/// current until then, that the write left stale, in the same change.
/// One whose mates now fail is left as it was. Each is reported:
/// `{"part", "ok", "moved"}`, with its error when it failed.
pub fn keep_current(session: &mut crate::session::Session, before: &[Id]) -> Result<Vec<Value>> {
    let mut out = Vec::new();
    for a in before {
        if !session.project().parts.contains_key(a) {
            continue;
        }
        let r = session.rebuilt(a)?;
        let Some(solve) = &r.solve else {
            continue;
        };
        if !solve.report.converged {
            let error = r
                .features
                .iter()
                .find_map(|(_, f)| {
                    f.as_ref()
                        .err()
                        .filter(|e| e.code == Code::AssemblyUnsolved)
                })
                .cloned();
            out.push(json!({"part": a, "ok": false, "error": error}));
            continue;
        }
        if !solve.stale() {
            continue;
        }
        // Only a stale assembly's result is copied out of the session: the
        // root's places every occurrence (100,000 at design 192's
        // benchmark), and every edit asks after it.
        let r = r.clone();
        let p = session.project().part(a)?.clone();
        let moved = Moved {
            result: r,
            held: false,
            moves: Vec::new(),
        };
        let writes = moved.writes(&p);
        let refs: Vec<(&Id, &Feature)> = writes.iter().map(|(id, f)| (id, f)).collect();
        crate::server::save(session, a, &refs, crate::server::order(&p)?)?;
        let ids: Vec<&Id> = writes.iter().map(|(id, _)| id).collect();
        out.push(json!({"part": a, "ok": true, "moved": ids}));
    }
    Ok(out)
}

/// Refuses a write that would leave assembly `proposed`'s mates unable to
/// all hold, where they hold now: `assembly_overconstrained`, with the
/// conflicting mates. One that fails already is left to fail.
pub fn refuse_overconstrained(session: &mut crate::session::Session, proposed: Part) -> Result<()> {
    let id = proposed.id.clone();
    let holds =
        |r: &crate::rebuild::PartResult| r.solve.as_ref().is_none_or(|s| s.report.converged);
    if !holds(session.rebuilt(&id)?) {
        return Ok(());
    }
    let r = session.rebuilt_as(proposed)?;
    match &r.solve {
        Some(s) if !s.report.converged => Err(Error::new(
            Code::AssemblyOverconstrained,
            format!("that would leave {id}'s mates unable to all hold"),
        )
        .with("part", &id)
        .with("conflicting", s.mate_ids(&s.report.conflicting))),
        _ => Ok(()),
    }
}

/// `assembly.solve`: assembly `part` solved as it is, or with `moves`
/// (instances and how each moves) held where they are put for a first
/// solve, writing nothing: each instance's pose, whether the moves were
/// held, and the solve's report. With `component`, only what the moved
/// instances' mates reach ([`component_request`]).
pub fn solve_request(
    session: &mut crate::session::Session,
    part: &str,
    moves: Vec<(String, crate::assembly::Move)>,
    component: bool,
) -> Result<Value> {
    let pid = crate::server::part_id(part)?;
    let unit = session.project().manifest.default_unit;
    let p = session.project().part(&pid)?.clone();
    if crate::server::part_kind(&p) != crate::doc::files::PartKind::Assembly {
        return Err(Error::new(
            Code::WrongPartKind,
            format!("{pid} is a part, not an assembly"),
        )
        .with("part", &pid));
    }
    let mut at = Vec::new();
    for (i, how) in moves {
        let iid = crate::server::feature_id(&i)?;
        let inst = crate::assembly::movable(&p, &iid)?;
        at.push((iid, crate::assembly::move_pose(&inst, how, unit, &p.env)?));
    }
    if component {
        if at.is_empty() {
            return Err(Error::new(
                Code::InvalidParam,
                "component solves what moved instances reach: name a move",
            )
            .with("pointer", "/component"));
        }
        if let Some(v) = component_request(session, &p, &at)? {
            return Ok(v);
        }
    }
    let solved = if at.is_empty() {
        Moved {
            result: session.rebuilt(&pid)?.clone(),
            held: false,
            moves: Vec::new(),
        }
    } else {
        after_moves(session, &pid, &at)?
    };
    let poses: serde_json::Map<String, Value> = solved
        .poses(&p)
        .into_iter()
        .map(|(id, pose)| {
            let moved = match p.features.get(&id) {
                Some(Ok(Feature::Instance(i))) => i.pose != pose,
                _ => false,
            };
            (
                id.to_string(),
                json!({"pose": pose.to_json(), "moved": moved}),
            )
        })
        .collect();
    let mut v = json!({"part": pid, "revision": p.revision, "converged": solved.converged(),
                       "instances": poses});
    if !at.is_empty() {
        v["held"] = json!(solved.held);
    }
    if let Some(s) = &solved.result.solve {
        v["solve"] = s.to_json();
    }
    Ok(v)
}

/// `assembly.solve` with `component`: the moved instances placed where
/// `moves` put them in what the assembly's last solve settled
/// ([`Solve::asm`]), and only what their mates reach solved again
/// (`ryumate::Assembly::component`), held where they were put and, if that
/// can't hold, free from there; no rebuild. Moves whose instances lie in
/// different components solve each of them. The answer is
/// [`solve_request`]'s for those instances alone (the fixed ones they
/// reach left out), with `component: true`. `None` when the assembly has
/// nothing settled to start from (no mates, or a moved instance that
/// didn't place): the whole request answers then.
fn component_request(
    session: &mut crate::session::Session,
    p: &Part,
    moves: &[(Id, crate::doc::assembly::Pose)],
) -> Result<Option<Value>> {
    let r = session.rebuilt(&p.id)?.clone();
    let Some(s) = r.solve.as_deref() else {
        return Ok(None);
    };
    let mut roots = Vec::new();
    let mut asm = s.asm.clone();
    for (id, pose) in moves {
        let Some(k) = s.instances.iter().position(|i| i == id) else {
            return Ok(None);
        };
        let k = k as u32;
        asm.bodies.get_mut(&k).expect("a body").pose = crate::connector::pose(&pose_rigid(pose)?);
        roots.push(k);
    }
    let solve = |held: bool| -> Result<ryumate::Solved> {
        let mut a = asm.clone();
        for k in &roots {
            a.bodies.get_mut(k).expect("a body").fixed |= held;
        }
        ryumate::solve(&a.component(&roots)).map_err(|e| {
            Error::new(
                Code::Internal,
                format!("{}: the solver refused a component: {e}", p.id),
            )
        })
    };
    let mut held = true;
    let mut solved = solve(true)?;
    if !solved.report.converged {
        held = false;
        solved = solve(false)?;
    }
    let sub = Solve {
        report: solved.report.clone(),
        instances: s.instances.clone(),
        mates: s.mates.clone(),
        moved: Vec::new(),
        asm: asm.component(&roots),
    };
    if !solved.report.converged && s.report.converged {
        return Err(Error::new(
            Code::AssemblyOverconstrained,
            format!("moved that way, {}'s mates can't all hold", p.id),
        )
        .with("part", &p.id)
        .with("conflicting", sub.mate_ids(&solved.report.conflicting)));
    }
    let mut instances = serde_json::Map::new();
    for (k, new) in &solved.poses {
        let body = &sub.asm.bodies[k];
        if body.fixed && !roots.contains(k) {
            continue;
        }
        let id = &s.instances[*k as usize];
        let Some(Ok(Feature::Instance(stored))) = p.features.get(id) else {
            continue;
        };
        let moved = moves.iter().find(|(m, _)| m == id);
        // As [`Moved::poses`]: a held move where it was put, one that
        // didn't move as its file says when the rebuild placed it there.
        let pose = match moved {
            Some((_, pose)) if held => *pose,
            _ if *new == s.asm.bodies[k].pose
                && pose_rigid(&stored.pose).is_ok_and(|r| crate::connector::pose(&r) == *new) =>
            {
                stored.pose
            }
            _ => doc_pose(new),
        };
        instances.insert(
            id.to_string(),
            json!({"pose": pose.to_json(), "moved": pose != stored.pose}),
        );
    }
    Ok(Some(json!({
        "part": p.id,
        "revision": p.revision,
        "converged": solved.report.converged,
        "held": held,
        "component": true,
        "instances": instances,
        "solve": sub.to_json(),
    })))
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use crate::assembly::tests::{id, place_in, project};
    use crate::doc::files::PartFile;
    use crate::doc::project::{feature_path, part_path, write_json};
    use crate::server::tests::call;
    use crate::session::Session;
    use crate::sketch::tests::face_along;
    use serde_json::Value;
    use std::path::Path;

    /// Writes feature `fid` (`v`, an assembly feature's fields) into
    /// assembly `asm`, listed last.
    pub(crate) fn add_in(root: &Path, asm: &str, fid: &str, ty: &str, mut v: Value) {
        v["format"] = json!(4);
        v["kind"] = json!("ryucad.feature");
        v["type"] = json!(ty);
        write_json(&feature_path(root, &id(asm), &id(fid)), &v).unwrap();
        let p = part_path(root, &id(asm));
        let mut list = PartFile::from_json(
            &crate::doc::canonical::parse(&std::fs::read_to_string(&p).unwrap()).unwrap(),
        )
        .unwrap();
        list.features.push(id(fid));
        write_json(&p, &list.to_json()).unwrap();
    }

    /// The name of the planar face of `part`'s solid `solid` facing `n`.
    pub(crate) fn face(s: &mut Session, part: &str, solid: &str, n: [f64; 3]) -> String {
        let body = call(s, "inspect", json!({"part": part, "feature": solid})).unwrap()["bodies"]
            [0]
        .clone();
        face_along(&body, n)
    }

    /// `frame` with the plate fixed at the origin and the bolt 50 mm off,
    /// and a revolute standing the bolt's foot on the plate's top, 20 mm
    /// along and 10 mm across.
    pub(crate) fn hinged() -> (tempfile::TempDir, Session) {
        let (dir, mut s) = project();
        let top = face(&mut s, "plate", "slab", [0.0, 0.0, 1.0]);
        let foot = face(&mut s, "bolt", "shank", [0.0, 0.0, -1.0]);
        let r = dir.path();
        place_in(r, "frame", "base", "plate", json!("working"), [0.0; 3]);
        let base = feature_path(r, &id("frame"), &id("base"));
        let mut v = crate::doc::canonical::parse(&std::fs::read_to_string(&base).unwrap()).unwrap();
        v["fixed"] = json!(true);
        write_json(&base, &v).unwrap();
        place_in(
            r,
            "frame",
            "pin",
            "bolt",
            json!("working"),
            [0.05, 0.0, 0.0],
        );
        add_in(
            r,
            "frame",
            "m1",
            "mate",
            json!({"form": "revolute",
                   "a": {"instance": ["base"], "at": {"feature": "slab", "name": top},
                         "offset": {"origin": ["20 mm", "10 mm", "0 mm"]}},
                   "b": {"instance": ["pin"], "at": {"feature": "shank", "name": foot},
                         "offset": {"flip": true}}}),
        );
        s.refresh().unwrap();
        (dir, s)
    }

    fn occurrence_origin(s: &mut Session, asm: &str, path: &str) -> [f64; 3] {
        let r = s.rebuilt(&id(asm)).unwrap().clone();
        let m = crate::assembly::Model::of_assembly(&r);
        let o = m
            .occurrences
            .iter()
            .find(|o| crate::assembly::path_text(&o.path) == path)
            .unwrap_or_else(|| panic!("no occurrence {path}"));
        let t = o.pose.translation_part();
        [t.x, t.y, t.z]
    }

    /// Occurrence `path` of `asm` is at `want`.
    pub(crate) fn near_pose(s: &mut Session, asm: &str, path: &str, want: [f64; 3]) {
        near(occurrence_origin(s, asm, path), want);
    }

    fn near(a: [f64; 3], b: [f64; 3]) {
        for k in 0..3 {
            assert!((a[k] - b[k]).abs() < 1e-9, "{a:?} vs {b:?}");
        }
    }

    #[test]
    fn mates_move_instances_and_report_their_freedom() {
        let (_dir, mut s) = hinged();
        near(
            occurrence_origin(&mut s, "frame", "pin"),
            [0.02, 0.01, 0.005],
        );
        let r = s.rebuilt(&id("frame")).unwrap().clone();
        let solve = r.solve.as_ref().expect("a solve");
        assert!(solve.report.converged);
        assert_eq!(solve.moved, [id("pin")]);
        let w = r.warnings(&id("frame"));
        assert_eq!(w[0]["code"], "assembly_stale");
        assert_eq!(w[0]["instances"], json!(["pin"]));
        let v = solve.to_json();
        assert_eq!(v["state"], "under");
        assert_eq!(v["dof"], 1);
        assert_eq!(v["mates"]["m1"], "well");
        assert_eq!(v["instances"]["base"]["dof"], 0);
        let pin = &v["instances"]["pin"];
        assert_eq!((&pin["state"], &pin["dof"]), (&json!("under"), &json!(1)));
        assert_eq!(pin["free"]["rotate"][0]["axis"], json!([0.0, 0.0, 1.0]));
        let through = &pin["free"]["rotate"][0]["through_m"];
        assert!((through[0].as_f64().unwrap() - 0.02).abs() < 1e-9);
        // The rebuild report says so too.
        let rep = call(&mut s, "rebuild", json!({"part": "frame"})).unwrap();
        let part = &rep["parts"][0];
        assert_eq!(part["solve"]["stale"], true);
        let m1 = part["features"]
            .as_array()
            .unwrap()
            .iter()
            .find(|f| f["id"] == "m1")
            .unwrap();
        assert_eq!(m1["mate"]["state"], "well");
        let pin = part["features"]
            .as_array()
            .unwrap()
            .iter()
            .find(|f| f["id"] == "pin")
            .unwrap();
        assert_eq!(pin["instance"]["solve"]["dof"], 1);
    }

    #[test]
    fn solved_poses_written_back_hold_as_stored() {
        let (dir, mut s) = hinged();
        let r = s.rebuilt(&id("frame")).unwrap().clone();
        let before = crate::assembly::Model::of_assembly(&r).hash;
        // Store the pin where the solve put it.
        let p = r
            .features
            .iter()
            .find(|(f, _)| f.as_str() == "pin")
            .and_then(|(_, o)| o.as_ref().ok()?.instance())
            .unwrap()
            .clone();
        let path = feature_path(dir.path(), &id("frame"), &id("pin"));
        let mut v = crate::doc::canonical::parse(&std::fs::read_to_string(&path).unwrap()).unwrap();
        v["pose"] = crate::assembly::rigid_pose(&p.pose).to_json();
        write_json(&path, &v).unwrap();
        s.refresh().unwrap();
        let r = s.rebuilt(&id("frame")).unwrap().clone();
        assert!(!r.solve.as_ref().unwrap().stale());
        assert!(r.warnings(&id("frame")).is_empty());
        assert_eq!(crate::assembly::Model::of_assembly(&r).hash, before);
    }

    #[test]
    fn mates_that_cant_hold_leave_their_instances_where_they_were() {
        let (dir, mut s) = hinged();
        let top = face(&mut s, "plate", "slab", [0.0, 0.0, 1.0]);
        let foot = face(&mut s, "bolt", "shank", [0.0, 0.0, -1.0]);
        // The same foot on the same face 5 mm further along.
        add_in(
            dir.path(),
            "frame",
            "m2",
            "mate",
            json!({"form": "revolute",
                   "a": {"instance": ["base"], "at": {"feature": "slab", "name": top},
                         "offset": {"origin": ["25 mm", "10 mm", "0 mm"]}},
                   "b": {"instance": ["pin"], "at": {"feature": "shank", "name": foot},
                         "offset": {"flip": true}}}),
        );
        s.refresh().unwrap();
        near(occurrence_origin(&mut s, "frame", "pin"), [0.05, 0.0, 0.0]);
        let r = s.rebuilt(&id("frame")).unwrap().clone();
        for m in ["m1", "m2"] {
            let e = r
                .features
                .iter()
                .find(|(f, _)| f.as_str() == m)
                .unwrap()
                .1
                .as_ref()
                .unwrap_err();
            assert_eq!(e.code, Code::AssemblyUnsolved);
            assert_eq!(e.details["conflicting"], json!([["m1", "m2"]]));
        }
        assert_eq!(
            r.solve.as_ref().unwrap().to_json()["instances"]["pin"]["state"],
            "failing"
        );
    }

    #[test]
    fn connectors_that_dont_resolve_fail_their_mate_alone() {
        let (dir, mut s) = hinged();
        // A name the plate doesn't have, and an instance the assembly
        // doesn't list.
        add_in(
            dir.path(),
            "frame",
            "m2",
            "mate",
            json!({"form": "ball",
                   "a": {"instance": ["base"], "at": {"feature": "slab", "name": "0123456789abcdef0123456789abcdef"}},
                   "b": {"instance": ["pin"], "at": "origin"}}),
        );
        add_in(
            dir.path(),
            "frame",
            "m3",
            "mate",
            json!({"form": "ball",
                   "a": {"instance": ["base"], "at": "origin"},
                   "b": {"instance": ["nut"], "at": "origin"}}),
        );
        s.refresh().unwrap();
        let r = s.rebuilt(&id("frame")).unwrap().clone();
        let err = |m: &str| {
            r.features
                .iter()
                .find(|(f, _)| f.as_str() == m)
                .unwrap()
                .1
                .as_ref()
                .unwrap_err()
                .clone()
        };
        let e = err("m2");
        assert_eq!(e.code, Code::ReferenceUnresolved);
        assert_eq!(
            (&e.details["role"], &e.details["part"]),
            (&json!("a"), &json!("plate"))
        );
        assert_eq!(err("m3").code, Code::NotFound);
        // The hinge still holds the pin.
        near(
            occurrence_origin(&mut s, "frame", "pin"),
            [0.02, 0.01, 0.005],
        );
    }

    /// Feature `fid`'s result in `asm`'s rebuild.
    fn result_of(s: &mut Session, asm: &str, fid: &str) -> Result<Output> {
        let r = s.rebuilt(&id(asm)).unwrap().clone();
        r.features
            .iter()
            .find(|(f, _)| f.as_str() == fid)
            .unwrap_or_else(|| panic!("no feature {fid}"))
            .1
            .clone()
    }

    /// Feature `fid` of `asm` failed with `code`, its details holding
    /// `want`'s keys.
    fn fails_with(s: &mut Session, asm: &str, fid: &str, code: Code, want: Value) {
        let e = result_of(s, asm, fid).expect_err(fid);
        assert_eq!(e.code, code, "{fid}: {e:?}");
        assert_eq!(e.details["feature"], fid, "{e:?}");
        for (k, v) in want.as_object().unwrap() {
            assert_eq!(&e.details[k], v, "{fid}'s {k}: {e:?}");
        }
    }

    /// Mate `fid` of `asm` built, in state `state`.
    fn mate_is(s: &mut Session, asm: &str, fid: &str, state: ryumate::MateState) {
        match result_of(s, asm, fid) {
            Ok(Output::Mate(m)) => assert_eq!(m.state, state, "{fid}"),
            other => panic!("{fid}: {other:?}"),
        }
    }

    #[test]
    fn a_value_the_solver_cant_take_fails_its_own_feature_alone() {
        let (dir, mut s) = hinged();
        let r = dir.path();
        // A connector offset past the size box, a distance past the
        // longest a mate drives, and a pattern spaced past it.
        add_in(
            r,
            "frame",
            "far",
            "mate",
            json!({"form": "planar",
                   "a": {"instance": ["base"], "at": "origin",
                         "offset": {"origin": ["600000 mm", "0 mm", "0 mm"]}},
                   "b": {"instance": ["pin"], "at": "origin"}}),
        );
        add_in(
            r,
            "frame",
            "long",
            "mate",
            json!({"form": "planar", "distance": "2000 m",
                   "a": {"instance": ["base"], "at": "origin"},
                   "b": {"instance": ["pin"], "at": "origin"}}),
        );
        add_in(
            r,
            "frame",
            "row",
            "instance_pattern",
            json!({"form": "linear", "instances": ["pin"], "direction": "x",
                   "spacing": "2000 m", "count": 2}),
        );
        s.refresh().unwrap();
        fails_with(
            &mut s,
            "frame",
            "far",
            Code::InvalidParam,
            json!({"param": "a"}),
        );
        fails_with(
            &mut s,
            "frame",
            "long",
            Code::InvalidParam,
            json!({"param": "distance"}),
        );
        fails_with(
            &mut s,
            "frame",
            "row",
            Code::InvalidParam,
            json!({"param": "spacing"}),
        );
        // The rest solves as before.
        mate_is(&mut s, "frame", "m1", ryumate::MateState::Well);
        near_pose(&mut s, "frame", "pin", [0.02, 0.01, 0.005]);
    }

    #[test]
    fn copies_and_instances_the_solve_puts_past_the_size_box_fail() {
        let (dir, mut s) = hinged();
        let r = dir.path();
        // Copies 200 m apart along x: the fourth would be 600 m out.
        add_in(
            r,
            "frame",
            "row",
            "instance_pattern",
            json!({"form": "linear", "instances": ["pin"], "direction": "x",
                   "spacing": "200000 mm", "count": 4}),
        );
        s.refresh().unwrap();
        fails_with(
            &mut s,
            "frame",
            "row",
            Code::InvalidParam,
            json!({"copy": [3, 0], "seed": "pin"}),
        );
        mate_is(&mut s, "frame", "m1", ryumate::MateState::Well);
        // A mate on a copy that can be placed is fine; one on a copy that
        // can't fails, naming it.
        add_in(
            r,
            "frame",
            "ring",
            "instance_pattern",
            json!({"form": "linear", "instances": ["base"], "direction": "x",
                   "spacing": "200000 mm", "count": 4}),
        );
        place_in(r, "frame", "cap", "bolt", json!("working"), [0.0, 0.0, 0.1]);
        add_in(
            r,
            "frame",
            "m2",
            "mate",
            json!({"form": "fastened",
                   "a": {"instance": ["ring:3", "base"], "at": "origin"},
                   "b": {"instance": ["cap"], "at": "origin"}}),
        );
        s.refresh().unwrap();
        let e = result_of(&mut s, "frame", "m2").unwrap_err();
        assert_eq!(e.code, Code::InvalidParam, "{e:?}");
        assert_eq!(
            e.details["unplaced"], "copy [3, 0] of base in ring",
            "{e:?}"
        );
        near_pose(&mut s, "frame", "cap", [0.0, 0.0, 0.1]);
        // A fastened mate that puts the pin 900 m out, with frames each
        // inside the box: the pin stays where it was, failing, and so does
        // its mate.
        let m1 = feature_path(r, &id("frame"), &id("m1"));
        let mut v = crate::doc::canonical::parse(&std::fs::read_to_string(&m1).unwrap()).unwrap();
        v["form"] = json!("fastened");
        v["a"] = json!({"instance": ["base"], "at": "origin",
                        "offset": {"origin": ["450 m", "0 m", "0 m"]}});
        v["b"] = json!({"instance": ["pin"], "at": "origin",
                        "offset": {"origin": ["-450 m", "0 m", "0 m"]}});
        write_json(&m1, &v).unwrap();
        s.refresh().unwrap();
        let e = result_of(&mut s, "frame", "m1").unwrap_err();
        assert_eq!(e.code, Code::InvalidParam, "{e:?}");
        assert_eq!(e.details["unplaced"], "pin", "{e:?}");
        near_pose(&mut s, "frame", "pin", [0.05, 0.0, 0.0]);
        let built = s.rebuilt(&id("frame")).unwrap().clone();
        let solve = built.solve.as_ref().expect("a solve");
        assert!(!solve.report.converged);
        assert!(!solve.moved.contains(&id("pin")));
        assert_eq!(solve.to_json()["instances"]["pin"]["state"], "failing");
    }

    #[test]
    fn patterns_place_copies_that_mates_can_name() {
        let (dir, mut s) = hinged();
        let r = dir.path();
        // Four pins round the plate's z axis, a quarter turn apart.
        add_in(
            r,
            "frame",
            "ring",
            "instance_pattern",
            json!({"form": "circular", "instances": ["pin"], "axis": "z", "count": 4,
                   "angle": "360 deg"}),
        );
        place_in(r, "frame", "cap", "bolt", json!("working"), [0.0, 0.0, 0.1]);
        // Turned a little off the copy, as an agent might leave it.
        let cap = feature_path(r, &id("frame"), &id("cap"));
        let mut v = crate::doc::canonical::parse(&std::fs::read_to_string(&cap).unwrap()).unwrap();
        v["pose"]["x"] = json!([-0.9, 0.1, 0.0]);
        write_json(&cap, &v).unwrap();
        // A cap fastened 20 mm up on copy 2: half a turn round, it is at
        // (-20, -10) mm.
        add_in(
            r,
            "frame",
            "m2",
            "mate",
            json!({"form": "fastened",
                   "a": {"instance": ["ring:2", "pin"], "at": "origin",
                         "offset": {"origin": ["0 mm", "0 mm", "20 mm"]}},
                   "b": {"instance": ["cap"], "at": "origin"}}),
        );
        s.refresh().unwrap();
        near(
            occurrence_origin(&mut s, "frame", "ring:1/pin"),
            [-0.01, 0.02, 0.005],
        );
        near(
            occurrence_origin(&mut s, "frame", "ring:2/pin"),
            [-0.02, -0.01, 0.005],
        );
        near(
            occurrence_origin(&mut s, "frame", "cap"),
            [-0.02, -0.01, 0.025],
        );
        let rep = call(&mut s, "rebuild", json!({"part": "frame"})).unwrap();
        let ring = rep["parts"][0]["features"]
            .as_array()
            .unwrap()
            .iter()
            .find(|f| f["id"] == "ring")
            .unwrap()
            .clone();
        assert_eq!(ring["copies"].as_array().unwrap().len(), 3);
        assert_eq!(ring["copies"][0]["copy"], "ring:1");
        // A copy the pattern doesn't place.
        let m2 = feature_path(r, &id("frame"), &id("m2"));
        let mut v = crate::doc::canonical::parse(&std::fs::read_to_string(&m2).unwrap()).unwrap();
        v["a"]["instance"] = json!(["ring:7", "pin"]);
        write_json(&m2, &v).unwrap();
        s.refresh().unwrap();
        let r = s.rebuilt(&id("frame")).unwrap().clone();
        let e = r
            .features
            .iter()
            .find(|(f, _)| f.as_str() == "m2")
            .unwrap()
            .1
            .as_ref()
            .unwrap_err();
        assert_eq!(e.code, Code::NotFound);
        // An instance a mate names can't be deleted.
        let e = call(
            &mut s,
            "feature.delete",
            json!({"part": "frame", "id": "pin"}),
        )
        .unwrap_err();
        assert_eq!(e.code, Code::HasDependents);
    }
}

#[cfg(test)]
mod protocol_tests {
    use super::tests::{face, near_pose};
    use crate::assembly::tests::project;
    use crate::error::Code;
    use crate::server::tests::call;
    use serde_json::json;

    #[test]
    fn mates_and_patterns_are_added_and_edited_through_the_protocol() {
        let (_dir, mut s) = project();
        let top = face(&mut s, "plate", "slab", [0.0, 0.0, 1.0]);
        let foot = face(&mut s, "bolt", "shank", [0.0, 0.0, -1.0]);
        for (inst, of, fixed) in [("base", "plate", true), ("pin", "bolt", false)] {
            call(
                &mut s,
                "feature.add",
                json!({"part": "frame", "type": "instance", "id": inst, "of": of,
                       "fixed": fixed}),
            )
            .unwrap();
        }
        let a = format!("frame/base/slab/{top}");
        let b = format!("frame/pin/shank/{foot}");
        let v = call(
            &mut s,
            "feature.add",
            json!({"part": "frame", "type": "mate", "id": "m1", "form": "cylindrical",
                   "a": a, "b": b, "a_offset": {"origin": ["20 mm", "10 mm", "0 mm"]},
                   "b_offset": {"flip": true}, "distance": "3 mm"}),
        )
        .unwrap();
        assert_eq!(v["feature"]["a"]["text"], a);
        assert_eq!(v["feature"]["distance"]["m"], 0.003);
        near_pose(&mut s, "frame", "pin", [0.02, 0.01, 0.008]);
        // A new form keeps only what it takes; a connector given again
        // keeps its offset.
        call(
            &mut s,
            "feature.edit",
            json!({"part": "frame", "id": "m1", "form": "revolute", "a": a, "angle": "30 deg"}),
        )
        .unwrap();
        let show = call(&mut s, "feature.show", json!({"part": "frame", "id": "m1"})).unwrap();
        assert_eq!(show["form"], "revolute");
        assert!(show.get("distance").is_none());
        assert_eq!(
            show["a"]["offset"]["origin"],
            json!(["20 mm", "10 mm", "0 mm"])
        );
        near_pose(&mut s, "frame", "pin", [0.02, 0.01, 0.005]);
        call(
            &mut s,
            "feature.edit",
            json!({"part": "frame", "id": "m1", "angle": "", "a_offset": {}}),
        )
        .unwrap();
        let show = call(&mut s, "feature.show", json!({"part": "frame", "id": "m1"})).unwrap();
        assert!(show.get("angle").is_none() && show["a"].get("offset").is_none());
        // A pattern of the pin round the plate's z axis.
        call(
            &mut s,
            "feature.add",
            json!({"part": "frame", "type": "instance_pattern", "id": "ring",
                   "instances": ["pin"], "axis": "z", "count": 3, "angle": "360 deg"}),
        )
        .unwrap();
        let show = call(
            &mut s,
            "feature.show",
            json!({"part": "frame", "id": "ring"}),
        )
        .unwrap();
        assert_eq!(show["form"], "circular");
        call(
            &mut s,
            "feature.edit",
            json!({"part": "frame", "id": "ring", "count": 4, "skip": [2]}),
        )
        .unwrap();
        let r = call(&mut s, "rebuild", json!({"part": "frame"})).unwrap();
        let ring = r["parts"][0]["features"]
            .as_array()
            .unwrap()
            .iter()
            .find(|f| f["id"] == "ring")
            .unwrap()
            .clone();
        let copies: Vec<&str> = ring["copies"]
            .as_array()
            .unwrap()
            .iter()
            .map(|c| c["copy"].as_str().unwrap())
            .collect();
        assert_eq!(copies, ["ring:1", "ring:3"]);
    }

    #[test]
    fn mate_writes_refuse_what_cant_resolve() {
        let (_dir, mut s) = project();
        let top = face(&mut s, "plate", "slab", [0.0, 0.0, 1.0]);
        call(
            &mut s,
            "feature.add",
            json!({"part": "frame", "type": "instance", "id": "base", "of": "plate"}),
        )
        .unwrap();
        let add = |s: &mut crate::session::Session, v: serde_json::Value| {
            let mut req = json!({"part": "frame", "type": "mate", "form": "ball"});
            for (k, x) in v.as_object().unwrap() {
                req[k] = x.clone();
            }
            call(s, "feature.add", req)
        };
        let good = format!("frame/base/slab/{top}");
        for (v, code) in [
            (json!({"a": good, "b": "frame/nut"}), Code::NotFound),
            (json!({"a": good}), Code::InvalidParam),
            (json!({"a": good, "b": "plate/slab"}), Code::InvalidParam),
            (
                json!({"a": "frame/base/slab/0123456789abcdef0123456789abcdef", "b": "frame/base"}),
                Code::ReferenceUnresolved,
            ),
            (
                json!({"a": good, "b": "frame/base", "angle": "3 deg"}),
                Code::InvalidDocument,
            ),
            (
                json!({"a": good, "b": "frame/base", "form": "weld"}),
                Code::InvalidParam,
            ),
        ] {
            let e = add(&mut s, v.clone()).unwrap_err();
            assert_eq!(e.code, code, "{v}: {e:?}");
        }
        // A mate in a part is the wrong kind of feature there.
        let here = format!("plate/base/slab/{top}");
        let e = call(
            &mut s,
            "feature.add",
            json!({"part": "plate", "type": "mate", "form": "ball", "a": here, "b": "plate/base"}),
        )
        .unwrap_err();
        assert_eq!(e.code, Code::WrongPartKind);
        // A mate of an instance with itself says nothing.
        let e = add(&mut s, json!({"a": good, "b": "frame/base"})).unwrap_err();
        assert_eq!(e.code, Code::InvalidParam);
        call(
            &mut s,
            "feature.add",
            json!({"part": "frame", "type": "instance", "id": "pin", "of": "bolt"}),
        )
        .unwrap();
        assert!(add(&mut s, json!({"a": good, "b": "frame/pin"})).is_ok());
        // Values the solver can't take are refused when written.
        for (v, param) in [
            (
                json!({"a": good, "b": "frame/pin",
                       "a_offset": {"origin": ["600000 mm", "0 mm", "0 mm"]}}),
                "a",
            ),
            (
                json!({"a": good, "b": "frame/pin", "form": "planar", "distance": "2000 m"}),
                "distance",
            ),
            (
                json!({"type": "instance_pattern", "form": "linear", "instances": ["pin"],
                       "direction": "x", "spacing": "2000 m", "count": 2}),
                "spacing",
            ),
        ] {
            let e = add(&mut s, v.clone()).unwrap_err();
            assert_eq!(e.code, Code::InvalidParam, "{v}: {e:?}");
            assert_eq!(e.details["param"], param, "{v}: {e:?}");
        }
    }
}

#[cfg(test)]
mod move_tests {
    use super::tests::{add_in, face, hinged, near_pose};
    use crate::assembly::tests::{id, place_in};
    use crate::doc::project::feature_path;
    use crate::error::Code;
    use crate::server::tests::call;
    use serde_json::json;

    /// The hinge, stored where it holds, with a cap fastened 20 mm up the
    /// pin.
    fn hinged_with_cap() -> (tempfile::TempDir, crate::session::Session) {
        let (dir, mut s) = hinged();
        let r = dir.path();
        place_in(r, "frame", "cap", "bolt", json!("working"), [0.0, 0.0, 0.1]);
        add_in(
            r,
            "frame",
            "m2",
            "mate",
            json!({"form": "fastened",
                   "a": {"instance": ["pin"], "at": "origin",
                         "offset": {"origin": ["0 mm", "0 mm", "20 mm"]}},
                   "b": {"instance": ["cap"], "at": "origin"}}),
        );
        s.refresh().unwrap();
        // Store the solved poses, as a write keeping it current would.
        let solved = super::after_moves(&mut s, &id("frame"), &[]).unwrap();
        let p = s.project().part(&id("frame")).unwrap().clone();
        let writes = solved.writes(&p);
        let refs: Vec<_> = writes.iter().map(|(i, f)| (i, f)).collect();
        let list = crate::server::order(&p).unwrap();
        crate::server::save(&mut s, &id("frame"), &refs, list).unwrap();
        assert!(
            !s.rebuilt(&id("frame"))
                .unwrap()
                .solve
                .as_ref()
                .unwrap()
                .stale()
        );
        (dir, s)
    }

    #[test]
    fn solving_reports_and_previews_moves_without_writing() {
        let (dir, mut s) = hinged_with_cap();
        let before =
            std::fs::read_to_string(feature_path(dir.path(), &id("frame"), &id("pin"))).unwrap();
        let show = call(
            &mut s,
            "feature.show",
            json!({"part": "frame", "id": "cap"}),
        )
        .unwrap();
        let x0: Vec<f64> = (0..3)
            .map(|k| show["pose"]["x"][k].as_f64().unwrap())
            .collect();
        let v = call(&mut s, "assembly.solve", json!({"part": "frame"})).unwrap();
        assert_eq!(v["converged"], true);
        assert_eq!(v["solve"]["dof"], 1);
        assert!(v.get("held").is_none());
        // Turned about the hinge: free, so held where it is put, and the
        // cap goes with it.
        let turn = json!({"part": "frame", "moves": [{"instance": "pin", "axis": [0, 0, 1],
                                                      "angle": "90 deg"}]});
        let v = call(&mut s, "assembly.solve", turn).unwrap();
        assert_eq!((&v["converged"], &v["held"]), (&json!(true), &json!(true)));
        assert_eq!(v["instances"]["pin"]["moved"], true);
        assert_eq!(v["instances"]["cap"]["moved"], true);
        // A quarter turn about z: x to y, y to -x.
        let x = &v["instances"]["cap"]["pose"]["x"];
        assert!(
            (x[0].as_f64().unwrap() + x0[1]).abs() < 1e-9,
            "{x} from {x0:?}"
        );
        assert!(
            (x[1].as_f64().unwrap() - x0[0]).abs() < 1e-9,
            "{x} from {x0:?}"
        );
        // Lifted off the plate: it can't be held there, so it is solved
        // free from where it was put, and the hinge pulls it back.
        let lift = json!({"part": "frame", "moves": [{"instance": "pin",
                                                      "translate": [0, 0, "1 mm"]}]});
        let v = call(&mut s, "assembly.solve", lift).unwrap();
        assert_eq!((&v["converged"], &v["held"]), (&json!(true), &json!(false)));
        let z = v["instances"]["pin"]["pose"]["origin_m"][2]
            .as_f64()
            .unwrap();
        assert!((z - 0.005).abs() < 1e-9, "{z}");
        // Nothing was written.
        let after =
            std::fs::read_to_string(feature_path(dir.path(), &id("frame"), &id("pin"))).unwrap();
        assert_eq!(before, after);
        let e = call(
            &mut s,
            "assembly.solve",
            json!({"part": "frame", "moves": [{"instance": "base", "translate": [1, 0, 0]}]}),
        )
        .unwrap_err();
        assert_eq!(e.code, Code::InstanceFixed);
    }

    /// [`hinged_with_cap`] with a second bolt, `arm`, on its own hinge on
    /// the plate, poses stored solved.
    fn two_hinges() -> (tempfile::TempDir, crate::session::Session) {
        let (dir, mut s) = hinged_with_cap();
        let top = face(&mut s, "plate", "slab", [0.0, 0.0, 1.0]);
        let foot = face(&mut s, "bolt", "shank", [0.0, 0.0, -1.0]);
        let r = dir.path();
        place_in(
            r,
            "frame",
            "arm",
            "bolt",
            json!("working"),
            [-0.05, 0.0, 0.0],
        );
        add_in(
            r,
            "frame",
            "m3",
            "mate",
            json!({"form": "revolute",
                   "a": {"instance": ["base"], "at": {"feature": "slab", "name": top},
                         "offset": {"origin": ["-20 mm", "10 mm", "0 mm"]}},
                   "b": {"instance": ["arm"], "at": {"feature": "shank", "name": foot},
                         "offset": {"flip": true}}}),
        );
        s.refresh().unwrap();
        let solved = super::after_moves(&mut s, &id("frame"), &[]).unwrap();
        let p = s.project().part(&id("frame")).unwrap().clone();
        let writes = solved.writes(&p);
        let refs: Vec<_> = writes.iter().map(|(i, f)| (i, f)).collect();
        let list = crate::server::order(&p).unwrap();
        crate::server::save(&mut s, &id("frame"), &refs, list).unwrap();
        (dir, s)
    }

    #[test]
    fn a_component_previews_what_the_moved_instance_reaches() {
        let (_dir, mut s) = two_hinges();
        let keys = |v: &serde_json::Value| -> Vec<String> {
            v["instances"]
                .as_object()
                .unwrap()
                .keys()
                .cloned()
                .collect()
        };
        let close = |a: &serde_json::Value, b: &serde_json::Value| {
            for f in ["origin_m", "z", "x"] {
                for k in 0..3 {
                    let (x, y) = (a[f][k].as_f64().unwrap(), b[f][k].as_f64().unwrap());
                    assert!((x - y).abs() < 1e-9, "{f}: {a} against {b}");
                }
            }
        };
        let turn = json!([{"instance": "pin", "axis": [0, 0, 1], "angle": "90 deg"}]);
        let lift = json!([{"instance": "pin", "translate": [0, 0, "1 mm"]}]);
        for (moves, held) in [(turn, true), (lift, false)] {
            let whole = call(
                &mut s,
                "assembly.solve",
                json!({"part": "frame", "moves": moves}),
            )
            .unwrap();
            let part = call(
                &mut s,
                "assembly.solve",
                json!({"part": "frame", "moves": moves, "component": true}),
            )
            .unwrap();
            assert_eq!(part["component"], true);
            assert_eq!(
                (&part["converged"], &part["held"]),
                (&json!(true), &json!(held))
            );
            // The pin and the cap fastened to it: not the fixed plate, nor
            // the arm on its own hinge.
            assert_eq!(keys(&part), ["cap", "pin"]);
            assert_eq!(keys(&whole), ["arm", "base", "cap", "pin"]);
            for i in ["cap", "pin"] {
                close(
                    &part["instances"][i]["pose"],
                    &whole["instances"][i]["pose"],
                );
                assert_eq!(part["instances"][i]["moved"], true);
            }
            // Held, the pin is a constant; free, it turns on its hinge.
            // The arm's turn is the whole's alone.
            let dof = whole["solve"]["dof"].as_u64().unwrap();
            assert_eq!(part["solve"]["dof"], json!(dof - 1));
            assert_eq!(part["solve"]["dof"], json!(u64::from(!held)));
        }
        // Moves in both components solve both.
        let both = json!({"part": "frame", "component": true, "moves": [
            {"instance": "pin", "axis": [0, 0, 1], "angle": "10 deg"},
            {"instance": "arm", "axis": [0, 0, 1], "angle": "20 deg"}]});
        let v = call(&mut s, "assembly.solve", both).unwrap();
        assert_eq!(keys(&v), ["arm", "cap", "pin"]);
        let e = call(
            &mut s,
            "assembly.solve",
            json!({"part": "frame", "component": true}),
        )
        .unwrap_err();
        assert_eq!(e.code, Code::InvalidParam);
    }

    #[test]
    fn a_move_writes_what_follows_in_one_change() {
        use crate::live::Live;
        use crate::server::tests::call_live;
        let (_dir, mut s) = hinged_with_cap();
        let show = call(
            &mut s,
            "feature.show",
            json!({"part": "frame", "id": "cap"}),
        )
        .unwrap();
        let x0 = show["pose"]["x"].clone();
        let mut live = Live::new();
        let v = call_live(
            &mut s,
            &mut live,
            None,
            "assembly.move",
            json!({"part": "frame", "instance": "pin", "axis": [0, 0, 1], "angle": "90 deg"}),
        )
        .unwrap();
        assert_eq!(v["moved"], json!(["pin", "cap"]));
        assert_eq!(v["held"], true);
        let r = s.rebuilt(&id("frame")).unwrap();
        assert!(!r.solve.as_ref().unwrap().stale());
        near_pose(&mut s, "frame", "cap", [0.02, 0.01, 0.025]);
        // One change: undone, both are back.
        call_live(&mut s, &mut live, None, "undo", json!({})).unwrap();
        let show = call(
            &mut s,
            "feature.show",
            json!({"part": "frame", "id": "cap"}),
        )
        .unwrap();
        assert_eq!(show["pose"]["x"], x0);
        // The tree says what each instance may still do.
        let t = call(&mut s, "assembly.tree", json!({"part": "frame"})).unwrap();
        let pin = t["instances"]
            .as_array()
            .unwrap()
            .iter()
            .find(|i| i["id"] == "pin")
            .unwrap();
        assert_eq!(
            (&pin["solve"]["dof"], &pin["solve"]["state"]),
            (&json!(1), &json!("under"))
        );
    }
}

#[cfg(test)]
mod current_tests {
    use super::tests::face;
    use crate::assembly::tests::{id, project};
    use crate::error::Code;
    use crate::server::tests::call;
    use serde_json::json;

    /// The plate fixed and the bolt free in `frame`, and the faces a
    /// revolute stands the bolt on.
    fn placed() -> (tempfile::TempDir, crate::session::Session, String, String) {
        let (dir, mut s) = project();
        let top = face(&mut s, "plate", "slab", [0.0, 0.0, 1.0]);
        let foot = face(&mut s, "bolt", "shank", [0.0, 0.0, -1.0]);
        for (inst, of, fixed) in [("base", "plate", true), ("pin", "bolt", false)] {
            call(
                &mut s,
                "feature.add",
                json!({"part": "frame", "type": "instance", "id": inst, "of": of,
                       "fixed": fixed, "placement": {"origin": ["50 mm", 0, 0]}}),
            )
            .unwrap();
        }
        (
            dir,
            s,
            format!("frame/base/slab/{top}"),
            format!("frame/pin/shank/{foot}"),
        )
    }

    fn stale(s: &mut crate::session::Session) -> bool {
        s.rebuilt(&id("frame"))
            .unwrap()
            .solve
            .as_ref()
            .unwrap()
            .stale()
    }

    #[test]
    fn writes_keep_assemblies_current() {
        let (_dir, mut s, a, b) = placed();
        let v = call(
            &mut s,
            "feature.add",
            json!({"part": "frame", "type": "mate", "id": "m1", "form": "revolute", "a": a,
                   "b": b, "a_offset": {"origin": ["20 mm", 0, 0]}, "b_offset": {"flip": true}}),
        )
        .unwrap();
        assert_eq!(
            v["assemblies"],
            json!([{"part": "frame", "ok": true, "moved": ["pin"]}])
        );
        assert!(!stale(&mut s));
        let show = call(
            &mut s,
            "feature.show",
            json!({"part": "frame", "id": "pin"}),
        )
        .unwrap();
        let z = show["pose"]["origin_m"][2].as_f64().unwrap();
        assert!((z - 0.005).abs() < 1e-9, "{z}");
        // The plate made thicker: the pin follows its top, written.
        let v = call(
            &mut s,
            "feature.edit",
            json!({"part": "plate", "id": "slab", "params": {"z": 8}}),
        )
        .unwrap();
        assert_eq!(v["assemblies"][0]["moved"], json!(["pin"]));
        assert!(!stale(&mut s));
        let show = call(
            &mut s,
            "feature.show",
            json!({"part": "frame", "id": "pin"}),
        )
        .unwrap();
        let z = show["pose"]["origin_m"][2].as_f64().unwrap();
        assert!((z - 0.008).abs() < 1e-9, "{z}");
        // The offset edited: the pin moves along.
        call(
            &mut s,
            "feature.edit",
            json!({"part": "frame", "id": "m1", "a_offset": {"origin": ["30 mm", 0, 0]}}),
        )
        .unwrap();
        assert!(!stale(&mut s));
        // A write that would make the mates conflict is refused, and leaves
        // the files as they were.
        let e = call(
            &mut s,
            "feature.add",
            json!({"part": "frame", "type": "mate", "id": "m2", "form": "revolute", "a": a,
                   "b": b, "a_offset": {"origin": ["40 mm", 0, 0]}, "b_offset": {"flip": true}}),
        )
        .unwrap_err();
        assert_eq!(e.code, Code::AssemblyOverconstrained);
        assert_eq!(e.details["conflicting"], json!([["m1", "m2"]]));
        let list = call(&mut s, "feature.list", json!({"part": "frame"})).unwrap();
        assert_eq!(list["features"].as_array().unwrap().len(), 3);
        // Deleting the mate leaves the pin where it is: nothing to solve.
        let v = call(
            &mut s,
            "feature.delete",
            json!({"part": "frame", "id": "m1"}),
        )
        .unwrap();
        assert!(v.get("assemblies").is_none());
    }
}

#[cfg(test)]
mod doctor_tests {
    use super::tests::{add_in, face, hinged};
    use crate::assembly::tests::id;
    use crate::server::tests::call;
    use serde_json::{Value, json};

    fn issue<'a>(r: &'a Value, code: &str) -> Option<&'a Value> {
        r["issues"]
            .as_array()
            .unwrap()
            .iter()
            .find(|i| i["code"] == code)
    }

    /// A daemon's doctor reads the session's rebuilds (issue #125: it
    /// rebuilt every assembly cold), and says what a cold doctor says.
    #[test]
    fn doctor_in_a_session_reuses_its_rebuilds() {
        let (dir, mut s) = hinged();
        let before = s.rebuilt(&id("frame")).unwrap().solve.clone().unwrap();
        let live = call(&mut s, "doctor", json!({})).unwrap();
        assert!(issue(&live, "assembly_stale").is_some());
        assert_eq!(live, crate::doctor::report(dir.path()).unwrap());
        let after = s.rebuilt(&id("frame")).unwrap().solve.clone().unwrap();
        assert!(std::sync::Arc::ptr_eq(&before, &after));
    }

    #[test]
    fn doctor_writes_stale_poses_and_check_finds_unsolved_mates() {
        let (dir, mut s) = hinged();
        // Stale: the pin is stored 50 mm off where the hinge holds it.
        let r = call(&mut s, "doctor", json!({})).unwrap();
        // The daemon's doctor rebuilds in its own session (issue #125), and
        // finds what a fresh one does.
        assert_eq!(r, crate::doctor::report(dir.path()).unwrap());
        let stale = issue(&r, "assembly_stale").expect("assembly_stale");
        assert_eq!(stale["severity"], "warning");
        assert_eq!(stale["files"], json!(["parts/frame/features/pin.json"]));
        let repair = stale["repairs"][0]["id"].clone();
        assert!(
            repair
                .as_str()
                .unwrap()
                .starts_with("assembly_stale:solve:")
        );
        // check reports it through doctor, and passes: it is a warning.
        let c = call(&mut s, "check", json!({"part": "frame"})).unwrap();
        assert!(
            c["findings"]
                .as_array()
                .unwrap()
                .iter()
                .any(|f| f["code"] == "assembly_stale")
        );
        call(&mut s, "doctor.repair", json!({"repairs": [repair]})).unwrap();
        assert!(
            !s.rebuilt(&id("frame"))
                .unwrap()
                .solve
                .as_ref()
                .unwrap()
                .stale()
        );
        let r = call(&mut s, "doctor", json!({})).unwrap();
        assert!(issue(&r, "assembly_stale").is_none());
        // A second hinge 5 mm along: check fails, naming the mates.
        let top = face(&mut s, "plate", "slab", [0.0, 0.0, 1.0]);
        let foot = face(&mut s, "bolt", "shank", [0.0, 0.0, -1.0]);
        add_in(
            dir.path(),
            "frame",
            "m2",
            "mate",
            json!({"form": "revolute",
                   "a": {"instance": ["base"], "at": {"feature": "slab", "name": top},
                         "offset": {"origin": ["25 mm", "10 mm", "0 mm"]}},
                   "b": {"instance": ["pin"], "at": {"feature": "shank", "name": foot},
                         "offset": {"flip": true}}}),
        );
        s.refresh().unwrap();
        let e = call(&mut s, "check", json!({"part": "frame"})).unwrap_err();
        let findings = e.details["findings"].as_array().unwrap();
        let unsolved: Vec<&Value> = findings
            .iter()
            .filter(|f| f["code"] == "assembly_unsolved")
            .collect();
        assert_eq!(unsolved.len(), 1);
        assert_eq!(unsolved[0]["mates"], json!(["m1", "m2"]));
        assert_eq!(unsolved[0]["conflicting"], json!([["m1", "m2"]]));
        assert!(!findings.iter().any(|f| f["code"] == "feature_failed"));
        // The tree places both instances, and says which mates fail
        // (issue #123).
        let t = call(&mut s, "assembly.tree", json!({"part": "frame"})).unwrap();
        let failing: Vec<&Value> = t["failing"].as_array().unwrap().iter().collect();
        assert_eq!(
            failing.iter().map(|f| &f["id"]).collect::<Vec<_>>(),
            [&json!("m1"), &json!("m2")]
        );
        assert_eq!(failing[0]["type"], "mate");
        assert_eq!(failing[0]["error"]["code"], "assembly_unsolved");
        let list = call(&mut s, "feature.list", json!({"part": "frame"})).unwrap();
        let ok: Vec<(&Value, &Value)> = list["features"]
            .as_array()
            .unwrap()
            .iter()
            .map(|f| (&f["id"], &f["ok"]))
            .collect();
        assert!(ok.contains(&(&json!("m2"), &json!(false))));
    }
}

#[cfg(test)]
mod diff_tests {
    use super::tests::hinged;
    use crate::server::tests::call;
    use serde_json::json;

    #[test]
    fn a_model_diff_says_how_the_solve_changed() {
        use crate::git::tests::git;
        let (dir, mut s) = hinged();
        let r = dir.path();
        git(r, &["init", "-q", "-b", "main"]);
        git(r, &["add", "-A"]);
        git(r, &["commit", "-q", "-m", "a hinge"]);
        // Driven to an angle, the pin can't turn any more.
        call(
            &mut s,
            "feature.edit",
            json!({"part": "frame", "id": "m1", "angle": "30 deg"}),
        )
        .unwrap();
        let d = call(&mut s, "diff", json!({"model": true})).unwrap();
        let f = &d["parts"]["frame"];
        assert_eq!(f["features"]["m1"]["status"], "changed");
        assert_eq!(
            f["solve"],
            json!({"from": {"state": "under", "dof": 1, "converged": true},
                   "to": {"state": "well", "dof": 0, "converged": true}})
        );
        assert_eq!(f["occurrences"]["moved"], json!(["pin"]));
    }
}
