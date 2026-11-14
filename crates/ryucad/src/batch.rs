//! `batch` (design 603, plan 31.B): many model edits as one checked,
//! logged, undoable change. The ops run in order in a capture session
//! over the working files, as `annotation.propose` runs one, each seeing
//! what the ones before it wrote; the first refusal stops the batch and
//! nothing is written. Then the files that differ are written in one pass
//! (content, the parts' lists, the root files, deletions), and each part
//! the batch changed is rebuilt once, with the assemblies over it.

use crate::doc::id::Id;
use crate::error::{Code, Error, Result};
use crate::live::Live;
use crate::proto::{BatchOp, Request};
use crate::session::Session;
use ryu_session::activity::{Bytes, Files};
use serde_json::Value;
use serde_json::json;
use std::collections::{BTreeMap, BTreeSet};

/// What `batch` is given.
pub struct Ask {
    pub ops: Vec<BatchOp>,
    pub expect: Option<String>,
    pub dry_run: bool,
    pub rebuild: bool,
}

/// `batch`.
pub fn run(
    session: &mut Session,
    mut live: Option<&mut Live>,
    me: Option<&str>,
    ask: Ask,
) -> Result<Value> {
    let requests = requests(&ask.ops)?;
    session.check_project_revision(ask.expect.as_deref())?;
    let working = session.snapshot();
    let mut capture = session.capturing(working.clone())?;
    let mut answers = Vec::with_capacity(requests.len());
    let mut named = BTreeSet::new();
    for (i, (op, req)) in ask.ops.iter().zip(requests).enumerate() {
        // A cancel stops a batch between its ops (and inside one, through
        // the job the capture shares), before anything is written.
        session.job().check().map_err(|e| at_op(e, i, &op.method))?;
        if let Some(p) = op
            .params
            .as_ref()
            .and_then(|p| p.get("part"))
            .and_then(Value::as_str)
        {
            named.insert(p.to_owned());
        }
        let result = crate::server::dispatch(&mut capture, live.as_deref_mut(), me, req)
            .map_err(|e| at_op(e, i, &op.method))?;
        answers.push(json!({"method": op.method, "result": result}));
    }
    session.job().check()?;
    let writes = changed(&working, capture.captured());
    let parts = statuses(&named, &working, &writes);
    let written = writes.len();
    let (revision, parts_now) = if ask.dry_run {
        (capture.project_revision()?, parts_json(&capture, &parts))
    } else {
        if !writes.is_empty() {
            write(session, ordered(writes))?;
        }
        (session.project_revision()?, parts_json(session, &parts))
    };
    let mut answer = json!({"ops": answers, "parts": parts_now, "written": written,
                            "revision": revision});
    if ask.dry_run {
        answer["dry_run"] = json!(true);
    } else if ask.rebuild {
        answer["rebuild"] = rebuild(session, &parts);
    }
    Ok(answer)
}

/// The requests a batch may hold: the model edits a person may propose
/// (`annotation::PROPOSABLE`), and making and deleting parts. Undo,
/// releases, PDM settings, dependencies, imports, branch switches,
/// doctor's repairs, merge resolutions and another batch can't be.
pub const BATCHABLE: [&str; 27] = [
    "part.create",
    "part.delete",
    "feature.add",
    "feature.edit",
    "feature.delete",
    "feature.move",
    "sketch.edit",
    "param.set",
    "param.delete",
    "param.rename",
    "assembly.move",
    "appearance.set",
    "appearance.clear",
    "materials.set",
    "materials.delete",
    "props.set",
    "bom.renumber",
    "view.save",
    "explode.save",
    "explode.delete",
    "motion.save",
    "motion.delete",
    "drawing.create",
    "drawing.edit",
    "drawing.delete",
    "drawing.rename",
    "drawing.layout",
];

/// The batch's ops as requests, before any runs: `invalid_param` at
/// `/ops/<i>/method` for one that isn't [`BATCHABLE`], and an op's own
/// refusal (with [`at_op`]'s details) for params that don't parse.
pub fn requests(ops: &[BatchOp]) -> Result<Vec<Request>> {
    ops.iter()
        .enumerate()
        .map(|(i, op)| {
            if !BATCHABLE.contains(&op.method.as_str()) {
                return Err(Error::new(
                    Code::InvalidParam,
                    format!(
                        "op {i}: {:?} can't be batched: only model edits and part.create and part.delete can (batch::BATCHABLE)",
                        op.method
                    ),
                )
                .with("op", i)
                .with("method", &op.method)
                .with("pointer", format!("/ops/{i}/method")));
            }
            Request::from_rpc(&op.method, op.params.clone()).map_err(|e| at_op(e, i, &op.method))
        })
        .collect()
}

/// `e`, op `i`'s refusal, saying which op it was: `op` and `method` in
/// its details, and a `pointer` into the op's params made one into the
/// batch's (`/ops/<i>/params…`). A pointer into a file, beside its
/// `path`, is left alone.
pub(crate) fn at_op(e: Error, i: usize, method: &str) -> Error {
    let pointer = match (
        e.details.get("pointer").and_then(Value::as_str),
        e.details.get("path"),
    ) {
        (Some(p), None) => Some(format!("/ops/{i}/params{p}")),
        _ => None,
    };
    let e = e.with("op", i).with("method", method);
    match pointer {
        Some(p) => e.with("pointer", p),
        None => e,
    }
}

/// What the capture wrote that differs from the working files: each
/// path's new bytes, or `None` for a deletion. A file written again as
/// it was, or made and deleted again, isn't a change.
fn changed(
    working: &Files,
    captured: BTreeMap<String, Option<Bytes>>,
) -> BTreeMap<String, Option<Bytes>> {
    captured
        .into_iter()
        .filter(|(p, b)| working.get(p).map(|w| &**w) != b.as_deref())
        .collect()
}

/// Where a write goes in the pass: undo's order (`activity::Project::
/// is_index`) reversed. Content files, then the parts' lists, then the
/// root files; then deletions, the files that list others last.
fn rank(path: &str, bytes: Option<&Bytes>) -> u8 {
    let list = path.ends_with("/part.json");
    let root = !path.contains('/');
    match (bytes.is_some(), list, root) {
        (true, false, false) => 0,
        (true, true, _) => 1,
        (true, false, true) => 2,
        (false, false, false) => 3,
        (false, true, _) => 4,
        (false, false, true) => 5,
    }
}

/// `writes` in the order the pass writes them: by [`rank`], then by
/// path. A crash part way leaves complete files: at worst feature files
/// no list names yet, or a list naming the old set, which `doctor`
/// reports and running the batch again finishes.
pub(crate) fn ordered(writes: BTreeMap<String, Option<Bytes>>) -> Vec<(String, Option<Bytes>)> {
    let mut v: Vec<(String, Option<Bytes>)> = writes.into_iter().collect();
    v.sort_by_key(|(p, b)| rank(p, b.as_ref()));
    v
}

/// Writes `writes` in order, each file atomically, through the session;
/// then takes them in, and the lock of what they place from other
/// projects. A write that fails leaves the ones before it, which the
/// activity log keeps, so `undo` takes them back.
fn write(session: &mut Session, writes: Vec<(String, Option<Bytes>)>) -> Result<()> {
    // A cancel no longer counts once the files begin to change.
    session.job().writing();
    // Whatever fails from here, the session takes in what is on disk, so
    // the activity log sees the files that did change and `undo` takes
    // them back; the first error is the answer.
    let done = put(session, &writes);
    let reloaded = session.reload();
    done?;
    reloaded?;
    // What the working files place from other projects may have moved.
    match crate::deps::keep_lock(session) {
        Ok(false) => Ok(()),
        Ok(true) => session.reload(),
        Err(e) => {
            let _ = session.reload();
            Err(e)
        }
    }
}

/// Writes and removes each file in order, then removes the folders of
/// the deleted parts, stopping at the first failure.
fn put(session: &mut Session, writes: &[(String, Option<Bytes>)]) -> Result<()> {
    let root = session.root().to_path_buf();
    let path = |rel: &str| rel.split('/').fold(root.clone(), |p, s| p.join(s));
    for (rel, bytes) in writes {
        match bytes {
            Some(b) => session.write_bytes(&path(rel), b)?,
            None => session.remove_file(&path(rel))?,
        }
    }
    // A deleted part's folder goes with what else lies in it, as
    // `part.delete` alone removes it.
    for (rel, bytes) in writes {
        if let (None, Some(part)) = (bytes, rel.strip_suffix("/part.json"))
            && part.starts_with("parts/")
        {
            let dir = path(part);
            // no session: what the files leave on disk (empty folders, stray files) goes too
            let removed = if dir.exists() {
                std::fs::remove_dir_all(&dir).map_err(|e| Error::io(&dir, &e))
            } else {
                Ok(())
            };
            crate::doc::source::note_written(&dir);
            removed?;
        }
    }
    Ok(())
}

/// Each part the ops named or the batch changed, and what became of it,
/// judged from the bytes: `created` (its list is new), `deleted` (its
/// list went), `updated` (any other file of it changed), else
/// `unchanged`. An op that writes the same bytes again changes nothing.
fn statuses(
    named: &BTreeSet<String>,
    working: &Files,
    writes: &BTreeMap<String, Option<Bytes>>,
) -> BTreeMap<String, &'static str> {
    let mut out: BTreeMap<String, &'static str> =
        named.iter().map(|p| (p.clone(), "unchanged")).collect();
    for (path, bytes) in writes {
        let Some(part) = path
            .strip_prefix("parts/")
            .and_then(|r| r.split('/').next())
        else {
            continue;
        };
        let list = path.ends_with("/part.json");
        let status = match (list, working.contains_key(path), bytes.is_some()) {
            (true, false, true) => "created",
            (true, true, false) => "deleted",
            _ => "updated",
        };
        let e = out.entry(part.to_owned()).or_insert("unchanged");
        if *e == "unchanged" || status != "updated" {
            *e = status;
        }
    }
    out
}

/// `parts` as the answer gives them: each one's status and its revision
/// as `s` has it now (null once deleted).
fn parts_json(s: &Session, parts: &BTreeMap<String, &'static str>) -> Value {
    let map: serde_json::Map<String, Value> = parts
        .iter()
        .map(|(p, status)| {
            let revision = crate::part::part_id(p)
                .ok()
                .and_then(|id| s.project().parts.get(&id))
                .map(|x| x.revision.clone());
            (p.clone(), json!({"status": status, "revision": revision}))
        })
        .collect();
    Value::Object(map)
}

/// Rebuilds each part the batch created or updated, then each assembly
/// that places it at working, each once: every one's report as `rebuild`
/// gives it, and how many features failed. A part that fails to build is
/// reported, never a refusal: the batch is written.
fn rebuild(session: &mut Session, parts: &BTreeMap<String, &'static str>) -> Value {
    let mut ids: Vec<Id> = Vec::new();
    let mut seen: BTreeSet<Id> = BTreeSet::new();
    for (p, status) in parts {
        if !matches!(*status, "created" | "updated") {
            continue;
        }
        let Ok(id) = crate::part::part_id(p) else {
            continue;
        };
        if !session.project().parts.contains_key(&id) {
            continue;
        }
        let over = crate::mate::reached(session.project(), &id);
        for a in std::iter::once(id).chain(over) {
            if seen.insert(a.clone()) {
                ids.push(a);
            }
        }
    }
    let mut reports = Vec::with_capacity(ids.len());
    let mut failed = 0;
    for id in &ids {
        match crate::part::part_report(session, id, false) {
            Ok((report, _)) => {
                failed += report["failed"].as_u64().unwrap_or(0);
                reports.push(report);
            }
            Err(e) => {
                failed += 1;
                reports.push(json!({"part": id, "error": e}));
            }
        }
    }
    json!({"parts": reports, "failed": failed})
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::annotation::propose::PROPOSABLE;
    use crate::annotation::propose::tests::{on_disk, proposable_requests, twin};
    use crate::proto::BatchOp;
    use crate::testing::{call, call_live};
    use ryu_session::job::Job;
    use serde_json::json;
    use std::collections::BTreeSet;

    /// What a project's files changed from `before` to `after`: each
    /// path's new bytes, or `None` for a deletion.
    fn change(
        before: &BTreeMap<String, Vec<u8>>,
        after: &BTreeMap<String, Vec<u8>>,
    ) -> BTreeMap<String, Option<Vec<u8>>> {
        before
            .keys()
            .chain(after.keys())
            .filter(|k| before.get(*k) != after.get(*k))
            .map(|k| (k.clone(), after.get(k).cloned()))
            .collect()
    }

    /// Asserts two twins changed the same (each twin has its own project
    /// ID), naming the files that differ.
    fn same(
        a: BTreeMap<String, Option<Vec<u8>>>,
        b: BTreeMap<String, Option<Vec<u8>>>,
        what: &str,
    ) {
        let text = |v: Option<&Option<Vec<u8>>>| match v {
            None => "(unchanged)".to_owned(),
            Some(None) => "(deleted)".to_owned(),
            Some(Some(v)) => String::from_utf8_lossy(v).into_owned(),
        };
        let paths: BTreeSet<&String> = a.keys().chain(b.keys()).collect();
        let diff: Vec<String> = paths
            .into_iter()
            .filter(|p| a.get(*p) != b.get(*p))
            .map(|p| {
                format!(
                    "{p}:\n  batch: {}\n  alone: {}",
                    text(a.get(p)),
                    text(b.get(p))
                )
            })
            .collect();
        assert!(diff.is_empty(), "{what}: {}", diff.join("\n"));
    }

    fn op(method: &str, params: Value) -> BatchOp {
        BatchOp {
            method: method.into(),
            params: Some(params),
        }
    }

    #[test]
    fn the_batchable_requests_are_the_model_edits_and_parts() {
        let want: BTreeSet<&str> = PROPOSABLE
            .iter()
            .copied()
            .chain(["part.create", "part.delete"])
            .collect();
        assert_eq!(BATCHABLE.iter().copied().collect::<BTreeSet<_>>(), want);
        assert_eq!(BATCHABLE.len(), want.len(), "no method twice");
    }

    #[test]
    fn an_unbatchable_op_is_refused_before_anything_runs() {
        for method in [
            "undo",
            "batch",
            "release.propose",
            "import",
            "merge.resolve",
        ] {
            let e = requests(&[
                op("part.create", json!({"part": "p"})),
                BatchOp {
                    method: method.into(),
                    params: None,
                },
            ])
            .unwrap_err();
            assert_eq!(e.code, Code::InvalidParam, "{method}");
            assert_eq!(e.details["op"], 1);
            assert_eq!(e.details["method"], method);
            assert_eq!(e.details["pointer"], "/ops/1/method");
        }
    }

    #[test]
    fn an_ops_bad_params_are_its_own_refusal() {
        let e = requests(&[op("part.create", json!({"part": "p", "bogus": 1}))]).unwrap_err();
        assert_eq!(e.code, Code::InvalidDocument);
        assert_eq!(
            (e.details["op"].clone(), e.details["method"].clone()),
            (json!(0), json!("part.create"))
        );
        let ok = requests(&[op("part.create", json!({"part": "p"}))]).unwrap();
        assert_eq!(ok[0].method(), "part.create");
        assert!(requests(&[]).unwrap().is_empty());
    }

    #[test]
    fn a_pointer_into_the_params_becomes_one_into_the_batch() {
        let e = at_op(
            Error::new(Code::InvalidParam, "x").with("pointer", "/parameters/w"),
            3,
            "param.set",
        );
        assert_eq!(e.details["pointer"], "/ops/3/params/parameters/w");
        assert_eq!(e.details["op"], 3);
        let in_file = at_op(
            Error::new(Code::InvalidDocument, "x")
                .with("path", "parts/p/part.json")
                .with("pointer", "/features/0"),
            0,
            "feature.add",
        );
        assert_eq!(in_file.details["pointer"], "/features/0");
        let none = at_op(Error::new(Code::NotFound, "x"), 2, "feature.edit");
        assert!(none.details.get("pointer").is_none());
    }

    #[test]
    fn each_op_sees_what_the_ones_before_it_wrote() {
        let (_dir, mut s) = twin();
        let v = call(
            &mut s,
            "batch",
            json!({"ops": [
                {"method": "feature.add", "params": {"part": "plate", "type": "cylinder", "id": "boss",
                                                     "params": {"radius": 2, "height": 3}}},
                {"method": "feature.edit", "params": {"part": "plate", "id": "boss", "params": {"radius": 4}}},
                {"method": "feature.move", "params": {"part": "plate", "id": "boss", "first": true}}
            ], "dry_run": true}),
        )
        .unwrap();
        let methods: Vec<&str> = v["ops"]
            .as_array()
            .unwrap()
            .iter()
            .map(|o| o["method"].as_str().unwrap())
            .collect();
        assert_eq!(methods, ["feature.add", "feature.edit", "feature.move"]);
    }

    #[test]
    fn the_first_refusal_stops_the_batch_and_nothing_is_written() {
        let (dir, mut s) = twin();
        let before = on_disk(dir.path());
        let e = call(
            &mut s,
            "batch",
            json!({"ops": [
                {"method": "feature.edit", "params": {"part": "plate", "id": "slab", "params": {"y": 30}}},
                {"method": "feature.edit", "params": {"part": "plate", "id": "nope", "params": {}}},
                {"method": "materials.set", "params": {"name": "alu", "density": "2.7 g/cm^3"}}
            ]}),
        )
        .unwrap_err();
        assert_eq!(e.code, Code::NotFound);
        assert_eq!(e.details["op"], 1);
        assert_eq!(e.details["method"], "feature.edit");
        assert_eq!(on_disk(dir.path()), before);
    }

    #[test]
    fn an_ops_expected_revision_is_checked_against_the_ops_before_it() {
        let (dir, mut s) = twin();
        let before = on_disk(dir.path());
        let plate = crate::part::part_id("plate").unwrap();
        let rev = s.project().part(&plate).unwrap().revision.clone();
        let e = call(
            &mut s,
            "batch",
            json!({"ops": [
                {"method": "feature.edit", "params": {"part": "plate", "id": "slab", "params": {"y": 30}}},
                {"method": "feature.edit", "params": {"part": "plate", "id": "slab", "params": {"z": 6},
                                                      "expected_revision": rev}}
            ]}),
        )
        .unwrap_err();
        assert_eq!(
            (e.code, e.details["op"].clone()),
            (Code::Conflict, json!(1))
        );
        assert_eq!(on_disk(dir.path()), before);
    }

    /// Every batchable request writes in a batch exactly what it writes
    /// alone, as `annotation::propose`'s test checks for proposals.
    #[test]
    fn every_batchable_request_writes_in_a_batch_what_it_writes_alone() {
        let mut asks = proposable_requests();
        asks.push(("part.create", json!({"part": "bolt", "kind": "assembly"})));
        asks.push(("part.delete", json!({"part": "rig"})));
        let methods: BTreeSet<&str> = asks.iter().map(|(m, _)| *m).collect();
        assert_eq!(methods, BATCHABLE.iter().copied().collect::<BTreeSet<_>>());
        for (method, params) in asks {
            let (a_dir, mut a) = twin();
            let (b_dir, mut b) = twin();
            let (a0, b0) = (on_disk(a_dir.path()), on_disk(b_dir.path()));
            call(
                &mut a,
                "batch",
                json!({"ops": [{"method": method, "params": params.clone()}]}),
            )
            .unwrap_or_else(|e| panic!("{method}: {e:?}"));
            call(&mut b, method, params.clone()).unwrap_or_else(|e| panic!("{method}: {e:?}"));
            same(
                change(&a0, &on_disk(a_dir.path())),
                change(&b0, &on_disk(b_dir.path())),
                method,
            );
        }
    }

    /// Many ops in one batch write what they write one by one.
    #[test]
    fn a_batch_writes_what_its_ops_write_one_by_one() {
        let ops = [
            ("part.create", json!({"part": "bolt"})),
            (
                "param.set",
                json!({"part": "bolt", "parameters": {"r": "3 mm"}}),
            ),
            (
                "feature.add",
                json!({"part": "bolt", "type": "cylinder", "id": "shank",
                                   "params": {"radius": "r", "height": 20}}),
            ),
            (
                "feature.edit",
                json!({"part": "plate", "id": "slab", "params": {"y": 30}}),
            ),
            ("feature.delete", json!({"part": "plate", "id": "extra"})),
            (
                "materials.set",
                json!({"name": "alu", "density": "2.7 g/cm^3"}),
            ),
            (
                "props.set",
                json!({"part": "bolt", "properties": {"name": "Bolt"}}),
            ),
            (
                "feature.add",
                json!({"part": "rig", "type": "instance", "id": "b1", "of": "bolt",
                                   "placement": {"origin": [30, 10, 5]}}),
            ),
            // Made and deleted again: nothing of it is written.
            ("part.create", json!({"part": "tmp"})),
            (
                "feature.add",
                json!({"part": "tmp", "type": "sphere", "id": "s", "params": {"radius": 1}}),
            ),
            ("part.delete", json!({"part": "tmp"})),
        ];
        let (a_dir, mut a) = twin();
        let (b_dir, mut b) = twin();
        let (a0, b0) = (on_disk(a_dir.path()), on_disk(b_dir.path()));
        let all = ops;
        let batch: Vec<Value> = all
            .iter()
            .map(|(m, p)| json!({"method": m, "params": p}))
            .collect();
        call(&mut a, "batch", json!({"ops": batch})).unwrap();
        for (m, p) in &all {
            call(&mut b, m, p.clone()).unwrap_or_else(|e| panic!("{m}: {e:?}"));
        }
        same(
            change(&a0, &on_disk(a_dir.path())),
            change(&b0, &on_disk(b_dir.path())),
            "one by one",
        );
        assert!(
            !a_dir.path().join("parts/tmp").exists(),
            "a part made and deleted leaves no folder"
        );
    }

    #[test]
    fn content_is_written_first_then_lists_then_root_files_then_deletions() {
        let b = |s: &str| -> Option<Bytes> { Some(Bytes::from(s.as_bytes())) };
        let writes: BTreeMap<String, Option<Bytes>> = [
            ("materials.json", b("{}")),
            ("parts/a/features/x.json", None),
            ("parts/a/part.json", b("{}")),
            ("parts/a/features/y.json", b("{}")),
            ("parts/z/part.json", None),
            ("parts/z/features/w.json", None),
            ("parts/b/features/n.json", b("{}")),
            ("ryucad.json", b("{}")),
        ]
        .into_iter()
        .map(|(p, w)| (p.to_owned(), w))
        .collect();
        let order: Vec<String> = ordered(writes).into_iter().map(|(p, _)| p).collect();
        assert_eq!(
            order,
            [
                "parts/a/features/y.json",
                "parts/b/features/n.json",
                "parts/a/part.json",
                "materials.json",
                "ryucad.json",
                "parts/a/features/x.json",
                "parts/z/features/w.json",
                "parts/z/part.json",
            ]
        );
    }

    #[test]
    fn each_part_says_what_became_of_it_from_the_bytes() {
        let (dir, mut s) = twin();
        call(
            &mut s,
            "props.set",
            json!({"part": "pin", "properties": {"name": "Pin"}}),
        )
        .unwrap();
        let ops = json!([
            {"method": "part.create", "params": {"part": "bolt"}},
            {"method": "feature.add", "params": {"part": "bolt", "type": "sphere", "id": "head",
                                                 "params": {"radius": 3}}},
            {"method": "feature.edit", "params": {"part": "plate", "id": "slab", "params": {"y": 30}}},
            {"method": "props.set", "params": {"part": "pin", "properties": {"name": "Pin"}}},
            {"method": "materials.set", "params": {"name": "steel", "density": "7850 kg/m^3"}},
            {"method": "part.delete", "params": {"part": "rig"}}
        ]);
        let before = on_disk(dir.path());
        // Run dry: the same answer, nothing written, no rebuild.
        let dry = call(&mut s, "batch", json!({"ops": ops, "dry_run": true})).unwrap();
        assert_eq!(on_disk(dir.path()), before);
        assert_eq!(dry["dry_run"], true);
        assert!(dry.get("rebuild").is_none());
        let v = call(&mut s, "batch", json!({"ops": ops, "rebuild": false})).unwrap();
        let status = |v: &Value, p: &str| v["parts"][p]["status"].as_str().unwrap().to_owned();
        for v in [&dry, &v] {
            assert_eq!(status(v, "bolt"), "created", "{v}");
            assert_eq!(status(v, "plate"), "updated");
            assert_eq!(status(v, "pin"), "unchanged", "the same properties again");
            assert_eq!(status(v, "rig"), "deleted");
            assert_eq!(v["parts"]["rig"]["revision"], Value::Null);
            assert_eq!(
                v["parts"].as_object().unwrap().len(),
                4,
                "materials name no part"
            );
        }
        assert_eq!(dry["written"], v["written"]);
        let bolt = crate::part::part_id("bolt").unwrap();
        assert_eq!(
            v["parts"]["bolt"]["revision"],
            json!(s.project().part(&bolt).unwrap().revision)
        );
        assert_eq!(v["revision"], json!(s.project_revision().unwrap()));
        assert_eq!(
            on_disk(dir.path())["materials.json"],
            before["materials.json"]
        );
        // The same edit again: unchanged, nothing written.
        let same = call(
            &mut s,
            "batch",
            json!({"ops": [{"method": "feature.edit", "params": {"part": "plate", "id": "slab", "params": {"y": 30}}}]}),
        )
        .unwrap();
        assert_eq!(
            (same["written"].clone(), status(&same, "plate")),
            (json!(0), "unchanged".to_owned())
        );
    }

    #[test]
    fn expect_is_the_project_revision_the_batch_was_written_against() {
        let (dir, mut s) = twin();
        let before = on_disk(dir.path());
        let edit = json!([{"method": "feature.edit",
                           "params": {"part": "plate", "id": "slab", "params": {"y": 30}}}]);
        let e = call(&mut s, "batch", json!({"ops": edit, "expect": "stale"})).unwrap_err();
        assert_eq!(e.code, Code::Conflict);
        assert_eq!(on_disk(dir.path()), before);
        let rev = s.project_revision().unwrap();
        // The project revision is its own settings' (the manifest, PDM,
        // materials, the lock, project parameters): a part's edit leaves it.
        let v = call(&mut s, "batch", json!({"ops": edit, "expect": rev})).unwrap();
        assert_eq!(v["revision"], json!(rev), "a part's edit leaves it");
        let materials = json!([{"method": "materials.set",
                                "params": {"name": "alu", "density": "2.7 g/cm^3"}}]);
        let v = call(&mut s, "batch", json!({"ops": materials, "expect": rev})).unwrap();
        assert_ne!(v["revision"], json!(rev), "the batch moved it");
        assert_eq!(v["revision"], json!(s.project_revision().unwrap()));
        let e = call(&mut s, "batch", json!({"ops": edit, "expect": rev})).unwrap_err();
        assert_eq!(e.code, Code::Conflict, "the old revision is stale now");
    }

    #[test]
    fn a_batch_rebuilds_what_it_changed_and_the_assemblies_over_it_once() {
        let (_dir, mut s) = twin();
        let edit = |r: u32| {
            json!({"ops": [
                {"method": "feature.edit", "params": {"part": "pin", "id": "shank", "params": {"radius": r}}},
                {"method": "feature.edit", "params": {"part": "pin", "id": "shank", "params": {"height": 12}}}
            ]})
        };
        let v = call(&mut s, "batch", edit(3)).unwrap();
        let rebuilt: Vec<&str> = v["rebuild"]["parts"]
            .as_array()
            .unwrap()
            .iter()
            .map(|r| r["part"].as_str().unwrap())
            .collect();
        assert_eq!(
            rebuilt,
            ["pin", "rig"],
            "pin once, then the assembly placing it: {v}"
        );
        assert_eq!(v["rebuild"]["failed"], 0);
        // Asked not to, it doesn't.
        let v = call(
            &mut s,
            "batch",
            json!({"ops": edit(4)["ops"], "rebuild": false}),
        )
        .unwrap();
        assert!(v.get("rebuild").is_none());
        // Nothing changed: nothing to rebuild.
        let v = call(&mut s, "batch", edit(4)).unwrap();
        assert_eq!(v["rebuild"]["parts"], json!([]));
    }

    #[test]
    fn a_batch_is_one_activity_entry_and_one_undo() {
        let (dir, mut s) = twin();
        let mut live = Live::new();
        let before = on_disk(dir.path());
        call_live(&mut s, &mut live, None, "batch", json!({"ops": [
            {"method": "feature.edit", "params": {"part": "plate", "id": "slab", "params": {"y": 30}}},
            {"method": "part.create", "params": {"part": "bolt"}},
            {"method": "feature.add", "params": {"part": "bolt", "type": "sphere", "id": "head",
                                                 "params": {"radius": 3}}}
        ]}))
        .unwrap();
        let log = |s: &mut Session, live: &mut Live| {
            call_live(s, live, None, "activity.log", json!({})).unwrap()["entries"]
                .as_array()
                .unwrap()
                .clone()
        };
        let e = log(&mut s, &mut live);
        assert_eq!(e.len(), 1, "{e:?}");
        assert_eq!(e[0]["method"], "batch");
        assert_eq!(e[0]["params"]["ops"].as_array().unwrap().len(), 3);
        call_live(&mut s, &mut live, None, "undo", json!({})).unwrap();
        assert_eq!(on_disk(dir.path()), before, "one undo takes the batch back");
        // A batch that changes nothing logs nothing.
        let n = log(&mut s, &mut live).len();
        call_live(
            &mut s,
            &mut live,
            None,
            "batch",
            json!({"ops": [
                {"method": "materials.set", "params": {"name": "steel", "density": "7850 kg/m^3"}}
            ]}),
        )
        .unwrap();
        assert_eq!(log(&mut s, &mut live).len(), n);
    }

    /// A write that fails part way leaves what it wrote, logged, so undo
    /// takes it back (design 603, "Error handling").
    #[cfg(unix)]
    #[test]
    fn a_write_that_fails_part_way_is_logged_and_undone() {
        use std::os::unix::fs::PermissionsExt;
        let (dir, mut s) = twin();
        let mut live = Live::new();
        let before = on_disk(dir.path());
        let locked = dir.path().join("parts/plate/features");
        std::fs::set_permissions(&locked, std::fs::Permissions::from_mode(0o555)).unwrap();
        // Root writes anyway: then there is nothing to test.
        if std::fs::write(locked.join("probe"), b"").is_ok() {
            std::fs::remove_file(locked.join("probe")).unwrap();
            std::fs::set_permissions(&locked, std::fs::Permissions::from_mode(0o755)).unwrap();
            eprintln!("skipped: this user writes into a read-only folder");
            return;
        }
        // `pin`'s new file is written first (by path), then `plate`'s fails.
        let e = call_live(&mut s, &mut live, None, "batch", json!({"ops": [
            {"method": "feature.add", "params": {"part": "pin", "type": "sphere", "id": "knob",
                                                 "params": {"radius": 1}}},
            {"method": "feature.add", "params": {"part": "plate", "type": "sphere", "id": "knob",
                                                 "params": {"radius": 1}}}
        ]}))
        .unwrap_err();
        std::fs::set_permissions(&locked, std::fs::Permissions::from_mode(0o755)).unwrap();
        assert_eq!(e.code, Code::Io, "{e:?}");
        assert!(dir.path().join("parts/pin/features/knob.json").is_file());
        let log = call_live(&mut s, &mut live, None, "activity.log", json!({})).unwrap();
        assert_eq!(log["entries"][0]["method"], "batch", "{log}");
        call_live(&mut s, &mut live, None, "undo", json!({})).unwrap();
        assert_eq!(on_disk(dir.path()), before);
    }

    /// A deleted part's folder that can't be removed (an empty folder in
    /// a read-only folder of it, which no file deletion names) leaves the
    /// part's files gone, logged, so undo takes them back.
    #[cfg(unix)]
    #[test]
    fn a_part_folder_that_wont_go_is_logged_and_undone() {
        use std::os::unix::fs::PermissionsExt;
        let (dir, mut s) = twin();
        let mut live = Live::new();
        let locked = dir.path().join("parts/rig/stray");
        std::fs::create_dir_all(locked.join("empty")).unwrap();
        let before = on_disk(dir.path());
        std::fs::set_permissions(&locked, std::fs::Permissions::from_mode(0o555)).unwrap();
        let unlock =
            || std::fs::set_permissions(&locked, std::fs::Permissions::from_mode(0o755)).unwrap();
        // Root removes it anyway: then there is nothing to test.
        if std::fs::write(locked.join("probe"), b"").is_ok() {
            std::fs::remove_file(locked.join("probe")).unwrap();
            unlock();
            eprintln!("skipped: this user writes into a read-only folder");
            return;
        }
        let e = call_live(&mut s, &mut live, None, "batch", json!({"ops": [
            {"method": "feature.edit", "params": {"part": "plate", "id": "slab", "params": {"y": 30}}},
            {"method": "part.delete", "params": {"part": "rig"}}
        ]}))
        .unwrap_err();
        unlock();
        assert_eq!(e.code, Code::Io, "{e:?}");
        assert!(e.message.contains("parts/rig"), "{e:?}");
        assert!(!dir.path().join("parts/rig/part.json").exists());
        assert!(locked.join("empty").is_dir());
        let log = call_live(&mut s, &mut live, None, "activity.log", json!({})).unwrap();
        assert_eq!(log["entries"][0]["method"], "batch", "{log}");
        call_live(&mut s, &mut live, None, "undo", json!({})).unwrap();
        assert_eq!(on_disk(dir.path()), before);
    }

    #[test]
    fn a_cancel_stops_a_batch_between_its_ops_and_writes_nothing() {
        let (dir, mut s) = twin();
        let before = on_disk(dir.path());
        s.set_job(Job::cancelled_at(2));
        let e = call(&mut s, "batch", json!({"ops": [
            {"method": "feature.edit", "params": {"part": "plate", "id": "slab", "params": {"y": 30}}},
            {"method": "feature.edit", "params": {"part": "plate", "id": "slab", "params": {"z": 6}}},
            {"method": "materials.set", "params": {"name": "alu", "density": "2.7 g/cm^3"}}
        ]}))
        .unwrap_err();
        s.set_job(Job::default());
        assert_eq!(e.code, Code::Cancelled);
        assert_eq!(on_disk(dir.path()), before);
    }
}
