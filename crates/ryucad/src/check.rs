//! `ryucad check`: the project rebuilt from its files alone, with nothing
//! reused, and everything verified that can be without a stored result:
//! `doctor`'s issues (stale sketches, conflict records and Git's setup
//! among them), that no file is left unmerged in Git's index, that every
//! feature builds, that every body passes ryukei's intersection check and
//! meshes without faults, and that the session's cached results are what a
//! rebuild from nothing gives. CI and hosted review run it.

use crate::doc::id::Id;
use crate::error::{Code, Error, Result};
use crate::rebuild::{Built, Output, rebuild_part};
use crate::session::Session;
use rayon::prelude::*;
use serde_json::{Value, json};

/// Every finding's code and severity: `doctor`'s issues keep their own.
pub const CODES: [(&str, &str); 6] = [
    ("feature_failed", "error"),
    ("faces_intersect", "error"),
    ("cannot_check", "warning"),
    ("mesh_faults", "error"),
    ("cache_mismatch", "error"),
    ("unmerged_path", "error"),
];

fn severity(code: &str) -> &'static str {
    CODES
        .iter()
        .find(|(c, _)| *c == code)
        .map_or("error", |(_, s)| s)
}

/// A finding about one feature.
fn finding(code: &str, part: &Id, feature: &Id, message: String) -> Value {
    json!({"code": code, "severity": severity(code), "part": part, "feature": feature,
           "message": message})
}

/// What ryukei finds wrong with one built body: faces that meet where they
/// shouldn't (or can't be checked), and its mesh at normal quality.
fn body_findings(
    part: &Id,
    feature: &Id,
    b: &Built,
    mesh: &crate::mesh::DisplayMesh,
) -> Vec<Value> {
    use ryukei::check::Fault;
    let name = |f| b.body.face(f).expect("live").name.to_string();
    let (mut crossing, mut unchecked) = (Vec::new(), Vec::new());
    for fault in ryukei::check::check_intersections(&b.body) {
        match fault {
            Fault::FacesCross { a, b } => {
                crossing.push(json!({"kind": "cross", "faces": [name(a), name(b)]}));
            }
            Fault::FacesOverlap { a, b } => {
                crossing.push(json!({"kind": "overlap", "faces": [name(a), name(b)]}));
            }
            Fault::CannotCheck { a, b } => unchecked.push(json!([name(a), name(b)])),
            other => crossing.push(json!({"kind": format!("{other:?}")})),
        }
    }
    let mut out = Vec::new();
    if !crossing.is_empty() {
        let mut f = finding(
            "faces_intersect",
            part,
            feature,
            format!(
                "{part}/{feature}: {} pair(s) of faces meet away from their edges",
                crossing.len()
            ),
        );
        f["faults"] = json!(crossing);
        out.push(f);
    }
    if !unchecked.is_empty() {
        let mut f = finding(
            "cannot_check",
            part,
            feature,
            format!(
                "{part}/{feature}: {} pair(s) of faces could not be checked",
                unchecked.len()
            ),
        );
        f["pairs"] = json!(unchecked);
        out.push(f);
    }
    let faults: Vec<Value> = match &mesh.mesh {
        Ok(m) => m
            .faults
            .iter()
            .map(|f| {
                json!({"face": b.body.face(f.face).expect("live").name.to_string(),
                       "kind": format!("{:?}", f.kind)})
            })
            .collect(),
        Err(e) => vec![json!({"error": e})],
    };
    if !faults.is_empty() {
        let mut f = finding(
            "mesh_faults",
            part,
            feature,
            format!("{part}/{feature}: the body doesn't mesh cleanly at normal quality"),
        );
        f["faults"] = json!(faults);
        out.push(f);
    }
    out
}

/// A feature's result as the check reports it.
fn result(r: &Result<Output>) -> Value {
    match r {
        Ok(o) => json!({"ok": true, "hash": o.hash()}),
        Err(e) => json!({"ok": false, "error": e.code}),
    }
}

/// `check`: the whole project, or one part (with the project's own
/// issues). With `against`, the report adds what changed from that
/// revision to the working files, the model included (a pull request's
/// check reads as what the branch did). Fails with `check_failed`, the
/// report in its details, when any finding is an error.
pub fn run(session: &mut Session, part: Option<&str>, against: Option<&str>) -> Result<Value> {
    // A revision that isn't there fails before anything is built.
    let changes = match against {
        Some(rev) => {
            let opts = crate::diff::Options {
                part: part.map(str::to_owned),
                ..Default::default()
            };
            Some(crate::server::diff(session, rev, "working", true, &opts)?)
        }
        None => None,
    };
    let ids: Vec<Id> = match part {
        Some(p) => {
            let id = crate::server::part_id(p)?;
            session.project().part(&id)?;
            vec![id]
        }
        None => session.project().parts.keys().cloned().collect(),
    };
    let mut findings: Vec<Value> = crate::doctor::diagnose(session.root())
        .into_iter()
        .filter(|i| {
            // A part's issues belong to it; the project's to everyone.
            i.files.is_empty()
                || i.files.iter().any(|f| {
                    let mut seg = f.split('/');
                    seg.next() != Some("parts")
                        || seg
                            .next()
                            .is_some_and(|p| ids.iter().any(|id| id.as_str() == p))
                })
        })
        .map(|i| {
            let mut v = i.to_json();
            v["source"] = json!("doctor");
            v
        })
        .collect();
    // What Git left unmerged: a merge to finish before anything is checked
    // in.
    if let Some(repo) = crate::git::Repo::open(session.root())? {
        for path in repo.unmerged()?.keys() {
            let mine = match path.split('/').collect::<Vec<_>>().as_slice() {
                ["parts", p, ..] => ids.iter().any(|id| id.as_str() == *p),
                _ => true,
            };
            if mine {
                findings.push(json!({
                    "code": "unmerged_path", "severity": severity("unmerged_path"),
                    "files": [path],
                    "message": format!("{path} is unmerged in Git's index: settle the merge (`ryucad merge status`), then `git add` it"),
                }));
            }
        }
    }
    let mut parts = Vec::new();
    for id in &ids {
        let p = session.project().part(id)?.clone();
        // From nothing: no cache, so the files alone decide.
        let fresh = rebuild_part(&p);
        let cached = session.rebuilt(id)?.clone();
        let mut features = Vec::new();
        let mut bodies = Vec::new();
        for (fid, r) in &fresh.features {
            features.push({
                let mut v = result(r);
                v["id"] = json!(fid);
                v
            });
            let was = cached
                .features
                .iter()
                .find(|(c, _)| c == fid)
                .map(|(_, r)| r);
            if was.map(result) != Some(result(r)) {
                let mut f = finding(
                    "cache_mismatch",
                    id,
                    fid,
                    format!(
                        "{id}/{fid}: the session's result differs from a rebuild from nothing (a ryucad bug)"
                    ),
                );
                f["fresh"] = result(r);
                f["cached"] = was.map_or(Value::Null, result);
                findings.push(f);
            }
            match r {
                // A conflict record is doctor's to report.
                Err(e) if e.code == Code::MergeConflict => {}
                Err(e) => {
                    let mut f = finding("feature_failed", id, fid, e.message.clone());
                    f["error"] = json!(session.explain(id, e.clone()));
                    findings.push(f);
                }
                Ok(Output::Solids(m)) => {
                    bodies.extend(m.solids.iter().map(|(_, b)| (fid.clone(), b.clone())));
                }
                // A stale sketch is doctor's to report, with its repair.
                Ok(Output::Sketch(_)) => {}
            }
        }
        let loaded: Vec<(Id, crate::doc::files::Feature)> = bodies
            .iter()
            .filter_map(|(fid, _)| Some((fid.clone(), p.features[fid].as_ref().ok()?.clone())))
            .collect();
        let items: Vec<(&crate::doc::files::Feature, &Built)> = bodies
            .iter()
            .zip(&loaded)
            .map(|((_, b), (_, f))| (f, b.as_ref()))
            .collect();
        let meshes =
            crate::mesh::Cache::default().get_all(&items, crate::mesh::Quality::Normal, usize::MAX);
        let found: Vec<Vec<Value>> = bodies
            .par_iter()
            .zip(&meshes)
            .map(|((fid, b), m)| body_findings(id, fid, b, m))
            .collect();
        findings.extend(found.into_iter().flatten());
        parts.push(json!({"part": id, "hash": fresh.hash, "features": features}));
    }
    let errors = findings.iter().filter(|f| f["severity"] == "error").count();
    let mut report = json!({
        "passed": errors == 0,
        "errors": errors,
        "warnings": findings.len() - errors,
        "findings": findings,
        "parts": parts,
    });
    if let Some(c) = changes {
        report["against"] = c;
    }
    if errors > 0 {
        return Err(Error {
            code: Code::CheckFailed,
            message: format!("{errors} check(s) failed"),
            details: report,
        });
    }
    Ok(report)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::server::tests::{call, fresh};

    fn id(s: &str) -> Id {
        Id::parse(s, "feature").unwrap()
    }

    /// A part `p` with a box, a sketch, an extrude from it and a sphere,
    /// and a part `q` with a sphere.
    fn project() -> (tempfile::TempDir, Session) {
        let (dir, mut s) = fresh();
        for part in ["p", "q"] {
            call(&mut s, "part.create", json!({"part": part})).unwrap();
        }
        for (part, f) in [
            (
                "p",
                json!({"type": "box", "id": "b", "params": {"x": 10, "y": 20, "z": 5}}),
            ),
            ("p", json!({"type": "sketch", "id": "s"})),
            (
                "q",
                json!({"type": "sphere", "id": "c", "params": {"radius": 1}}),
            ),
        ] {
            let mut f = f;
            f["part"] = json!(part);
            call(&mut s, "feature.add", f).unwrap();
        }
        call(
            &mut s,
            "sketch.edit",
            json!({"part": "p", "sketch": "s", "ops": crate::sketch::tests::rectangle_ops()}),
        )
        .unwrap();
        call(
            &mut s,
            "feature.add",
            json!({"part": "p", "type": "extrude", "id": "e", "sketch": "s",
                   "extent": {"length": 4}}),
        )
        .unwrap();
        (dir, s)
    }

    fn codes(v: &Value) -> Vec<String> {
        v["findings"]
            .as_array()
            .unwrap()
            .iter()
            .map(|f| {
                let at = f["feature"]
                    .as_str()
                    .map_or(String::new(), |x| format!(" {x}"));
                format!("{}{at}", f["code"].as_str().unwrap())
            })
            .collect()
    }

    #[test]
    fn a_sound_project_passes() {
        let (_dir, mut s) = project();
        let v = call(&mut s, "check", json!({})).unwrap();
        assert_eq!(v["passed"], true, "{v}");
        assert_eq!(
            (v["errors"].as_u64(), v["warnings"].as_u64()),
            (Some(0), Some(0))
        );
        let rebuilt = call(&mut s, "rebuild", json!({})).unwrap();
        for (c, r) in v["parts"]
            .as_array()
            .unwrap()
            .iter()
            .zip(rebuilt["parts"].as_array().unwrap())
        {
            assert_eq!(c["hash"], r["hash"]);
        }
        let p = &v["parts"][0];
        assert_eq!(
            p["features"][2],
            json!({"id": "e", "ok": true, "hash": rebuilt["parts"][0]["features"][2]["hash"]})
        );
    }

    #[test]
    fn what_fails_is_found() {
        let (dir, mut s) = project();
        let root = dir.path();
        // A torus that can't be built, and a feature file that doesn't read.
        call(
            &mut s,
            "feature.add",
            json!({"part": "q", "type": "torus", "id": "t",
                   "params": {"major_radius": 1, "minor_radius": 1}}),
        )
        .unwrap();
        std::fs::write(root.join("parts/q/features/c.json"), "{ nope").unwrap();
        let e = call(&mut s, "check", json!({})).unwrap_err();
        assert_eq!(e.code, Code::CheckFailed);
        assert_eq!(e.message, "3 check(s) failed");
        let v = &e.details;
        assert_eq!(v["passed"], false);
        assert_eq!(
            codes(v),
            ["invalid_json", "feature_failed c", "feature_failed t"]
        );
        assert_eq!(v["findings"][0]["source"], "doctor");
        assert_eq!(v["findings"][2]["error"]["code"], "rebuild_failed");
        assert_eq!(v["findings"][2]["part"], "q");
        // One part: its own findings, and none of the others'.
        let p = call(&mut s, "check", json!({"part": "p"})).unwrap();
        assert_eq!(p["passed"], true);
        assert_eq!(p["parts"].as_array().unwrap().len(), 1);
        let e = call(&mut s, "check", json!({"part": "nope"})).unwrap_err();
        assert_eq!(e.code, Code::NotFound);
    }

    #[test]
    fn parts_numbered_alike_fail() {
        let (dir, mut s) = project();
        call(
            &mut s,
            "props.set",
            json!({"part": "p", "properties": {"number": "7"}}),
        )
        .unwrap();
        // By hand, as a merge of two branches might leave it.
        let q = crate::doc::canonical::to_string(
            &json!({"format": 1, "kind": "ryupdm.properties", "number": "7"}),
        );
        std::fs::write(dir.path().join("parts/q/properties.json"), q).unwrap();
        let e = call(&mut s, "check", json!({})).unwrap_err();
        assert_eq!(
            codes(&e.details),
            ["part_number_duplicate", "part_number_duplicate"]
        );
        assert_eq!(e.details["findings"][0]["source"], "doctor");
        // Each involves both parts, so checking either keeps both.
        let e = call(&mut s, "check", json!({"part": "q"})).unwrap_err();
        assert_eq!(e.details["errors"], 2);
    }

    #[test]
    fn a_stale_sketch_is_a_warning() {
        let (dir, mut s) = project();
        let path = dir.path().join("parts/p/features/s.json");
        let text = std::fs::read_to_string(&path).unwrap();
        let mut v: Value = serde_json::from_str(&text).unwrap();
        v["entities"]["p3"]["m"] = json!([0.5, 0.5]);
        crate::doc::project::write_json(&path, &v).unwrap();
        let v = call(&mut s, "check", json!({})).unwrap();
        assert_eq!(v["passed"], true);
        assert_eq!(codes(&v), ["sketch_stale"]);
        assert_eq!(v["findings"][0]["severity"], "warning");
        assert_eq!(v["findings"][0]["source"], "doctor");
    }

    #[test]
    fn against_a_revision_it_says_what_changed() {
        use crate::git::tests::git;
        let (dir, mut s) = project();
        let root = dir.path();
        git(root, &["init", "-q", "-b", "main"]);
        git(
            root,
            &["config", crate::drivers::TEXTCONV, "ryucad git textconv"],
        );
        git(root, &["add", "."]);
        git(root, &["commit", "-q", "-m", "start"]);
        call(
            &mut s,
            "feature.edit",
            json!({"part": "q", "id": "c", "params": {"radius": 2}}),
        )
        .unwrap();
        let v = call(&mut s, "check", json!({"against": "HEAD"})).unwrap();
        assert_eq!(v["passed"], true, "{v}");
        let c = &v["against"]["parts"]["q"]["features"]["c"];
        assert_eq!(c["changes"][0]["to"], "2 mm");
        assert_ne!(c["model"]["from"]["hash"], c["model"]["to"]["hash"]);
        assert_eq!(v["against"]["from"]["rev"], "HEAD");
        // Only the part asked about.
        let p = call(&mut s, "check", json!({"part": "p", "against": "HEAD"})).unwrap();
        assert_eq!(p["against"]["parts"], json!({}));
        // A failing check keeps what changed in its report.
        call(
            &mut s,
            "feature.edit",
            json!({"part": "q", "id": "c", "params": {"radius": 1000000}}),
        )
        .unwrap();
        let e = call(&mut s, "check", json!({"against": "HEAD"})).unwrap_err();
        assert_eq!(e.code, Code::CheckFailed);
        assert_eq!(
            e.details["against"]["parts"]["q"]["features"]["c"]["model"]["to"]["ok"],
            false
        );
        let e = call(&mut s, "check", json!({"against": "nowhere"})).unwrap_err();
        assert_eq!(e.code, Code::RevNotFound);
    }

    #[test]
    fn an_unfinished_merge_fails() {
        use crate::validate::tests::{both_edit, boxed};
        let mut m = boxed();
        both_edit(&mut m, json!({"x": 15}), json!({"x": 12}));
        m.merge("theirs");
        let e = m.call("check", json!({})).unwrap_err();
        assert_eq!(e.code, Code::CheckFailed);
        let found: Vec<(&str, &str)> = e.details["findings"]
            .as_array()
            .unwrap()
            .iter()
            .map(|f| {
                (
                    f["code"].as_str().unwrap(),
                    f["files"][0].as_str().unwrap_or(""),
                )
            })
            .filter(|(c, _)| !c.starts_with("git_"))
            .collect();
        assert_eq!(
            found,
            [
                ("merge_conflict", "parts/p/features/b.json"),
                ("unmerged_path", "parts/p/features/b.json")
            ]
        );
        // Settled but not added: still unmerged.
        m.call(
            "merge.resolve",
            json!({"resolutions": [{"conflict": "parts/p/features/b.json#/params/x",
                                    "choice": "theirs"}]}),
        )
        .unwrap();
        let e = m.call("check", json!({})).unwrap_err();
        assert_eq!(e.details["errors"], 1);
        m.git(&["add", "-A"]);
        let v = m.call("check", json!({})).unwrap();
        assert_eq!(v["passed"], true, "{v}");
    }

    #[test]
    fn a_cache_that_disagrees_with_the_files_fails() {
        let (_dir, mut s) = project();
        s.rebuilt(&id("q")).unwrap();
        let wrong = Error::new(Code::Internal, "not what the files say");
        s.cached_mut(&id("q")).unwrap().features[0].1 = Err(wrong);
        let e = call(&mut s, "check", json!({})).unwrap_err();
        assert_eq!(codes(&e.details), ["cache_mismatch c"]);
        assert_eq!(e.details["findings"][0]["cached"]["error"], "internal");
    }
}
