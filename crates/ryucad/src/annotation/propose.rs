//! Proposed changes (design 374): a request a form, label, drag or handle
//! would have sent, run against the files as its proposer sees them (the
//! working files with the proposer's own open changes laid over them, its
//! draft) in a capture session, so nothing reaches the folder. What it
//! would have written is kept in the annotation; `annotation:<id>` names
//! the files with it laid over; `annotation.apply` writes it.

use super::{Annotation, By, Change, hash, is_in, lay, read_all, readable};
use crate::activity::Files;
use crate::doc::id::Id;
use crate::error::{Code, Error, Result};
use crate::session::Session;
use serde_json::{Value, json};
use std::collections::{BTreeMap, BTreeSet};

/// The proposer's open annotations whose changes aren't in `files` yet:
/// each after every one its `after` names, else by when it was made, then
/// by ID.
pub fn mine<'a>(all: &'a BTreeMap<Id, Annotation>, by: &By, files: &Files) -> Vec<&'a Annotation> {
    let mut left: Vec<&Annotation> = all
        .values()
        .filter(|a| a.by == *by && a.status.is_open())
        .filter(|a| a.change.as_ref().is_some_and(|c| !is_in(files, c)))
        .collect();
    left.sort_by(|a, b| (&a.at, &a.id).cmp(&(&b.at, &b.id)));
    let mut out: Vec<&Annotation> = Vec::new();
    while !left.is_empty() {
        let placed: BTreeSet<&Id> = out.iter().map(|a| &a.id).collect();
        let waiting: BTreeSet<&Id> = left.iter().map(|a| &a.id).collect();
        // The first whose changes it stands on are laid already; a cycle
        // (files edited by hand) is broken by taking the first left.
        let i = left
            .iter()
            .position(|a| {
                a.change
                    .as_ref()
                    .expect("filtered")
                    .after
                    .iter()
                    .all(|x| placed.contains(x) || !waiting.contains(x))
            })
            .unwrap_or(0);
        out.push(left.remove(i));
    }
    out
}

/// `by`'s draft: the working files with `by`'s open changes laid over
/// them in order.
pub fn draft(session: &Session, by: &By) -> Files {
    let working = session.snapshot();
    let all = readable(&read_all(&working));
    let mut files = working.clone();
    for a in mine(&all, by, &working) {
        lay(&mut files, a.change.as_ref().expect("a change"));
    }
    files
}

/// The requests a person's edit may be proposed as: model edits only, a
/// subset of `Request::mutates`. Releases, PDM settings, dependencies,
/// imports, doctor's repairs, merge resolutions, undo and redo need
/// Direct edit.
pub const PROPOSABLE: [&str; 18] = [
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
];

/// What `annotation.propose` is given.
pub struct Ask {
    pub request: Value,
    pub comment: Option<String>,
    pub pins: Vec<crate::proto::PinArg>,
    pub rough: bool,
    pub by: By,
}

/// `annotation.propose`: runs the request over `by`'s draft in a capture
/// session, exactly as it would run, and keeps what it would have written
/// in a new annotation; answers the annotation and the request's own
/// answer. A refusal is the request's own, and no annotation is made.
pub fn propose(
    session: &mut Session,
    live: Option<&mut crate::live::Live>,
    me: Option<&str>,
    ask: Ask,
) -> Result<Value> {
    let method = ask.request["method"]
        .as_str()
        .unwrap_or_default()
        .to_owned();
    if !PROPOSABLE.contains(&method.as_str()) {
        return Err(Error::new(
            Code::InvalidParam,
            format!("{method:?} can't be proposed: only model edits can (annotation::PROPOSABLE); this one needs Direct edit"),
        )
        .with("method", &method));
    }
    let params = ask
        .request
        .get("params")
        .cloned()
        .unwrap_or_else(|| json!({}));
    let inner = crate::proto::Request::from_rpc(&method, Some(params.clone()))?;
    let working = session.snapshot();
    let all = readable(&read_all(&working));
    let earlier: Vec<Annotation> = super::propose::mine(&all, &ask.by, &working)
        .into_iter()
        .cloned()
        .collect();
    let drafted = draft(session, &ask.by);
    let mut capture = session.capturing(drafted.clone())?;
    let pins = super::pins_from(&mut capture, &ask.pins)?;
    let answer = crate::server::dispatch(&mut capture, live, me, inner)?;
    let mut base = BTreeMap::new();
    let mut writes = BTreeMap::new();
    for (path, bytes) in capture.captured() {
        let was = drafted.get(&path);
        if was.map(|b| b.as_ref()) == bytes.as_deref() {
            continue; // written again as it was
        }
        let content = match &bytes {
            None => None,
            Some(b) => Some(
                std::str::from_utf8(b)
                    .ok()
                    .and_then(|t| crate::doc::canonical::parse(t).ok())
                    .ok_or_else(|| {
                        Error::new(
                            Code::Internal,
                            format!("{method} wrote {path}, which isn't JSON"),
                        )
                        .with("path", &path)
                    })?,
            ),
        };
        base.insert(path.clone(), was.map(|b| hash(b)));
        writes.insert(path, content);
    }
    if writes.is_empty() {
        return Err(Error::new(
            Code::OperationNoEffect,
            format!("{method} would change nothing: no annotation made"),
        )
        .with("method", &method));
    }
    let after: Vec<Id> = earlier
        .iter()
        .filter(|a| {
            a.change
                .as_ref()
                .is_some_and(|c| c.writes.keys().any(|p| writes.contains_key(p)))
        })
        .map(|a| a.id.clone())
        .collect();
    let taken = super::taken(session);
    let a = Annotation {
        id: super::new_id(|s| taken.contains(s))?,
        by: ask.by,
        at: super::now(),
        status: crate::doc::annotation::Status::Open,
        claimed_by: None,
        pins,
        comment: ask.comment.unwrap_or_default(),
        view: None,
        change: Some(Change {
            request: json!({"method": method, "params": params}),
            rough: ask.rough,
            base,
            writes,
            after,
        }),
        replies: BTreeMap::new(),
        resolution: None,
    };
    super::save(session, &a)?;
    let all = read_all(&session.snapshot());
    Ok(json!({"annotation": super::entry(session, &all, &a), "answer": answer}))
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use crate::annotation::AuthorKind;
    use crate::annotation::tests::{comment, plate, put};
    use crate::server::tests::{call, fresh};
    use serde_json::json;

    /// `a` carrying a change that writes `value` to `rel`, over `base`.
    pub(crate) fn changing(
        id: &str,
        by: &By,
        at: &str,
        rel: &str,
        base: Option<&[u8]>,
        value: Value,
        after: Vec<&str>,
    ) -> Annotation {
        let mut a = comment(id, vec![]);
        a.by = by.clone();
        a.at = at.into();
        a.change = Some(Change {
            request: json!({"method": "feature.edit", "params": {}}),
            rough: false,
            base: [(rel.to_owned(), base.map(crate::annotation::hash))].into(),
            writes: [(rel.to_owned(), Some(value))].into(),
            after: after
                .into_iter()
                .map(|s| crate::doc::annotation::check_id(s).unwrap())
                .collect(),
        });
        a
    }

    #[test]
    fn a_draft_lays_the_proposers_open_changes_in_order() {
        let (_dir, mut s, _) = plate();
        let dave = By {
            participant: "dave (window 1)".into(),
            kind: AuthorKind::Human,
        };
        let rel = "parts/plate/features/slab.json";
        let now = s.snapshot()[rel].clone();
        let mut v: Value = serde_json::from_slice(&now).unwrap();
        v["params"]["x"] = json!("50 mm");
        let first = changing(
            "aaaaaaaaaa",
            &dave,
            "2026-11-05T14:00:00Z",
            rel,
            Some(&now),
            v.clone(),
            vec![],
        );
        let one = crate::annotation::bytes_of(&Some(v.clone())).unwrap();
        v["params"]["y"] = json!("30 mm");
        // Made in the same second, but laid after the change it stands on.
        let second = changing(
            "bbbbbbbbbb",
            &dave,
            "2026-11-05T14:00:00Z",
            rel,
            Some(&one),
            v.clone(),
            vec!["aaaaaaaaaa"],
        );
        let theirs = changing(
            "cccccccccc",
            &By::agent("agent"),
            "2026-11-05T13:00:00Z",
            rel,
            Some(&now),
            json!({}),
            vec![],
        );
        put(&mut s, &second);
        put(&mut s, &first);
        put(&mut s, &theirs);
        let all = crate::annotation::readable(&crate::annotation::read_all(&s.snapshot()));
        let order: Vec<&str> = mine(&all, &dave, &s.snapshot())
            .iter()
            .map(|a| a.id.as_str())
            .collect();
        assert_eq!(order, ["aaaaaaaaaa", "bbbbbbbbbb"]);
        let d = draft(&s, &dave);
        assert_eq!(
            d[rel].to_vec(),
            crate::annotation::bytes_of(&Some(v)).unwrap()
        );
        // Another participant's draft is the working files.
        assert_eq!(draft(&s, &By::agent("other"))[rel], now);
    }

    /// A project every proposable request has something to act on: part
    /// `plate` (a box `slab` sized by parameter `w`, a sphere `extra`, a
    /// sketch `s`, an unused parameter), part `pin`, coloured, assembly
    /// `rig` placing both with an explode `apart`, materials `steel` and
    /// `spare`, and PDM set up.
    pub(crate) fn twin() -> (tempfile::TempDir, Session) {
        let (dir, mut s) = fresh();
        let c = |s: &mut Session, m: &str, p: Value| call(s, m, p).unwrap();
        c(&mut s, "pdm.init", json!({}));
        c(&mut s, "part.create", json!({"part": "plate"}));
        c(
            &mut s,
            "param.set",
            json!({"part": "plate", "parameters": {"w": "40 mm", "unused": "1 mm"}}),
        );
        c(
            &mut s,
            "feature.add",
            json!({"part": "plate", "type": "box", "id": "slab",
                                       "params": {"x": "w", "y": 20, "z": 5}}),
        );
        c(
            &mut s,
            "feature.add",
            json!({"part": "plate", "type": "sphere", "id": "extra",
                                       "params": {"radius": 2}, "placement": {"origin": [100, 0, 0]}}),
        );
        c(
            &mut s,
            "feature.add",
            json!({"part": "plate", "type": "sketch", "id": "s"}),
        );
        c(&mut s, "part.create", json!({"part": "pin"}));
        c(
            &mut s,
            "feature.add",
            json!({"part": "pin", "type": "cylinder", "id": "shank",
                                       "params": {"radius": 2, "height": 10}}),
        );
        c(
            &mut s,
            "appearance.set",
            json!({"entities": ["pin/shank"], "colour": "#2060d0"}),
        );
        c(
            &mut s,
            "part.create",
            json!({"part": "rig", "kind": "assembly"}),
        );
        c(
            &mut s,
            "feature.add",
            json!({"part": "rig", "type": "instance", "id": "base",
                                       "of": "plate", "fixed": true}),
        );
        c(
            &mut s,
            "feature.add",
            json!({"part": "rig", "type": "instance", "id": "p1",
                                       "of": "pin", "placement": {"origin": [10, 10, 5]}}),
        );
        c(
            &mut s,
            "explode.save",
            json!({"part": "rig", "explode": "apart",
                                       "steps": [{"instances": ["p1"], "translate": [0, 0, 20]}]}),
        );
        c(
            &mut s,
            "materials.set",
            json!({"name": "steel", "density": "7850 kg/m^3"}),
        );
        c(
            &mut s,
            "materials.set",
            json!({"name": "spare", "density": "1000 kg/m^3"}),
        );
        (dir, s)
    }

    /// Every proposable request, as a person's window would send it.
    pub(crate) fn proposable_requests() -> Vec<(&'static str, Value)> {
        vec![
            (
                "feature.add",
                json!({"part": "plate", "type": "cylinder", "id": "boss",
                                   "params": {"radius": 2, "height": 3}}),
            ),
            (
                "feature.edit",
                json!({"part": "plate", "id": "slab", "params": {"y": 25}}),
            ),
            ("feature.delete", json!({"part": "plate", "id": "extra"})),
            (
                "feature.move",
                json!({"part": "plate", "id": "extra", "first": true}),
            ),
            (
                "sketch.edit",
                json!({"part": "plate", "sketch": "s",
                                   "ops": [{"op": "add", "type": "point", "at": [1, 2]}]}),
            ),
            (
                "param.set",
                json!({"part": "plate", "parameters": {"w": "45 mm"}}),
            ),
            ("param.delete", json!({"part": "plate", "name": "unused"})),
            (
                "param.rename",
                json!({"part": "plate", "from": "w", "to": "width"}),
            ),
            (
                "assembly.move",
                json!({"part": "rig", "instance": "p1", "translate": ["5 mm", 0, 0]}),
            ),
            (
                "appearance.set",
                json!({"entities": ["plate/slab"], "colour": "#d03030"}),
            ),
            ("appearance.clear", json!({"entities": ["pin/shank"]})),
            (
                "materials.set",
                json!({"name": "alu", "density": "2.7 g/cm^3"}),
            ),
            ("materials.delete", json!({"name": "spare"})),
            (
                "props.set",
                json!({"part": "plate", "properties": {"name": "Plate"}}),
            ),
            ("bom.renumber", json!({"part": "rig", "items": {"pin": 7}})),
            (
                "view.save",
                json!({"part": "plate", "view": "corner", "camera": {
                "eye": [100, -100, 100], "target": [0, 0, 0], "up": [0, 0, 1],
                "projection": {"perspective": {"fov_deg": 40}}}}),
            ),
            (
                "explode.save",
                json!({"part": "rig", "explode": "up",
                                    "steps": [{"instances": ["p1"], "translate": [0, 0, 10]}]}),
            ),
            ("explode.delete", json!({"part": "rig", "explode": "apart"})),
        ]
    }

    /// The project's files on disk, by relative path.
    pub(crate) fn on_disk(root: &std::path::Path) -> BTreeMap<String, Vec<u8>> {
        let mut out = BTreeMap::new();
        let mut walk = vec![root.to_path_buf()];
        while let Some(d) = walk.pop() {
            for e in std::fs::read_dir(&d).unwrap().flatten() {
                let p = e.path();
                let rel = crate::doc::source::relative(root, &p);
                if rel.starts_with(".git") {
                    continue;
                }
                if p.is_dir() {
                    walk.push(p)
                } else {
                    out.insert(rel, std::fs::read(&p).unwrap());
                }
            }
        }
        out
    }

    #[test]
    fn every_proposable_request_is_captured_as_it_would_be_written() {
        let methods: BTreeSet<&str> = proposable_requests().iter().map(|(m, _)| *m).collect();
        assert_eq!(methods, PROPOSABLE.iter().copied().collect::<BTreeSet<_>>());
        for (method, params) in proposable_requests() {
            // Proposed on one copy of the project, written on another.
            let (a_dir, mut a) = twin();
            let (b_dir, mut b) = twin();
            let before = on_disk(a_dir.path());
            let v = call(
                &mut a,
                "annotation.propose",
                json!({"request": {"method": method, "params": params.clone()},
                                "comment": format!("try {method}")}),
            )
            .unwrap_or_else(|e| panic!("{method}: {e:?}"));
            let id = v["annotation"]["id"].as_str().unwrap().to_owned();
            let after = on_disk(a_dir.path());
            let mut grown: Vec<&String> = after
                .keys()
                .filter(|k| before.get(*k) != after.get(*k))
                .collect();
            grown.retain(|k| !k.starts_with("annotations/"));
            assert!(grown.is_empty(), "{method} wrote {grown:?}");
            assert_eq!(
                after.len(),
                before.len() + 1,
                "{method}: only the annotation is new"
            );
            let b_before = on_disk(b_dir.path());
            call(&mut b, method, params.clone()).unwrap_or_else(|e| panic!("{method}: {e:?}"));
            let b_after = on_disk(b_dir.path());
            let written: BTreeMap<String, Option<Vec<u8>>> = b_before
                .keys()
                .chain(b_after.keys())
                .filter(|k| b_before.get(*k) != b_after.get(*k))
                .map(|k| (k.clone(), b_after.get(k).cloned()))
                .collect();
            let file: Value =
                serde_json::from_slice(&after[&format!("annotations/{id}.json")]).unwrap();
            let a = Annotation::from_json(&file).unwrap();
            let change = a.change.unwrap();
            let captured: BTreeMap<String, Option<Vec<u8>>> = change
                .writes
                .iter()
                .map(|(p, w)| (p.clone(), crate::annotation::bytes_of(w)))
                .collect();
            assert_eq!(
                captured, written,
                "{method}: the capture is the write, byte for byte"
            );
            for (p, h) in &change.base {
                assert_eq!(*h, before.get(p).map(|b| hash(b)), "{method}: base of {p}");
            }
            assert_eq!(change.request["method"], method);
        }
    }

    #[test]
    fn a_proposal_stands_on_the_proposers_earlier_ones() {
        let (dir, mut s) = twin();
        let first = call(&mut s, "annotation.propose", json!({
            "request": {"method": "materials.set", "params": {"name": "alu", "density": "2.7 g/cm^3"}}}))
            .unwrap();
        // Only the draft has alu, and its revision.
        let rev = first["answer"].as_str().map(str::to_owned);
        let second = call(
            &mut s,
            "annotation.propose",
            json!({
            "request": {"method": "materials.delete",
                        "params": {"name": "alu", "expected_revision": rev}}}),
        )
        .unwrap();
        assert_eq!(
            second["annotation"]["change"]["after"],
            json!([first["annotation"]["id"]])
        );
        assert!(
            !on_disk(dir.path())["materials.json"]
                .windows(3)
                .any(|w| w == b"alu")
        );
        // Another participant's draft has no alu to delete.
        let e = call(
            &mut s,
            "annotation.propose",
            json!({
            "request": {"method": "materials.delete", "params": {"name": "alu"}}, "as": "other"}),
        )
        .unwrap_err();
        assert_eq!(e.code, Code::NotFound);
    }

    #[test]
    fn refusals_are_the_requests_own_and_make_no_annotation() {
        let (dir, mut s) = twin();
        let before = on_disk(dir.path());
        let e = call(&mut s, "annotation.propose", json!({
            "request": {"method": "feature.edit", "params": {"part": "plate", "id": "nope", "params": {}}}}))
            .unwrap_err();
        assert_eq!(e.code, Code::NotFound);
        let e = call(
            &mut s,
            "annotation.propose",
            json!({
            "request": {"method": "release.propose", "params": {"part": "plate"}}}),
        )
        .unwrap_err();
        assert_eq!(e.code, Code::InvalidParam);
        assert_eq!(e.details["method"], "release.propose");
        let e = call(
            &mut s,
            "annotation.propose",
            json!({
            "request": {"method": "feature.edit",
                        "params": {"part": "plate", "id": "slab", "params": {"y": 20}}}}),
        )
        .unwrap_err();
        assert_eq!(e.code, Code::OperationNoEffect);
        assert_eq!(on_disk(dir.path()), before);
    }
}
