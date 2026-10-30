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

/// Each occurrence of `model` by path, where its assembly puts it, and
/// the box of its solids in its part's frame (`None` for none).
type Boxes = Vec<(String, Rigid, Option<([f64; 3], [f64; 3])>)>;

fn boxes(model: &crate::assembly::Model) -> Boxes {
    model
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
        .collect()
}

/// The middle of each occurrence's box where its assembly puts it, by
/// path: where its trail starts.
pub fn middles(model: &crate::assembly::Model) -> BTreeMap<String, [f64; 3]> {
    boxes(model)
        .into_iter()
        .filter_map(|(p, pose, local)| {
            let (lo, hi) = local?;
            let mut b = None;
            placed_box(lo, hi, &pose, &Offset::NONE, &mut b);
            let (lo, hi) = b?;
            Some((p, [0, 1, 2].map(|k| (lo[k] + hi[k]) / 2.0)))
        })
        .collect()
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
    let boxes = boxes(&model);
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

/// The exploded view a participant's window shows: an assembly's
/// explode, a fraction of the way through.
#[derive(Clone, Debug, PartialEq, serde::Serialize)]
pub struct Shown {
    pub part: Id,
    pub explode: Id,
    pub fraction: f64,
}

/// One explode as `explode.list` gives it: its steps as written, and as
/// they resolve now with their warnings, or the file's error.
fn entry(session: &mut Session, part: &Id, id: &Id) -> Value {
    let file = match &session
        .project()
        .part(part)
        .map(|p| p.explodes.get(id).cloned())
    {
        Ok(Some(Ok(f))) => f.clone(),
        Ok(Some(Err(e))) => return json!({"part": part, "explode": id, "error": e}),
        _ => {
            return json!({"part": part, "explode": id, "error": not_found(session, part, id.as_str())});
        }
    };
    let steps = file.to_json()["steps"].take();
    match resolve(session, part, id.as_str()) {
        Ok(r) => json!({
            "part": part,
            "explode": id,
            "steps": steps,
            "resolved": r.steps.iter().map(Step::to_json).collect::<Vec<_>>(),
            "warnings": r.warnings,
        }),
        Err(e) => json!({"part": part, "explode": id, "steps": steps, "error": e}),
    }
}

/// The explodes of assembly `part`, or of every assembly, with each listed
/// assembly's revision.
pub fn list(session: &mut Session, part: Option<&str>) -> Result<Value> {
    let parts: Vec<Id> = match part {
        Some(p) => {
            let id = crate::server::part_id(p)?;
            let found = session.project().part(&id)?;
            if crate::server::part_kind(found) != PartKind::Assembly {
                return Err(crate::doc::explode::not_an_assembly(p));
            }
            vec![id]
        }
        None => session.project().parts.keys().cloned().collect(),
    };
    let mut explodes = Vec::new();
    let mut revisions = serde_json::Map::new();
    for id in &parts {
        let p = session.project().part(id)?;
        if part.is_some() || !p.explodes.is_empty() {
            revisions.insert(id.to_string(), json!(p.revision));
        }
        let ids: Vec<Id> = p.explodes.keys().cloned().collect();
        for e in ids {
            explodes.push(entry(session, id, &e));
        }
    }
    Ok(json!({"explodes": explodes, "revisions": revisions}))
}

fn path(session: &Session, part: &Id, explode: &Id) -> std::path::PathBuf {
    crate::doc::project::part_dir(session.root(), part)
        .join(crate::doc::explode::explode_file(explode))
}

/// The assembly `part`, refusing a part (`wrong_part_kind`) and a stale
/// revision.
fn assembly(session: &Session, part: &str, expected: Option<&str>) -> Result<Id> {
    let pid = crate::server::part_id(part)?;
    let p = session.project().part(&pid)?;
    if crate::server::part_kind(p) != PartKind::Assembly {
        return Err(crate::doc::explode::not_an_assembly(part));
    }
    crate::server::check_revision(p, expected)?;
    Ok(pid)
}

/// Saves `steps` (as a file writes them) as `part`'s explode `explode`,
/// replacing one of that ID; read against the assembly's parameters
/// first, so what doesn't read is never written.
pub fn save(
    session: &mut Session,
    part: &str,
    explode: &str,
    steps: &Value,
    expected: Option<&str>,
) -> Result<Value> {
    let pid = assembly(session, part, expected)?;
    let eid = crate::doc::explode::check_id(explode)?;
    let p = session.project().part(&pid)?;
    let replaced = p.explodes.contains_key(&eid);
    let unit = session.project().manifest.default_unit;
    let doc = json!({
        "format": crate::doc::explode::FORMAT,
        "kind": crate::doc::explode::KIND,
        "steps": steps,
    });
    let file = crate::doc::explode::ExplodeFile::from_json(&doc, unit, &p.env).map_err(|e| {
        Error::new(Code::InvalidParam, e.message.clone())
            .with("pointer", e.details["pointer"].clone())
    })?;
    crate::doc::project::write_json(&path(session, &pid, &eid), &file.to_json())?;
    session.reload()?;
    let mut v = entry(session, &pid, &eid);
    v["replaced"] = json!(replaced);
    v["revision"] = json!(session.project().part(&pid)?.revision);
    Ok(v)
}

/// Deletes `part`'s explode `explode`; a file that doesn't read can be
/// deleted too.
pub fn delete(
    session: &mut Session,
    part: &str,
    explode: &str,
    expected: Option<&str>,
) -> Result<Value> {
    let pid = assembly(session, part, expected)?;
    let p = session.project().part(&pid)?;
    let eid = Id::parse(explode, "explode")
        .ok()
        .filter(|e| p.explodes.contains_key(e))
        .ok_or_else(|| not_found(session, &pid, explode))?;
    crate::doc::project::remove_file(session.root(), &path(session, &pid, &eid))?;
    session.reload()?;
    let revision = session.project().part(&pid)?.revision.clone();
    Ok(json!({"part": pid, "explode": eid, "revision": revision}))
}

/// What `explode.show` asks: an explode of an assembly (`part`, else the
/// participant's focus, else the project's model when that is one
/// assembly) and a fraction, or with only `fraction` the one shown, or
/// `off`.
#[derive(Clone, Debug, Default)]
pub struct ShowAsk {
    pub part: Option<String>,
    pub explode: Option<String>,
    pub fraction: Option<f64>,
    pub off: bool,
}

/// What a participant shows after `ask`, given what it shows now and its
/// focus.
pub fn show(
    session: &mut Session,
    now: Option<&Shown>,
    focus: Option<&Id>,
    ask: &ShowAsk,
) -> Result<Option<Shown>> {
    if ask.off {
        if ask.explode.is_some() || ask.fraction.is_some() {
            return Err(Error::new(
                Code::Usage,
                "give an explode to show, or off, not both",
            ));
        }
        return Ok(None);
    }
    if let Some(f) = ask.fraction
        && !(0.0..=1.0).contains(&f)
    {
        return Err(Error::new(
            Code::InvalidParam,
            "an explode's fraction is from 0 (assembled) to 1 (exploded)",
        )
        .with("fraction", f));
    }
    let (part, explode) = match (&ask.explode, now) {
        (Some(e), _) => {
            let part =
                crate::views::part_for(session, ask.part.as_deref(), focus)?.ok_or_else(|| {
                    Error::new(
                        Code::InvalidParam,
                        "the project shows several parts: name the assembly whose explode it is",
                    )
                })?;
            (part, e.clone())
        }
        (None, Some(n)) if ask.part.is_none() => (n.part.clone(), n.explode.to_string()),
        _ => {
            return Err(Error::new(
                Code::Usage,
                "name an explode to show (none is shown now), or off",
            ));
        }
    };
    // It must resolve now.
    resolve(session, &part, &explode)?;
    let fraction = ask
        .fraction
        .or(now
            .filter(|n| n.part == part && n.explode.as_str() == explode)
            .map(|n| n.fraction))
        .unwrap_or(1.0);
    Ok(Some(Shown {
        part,
        explode: Id::parse(&explode, "explode")?,
        fraction,
    }))
}

/// `explode.show`'s answer: who, what it shows, and how that resolves.
pub fn shown_json(session: &mut Session, participant: &str, shown: Option<&Shown>) -> Value {
    let mut v = json!({"participant": participant, "explode": shown});
    if let Some(s) = shown
        && let Ok(r) = resolve(session, &s.part, s.explode.as_str())
    {
        v["resolved"] = json!(r.steps.iter().map(Step::to_json).collect::<Vec<_>>());
        v["warnings"] = json!(r.warnings);
    }
    v
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

    #[test]
    fn explodes_are_saved_listed_and_deleted() {
        let (_dir, mut s) = project();
        let rev =
            call(&mut s, "explode.list", json!({"part": "frame"})).unwrap()["revisions"]["frame"]
                .as_str()
                .unwrap()
                .to_owned();
        let steps = json!([{"instances": ["p1"], "translate": [0, 0, 20]}]);
        let v = call(
            &mut s,
            "explode.save",
            json!({"part": "frame", "explode": "apart", "steps": steps,
                   "expected_revision": rev}),
        )
        .unwrap();
        assert_eq!(v["replaced"], false);
        assert_eq!(
            v["steps"],
            json!([{"instances": ["p1"], "translate": ["0 mm", "0 mm", "20 mm"]}])
        );
        assert_eq!(
            v["resolved"][0]["occurrences"],
            json!(["p1/left", "p1/right"])
        );
        assert_ne!(v["revision"], json!(rev));
        // A stale revision, a part, steps that don't read: nothing written.
        let e = call(
            &mut s,
            "explode.save",
            json!({"part": "frame", "explode": "other", "steps": steps,
                   "expected_revision": rev}),
        )
        .unwrap_err();
        assert_eq!(e.code, Code::Conflict);
        let e = call(
            &mut s,
            "explode.save",
            json!({"part": "bolt", "explode": "apart", "steps": steps}),
        )
        .unwrap_err();
        assert_eq!(e.code, Code::WrongPartKind);
        let e = call(
            &mut s,
            "explode.save",
            json!({"part": "frame", "explode": "bad",
                   "steps": [{"instances": ["p1"], "translate": [0, 0]}]}),
        )
        .unwrap_err();
        assert_eq!(
            (e.code, &e.details["pointer"]),
            (Code::InvalidParam, &json!("/steps/0/translate"))
        );
        let e = call(
            &mut s,
            "explode.save",
            json!({"part": "frame", "explode": "Bad", "steps": steps}),
        )
        .unwrap_err();
        assert_eq!(e.code, Code::InvalidId);
        // Replaced, with a step naming an instance that isn't there.
        let v = call(
            &mut s,
            "explode.save",
            json!({"part": "frame", "explode": "apart",
                   "steps": [{"instances": ["p1", "nope"], "translate": [0, 0, 20]}]}),
        )
        .unwrap();
        assert_eq!(v["replaced"], true);
        assert_eq!(v["warnings"][0]["code"], "explode_unresolved");
        let list = call(&mut s, "explode.list", json!({})).unwrap();
        assert_eq!(list["explodes"].as_array().unwrap().len(), 1, "{list}");
        assert_eq!(list["explodes"][0]["explode"], "apart");
        assert!(list["revisions"].get("bolt").is_none(), "{list}");
        assert_eq!(
            call(&mut s, "explode.list", json!({"part": "bolt"}))
                .unwrap_err()
                .code,
            Code::WrongPartKind
        );
        let e = call(
            &mut s,
            "explode.delete",
            json!({"part": "frame", "explode": "other"}),
        )
        .unwrap_err();
        assert_eq!(
            (e.code, &e.details["explodes"]),
            (Code::ExplodeNotFound, &json!(["apart"]))
        );
        call(
            &mut s,
            "explode.delete",
            json!({"part": "frame", "explode": "apart"}),
        )
        .unwrap();
        assert_eq!(
            call(&mut s, "explode.list", json!({})).unwrap()["explodes"],
            json!([])
        );
        assert!(!_dir.path().join("parts/frame/explodes").exists());
    }

    #[test]
    fn an_explode_changes_nothing_but_what_is_drawn() {
        use crate::live::{Kind, Live};
        use crate::server::tests::call_live;
        let (dir, mut s) = project();
        let read = |s: &mut Session| {
            let built = call(s, "rebuild", json!({"part": "frame"})).unwrap();
            let mut clash = call(s, "interference", json!({"part": "frame"})).unwrap();
            // The explode's file is the assembly's: its revision moves.
            clash.as_object_mut().unwrap().remove("revision");
            let bom = call(s, "bom.get", json!({"part": "frame", "flat": true})).unwrap();
            let step = crate::export::tests::exported(
                s,
                json!({"part": "frame", "time_stamp": "2026-10-29T00:00:00"}),
            )
            .unwrap()
            .1;
            (
                built["parts"][0]["occurrences"].clone(),
                clash,
                bom["flat"].clone(),
                step,
            )
        };
        let before = read(&mut s);
        assert_eq!(before.0.as_array().map(Vec::len), Some(8), "{:?}", before.0);
        assert!(before.1["pairs"].as_u64().unwrap() > 0);
        assert!(!before.2.as_array().unwrap().is_empty());
        write(
            dir.path(),
            "apart",
            json!([{"instances": ["p1", "pin"], "translate": [0, 0, 30]},
                   {"instances": ["base"], "rotate": {"axis": [1, 0, 0], "angle": "30 deg"}}]),
        );
        s.refresh().unwrap();
        let mut live = Live::new();
        live.hello(Kind::Human, Some("ann"), None).unwrap();
        call_live(
            &mut s,
            &mut live,
            None,
            "explode.show",
            json!({"explode": "apart"}),
        )
        .unwrap();
        let after = read(&mut s);
        assert_eq!(after.0, before.0, "poses");
        assert_eq!(after.1, before.1, "interference");
        assert_eq!(after.2, before.2, "BOM");
        assert_eq!(after.3, before.3, "STEP");
    }

    #[test]
    fn a_participants_window_shows_an_explode() {
        use crate::live::{Kind, Live};
        use crate::server::tests::call_live;
        let (dir, mut s) = project();
        write(
            dir.path(),
            "apart",
            json!([{"instances": ["p1"], "translate": [0, 0, 20]},
                   {"instances": ["base"], "translate": [0, 0, -20]}]),
        );
        s.refresh().unwrap();
        let mut live = Live::new();
        let human = live.hello(Kind::Human, Some("ann"), None).unwrap();
        let show = |s: &mut Session, live: &mut Live, v: Value| {
            call_live(s, live, None, "explode.show", v)
        };
        // The project's model is `frame`, its one root assembly.
        let v = show(&mut s, &mut live, json!({"explode": "apart"})).unwrap();
        assert_eq!(v["participant"], json!(human));
        assert_eq!(
            v["explode"],
            json!({"part": "frame", "explode": "apart", "fraction": 1.0})
        );
        assert_eq!(v["resolved"].as_array().unwrap().len(), 2);
        assert_eq!(live.get(&human).unwrap().to_json()["explode"], v["explode"]);
        // A fraction alone moves the one shown.
        let v = show(&mut s, &mut live, json!({"fraction": 0.25})).unwrap();
        assert_eq!(v["explode"]["fraction"], 0.25);
        let e = show(&mut s, &mut live, json!({"fraction": 1.5})).unwrap_err();
        assert_eq!(e.code, Code::InvalidParam);
        let e = show(&mut s, &mut live, json!({"explode": "nothing"})).unwrap_err();
        assert_eq!(e.code, Code::ExplodeNotFound);
        assert_eq!(
            live.get(&human)
                .unwrap()
                .view
                .explode
                .as_ref()
                .unwrap()
                .fraction,
            0.25
        );
        let e = show(
            &mut s,
            &mut live,
            json!({"explode": "apart", "part": "bolt"}),
        )
        .unwrap_err();
        assert_eq!(e.code, Code::WrongPartKind);
        let v = show(&mut s, &mut live, json!({"off": true})).unwrap();
        assert_eq!(v["explode"], Value::Null);
        let e = show(&mut s, &mut live, json!({"fraction": 0.5})).unwrap_err();
        assert_eq!(e.code, Code::Usage);
        let e = call(&mut s, "explode.show", json!({"explode": "apart"})).unwrap_err();
        assert_eq!(e.code, Code::NoLiveSession);
    }

    #[test]
    fn renders_draw_explodes_and_nothing_else_moves() {
        use crate::live::{Kind, Live};
        let (dir, s) = project();
        drop(s);
        let r = dir.path();
        write(
            r,
            "apart",
            json!([{"instances": ["p1", "pin"], "translate": [0, 0, 30]},
                   {"instances": ["base"], "rotate": {"axis": [1, 0, 0], "angle": "30 deg"}}]),
        );
        let small = json!({"part": "frame", "width": 64, "height": 48});
        let mut live = Live::new();
        let human = live.hello(Kind::Human, Some("ann"), None).unwrap();
        let mut slot = Some(Session::open(r).unwrap());
        let mut ask = |me: Option<&str>, m: &str, v: Value| {
            let req = crate::proto::Request::from_rpc(m, Some(v)).unwrap();
            crate::server::handle_safely(&mut slot, Some(&mut live), me, r, req)
        };
        let poses =
            |ask: &mut dyn FnMut(Option<&str>, &str, Value) -> Result<crate::proto::Answer>| {
                ask(None, "rebuild", json!({"part": "frame"}))
                    .unwrap()
                    .result
            };
        let before = poses(&mut ask);
        let whole = ask(None, "render", small.clone()).unwrap().binary;
        let mut named = small.clone();
        named["explode"] = json!("apart");
        let apart = ask(None, "render", named.clone()).unwrap().binary;
        assert_ne!(whole, apart);
        named["fraction"] = json!(0.0);
        assert_eq!(ask(None, "render", named.clone()).unwrap().binary, whole);
        named["fraction"] = json!(0.5);
        let half = ask(None, "render", named.clone()).unwrap().binary;
        assert!(half != whole && half != apart);
        named["fraction"] = json!(2.0);
        assert_eq!(
            ask(None, "render", named).unwrap_err().code,
            Code::InvalidParam
        );
        // A window's explode, by default; "" draws none.
        ask(None, "explode.show", json!({"explode": "apart"})).unwrap();
        assert_eq!(
            ask(Some(&human), "render", small.clone()).unwrap().binary,
            apart
        );
        let mut none = small.clone();
        none["explode"] = json!("");
        assert_eq!(ask(None, "render", none).unwrap().binary, whole);
        // Nothing an explode shows moves the assembly.
        assert_eq!(poses(&mut ask), before);
        let mut part = small;
        part["part"] = json!("bolt");
        part["explode"] = json!("apart");
        assert_eq!(
            ask(None, "render", part).unwrap_err().code,
            Code::WrongPartKind
        );
    }
}
