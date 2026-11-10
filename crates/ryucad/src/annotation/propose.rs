//! Proposed changes (design 374): a request a form, label, drag or handle
//! would have sent, run against the files as its proposer sees them (the
//! working files with the proposer's own open changes laid over them, its
//! draft) in a capture session, so nothing reaches the folder. What it
//! would have written is kept in the annotation; `annotation:<id>` names
//! the files with it laid over; `annotation.apply` writes it.

use super::{Annotation, By, Change, chain, hash, lay, read_all, readable};
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
        .filter(|a| a.change.is_some() && !super::applied(all, a, files))
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
    laid(session, by).0
}

/// [`draft`], and the changes it laid: each of [`mine`] whose files are,
/// as the ones before leave them, the bytes it saw. A stale change isn't
/// laid (it would put back bytes someone has since changed), and nothing
/// proposed over the draft stands on it.
pub fn laid(session: &Session, by: &By) -> (Files, Vec<Annotation>) {
    let working = session.snapshot();
    let all = readable(&read_all(&working));
    let mut files = working.clone();
    let mut out = Vec::new();
    for a in mine(&all, by, &working) {
        let c = a.change.as_ref().expect("a change");
        if c.base
            .iter()
            .all(|(p, b)| files.get(p).map(|x| hash(x)) == *b)
        {
            lay(&mut files, c);
            out.push(a.clone());
        }
    }
    (files, out)
}

/// The requests a person's edit may be proposed as: model edits only, a
/// subset of `Request::mutates`. Releases, PDM settings, dependencies,
/// imports, doctor's repairs, merge resolutions, undo and redo need
/// Direct edit.
pub const PROPOSABLE: [&str; 23] = [
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
    "drawing.create",
    "drawing.edit",
    "drawing.delete",
    "drawing.rename",
    "drawing.layout",
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
    let (drafted, earlier) = laid(session, &ask.by);
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

/// `annotation.apply`.
pub fn apply(
    session: &mut Session,
    id: &str,
    keep_open: bool,
    expect: Option<&str>,
    by: &By,
) -> Result<Value> {
    let a = super::one(session, id, expect)?;
    if a.change.is_none() {
        return Err(Error::new(
            Code::InvalidParam,
            format!("annotation {id} proposes no change"),
        )
        .with("annotation", id));
    };
    let working = session.snapshot();
    if super::applied(&readable(&read_all(&working)), &a, &working) {
        return Err(Error::new(
            Code::OperationNoEffect,
            format!("annotation {id}'s change is in the working files already"),
        )
        .with("annotation", id));
    }
    let all = readable(&read_all(&working));
    let mut steps: Vec<Annotation> = chain(&all, &a, &working).into_iter().cloned().collect();
    steps.push(a.clone());
    // Every step checked against the files as the steps before leave
    // them, before anything is written.
    let mut files = working.clone();
    for step in &steps {
        let c = step.change.as_ref().expect("a change");
        let moved: Vec<Value> = c
            .base
            .iter()
            .filter(|(p, b)| files.get(*p).map(|x| hash(x)) != **b)
            .map(|(p, b)| json!({"path": p, "base": b, "now": files.get(p).map(|x| hash(x))}))
            .collect();
        if !moved.is_empty() {
            return Err(Error::new(
                Code::AnnotationStale,
                format!(
                    "annotation {}'s change no longer applies: {} file(s) moved since it was proposed; make the edit yourself and resolve it with a note",
                    step.id,
                    moved.len()
                ),
            )
            .with("annotation", &step.id)
            .with("files", moved));
        }
        lay(&mut files, c);
    }
    let root = session.root().to_path_buf();
    let path = |rel: &str| rel.split('/').fold(root.clone(), |p, s| p.join(s));
    for step in &steps {
        for (rel, w) in &step.change.as_ref().expect("a change").writes {
            match w {
                Some(v) => session.write_json(&path(rel), v)?,
                None => session.remove_file(&path(rel))?,
            }
        }
    }
    let at = super::now();
    for step in &steps {
        let mut s = step.clone();
        if !keep_open {
            s.status = crate::doc::annotation::Status::Done;
            s.claimed_by = None;
            s.resolution = Some(crate::doc::annotation::Resolution {
                by: by.clone(),
                at: at.clone(),
                note: None,
                applied: true,
            });
        }
        super::save(session, &s)?;
    }
    session.reload()?;
    // What the working files place from other projects may have moved.
    if crate::deps::keep_lock(session)? {
        session.reload()?;
    }
    let applied: Vec<&Id> = steps.iter().map(|s| &s.id).collect();
    Ok(json!({"applied": applied, "keep_open": keep_open}))
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
    fn a_persons_draft_is_a_revision() {
        let (_dir, mut s, _) = plate();
        let dave = By {
            participant: "dave (window 1)".into(),
            kind: AuthorKind::Human,
        };
        let rel = "parts/plate/features/slab.json";
        let now = s.snapshot()[rel].clone();
        let mut v: Value = serde_json::from_slice(&now).unwrap();
        v["params"]["x"] = json!("50 mm");
        put(
            &mut s,
            &changing(
                "aaaaaaaaaa",
                &dave,
                "2026-11-05T14:00:00Z",
                rel,
                Some(&now),
                v,
                vec![],
            ),
        );
        let rev = "draft:dave (window 1)";
        // The draft has dave's change; the working files don't.
        let i = call(&mut s, "inspect", json!({"part": "plate", "rev": rev})).unwrap();
        let w = call(&mut s, "inspect", json!({"part": "plate"})).unwrap();
        assert_ne!(i["bodies"][0]["hash"], w["bodies"][0]["hash"]);
        let d = call(
            &mut s,
            "diff",
            json!({"from": "working", "to": rev, "model": true}),
        )
        .unwrap();
        assert_eq!(d["to"]["draft"], "dave (window 1)");
        assert_eq!(d["to"]["annotations"], json!(["aaaaaaaaaa"]));
        assert_eq!(d["parts"]["plate"]["features"]["slab"]["status"], "changed");
        // Someone with no open changes has the working files as a draft.
        let e = call(
            &mut s,
            "inspect",
            json!({"part": "plate", "rev": "draft:erin (window 1)"}),
        )
        .unwrap();
        assert_eq!(e["bodies"][0]["hash"], w["bodies"][0]["hash"]);
        // A draft names its participant.
        let e = call(&mut s, "inspect", json!({"part": "plate", "rev": "draft:"})).unwrap_err();
        assert_eq!(e.code, crate::error::Code::InvalidParam);
    }

    #[test]
    fn forms_and_drawings_read_a_draft() {
        let (_dir, mut s, _) = plate();
        let dave = By {
            participant: "dave (window 1)".into(),
            kind: AuthorKind::Human,
        };
        let rel = "parts/plate/features/slab.json";
        let now = s.snapshot()[rel].clone();
        let mut v: Value = serde_json::from_slice(&now).unwrap();
        v["params"]["x"] = json!("50 mm");
        put(
            &mut s,
            &changing(
                "aaaaaaaaaa",
                &dave,
                "2026-11-05T14:00:00Z",
                rel,
                Some(&now),
                v,
                vec![],
            ),
        );
        let rev = "draft:dave (window 1)";
        let drafted = call(
            &mut s,
            "feature.show",
            json!({"part": "plate", "id": "slab", "rev": rev}),
        )
        .unwrap();
        assert_eq!(drafted["params"]["x"]["value"], "50 mm", "{drafted}");
        let working = call(
            &mut s,
            "feature.show",
            json!({"part": "plate", "id": "slab"}),
        )
        .unwrap();
        assert_eq!(working["params"]["x"]["value"], "40 mm", "{working}");
        // The revision it was read at is the draft's: a proposal made from
        // it is guarded by it.
        assert_ne!(drafted["revision"], working["revision"]);
        for (method, params) in [
            ("param.list", json!({"part": "plate", "rev": rev})),
            ("props.get", json!({"part": "plate", "rev": rev})),
        ] {
            call(&mut s, method, params).unwrap();
        }
    }

    #[test]
    fn a_solve_reads_a_draft() {
        // dave proposes the pin 5 mm up; the working files keep it where it
        // is, and assembly.solve at his draft sees the proposal.
        let (_dir, mut s, _, _) = crate::mate::current_tests::placed();
        let dave = By {
            participant: "dave (window 1)".into(),
            kind: AuthorKind::Human,
        };
        let rel = "parts/frame/features/pin.json";
        let now = s.snapshot()[rel].clone();
        let mut v: Value = serde_json::from_slice(&now).unwrap();
        let z = v["pose"]["origin_m"][2].as_f64().unwrap();
        v["pose"]["origin_m"][2] = json!(z + 0.005);
        put(
            &mut s,
            &changing(
                "aaaaaaaaaa",
                &dave,
                "2026-11-05T14:00:00Z",
                rel,
                Some(&now),
                v,
                vec![],
            ),
        );
        let at = |s: &mut Session, rev: Option<&str>| {
            let mut p = json!({"part": "frame"});
            if let Some(r) = rev {
                p["rev"] = json!(r);
            }
            let v = call(s, "assembly.solve", p).unwrap();
            (
                v["instances"]["pin"]["pose"]["origin_m"][2]
                    .as_f64()
                    .unwrap(),
                v,
            )
        };
        let (working, _) = at(&mut s, None);
        let (drafted, v) = at(&mut s, Some("draft:dave (window 1)"));
        assert!(
            (drafted - working - 0.005).abs() < 1e-9,
            "{working} → {drafted}"
        );
        assert_eq!(v["rev"]["draft"], "dave (window 1)", "{v}");
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
        c(
            &mut s,
            "drawing.create",
            json!({"part": "plate", "drawing": "main", "views": "standard"}),
        );
        // Moved by hand: laying it out again moves it back.
        c(
            &mut s,
            "drawing.edit",
            json!({"part": "plate", "drawing": "main",
                   "ops": [{"op": "move", "id": "v1", "at": ["200 mm", "200 mm"]}]}),
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
            (
                "drawing.create",
                json!({"part": "plate", "drawing": "second", "size": "iso_a4"}),
            ),
            (
                "drawing.edit",
                json!({"part": "plate", "drawing": "main",
                       "ops": [{"op": "set", "id": "v2", "fields": {"hidden": "dashed"}}]}),
            ),
            (
                "drawing.delete",
                json!({"part": "plate", "drawing": "main"}),
            ),
            (
                "drawing.rename",
                json!({"part": "plate", "drawing": "main", "to": "front"}),
            ),
            (
                "drawing.layout",
                json!({"part": "plate", "drawing": "main"}),
            ),
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
    #[test]
    fn an_annotation_is_a_revision() {
        let (_dir, mut s) = twin();
        let v = call(&mut s, "annotation.propose", json!({
            "request": {"method": "feature.edit", "params": {"part": "plate", "id": "slab", "params": {"y": 30}}}}))
            .unwrap();
        let rev = format!("annotation:{}", v["annotation"]["id"].as_str().unwrap());
        let d = call(
            &mut s,
            "diff",
            json!({"from": "working", "to": rev, "model": true}),
        )
        .unwrap();
        assert_eq!(d["to"]["annotation"], v["annotation"]["id"]);
        assert_eq!(d["parts"]["plate"]["features"]["slab"]["status"], "changed");
        assert!(d["parts"]["plate"]["features"]["slab"]["model"].is_object());
        let i = call(&mut s, "inspect", json!({"part": "plate", "rev": rev})).unwrap();
        assert!(i.get("warnings").is_none());
        // The model moves under it: still answers, warning it is stale.
        call(
            &mut s,
            "feature.edit",
            json!({"part": "plate", "id": "slab", "params": {"z": 6}}),
        )
        .unwrap();
        let i = crate::server::handle(
            &mut s,
            crate::proto::Request::from_rpc("inspect", Some(json!({"part": "plate", "rev": rev})))
                .unwrap(),
        )
        .unwrap();
        assert_eq!(i["warnings"][0]["code"], "annotation_stale");
        assert_eq!(
            i["warnings"][0]["files"][0]["path"],
            "parts/plate/features/slab.json"
        );
        let e = call(
            &mut s,
            "inspect",
            json!({"part": "plate", "rev": "annotation:aaaaaaaaaa"}),
        )
        .unwrap_err();
        assert_eq!(e.code, Code::AnnotationNotFound);
    }
    /// Annotation `id` as listings give it.
    fn listed(s: &mut Session, id: &str) -> Value {
        let a = crate::annotation::one(s, id, None).unwrap();
        let all = crate::annotation::read_all(&s.snapshot());
        crate::annotation::entry(s, &all, &a)
    }

    fn propose_edit(s: &mut Session, params: Value) -> String {
        let v = call(
            s,
            "annotation.propose",
            json!({
            "request": {"method": "feature.edit", "params": params}}),
        )
        .unwrap();
        v["annotation"]["id"].as_str().unwrap().to_owned()
    }

    #[test]
    fn apply_writes_the_chain_first_then_closes_each() {
        let (dir, mut s) = twin();
        let a = propose_edit(
            &mut s,
            json!({"part": "plate", "id": "slab", "params": {"y": 30}}),
        );
        let b = propose_edit(
            &mut s,
            json!({"part": "plate", "id": "slab", "params": {"z": 8}}),
        );
        let v = call(&mut s, "annotation.apply", json!({"id": b})).unwrap();
        assert_eq!(v["applied"], json!([a, b]));
        let slab: Value = serde_json::from_slice(
            &std::fs::read(dir.path().join("parts/plate/features/slab.json")).unwrap(),
        )
        .unwrap();
        assert_eq!(slab["params"]["y"], "30 mm");
        assert_eq!(slab["params"]["z"], "8 mm");
        for id in [&a, &b] {
            let x = listed(&mut s, id);
            assert_eq!(x["status"], "done");
            assert_eq!(x["resolution"]["applied"], true);
            assert_eq!(x["applied"], true);
        }
    }

    #[test]
    fn a_stale_change_is_refused_and_keep_open_keeps_it_open() {
        let (_dir, mut s) = twin();
        let a = propose_edit(
            &mut s,
            json!({"part": "plate", "id": "slab", "params": {"y": 30}}),
        );
        call(
            &mut s,
            "feature.edit",
            json!({"part": "plate", "id": "slab", "params": {"z": 6}}),
        )
        .unwrap();
        let e = call(&mut s, "annotation.apply", json!({"id": a})).unwrap_err();
        assert_eq!(e.code, Code::AnnotationStale);
        assert_eq!(
            e.details["files"][0]["path"],
            "parts/plate/features/slab.json"
        );
        let b = propose_edit(
            &mut s,
            json!({"part": "plate", "id": "slab", "params": {"x": 60}}),
        );
        call(
            &mut s,
            "annotation.apply",
            json!({"id": b, "keep_open": true}),
        )
        .unwrap();
        let x = listed(&mut s, &b);
        assert_eq!(x["status"], "open");
        assert_eq!(x["applied"], true);
        // Applied already: nothing left to apply.
        let e = call(&mut s, "annotation.apply", json!({"id": b})).unwrap_err();
        assert_eq!(e.code, Code::OperationNoEffect);
    }
}
