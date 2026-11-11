//! A case's stages, each one or more `ryucad` commands in folders of the
//! case's own, stopping at the first that fails. An `open` case (a pulled
//! file whose bytes aren't what its source listed) fails its one stage,
//! `open`, without running anything.
//!
//! A body: `import` (healing and rebuild), `check`, `export`, `reimport`
//! (the export into a second project: the same counts), `volume` (both
//! projects' volumes agree within their bounds), `fixpoint` (the second
//! project exported in meters, imported into a third and exported again:
//! the same bytes). A failed import is tried once more under another part ID
//! (`key`).
//!
//! On a binary whose `import` takes `--solid` (design 364), every stage
//! above asks for healed solids, so it judges healing as it always did,
//! and a body case then runs the reference stages whatever the others
//! gave: `ref_import` (the body as a reference), `ref_mesh` (`check`,
//! which never fails on a reference: its `reference_*` findings are the
//! stage's), `ref_export` (STEP, the file's own representation), and
//! `ref_reimport` (the export imported again: the same counts and body)
//! and `ref_fixpoint` (exported again: the same bytes).
//!
//! A whole file: `whole_import` (the file as an agent imports it),
//! `whole_export` (each root), `whole_reimport` (each export imported
//! again: the same parts, kinds and features as the root reached, up to
//! the part IDs ryucad made from the files' stems). A part that fails in
//! `whole_import` is imported once more alone, `--into` a part of another
//! ID (`key`): if it builds there, no body case sees the failure, so the
//! whole case signs it.
//!
//! With `--stages defeature` (design 445, plan 22.B) a body case runs only
//! its solid `import` and `defeature`: the body's bounds (`inspect`), its
//! blends at most a hundredth of its diagonal (`blends --below`), and if
//! there are any a `delete_blend --below` of that size, rebuilt and
//! checked. A panic, a hang or a body that fails `check` is a finding; a
//! typed refusal (`tweak_failed`, but for `kernel` and `inside_out`, and
//! `edit_budget_exceeded`) is `Refused`, counted; a body with no blend
//! that small passes untouched. A whole-file case runs nothing.

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use crate::body;
use crate::call::{Ended, Ryucad};
use crate::split::{Case, Kind};

/// The time stamp every export writes, so exports compare by bytes.
pub const TIME_STAMP: &str = "2000-01-01T00:00:00";

/// How a stage ended.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "outcome", rename_all = "snake_case")]
pub enum Outcome {
    Ok,
    /// ryucad refused: its error, and what the instances it names lie on.
    Failed {
        error: Value,
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        kinds: Vec<String>,
    },
    /// Everything answered, but the round trip differs.
    Mismatch {
        what: String,
        details: Value,
    },
    Timeout,
    Crash {
        exit: Option<i32>,
        stderr: String,
    },
    /// The stage answered, with findings of its own (a reference's
    /// faces left out): a finding, but later stages run.
    Found {
        codes: Vec<String>,
    },
    /// ryucad refused, typed, where the kernel is expected to do better
    /// one day (`defeature`'s `tweak_failed` or `edit_budget_exceeded`):
    /// counted against the ledger's `count` lines, never a finding.
    Refused {
        error: Value,
    },
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Stage {
    pub stage: String,
    pub seconds: f64,
    #[serde(flatten)]
    pub outcome: Outcome,
}

/// What running one case gave.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Ran {
    pub stages: Vec<Stage>,
    /// The import tried again under another part ID, after it failed.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub key: Option<Stage>,
    /// What `whole_import` interpreted, retained, found unreferenced and
    /// failed, as `<category>/<bucket>` counts ([`transfer_counts`]):
    /// counts for campaigns, never findings.
    #[serde(default, skip_serializing_if = "std::collections::BTreeMap::is_empty")]
    pub transfer: std::collections::BTreeMap<String, u64>,
}

/// A stage's running total: commands' seconds add, the first that doesn't
/// answer ends it.
struct Run<'a> {
    ryucad: &'a Ryucad,
    home: &'a Path,
    stage: &'static str,
    seconds: f64,
}

/// What a stage's commands give back: an answer's result, or how the
/// stage ended.
type Step<T> = Result<T, Outcome>;

impl Run<'_> {
    /// The command's result (the envelope's `result`), or the stage's end.
    fn cmd<S: AsRef<std::ffi::OsStr>>(&mut self, dir: &Path, args: &[S]) -> Step<Value> {
        let c = self.ryucad.call(self.home, dir, self.stage, args);
        self.seconds += c.seconds;
        match c.ended {
            Ended::Answered(v) if v["ok"] == true => Ok(v["result"].clone()),
            Ended::Answered(v) => Err(Outcome::Failed {
                error: v["error"].clone(),
                kinds: Vec::new(),
            }),
            Ended::Crashed { exit, stderr } => Err(Outcome::Crash { exit, stderr }),
            Ended::TimedOut => Err(Outcome::Timeout),
        }
    }

    /// A new project `name` under the case's folder.
    fn project(&mut self, name: &str) -> Step<PathBuf> {
        let dir = self.home.join(name);
        std::fs::create_dir_all(&dir).map_err(|e| Outcome::Crash {
            exit: None,
            stderr: format!("{e}"),
        })?;
        self.cmd(&dir, &["init", "--unit", "mm"])?;
        Ok(dir)
    }

    /// Imports `file` as part `part` into a new project `name`: the
    /// project and its one feature's counts. A feature that fails is the
    /// stage's failure, with what the instances it names lie on in `file`.
    fn import(&mut self, name: &str, file: &Path, part: &str) -> Step<(PathBuf, Value)> {
        let solid = self.ryucad.modes;
        self.import_as(name, file, part, solid)
            .map(|(dir, counts, _)| (dir, counts))
    }

    /// [`Run::import`] healed with `solid` (on a binary that takes it),
    /// else as a reference: the project, the feature's counts and its
    /// solid's hash.
    fn import_as(
        &mut self,
        name: &str,
        file: &Path,
        part: &str,
        solid: bool,
    ) -> Step<(PathBuf, Value, Value)> {
        let dir = self.project(name)?;
        let mut args = vec![
            OsStrRef::s("import"),
            OsStrRef::p(file),
            OsStrRef::s("--part"),
            OsStrRef::s(part),
        ];
        if solid {
            args.push(OsStrRef::s("--solid"));
        }
        let v = self.cmd(&dir, &args)?;
        let f = &v["parts"][0]["features"][0];
        if f["ok"] == true {
            return Ok((dir, f["counts"].clone(), f["hash"].clone()));
        }
        let error = if f["error"].is_null() {
            json!({"code": "no_feature", "message": "the import answered no feature", "details": v})
        } else {
            f["error"].clone()
        };
        Err(failed_in(error, file))
    }

    /// Imports `product`'s bodies from `file` (the file's one product
    /// without it) into a new project `name`'s part `body`, made empty
    /// first: one of a whole file's parts under another part ID.
    fn import_into(&mut self, name: &str, file: &Path, product: Option<&str>) -> Step<()> {
        let dir = self.project(name)?;
        self.cmd(&dir, &["part", "create", "body"])?;
        let mut args = vec![
            OsStrRef::s("import"),
            OsStrRef::p(file),
            OsStrRef::s("--into"),
            OsStrRef::s("body"),
        ];
        if self.ryucad.modes {
            args.push(OsStrRef::s("--solid"));
        }
        if let Some(p) = product {
            args.extend([OsStrRef::s("--product"), OsStrRef::s(p)]);
        }
        let v = self.cmd(&dir, &args)?;
        match first_error(&v) {
            Some(error) => Err(failed_in(error, file)),
            None => Ok(()),
        }
    }

    /// Exports `part` to `out`, in `unit` (`mm` or `m`).
    fn export(&mut self, dir: &Path, part: &str, out: &Path, unit: &str) -> Step<Vec<u8>> {
        if let Some(folder) = out.parent() {
            std::fs::create_dir_all(folder).map_err(|e| Outcome::Crash {
                exit: None,
                stderr: format!("{}: {e}", folder.display()),
            })?;
        }
        self.cmd(
            dir,
            &[
                OsStrRef::s("export"),
                OsStrRef::s(part),
                OsStrRef::s("--out"),
                OsStrRef::p(out),
                OsStrRef::s("--time-stamp"),
                OsStrRef::s(TIME_STAMP),
                OsStrRef::s("--unit"),
                OsStrRef::s(unit),
            ],
        )?;
        std::fs::read(out).map_err(|e| Outcome::Crash {
            exit: None,
            stderr: format!("the export wrote no {}: {e}", out.display()),
        })
    }

    /// The volume of `part` and its bound, in m³; a part of sheets, which
    /// `mass` refuses (`wrong_body_kind`), its first sheet's area in m².
    fn volume(&mut self, dir: &Path, part: &str) -> Step<(f64, f64)> {
        let v = match self.cmd(dir, &["mass", part]) {
            Err(Outcome::Failed { error, .. }) if error["code"] == "wrong_body_kind" => {
                let r = self.cmd(dir, &["rebuild", part])?;
                let solid = r["parts"][0]["solids"][0]["solid"]
                    .as_str()
                    .unwrap_or("body")
                    .to_owned();
                self.cmd(dir, &["measure", &format!("{part}/{solid}")])?
            }
            v => v?,
        };
        measure_of(&v).ok_or(Outcome::Mismatch {
            what: "no_volume".into(),
            details: v,
        })
    }

    fn end(self, outcome: Outcome) -> Stage {
        Stage {
            stage: self.stage.to_owned(),
            seconds: self.seconds,
            outcome,
        }
    }
}

/// A refusal of importing `file`, with what the instances it names lie on
/// there.
fn failed_in(error: Value, file: &Path) -> Outcome {
    let step: Vec<String> = error["details"]["step"]
        .as_array()
        .map(|a| {
            a.iter()
                .filter_map(|s| s.as_str().map(str::to_owned))
                .collect()
        })
        .unwrap_or_default();
    let kinds = std::fs::read(file)
        .ok()
        .and_then(|b| ryu_part21::parse(&b).ok())
        .map(|f| body::named_kinds(&f, &step))
        .unwrap_or_default();
    Outcome::Failed { error, kinds }
}

/// The first failing feature's error in an import's answer.
fn first_error(answer: &Value) -> Option<Value> {
    answer["parts"]
        .as_array()?
        .iter()
        .flat_map(|p| p["features"].as_array().into_iter().flatten())
        .find(|f| f["ok"] == false)
        .map(|f| f["error"].clone())
}

/// Arguments that are text or paths, for one command line.
enum OsStrRef<'a> {
    S(&'a str),
    P(&'a Path),
}

impl<'a> OsStrRef<'a> {
    fn s(s: &'a str) -> Self {
        OsStrRef::S(s)
    }
    fn p(p: &'a Path) -> Self {
        OsStrRef::P(p)
    }
}

impl AsRef<std::ffi::OsStr> for OsStrRef<'_> {
    fn as_ref(&self) -> &std::ffi::OsStr {
        match self {
            OsStrRef::S(s) => s.as_ref(),
            OsStrRef::P(p) => p.as_os_str(),
        }
    }
}

/// Runs `stage` (`f` over a fresh total) and appends it; whether it passed.
fn stage(
    stages: &mut Vec<Stage>,
    ryucad: &Ryucad,
    home: &Path,
    name: &'static str,
    f: impl FnOnce(&mut Run) -> Step<()>,
) -> bool {
    let mut run = Run {
        ryucad,
        home,
        stage: name,
        seconds: 0.0,
    };
    let outcome = match f(&mut run) {
        Ok(()) => Outcome::Ok,
        Err(o) => o,
    };
    let ok = matches!(outcome, Outcome::Ok | Outcome::Found { .. });
    stages.push(run.end(outcome));
    ok
}

/// A `mass` answer's volume, or a `measure` answer's area, with its
/// bound (0 when exact), in m³ or m².
fn measure_of(v: &Value) -> Option<(f64, f64)> {
    for (key, unit) in [("volume", "m3"), ("area", "m2")] {
        if let Some(m) = v[key][unit].as_f64() {
            return Some((m, v[key][format!("bound_{unit}")].as_f64().unwrap_or(0.0)));
        }
    }
    None
}

/// Whether two volumes agree within their bounds (and a part in 10⁹ for
/// rounding).
fn volumes_agree(a: (f64, f64), b: (f64, f64)) -> bool {
    (a.0 - b.0).abs() <= a.1 + b.1 + 1e-9 * a.0.abs().max(b.0.abs())
}

/// The first line where two files differ, from 1.
fn first_difference(a: &[u8], b: &[u8]) -> usize {
    let mut line = 1;
    for (x, y) in a.iter().zip(b) {
        if x != y {
            return line;
        }
        if *x == b'\n' {
            line += 1;
        }
    }
    line
}

/// Runs a body case's stages in `home`, then, on a binary that takes
/// `--solid`, its reference stages.
pub fn body(ryucad: &Ryucad, case: &Case, home: &Path) -> Ran {
    let mut ran = if ryucad.only_reference {
        Ran {
            stages: Vec::new(),
            key: None,
            transfer: Default::default(),
        }
    } else {
        solid_body(ryucad, case, home)
    };
    if ryucad.modes {
        reference(ryucad, case, home, &mut ran.stages);
    }
    ran
}

/// A body case's reference stages (design 364), appended to `stages`.
fn reference(ryucad: &Ryucad, case: &Case, home: &Path, stages: &mut Vec<Stage>) {
    let mut r1 = PathBuf::new();
    let (mut counts, mut hash) = (Value::Null, Value::Null);
    let ok = stage(stages, ryucad, home, "ref_import", |r| {
        (r1, counts, hash) = r.import_as("r1", &case.path, "ref", false)?;
        Ok(())
    }) && stage(stages, ryucad, home, "ref_mesh", |r| {
        let v = r.cmd(&r1, &["check", "ref"])?;
        let mut codes: Vec<String> = v["findings"]
            .as_array()
            .into_iter()
            .flatten()
            .filter_map(|f| f["code"].as_str())
            .filter(|c| c.starts_with("reference_"))
            .map(str::to_owned)
            .collect();
        codes.sort_unstable();
        codes.dedup();
        if codes.is_empty() {
            Ok(())
        } else {
            Err(Outcome::Found { codes })
        }
    }) && stage(stages, ryucad, home, "ref_export", |r| {
        r.export(&r1, "ref", &home.join("ra.step"), "m").map(|_| ())
    });
    if !ok {
        return;
    }
    let mut r2 = PathBuf::new();
    let ok = stage(stages, ryucad, home, "ref_reimport", |r| {
        let (dir, again, h) = r.import_as("r2", &home.join("ra.step"), "ref", false)?;
        r2 = dir;
        if again != counts || h != hash {
            return Err(Outcome::Mismatch {
                what: "hash".into(),
                details: json!({"from": {"counts": counts, "hash": hash},
                                "to": {"counts": again, "hash": h}}),
            });
        }
        Ok(())
    });
    if ok {
        stage(stages, ryucad, home, "ref_fixpoint", |r| {
            let a = std::fs::read(home.join("ra.step")).unwrap_or_default();
            let b = r.export(&r2, "ref", &home.join("rb").join("ra.step"), "m")?;
            if a != b {
                return Err(Outcome::Mismatch {
                    what: "bytes".into(),
                    details: json!({"a": body::case_id(&a), "b": body::case_id(&b), "line": first_difference(&a, &b)}),
                });
            }
            Ok(())
        });
    }
}

/// A body case's stages healing it (every binary's).
fn solid_body(ryucad: &Ryucad, case: &Case, home: &Path) -> Ran {
    let mut stages = Vec::new();
    let mut key = None;
    let mut p1 = PathBuf::new();
    let mut counts = Value::Null;
    let imported = stage(&mut stages, ryucad, home, "import", |r| {
        (p1, counts) = r.import("p1", &case.path, "body")?;
        Ok(())
    });
    if !imported {
        if matches!(stages[0].outcome, Outcome::Failed { .. }) {
            let mut again = Vec::new();
            stage(&mut again, ryucad, home, "import", |r| {
                r.import("p1-key", &case.path, "body-key").map(|_| ())
            });
            key = again.pop();
        }
        return Ran {
            stages,
            key,
            transfer: Default::default(),
        };
    }
    let ok = stage(&mut stages, ryucad, home, "check", |r| {
        r.cmd(&p1, &["check", "body"]).map(|_| ())
    }) && stage(&mut stages, ryucad, home, "export", |r| {
        r.export(&p1, "body", &home.join("a.step"), "mm")
            .map(|_| ())
    });
    if !ok {
        return Ran {
            stages,
            key,
            transfer: Default::default(),
        };
    }
    let mut p2 = PathBuf::new();
    let ok = stage(&mut stages, ryucad, home, "reimport", |r| {
        let (dir, again) = r.import("p2", &home.join("a.step"), "body")?;
        p2 = dir;
        if again != counts {
            return Err(Outcome::Mismatch {
                what: "counts".into(),
                details: json!({"from": counts, "to": again}),
            });
        }
        Ok(())
    }) && stage(&mut stages, ryucad, home, "volume", |r| {
        let (a, b) = (r.volume(&p1, "body")?, r.volume(&p2, "body")?);
        if !volumes_agree(a, b) {
            return Err(Outcome::Mismatch {
                what: "volume".into(),
                details: json!({"from": {"m3": a.0, "bound_m3": a.1}, "to": {"m3": b.0, "bound_m3": b.1}}),
            });
        }
        Ok(())
    });
    if ok {
        stage(&mut stages, ryucad, home, "fixpoint", |r| {
            // One file name both times: an export's header names its file.
            // In meters: millimetre files come back within a few ulps
            // (step.md), meters bit for bit.
            let b = r.export(&p2, "body", &home.join("b").join("body.step"), "m")?;
            let (p3, _) = r.import("p3", &home.join("b").join("body.step"), "body")?;
            let c = r.export(&p3, "body", &home.join("c").join("body.step"), "m")?;
            if b != c {
                return Err(Outcome::Mismatch {
                    what: "bytes".into(),
                    details: json!({"b": body::case_id(&b), "c": body::case_id(&c), "line": first_difference(&b, &c)}),
                });
            }
            Ok(())
        });
    }
    Ran {
        stages,
        key,
        transfer: Default::default(),
    }
}

/// The parts an import answer made: `part -> (kind, features, built, placed)`.
type Made = BTreeMap<String, (String, usize, usize, BTreeSet<String>)>;

fn made(v: &Value) -> Made {
    let mut out = Made::new();
    for p in v["parts"].as_array().into_iter().flatten() {
        let fs = p["features"].as_array().cloned().unwrap_or_default();
        let placed = fs
            .iter()
            .filter_map(|f| f["of"].as_str().map(str::to_owned))
            .collect();
        out.insert(
            p["part"].as_str().unwrap_or_default().to_owned(),
            (
                p["kind"].as_str().unwrap_or_default().to_owned(),
                fs.len(),
                fs.iter().filter(|f| f["ok"] == true).count(),
                placed,
            ),
        );
    }
    out
}

/// The parts no other part places, and everything each reaches.
fn roots(m: &Made) -> Vec<(String, Made)> {
    let placed: BTreeSet<&String> = m.values().flat_map(|p| &p.3).collect();
    m.keys()
        .filter(|k| !placed.contains(k))
        .map(|root| {
            let mut reach = Made::new();
            let mut stack = vec![root.clone()];
            while let Some(p) = stack.pop() {
                if let Some(v) = m.get(&p)
                    && reach.insert(p.clone(), v.clone()).is_none()
                {
                    stack.extend(v.3.iter().cloned());
                }
            }
            (root.clone(), reach)
        })
        .collect()
}

/// Whether `id` is a part ID ryucad made from a file's stem (`stem`, or
/// `stem-<n>` on a collision): what it names a part whose product has no id
/// or name.
fn from_stem(id: &str, stem: &str) -> bool {
    id == stem
        || id
            .strip_prefix(stem)
            .and_then(|r| r.strip_prefix('-'))
            .is_some_and(|n| !n.is_empty() && n.bytes().all(|b| b.is_ascii_digit()))
}

/// A part and what it places as a tree, its ID left out where ryucad made
/// it from the file's stem: a round trip writes the file under another name.
fn shape(m: &Made, part: &str, stem: &str) -> Value {
    let Some((kind, n, built, placed)) = m.get(part) else {
        return json!({"missing": part});
    };
    let mut children: Vec<Value> = placed.iter().map(|c| shape(m, c, stem)).collect();
    children.sort_by_key(|c| c.to_string());
    let id = if from_stem(part, stem) { "" } else { part };
    json!({"id": id, "kind": kind, "features": n, "built": built, "places": children})
}

/// Whether a root re-imported (`again`, from a file of stem `stem_b`) has
/// the structure it had (`reach` from `root`, from a file of stem `stem_a`):
/// the same parts, kinds and features, up to the IDs file stems gave.
fn same_structure(root: &str, reach: &Made, stem_a: &str, again: &Made, stem_b: &str) -> bool {
    if again == reach {
        return true;
    }
    match roots(again).as_slice() {
        [(top, reached)] if reached.len() == again.len() => {
            shape(reach, root, stem_a) == shape(again, top, stem_b)
        }
        _ => false,
    }
}

/// Runs a whole-file case's stages in `home`, then, on a binary that
/// takes `--solid`, `whole_ref_import`: the file imported whole as
/// references and each root that built exported.
pub fn whole(ryucad: &Ryucad, case: &Case, home: &Path) -> Ran {
    let mut ran = if ryucad.only_reference {
        Ran {
            stages: Vec::new(),
            key: None,
            transfer: Default::default(),
        }
    } else {
        solid_whole(ryucad, case, home)
    };
    if ryucad.modes {
        stage(&mut ran.stages, ryucad, home, "whole_ref_import", |r| {
            let dir = r.project("wr")?;
            let answer = r.cmd(&dir, &[OsStrRef::s("import"), OsStrRef::p(&case.path)])?;
            // A part that failed is its body case's `ref_import` finding,
            // and its root can't be exported.
            let tops: Vec<(String, Made)> = roots(&made(&answer))
                .into_iter()
                .filter(|(_, reach)| reach.values().all(|p| p.1 == p.2))
                .collect();
            for (k, (root, _)) in tops.iter().enumerate() {
                r.export(&dir, root, &home.join(format!("ref-root-{k}.step")), "m")?;
            }
            Ok(())
        });
    }
    ran
}

/// A whole-file case's stages healing it (every binary's).
fn solid_whole(ryucad: &Ryucad, case: &Case, home: &Path) -> Ran {
    let mut stages = Vec::new();
    let mut w1 = PathBuf::new();
    let mut first = Made::new();
    let stem = case
        .path
        .file_stem()
        .map(|s| s.to_string_lossy().into_owned())
        .unwrap_or_default();
    let mut answer = Value::Null;
    let ok = stage(&mut stages, ryucad, home, "whole_import", |r| {
        w1 = r.project("w1")?;
        let mut args = vec![OsStrRef::s("import"), OsStrRef::p(&case.path)];
        if ryucad.modes {
            args.push(OsStrRef::s("--solid"));
        }
        answer = r.cmd(&w1, &args)?;
        first = made(&answer);
        Ok(())
    });
    // What the import made of the file, for the campaign's counts.
    let transfer = transfer_counts(&answer);
    if !ok {
        return Ran {
            stages,
            key: None,
            transfer: transfer.clone(),
        };
    }
    // A part that failed to import is its body case's finding, unless it
    // builds alone under another part ID: healing can depend on the ID
    // (#285), and a body case imports under `body`. Then it is this case's.
    // Each part is tried in a project of its own: `init` refuses a folder
    // that holds one already.
    for (i, p) in answer["parts"].as_array().into_iter().flatten().enumerate() {
        let Some(error) = first_error(&json!({"parts": [p]})) else {
            continue;
        };
        let product = &p["product"];
        let product = [&product["id"], &product["name"]]
            .into_iter()
            .filter_map(|v| v.as_str())
            .find(|s| !s.is_empty());
        let mut again = Vec::new();
        stage(&mut again, ryucad, home, "whole_import", |r| {
            r.import_into(&format!("w1-key-{i}"), &case.path, product)
        });
        if again[0].outcome == Outcome::Ok {
            stages[0].outcome = failed_in(error, &case.path);
            return Ran {
                stages,
                key: again.pop(),
                transfer: transfer.clone(),
            };
        }
    }
    // Roots whose every part built: a part that failed to import is its
    // body case's finding, and its root can't be exported.
    let tops: Vec<(String, Made)> = roots(&first)
        .into_iter()
        .filter(|(_, reach)| reach.values().all(|p| p.1 == p.2))
        .collect();
    let ok = stage(&mut stages, ryucad, home, "whole_export", |r| {
        for (k, (root, _)) in tops.iter().enumerate() {
            r.export(&w1, root, &home.join(format!("root-{k}.step")), "mm")?;
        }
        Ok(())
    });
    if ok {
        stage(&mut stages, ryucad, home, "whole_reimport", |r| {
            for (k, (root, reach)) in tops.iter().enumerate() {
                let dir = r.project(&format!("w2-{k}"))?;
                let file = home.join(format!("root-{k}.step"));
                let mut args = vec![OsStrRef::s("import"), OsStrRef::p(&file)];
                if ryucad.modes {
                    args.push(OsStrRef::s("--solid"));
                }
                let answer = r.cmd(&dir, &args)?;
                // Every part of the root built the first time: a feature
                // failing now is the round trip's refusal, signed as one.
                if let Some(error) = first_error(&answer) {
                    return Err(failed_in(error, &file));
                }
                let again = made(&answer);
                if !same_structure(root, reach, &stem, &again, &format!("root-{k}")) {
                    let show = |m: &Made| -> Value {
                        m.iter()
                            .map(|(p, (kind, n, built, _))| (p.clone(), json!([kind, n, built])))
                            .collect::<serde_json::Map<_, _>>()
                            .into()
                    };
                    return Err(Outcome::Mismatch {
                        what: "structure".into(),
                        details: json!({"root": root, "from": show(reach), "to": show(&again)}),
                    });
                }
            }
            Ok(())
        });
    }
    Ran {
        stages,
        key: None,
        transfer: transfer.clone(),
    }
}

/// An import answer's transfer report (design 448) as
/// `<category>/<bucket>` counts, zeros left out; empty for a binary
/// older than 24.A.
pub fn transfer_counts(v: &serde_json::Value) -> std::collections::BTreeMap<String, u64> {
    let mut out = std::collections::BTreeMap::new();
    for c in v["transfer"]["categories"].as_array().into_iter().flatten() {
        let Some(cat) = c["category"].as_str() else {
            continue;
        };
        for b in ["interpreted", "retained", "unreferenced", "failed"] {
            if let Some(n) = c[b]["count"].as_u64().filter(|n| *n > 0) {
                out.insert(format!("{cat}/{b}"), n);
            }
        }
    }
    out
}

/// Runs a case in a folder of its own under `work` (kept with `keep`).
/// Whether an error is a typed refusal the `defeature` stage counts and
/// doesn't file: the kernel's (`tweak_failed`, but not `kernel`, an
/// untyped kernel error, nor `inside_out`, ryucad's guard catching a
/// kernel fault, #692) or the edit budget's.
pub fn typed_refusal(error: &Value) -> bool {
    match error["code"].as_str() {
        Some("edit_budget_exceeded") => true,
        Some("tweak_failed") => !matches!(
            error["details"]["reason"].as_str(),
            None | Some("kernel" | "inside_out")
        ),
        _ => false,
    }
}

/// A command's refusal as `defeature` judges it: typed, counted.
fn refused(o: Outcome) -> Outcome {
    match o {
        Outcome::Failed { error, .. } if typed_refusal(&error) => Outcome::Refused { error },
        o => o,
    }
}

/// A body case's solid `import`, then `defeature` (see the module doc).
fn defeature(ryucad: &Ryucad, case: &Case, home: &Path) -> Ran {
    let mut stages = Vec::new();
    let mut p1 = PathBuf::new();
    let imported = stage(&mut stages, ryucad, home, "import", |r| {
        (p1, _) = r.import("p1", &case.path, "body")?;
        Ok(())
    });
    if !imported {
        return Ran { stages, key: None };
    }
    stage(&mut stages, ryucad, home, "defeature", |r| {
        let v = r.cmd(&p1, &["inspect", "body"])?;
        let b = &v["bodies"][0]["bounds"];
        let side = |i: usize| Some(b["max"][i].as_f64()? - b["min"][i].as_f64()?);
        let diagonal = (0..3)
            .map(|i| side(i).map(|d| d * d))
            .sum::<Option<f64>>()
            .map(f64::sqrt)
            .ok_or_else(|| Outcome::Mismatch {
                what: "no_bounds".into(),
                details: v.clone(),
            })?;
        // A hundredth of the diagonal, in millimetres (the project's unit).
        let below = format!("{} mm", diagonal * 1e3 / 100.0);
        let listed = r
            .cmd(&p1, &["blends", "body", "--below", &below])
            .map_err(refused)?;
        if listed["blends"].as_array().is_none_or(Vec::is_empty) {
            return Ok(());
        }
        r.cmd(
            &p1,
            &[
                "feature",
                "add",
                "body",
                "delete_blend",
                "--id",
                "defeature",
                "--below",
                &below,
            ],
        )
        .map_err(refused)?;
        // A feature that fails makes `rebuild` refuse, `rebuild_failed`,
        // with the part's report in its details.
        let rebuilt = match r.cmd(&p1, &["rebuild", "body"]) {
            Ok(v) => v,
            Err(Outcome::Failed { error, .. }) if error["code"] == "rebuild_failed" => {
                error["details"].clone()
            }
            Err(o) => return Err(o),
        };
        let f = rebuilt["parts"][0]["features"]
            .as_array()
            .into_iter()
            .flatten()
            .find(|f| f["id"] == "defeature")
            .cloned()
            .unwrap_or(Value::Null);
        if f["ok"] != true {
            let error = if f.is_null() {
                json!({"code": "no_feature", "message": "the rebuild answered no defeature", "details": rebuilt})
            } else {
                f["error"].clone()
            };
            return Err(refused(Outcome::Failed {
                error,
                kinds: Vec::new(),
            }));
        }
        r.cmd(&p1, &["check", "body"]).map(|_| ())
    });
    Ran { stages, key: None }
}

pub fn run(ryucad: &Ryucad, case: &Case, work: &Path, keep: bool) -> Ran {
    let home = work.join("runs").join(&case.id);
    let _ = std::fs::remove_dir_all(&home);
    let _ = std::fs::create_dir_all(&home);
    let ran = match case.kind {
        Kind::Body if ryucad.defeature => defeature(ryucad, case, &home),
        Kind::Whole if ryucad.defeature => Ran {
            stages: Vec::new(),
            key: None,
        },
        Kind::Body => body(ryucad, case, &home),
        Kind::Whole => whole(ryucad, case, &home),
        Kind::Open => Ran {
            stages: vec![Stage {
                stage: "open".into(),
                seconds: 0.0,
                outcome: Outcome::Failed {
                    error: case.refused.clone().unwrap_or(Value::Null),
                    kinds: Vec::new(),
                },
            }],
            key: None,
            transfer: Default::default(),
        },
    };
    if !keep {
        let _ = std::fs::remove_dir_all(&home);
    }
    ran
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn transfer_counts_are_read_from_an_import_answer() {
        let v = serde_json::json!({"transfer": {"categories": [
            {"category": "geometry", "interpreted": {"count": 10}, "retained": {"count": 2},
             "unreferenced": {"count": 0}, "failed": {"count": 1}},
            {"category": "appearance", "interpreted": {"count": 0}, "retained": {"count": 7},
             "unreferenced": {"count": 0}, "failed": {"count": 0}}]}});
        let c = transfer_counts(&v);
        assert_eq!(c.get("geometry/interpreted"), Some(&10));
        assert_eq!(c.get("geometry/failed"), Some(&1));
        assert_eq!(c.get("appearance/retained"), Some(&7));
        assert!(
            !c.contains_key("appearance/interpreted"),
            "zeros are left out"
        );
        assert!(
            transfer_counts(&serde_json::json!({})).is_empty(),
            "an older binary's answer"
        );
    }

    #[test]
    fn a_sheet_is_measured_by_its_area() {
        use serde_json::json;
        let mass = json!({"volume": {"m3": 1e-6, "bound_m3": 1e-12}});
        assert_eq!(measure_of(&mass), Some((1e-6, 1e-12)));
        let area = json!({"area": {"m2": 2e-4, "bound_m2": 1e-9}});
        assert_eq!(measure_of(&area), Some((2e-4, 1e-9)));
        let exact = json!({"area": {"m2": 2e-4, "exact": true}});
        assert_eq!(measure_of(&exact), Some((2e-4, 0.0)));
        assert_eq!(measure_of(&json!({})), None);
    }

    #[test]
    fn volumes_agree_within_bounds() {
        assert!(volumes_agree((1.0, 0.0), (1.0, 0.0)));
        assert!(volumes_agree((1.0, 1e-6), (1.000001, 0.0)));
        assert!(!volumes_agree((1.0, 1e-7), (1.001, 1e-7)));
    }

    #[test]
    fn differences_are_found_by_line() {
        assert_eq!(first_difference(b"a\nb\nc", b"a\nb\nd"), 3);
        assert_eq!(first_difference(b"same", b"same"), 1);
    }

    #[test]
    fn stems_are_told_from_ids() {
        assert!(from_stem("8f897bf39399a713", "8f897bf39399a713"));
        assert!(from_stem("8f897bf39399a713-2", "8f897bf39399a713"));
        assert!(from_stem("root-0", "root-0"));
        assert!(!from_stem("root-0-x", "root-0"));
        assert!(!from_stem("root-", "root"));
        assert!(!from_stem("nist_ctc_03_asme1", "root-0"));
    }

    #[test]
    fn a_round_trip_is_judged_up_to_ids_from_stems() {
        let part = |name: &str| {
            made(
                &json!({"parts": [{"part": name, "kind": "part", "features": [{"id": "body", "ok": true}]}]}),
            )
        };
        // A product with no id or name: the part is named after each file.
        assert!(same_structure(
            "8f897bf39399a713",
            &part("8f897bf39399a713"),
            "8f897bf39399a713",
            &part("root-0"),
            "root-0"
        ));
        // A product's own id is kept, so a change of it is a change.
        assert!(same_structure(
            "bracket",
            &part("bracket"),
            "case",
            &part("bracket"),
            "root-0"
        ));
        assert!(!same_structure(
            "bracket",
            &part("bracket"),
            "case",
            &part("root-0"),
            "root-0"
        ));
        // Other counts are a change whatever the names.
        let two = made(
            &json!({"parts": [{"part": "root-0", "kind": "part", "features": [{"id": "body", "ok": true}, {"id": "b2", "ok": true}]}]}),
        );
        assert!(!same_structure("c", &part("c"), "c", &two, "root-0"));
        // An assembly over stem-named parts.
        let asm = |top: &str, pin: &str| {
            made(&json!({"parts": [
                {"part": top, "kind": "assembly", "features": [{"id": "i1", "ok": true, "of": pin}]},
                {"part": pin, "kind": "part", "features": [{"id": "body", "ok": true}]},
            ]}))
        };
        assert!(same_structure(
            "c",
            &asm("c", "c-2"),
            "c",
            &asm("root-0", "root-0-2"),
            "root-0"
        ));
        assert!(!same_structure(
            "c",
            &asm("c", "pin"),
            "c",
            &asm("root-0", "root-0-2"),
            "root-0"
        ));
    }

    #[test]
    fn roots_reach_what_they_place() {
        let v = json!({"parts": [
            {"part": "top", "kind": "assembly", "features": [{"id": "i1", "ok": true, "of": "pin"}, {"id": "i2", "ok": true, "of": "sub"}]},
            {"part": "sub", "kind": "assembly", "features": [{"id": "i", "ok": true, "of": "pin"}]},
            {"part": "pin", "kind": "part", "features": [{"id": "body", "ok": false}]},
            {"part": "loose", "kind": "part", "features": []},
        ]});
        let r = roots(&made(&v));
        let names: Vec<&str> = r.iter().map(|(n, _)| n.as_str()).collect();
        assert_eq!(names, ["loose", "top"]);
        assert_eq!(r[1].1.len(), 3);
        assert_eq!(r[1].1["pin"].2, 0);
    }

    #[test]
    fn typed_refusals_are_told_from_faults() {
        let e = |code: &str, reason: Option<&str>| match reason {
            Some(r) => json!({"code": code, "details": {"reason": r}}),
            None => json!({"code": code, "details": {}}),
        };
        assert!(typed_refusal(&e("tweak_failed", Some("blend_lost"))));
        assert!(typed_refusal(&e("tweak_failed", Some("gap_not_healed"))));
        assert!(typed_refusal(&e("edit_budget_exceeded", None)));
        assert!(!typed_refusal(&e("tweak_failed", Some("kernel"))));
        assert!(!typed_refusal(&e("tweak_failed", Some("inside_out"))));
        assert!(!typed_refusal(&e("internal", None)));
    }
}
