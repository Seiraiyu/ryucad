//! Exploded views of an assembly (`parts/<asm>/explodes/<id>.json`,
//! [`crate::doc::explode`]), resolved against the model it draws: which
//! occurrences each step moves, about which point it turns them, and where
//! a fraction of the way through puts each occurrence. An explode is a
//! display offset applied to each occurrence after the solve, on its
//! left: it never changes a pose, a mate, interference, clearance, mass,
//! a BOM or an export, none of which read it.
//!
//! A step's instance path reaches every occurrence under it (`p1`
//! reaches `p1/left` and `p1/right`), and a seed's instance reaches its
//! copies too (`pin` reaches `ring:2/pin`), as a copy moves with its
//! seed. A path that reaches nothing is an `explode_unresolved` warning;
//! the step moves the rest. A step turns what it moves about an axis
//! through the middle of their boxes as the earlier steps leave them,
//! then moves it; with `n` steps, a fraction `f` takes step `k` (from 0)
//! `clamp(f n - k, 0, 1)` of the way.

use crate::doc::files::{Feature, PartKind};
use crate::doc::id::Id;
use crate::error::{Code, Error, Result};
use crate::session::Session;
use ryukei::geom::{Point3, Rigid, UnitVector3, Vector3};
use serde_json::{Value, json};
use std::collections::BTreeMap;

/// One step, resolved: the occurrences it moves (paths as
/// [`crate::assembly::path_text`] writes them), how far, and the turn
/// about the middle of what it moves.
#[derive(Clone, Debug, PartialEq)]
pub struct Step {
    pub occurrences: Vec<String>,
    pub translate_m: [f64; 3],
    /// A unit axis and an angle in radians.
    pub rotate: Option<([f64; 3], f64)>,
    /// The middle of the moved occurrences' boxes once the steps before
    /// have moved them all the way.
    pub centre_m: [f64; 3],
}

/// An explode resolved against its assembly's model.
#[derive(Clone, Debug, PartialEq)]
pub struct Resolved {
    pub steps: Vec<Step>,
    /// `explode_unresolved`, one per instance path that reaches nothing.
    pub warnings: Vec<Value>,
}

/// Where an explode puts an occurrence, relative to where its assembly
/// does: applied after its pose.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Offset(pub Rigid);

impl Offset {
    pub const NONE: Offset = Offset(Rigid::IDENTITY);

    /// A point moved.
    pub fn apply(&self, p: [f64; 3]) -> [f64; 3] {
        self.0.apply_point(Point3::new(p[0], p[1], p[2])).to_array()
    }

    /// A direction turned.
    pub fn turn(&self, v: [f64; 3]) -> [f64; 3] {
        self.0
            .apply_vector(Vector3::new(v[0], v[1], v[2]))
            .to_array()
    }
}

/// How far each of `n` steps has gone at `fraction` (clamped to `[0, 1]`).
pub fn amounts(n: usize, fraction: f64) -> Vec<f64> {
    let f = if fraction.is_nan() {
        0.0
    } else {
        fraction.clamp(0.0, 1.0)
    };
    (0..n)
        .map(|k| (f * n as f64 - k as f64).clamp(0.0, 1.0))
        .collect()
}

impl Step {
    /// This step's motion `u` of the way: turned about its centre, then
    /// moved.
    pub fn motion(&self, u: f64) -> Rigid {
        let c = Vector3::new(self.centre_m[0], self.centre_m[1], self.centre_m[2]);
        let t = Vector3::new(
            self.translate_m[0] * u,
            self.translate_m[1] * u,
            self.translate_m[2] * u,
        );
        match self.rotate {
            Some((axis, angle)) if u != 0.0 => {
                let a = Vector3::new(axis[0], axis[1], axis[2])
                    .normalize()
                    .unwrap_or(UnitVector3::Z);
                Rigid::translation(c + t)
                    .then_after(&Rigid::rotation(a, angle * u))
                    .then_after(&Rigid::translation(-c))
            }
            _ => Rigid::translation(t),
        }
    }
}

/// Each moved occurrence's offset at `fraction` of the way through
/// `steps`, by path; occurrences no step moves are left out.
pub fn offsets(steps: &[Step], fraction: f64) -> BTreeMap<String, Offset> {
    let mut out: BTreeMap<String, Offset> = BTreeMap::new();
    for (s, u) in steps.iter().zip(amounts(steps.len(), fraction)) {
        if u == 0.0 {
            continue;
        }
        let m = s.motion(u);
        for o in &s.occurrences {
            let at = out.entry(o.clone()).or_insert(Offset::NONE);
            *at = Offset(m.then_after(&at.0));
        }
    }
    out
}

/// The way `start` (an occurrence's middle where its assembly puts it)
/// goes as `steps` move occurrence `path` up to `fraction`: where it
/// starts, then where each step it is in leaves it. Just the start when
/// nothing moves it.
pub fn trail(steps: &[Step], path: &str, start: [f64; 3], fraction: f64) -> Vec<[f64; 3]> {
    let mut at = Offset::NONE;
    let mut out = vec![start];
    for (s, u) in steps.iter().zip(amounts(steps.len(), fraction)) {
        if u == 0.0 || !s.occurrences.iter().any(|o| o == path) {
            continue;
        }
        at = Offset(s.motion(u).then_after(&at.0));
        out.push(at.apply(start));
    }
    out
}

/// Whether instance path `step` reaches the occurrence at `path`: it is
/// the path or one above it, or it is `seeds`' instance and the
/// occurrence is a copy of it (`<pattern>:<i>/<step>…`, the pattern's
/// seeds holding the step's first instance).
pub fn reaches(step: &str, path: &str, seeds: &BTreeMap<String, Vec<String>>) -> bool {
    let under = |p: &str| p == step || p.starts_with(&format!("{step}/"));
    if under(path) {
        return true;
    }
    let first = step.split('/').next().unwrap_or_default();
    match path.split_once('/') {
        Some((head, rest)) => match head.split_once(':') {
            Some((pattern, _)) => {
                seeds
                    .get(pattern)
                    .is_some_and(|s| s.iter().any(|x| x == first))
                    && under(rest)
            }
            None => false,
        },
        None => false,
    }
}

/// The box of `pose`'s placement of `extent`, as `offset` then moves it.
fn placed_box(
    lo: [f64; 3],
    hi: [f64; 3],
    pose: &Rigid,
    offset: &Offset,
    into: &mut Option<([f64; 3], [f64; 3])>,
) {
    for i in 0..8 {
        let c = [0, 1, 2].map(|k| if i >> k & 1 == 0 { lo[k] } else { hi[k] });
        let p = offset.apply(pose.apply_point(Point3::new(c[0], c[1], c[2])).to_array());
        let b = into.get_or_insert((p, p));
        for k in 0..3 {
            b.0[k] = b.0[k].min(p[k]);
            b.1[k] = b.1[k].max(p[k]);
        }
    }
}

/// `explode_not_found` for `id` in `asm`, naming the explodes it has.
pub fn not_found(session: &Session, asm: &Id, id: &str) -> Error {
    let known: Vec<String> = session
        .project()
        .part(asm)
        .map(|p| p.explodes.keys().map(Id::to_string).collect())
        .unwrap_or_default();
    Error::new(
        Code::ExplodeNotFound,
        format!("{asm} has no exploded view {id}"),
    )
    .with("part", asm.as_str())
    .with("explode", id)
    .with("explodes", known)
}

/// The explode `id` of assembly `asm` resolved against the assembly's
/// model as the working files have it.
pub fn resolve(session: &mut Session, asm: &Id, id: &str) -> Result<Resolved> {
    let part = session.project().part(asm)?;
    if crate::server::part_kind(part) != PartKind::Assembly {
        return Err(crate::doc::explode::not_an_assembly(asm.as_str()));
    }
    let file = match Id::parse(id, "explode")
        .ok()
        .and_then(|k| part.explodes.get(&k))
    {
        Some(Ok(f)) => f.clone(),
        Some(Err(e)) => return Err(e.clone()),
        None => return Err(not_found(session, asm, id)),
    };
    // Each instance pattern's seeds, by pattern.
    let seeds: BTreeMap<String, Vec<String>> = part
        .order()
        .iter()
        .filter_map(|f| match part.features.get(f) {
            Some(Ok(Feature::InstancePattern(p))) => Some((
                f.to_string(),
                p.instances.iter().map(Id::to_string).collect(),
            )),
            _ => None,
        })
        .collect();
    let model = session.model(asm, &crate::doc::assembly::Rev::Working)?;
    // Each occurrence's box where its assembly puts it.
    let boxes: Vec<(String, Rigid, Option<([f64; 3], [f64; 3])>)> = model
        .occurrences
        .iter()
        .map(|o| {
            let mut local: Option<([f64; 3], [f64; 3])> = None;
            for s in &o.solids {
                if let Some((lo, hi)) = crate::inspect::body_extent(&s.built.body) {
                    let b = local.get_or_insert((lo, hi));
                    for k in 0..3 {
                        b.0[k] = b.0[k].min(lo[k]);
                        b.1[k] = b.1[k].max(hi[k]);
                    }
                }
            }
            (crate::assembly::path_text(&o.path), o.pose, local)
        })
        .collect();
    let mut warnings = Vec::new();
    let mut steps: Vec<Step> = Vec::with_capacity(file.steps.len());
    for (k, s) in file.steps.iter().enumerate() {
        let mut occurrences: Vec<String> = Vec::new();
        for (i, path) in s.instances.iter().enumerate() {
            let reached: Vec<&String> = boxes
                .iter()
                .map(|(p, ..)| p)
                .filter(|p| reaches(path, p, &seeds))
                .collect();
            if reached.is_empty() {
                warnings.push(json!({
                    "code": Code::ExplodeUnresolved,
                    "message": format!("step {} of {asm}'s explode {id} names {path}, which {asm} no longer places", k + 1),
                    "part": asm.as_str(),
                    "explode": id,
                    "pointer": format!("/steps/{k}/instances/{i}"),
                    "instance": path,
                }));
            }
            for p in reached {
                if !occurrences.contains(p) {
                    occurrences.push(p.clone());
                }
            }
        }
        // Where the steps before leave what this one moves.
        let before = offsets(&steps, 1.0);
        let mut b = None;
        for (p, pose, local) in &boxes {
            if let (true, Some((lo, hi))) = (occurrences.contains(p), local) {
                let off = before.get(p).copied().unwrap_or(Offset::NONE);
                placed_box(*lo, *hi, pose, &off, &mut b);
            }
        }
        let centre_m = b.map_or([0.0; 3], |(lo, hi)| {
            [0, 1, 2].map(|k| (lo[k] + hi[k]) / 2.0)
        });
        let translate_m = s
            .translate
            .as_ref()
            .map_or([0.0; 3], |t| [t[0].meters(), t[1].meters(), t[2].meters()]);
        let rotate = s.rotate.as_ref().map(|r| {
            let n = r.axis.iter().map(|x| x * x).sum::<f64>().sqrt();
            (r.axis.map(|x| x / n), r.angle.radians())
        });
        steps.push(Step {
            occurrences,
            translate_m,
            rotate,
            centre_m,
        });
    }
    Ok(Resolved { steps, warnings })
}

impl Step {
    /// How `explode.list` gives it: what it moves and how, in meters and
    /// radians.
    pub fn to_json(&self) -> Value {
        let mut v = json!({
            "occurrences": self.occurrences,
            "translate_m": self.translate_m,
            "centre_m": self.centre_m,
        });
        if let Some((axis, angle)) = self.rotate {
            v["rotate"] = json!({"axis": axis, "angle_rad": angle});
        }
        v
    }

    /// A step as [`Step::to_json`] wrote it.
    pub fn from_json(v: &Value) -> Option<Step> {
        let three = |v: &Value| -> Option<[f64; 3]> {
            let a = v.as_array()?;
            Some([
                a.first()?.as_f64()?,
                a.get(1)?.as_f64()?,
                a.get(2)?.as_f64()?,
            ])
        };
        Some(Step {
            occurrences: v["occurrences"]
                .as_array()?
                .iter()
                .filter_map(|x| x.as_str().map(str::to_owned))
                .collect(),
            translate_m: three(&v["translate_m"])?,
            centre_m: three(&v["centre_m"])?,
            rotate: match v.get("rotate") {
                Some(r) => Some((three(&r["axis"])?, r["angle_rad"].as_f64()?)),
                None => None,
            },
        })
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use crate::server::tests::{call, fresh};

    /// Part `bolt` (a 4 mm shank 10 mm tall), part `plate` (60 × 30 × 6),
    /// assembly `pair` (two bolts, `left` and `right`) and assembly
    /// `frame` (the plate as `base`, pairs `p1` and `p2`, a bolt `pin`
    /// and a row of two more copies of it, `row`).
    pub(crate) fn project() -> (tempfile::TempDir, Session) {
        let (dir, mut s) = fresh();
        let mut ok = |m: &str, v: Value| {
            call(&mut s, m, v).unwrap_or_else(|e| panic!("{m}: {e}"));
        };
        ok("part.create", json!({"part": "bolt"}));
        ok(
            "feature.add",
            json!({"part": "bolt", "type": "cylinder", "id": "shank",
                   "params": {"radius": 2, "height": 10}}),
        );
        ok("part.create", json!({"part": "plate"}));
        ok(
            "feature.add",
            json!({"part": "plate", "type": "box", "id": "body",
                   "params": {"x": 60, "y": 30, "z": 6}}),
        );
        for a in ["pair", "frame"] {
            ok("part.create", json!({"part": a, "kind": "assembly"}));
        }
        for (id, x) in [("left", -8), ("right", 8)] {
            ok(
                "feature.add",
                json!({"part": "pair", "type": "instance", "id": id, "of": "bolt",
                       "placement": {"origin": [x, 0, 0]}}),
            );
        }
        ok(
            "feature.add",
            json!({"part": "frame", "type": "instance", "id": "base", "of": "plate"}),
        );
        for (id, x) in [("p1", 15), ("p2", 45)] {
            ok(
                "feature.add",
                json!({"part": "frame", "type": "instance", "id": id, "of": "pair",
                       "placement": {"origin": [x, 15, 6]}}),
            );
        }
        ok(
            "feature.add",
            json!({"part": "frame", "type": "instance", "id": "pin", "of": "bolt",
                   "placement": {"origin": [5, 5, 6]}}),
        );
        ok(
            "feature.add",
            json!({"part": "frame", "type": "instance_pattern", "id": "row",
                   "instances": ["pin"], "direction": "x", "spacing": "10 mm", "count": 3}),
        );
        (dir, s)
    }

    /// Writes explode `id` of `frame` as `steps`.
    pub(crate) fn write(dir: &std::path::Path, id: &str, steps: Value) {
        let at = dir.join("parts/frame/explodes");
        std::fs::create_dir_all(&at).unwrap();
        crate::doc::project::write_json(
            &at.join(format!("{id}.json")),
            &json!({"format": 1, "kind": "ryucad.explode", "steps": steps}),
        )
        .unwrap();
    }

    fn frame() -> Id {
        Id::parse("frame", "part").unwrap()
    }

    #[test]
    fn steps_reach_occurrences_under_them_and_copies_of_seeds() {
        let seeds = BTreeMap::from([("row".to_owned(), vec!["pin".to_owned()])]);
        assert!(reaches("p1", "p1/left", &seeds));
        assert!(reaches("p1/left", "p1/left", &seeds));
        assert!(!reaches("p1/left", "p1/right", &seeds));
        assert!(!reaches("p1", "p10", &seeds));
        assert!(reaches("pin", "row:1/pin", &seeds));
        assert!(reaches("row:1/pin", "row:1/pin", &seeds));
        assert!(!reaches("row:1/pin", "row:2/pin", &seeds));
        assert!(!reaches("base", "row:1/pin", &seeds));
        let (dir, mut s) = project();
        write(
            dir.path(),
            "apart",
            json!([
                {"instances": ["p1", "pin"], "translate": [0, 0, 20]},
                {"instances": ["gone", "base"], "translate": [0, -40, 0]}
            ]),
        );
        s.refresh().unwrap();
        let r = resolve(&mut s, &frame(), "apart").unwrap();
        assert_eq!(
            r.steps[0].occurrences,
            ["p1/left", "p1/right", "pin", "row:1/pin", "row:2/pin"]
        );
        assert_eq!(r.steps[0].translate_m, [0.0, 0.0, 0.02]);
        assert_eq!(r.steps[1].occurrences, ["base"]);
        // The unresolved path is a warning; the step moves the rest.
        assert_eq!(r.warnings.len(), 1, "{:?}", r.warnings);
        assert_eq!(r.warnings[0]["code"], "explode_unresolved");
        assert_eq!(r.warnings[0]["instance"], "gone");
        assert_eq!(r.warnings[0]["pointer"], "/steps/1/instances/0");
        // The plate's middle, where it is.
        assert_eq!(r.steps[1].centre_m, [0.03, 0.015, 0.003]);
    }

    #[test]
    fn fractions_take_steps_in_turn() {
        assert_eq!(amounts(2, 0.0), [0.0, 0.0]);
        assert_eq!(amounts(2, 0.25), [0.5, 0.0]);
        assert_eq!(amounts(2, 0.5), [1.0, 0.0]);
        assert_eq!(amounts(2, 0.75), [1.0, 0.5]);
        assert_eq!(amounts(2, 2.0), [1.0, 1.0]);
        assert_eq!(amounts(3, f64::NAN), [0.0; 3]);
        let step = |o: &[&str], t: [f64; 3]| Step {
            occurrences: o.iter().map(|x| x.to_string()).collect(),
            translate_m: t,
            rotate: None,
            centre_m: [0.0; 3],
        };
        let steps = [
            step(&["a", "b"], [0.0, 0.0, 1.0]),
            step(&["b"], [1.0, 0.0, 0.0]),
        ];
        let half = offsets(&steps, 0.5);
        assert_eq!(half["a"].apply([0.0; 3]), [0.0, 0.0, 1.0]);
        assert_eq!(half["b"].apply([0.0; 3]), [0.0, 0.0, 1.0]);
        let all = offsets(&steps, 1.0);
        assert_eq!(all["b"].apply([0.0; 3]), [1.0, 0.0, 1.0]);
        assert!(offsets(&steps, 0.0).is_empty());
        assert_eq!(
            trail(&steps, "b", [0.0; 3], 0.75),
            [[0.0; 3], [0.0, 0.0, 1.0], [0.5, 0.0, 1.0]]
        );
        assert_eq!(trail(&steps, "c", [1.0; 3], 1.0), [[1.0; 3]]);
    }

    #[test]
    fn a_turn_is_about_the_middle_of_what_it_moves() {
        let (dir, mut s) = project();
        write(
            dir.path(),
            "spin",
            json!([
                {"instances": ["base"], "translate": [0, 0, -50]},
                {"instances": ["base"], "rotate": {"axis": [0, 0, 2], "angle": "180 deg"}}
            ]),
        );
        s.refresh().unwrap();
        let r = resolve(&mut s, &frame(), "spin").unwrap();
        // Its middle after the first step.
        let c = r.steps[1].centre_m;
        assert!(
            (c[0] - 0.03).abs() < 1e-12 && (c[2] + 0.047).abs() < 1e-12,
            "{c:?}"
        );
        assert_eq!(r.steps[1].rotate.unwrap().0, [0.0, 0.0, 1.0]);
        let all = offsets(&r.steps, 1.0);
        // The middle stays put; a corner goes to the opposite corner.
        let m = all["base"].apply([0.03, 0.015, 0.003]);
        assert!(
            (m[0] - 0.03).abs() < 1e-12 && (m[1] - 0.015).abs() < 1e-12,
            "{m:?}"
        );
        assert!((m[2] + 0.047).abs() < 1e-12, "{m:?}");
        let corner = all["base"].apply([0.0, 0.0, 0.0]);
        assert!((corner[0] - 0.06).abs() < 1e-12 && (corner[1] - 0.03).abs() < 1e-12);
        // As JSON and back.
        let back: Vec<Step> = r
            .steps
            .iter()
            .map(|s| Step::from_json(&s.to_json()).unwrap())
            .collect();
        assert_eq!(back, r.steps);
    }

    #[test]
    fn what_can_t_be_resolved_says_why() {
        let (dir, mut s) = project();
        let e = resolve(&mut s, &frame(), "nothing").unwrap_err();
        assert_eq!(e.code, Code::ExplodeNotFound);
        write(
            dir.path(),
            "apart",
            json!([{"instances": ["p1"], "translate": [0, 0, 1]}]),
        );
        s.refresh().unwrap();
        let e = resolve(&mut s, &frame(), "nothing").unwrap_err();
        assert_eq!(e.details["explodes"], json!(["apart"]));
        let bolt = Id::parse("bolt", "part").unwrap();
        assert_eq!(
            resolve(&mut s, &bolt, "apart").unwrap_err().code,
            Code::WrongPartKind
        );
        write(dir.path(), "broken", json!([]));
        s.refresh().unwrap();
        let e = resolve(&mut s, &frame(), "broken").unwrap_err();
        assert_eq!(e.code, Code::InvalidDocument);
    }
}
