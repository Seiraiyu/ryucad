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
            "a": doc_pose(&self.a).to_json(),
            "b": doc_pose(&self.b).to_json(),
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
                let a = resolve(id, "a", &m.a, &scope)?;
                let b = resolve(id, "b", &m.b, &scope)?;
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
    let solved = match ryumate::solve(&asm) {
        Ok(s) => s,
        Err(e) => {
            // Connectors' frames are orthonormal and inside the size box
            // by construction; a refusal is ryucad's fault.
            for id in &solved_mates {
                features.insert(
                    id.clone(),
                    Err(
                        Error::new(Code::Internal, format!("{id}: the solver refused it: {e}"))
                            .with("feature", id),
                    ),
                );
            }
            return None;
        }
    };
    let report = &solved.report;
    // Instances: solved poses, but where their mates fail, as stored.
    let mut moved = Vec::new();
    let mut at = asm.clone();
    for (k, id) in instances.iter().enumerate() {
        let k = k as u32;
        let failing = report.bodies[&k].state == ryumate::BodyState::Failing;
        let new = solved.poses[&k];
        if failing || new == asm.bodies[&k].pose {
            continue;
        }
        let Ok(rigid) = pose_rigid(&doc_pose(&new)) else {
            continue;
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
    // Mates: their state, and their frames where the bodies now are.
    let frame_at = |c: &ryumate::Connector| -> ryumate::Pose {
        match c.on {
            ryumate::On::Body(b) => at.bodies[&b].pose.then_after(&c.frame),
            ryumate::On::Copy {
                pattern,
                seed,
                index,
            } => ryumate::copy_poses(&at, &[(pattern, seed, index)])[0].then_after(&c.frame),
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
            (k, state, frame_at(&m.a), frame_at(&m.b))
        });
        let out = match out {
            Err(e) => Err(e),
            Ok((k, ryumate::MateState::Conflicting | ryumate::MateState::Failing, _, _)) => {
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
            Ok((_, state, a, b)) => {
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
    // Patterns: their copies where the seeds now are.
    for (id, r) in patterns {
        let out = r.map(|r| {
            let k = pattern_of(&id);
            let mut copies = Vec::new();
            let mut h = blake3::Hasher::new();
            for index in &r.copies {
                for seed in &r.seeds {
                    let b = body_of(seed);
                    let p = ryumate::copy_poses(&at, &[(k, b, *index)])[0];
                    let seed_placed = features[seed]
                        .as_ref()
                        .ok()
                        .and_then(|o| o.instance())
                        .expect("a seed placed");
                    let rigid = pose_rigid(&doc_pose(&p)).unwrap_or(seed_placed.pose);
                    let placed = placed_at(seed_placed, rigid);
                    h.update(format!("{index:?} {seed} {}\n", placed.hash).as_bytes());
                    copies.push(Copy {
                        index: *index,
                        seed: seed.clone(),
                        placed: Arc::new(placed),
                    });
                }
            }
            Output::Copies(Arc::new(Copies {
                copies,
                hash: h.finalize().to_hex()[..32].to_owned(),
            }))
        });
        features.insert(id, out);
    }
    Some(Arc::new(Solve {
        report: solved.report,
        instances,
        mates: solved_mates,
        moved,
    }))
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
