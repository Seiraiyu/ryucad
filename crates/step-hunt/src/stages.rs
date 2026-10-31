//! A case's stages, each one or more `ryucad` commands in folders of the
//! case's own, stopping at the first that fails.
//!
//! A body: `import` (healing and rebuild), `check`, `export`, `reimport`
//! (the export into a second project: the same counts), `volume` (both
//! projects' volumes agree within their bounds), `fixpoint` (the second
//! project exported in meters, imported into a third and exported again:
//! the same bytes). A failed import is tried once more under another part ID
//! (`key`).
//!
//! A whole file: `whole_import` (the file as an agent imports it),
//! `whole_export` (each root), `whole_reimport` (each export imported
//! again: the same parts, kinds and features as the root reached).

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
        let dir = self.project(name)?;
        let v = self.cmd(
            &dir,
            &[
                OsStrRef::s("import"),
                OsStrRef::p(file),
                OsStrRef::s("--part"),
                OsStrRef::s(part),
            ],
        )?;
        let f = &v["parts"][0]["features"][0];
        if f["ok"] == true {
            return Ok((dir, f["counts"].clone()));
        }
        let error = if f["error"].is_null() {
            json!({"code": "no_feature", "message": "the import answered no feature", "details": v})
        } else {
            f["error"].clone()
        };
        Err(failed_in(error, file))
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

    /// The volume of `part` and its bound, in m³.
    fn volume(&mut self, dir: &Path, part: &str) -> Step<(f64, f64)> {
        let v = self.cmd(dir, &["mass", part])?;
        let vol = &v["volume"];
        match vol["m3"].as_f64() {
            Some(m) => Ok((m, vol["bound_m3"].as_f64().unwrap_or(0.0))),
            None => Err(Outcome::Mismatch {
                what: "no_volume".into(),
                details: v,
            }),
        }
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
    let ok = outcome == Outcome::Ok;
    stages.push(run.end(outcome));
    ok
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

/// Runs a body case's stages in `home`.
pub fn body(ryucad: &Ryucad, case: &Case, home: &Path) -> Ran {
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
        return Ran { stages, key };
    }
    let ok = stage(&mut stages, ryucad, home, "check", |r| {
        r.cmd(&p1, &["check", "body"]).map(|_| ())
    }) && stage(&mut stages, ryucad, home, "export", |r| {
        r.export(&p1, "body", &home.join("a.step"), "mm")
            .map(|_| ())
    });
    if !ok {
        return Ran { stages, key };
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
    Ran { stages, key }
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

/// Runs a whole-file case's stages in `home`.
pub fn whole(ryucad: &Ryucad, case: &Case, home: &Path) -> Ran {
    let mut stages = Vec::new();
    let mut w1 = PathBuf::new();
    let mut first = Made::new();
    let ok = stage(&mut stages, ryucad, home, "whole_import", |r| {
        w1 = r.project("w1")?;
        first = made(&r.cmd(&w1, &[OsStrRef::s("import"), OsStrRef::p(&case.path)])?);
        Ok(())
    });
    if !ok {
        return Ran { stages, key: None };
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
                let answer = r.cmd(&dir, &[OsStrRef::s("import"), OsStrRef::p(&file)])?;
                // Every part of the root built the first time: a feature
                // failing now is the round trip's refusal, signed as one.
                if let Some(error) = first_error(&answer) {
                    return Err(failed_in(error, &file));
                }
                let again = made(&answer);
                if &again != reach {
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
    Ran { stages, key: None }
}

/// Runs a case in a folder of its own under `work` (kept with `keep`).
pub fn run(ryucad: &Ryucad, case: &Case, work: &Path, keep: bool) -> Ran {
    let home = work.join("runs").join(&case.id);
    let _ = std::fs::remove_dir_all(&home);
    let _ = std::fs::create_dir_all(&home);
    let ran = match case.kind {
        Kind::Body => body(ryucad, case, &home),
        Kind::Whole => whole(ryucad, case, &home),
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
}
