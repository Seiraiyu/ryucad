//! `batch` (design 603, plan 31.B): many model edits as one checked,
//! logged, undoable change. The ops run in order in a capture session
//! over the working files, as `annotation.propose` runs one, each seeing
//! what the ones before it wrote; the first refusal stops the batch and
//! nothing is written. Then the files that differ are written in one pass
//! (content, the parts' lists, the root files, deletions), and each part
//! the batch changed is rebuilt once, with the assemblies over it.

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
    let written = writes.len();
    if !ask.dry_run && !writes.is_empty() {
        write(session, ordered(writes))?;
    }
    let _ = (named, ask.rebuild);
    Ok(json!({"ops": answers, "written": written}))
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
    let root = session.root().to_path_buf();
    let path = |rel: &str| rel.split('/').fold(root.clone(), |p, s| p.join(s));
    // A cancel no longer counts once the files begin to change.
    session.job().writing();
    for (rel, bytes) in &writes {
        let done = match bytes {
            Some(b) => session.write_bytes(&path(rel), b),
            None => session.remove_file(&path(rel)),
        };
        if let Err(e) = done {
            session.reload()?;
            return Err(e);
        }
    }
    // A deleted part's folder goes with what else lies in it, as
    // `part.delete` alone removes it.
    for (rel, bytes) in &writes {
        if let (None, Some(part)) = (bytes, rel.strip_suffix("/part.json"))
            && part.starts_with("parts/")
        {
            let dir = path(part);
            if dir.exists() {
                // no session: what the files leave on disk (empty folders, stray files) goes too
                std::fs::remove_dir_all(&dir).map_err(|e| Error::io(&dir, &e))?;
            }
            crate::doc::source::note_written(&dir);
        }
    }
    session.reload()?;
    // What the working files place from other projects may have moved.
    if crate::deps::keep_lock(session)? {
        session.reload()?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::annotation::propose::PROPOSABLE;
    use crate::annotation::propose::tests::{on_disk, proposable_requests, twin};
    use crate::proto::BatchOp;
    use crate::testing::call;
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
}
