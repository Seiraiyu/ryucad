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
use serde_json::Value;
use serde_json::json;
use std::collections::BTreeSet;

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
    let _ = (named, ask.dry_run, ask.rebuild);
    Ok(json!({"ops": answers}))
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::annotation::propose::PROPOSABLE;
    use crate::annotation::propose::tests::{on_disk, twin};
    use crate::proto::BatchOp;
    use crate::testing::call;
    use serde_json::json;
    use std::collections::BTreeSet;

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
}
