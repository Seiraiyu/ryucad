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

use crate::doc::files::{Feature, PartKind};
use crate::doc::id::Id;
use crate::doc::motion::{Driven, MotionFile};
use crate::doc::project::Part;
use crate::error::{Code, Error, Result};
use crate::rebuild::{Output, PartResult};
use crate::session::Session;
use serde_json::{Value, json};

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
}
