//! `part.ensure` (design 603, plan 31.C): a part made exactly as a
//! script declares it, writing only what differs, with what became of
//! the part, each feature, its parameters and its properties.

use crate::doc::files::{Feature, PartKind};
use crate::doc::id::Id;
use crate::doc::project::feature_path;
use crate::error::{Code, Error, Result};
use crate::live::Live;
use crate::part::{check_revision, order, part_id, part_kind};
use crate::proto::Request;
use crate::server::dispatch;
use crate::session::Session;
use ryu_session::activity::Files;
use serde_json::{Map, Value, json};
use std::collections::BTreeMap;

/// What `part.ensure` is given.
#[derive(Clone, Debug)]
pub(crate) struct Ask {
    pub part: String,
    pub kind: Option<String>,
    pub params: Option<Map<String, Value>>,
    pub features: Vec<Value>,
    pub props: Option<Map<String, Value>>,
    pub expected: Option<String>,
}

impl Ask {
    /// The request's params again, as a batch's op holds them.
    pub(crate) fn to_params(&self) -> Value {
        let mut v = json!({"part": self.part, "features": self.features});
        if let Some(k) = &self.kind {
            v["kind"] = json!(k);
        }
        if let Some(p) = &self.params {
            v["params"] = json!(p);
        }
        if let Some(p) = &self.props {
            v["props"] = json!(p);
        }
        if let Some(r) = &self.expected {
            v["expected_revision"] = json!(r);
        }
        v
    }
}

/// `part.ensure`: in a capture session (a batch's), the end state built
/// there; alone, a one-op batch.
pub(crate) fn run(
    session: &mut Session,
    live: Option<&mut Live>,
    me: Option<&str>,
    ask: Ask,
) -> Result<Value> {
    if session.captures() {
        return apply(session, live, me, ask);
    }
    alone(session, live, me, ask)
}

/// `part.ensure` alone: a one-op batch, so one capture, one write pass,
/// one activity entry, one `undo` and the rebuild of what it changed,
/// answered as the op's own answer with the batch's status, revision,
/// rebuild, solves and warnings.
fn alone(
    session: &mut Session,
    live: Option<&mut Live>,
    me: Option<&str>,
    ask: Ask,
) -> Result<Value> {
    let pid = part_id(&ask.part)?;
    let op = crate::proto::BatchOp {
        method: "part.ensure".into(),
        params: Some(ask.to_params()),
    };
    let mut v = crate::batch::run(
        session,
        live,
        me,
        crate::batch::Ask {
            ops: vec![op],
            expect: None,
            dry_run: false,
            rebuild: true,
            prune: false,
        },
    )
    .map_err(unwrapped)?;
    let mut out = v["ops"][0]["result"].take();
    let part = &v["parts"][pid.as_str()];
    out["status"] = part["status"].clone();
    out["revision"] = part["revision"].clone();
    for key in ["rebuild", "assemblies"] {
        if let Some(x) = v.get(key) {
            out[key] = x.clone();
        }
    }
    if let Some(Value::Array(w)) = v.get("warnings") {
        let mut all = out["warnings"].as_array().cloned().unwrap_or_default();
        all.extend(w.iter().cloned());
        out["warnings"] = json!(all);
    }
    Ok(out)
}

/// A refusal of the one-op batch an ensure alone runs as, read as the
/// request's own: no `op` or `method`, and a pointer into its params as
/// one into the request.
fn unwrapped(mut e: Error) -> Error {
    let p = e
        .details
        .get("pointer")
        .and_then(Value::as_str)
        .and_then(|p| p.strip_prefix("/ops/0/params"))
        .map(str::to_owned);
    if let Some(d) = e.details.as_object_mut() {
        d.remove("op");
        d.remove("method");
    }
    match p {
        Some(p) => e.with("pointer", p),
        None => e,
    }
}

/// The part made as declared, in a capture session: made if missing; its
/// parameters file written whole; its features after the kept prefix
/// taken out unchecked and the declared rest added in order through
/// `feature.add`, each checked against the declared features before it,
/// a sketch then drawn by its ops; then its properties.
fn apply(
    session: &mut Session,
    mut live: Option<&mut Live>,
    me: Option<&str>,
    ask: Ask,
) -> Result<Value> {
    let pid = part_id(&ask.part)?;
    let kind = ask
        .kind
        .as_deref()
        .map(PartKind::parse)
        .transpose()
        .map_err(|e| e.with("pointer", "/kind"))?;
    let declared = entries(&ask.part, &ask.features)?;
    let before = session.snapshot();
    let existed = session.project().parts.contains_key(&pid);
    let old: Vec<Id> = if existed {
        let p = session.project().part(&pid)?;
        check_revision(p, ask.expected.as_deref())?;
        let is = part_kind(p);
        if let Some(k) = kind
            && k != is
        {
            return Err(Error::new(
                Code::InvalidParam,
                format!(
                    "{pid} is a {}; ensure doesn't change a part's kind (delete it first)",
                    is.as_str()
                ),
            )
            .with("part", &pid)
            .with("pointer", "/kind"));
        }
        let list = order(p)?;
        if let Some(f) = list
            .iter()
            .find(|f| matches!(p.features.get(*f), Some(Ok(Feature::Import(_)))))
        {
            return Err(Error::new(
                Code::InvalidParam,
                format!(
                    "{pid} holds the import {f}, which ensure can't write: ensure parts built from feature add's types"
                ),
            )
            .with("part", &pid)
            .with("feature", f));
        }
        list
    } else {
        if ask.expected.is_some() {
            return Err(Error::new(
                Code::Conflict,
                format!("part {pid} doesn't exist, so it has no revision to expect"),
            )
            .with("part", &pid));
        }
        let kind = kind.unwrap_or(PartKind::Part).as_str().to_owned();
        dispatch(
            session,
            live.as_deref_mut(),
            me,
            Request::PartCreate {
                part: ask.part.clone(),
                kind: Some(kind),
            },
        )?;
        Vec::new()
    };
    if let Some(params) = &ask.params {
        crate::param::replace(session, &ask.part, params)?;
    }
    // With the parameters as they were, what already matches stays.
    let params_moved = ask.params.is_some() && {
        let rel = format!("parts/{pid}/{}", crate::doc::project::PARAMETERS);
        status_of(&before, &session.snapshot(), &rel) != "unchanged"
    };
    let keep = if params_moved {
        0
    } else {
        kept(session, &pid, &old, &declared)
    };
    clear(session, &pid, &old[..keep])?;
    let mut warnings = Vec::new();
    for (j, e) in declared.iter().enumerate().skip(keep) {
        let v = dispatch(session, live.as_deref_mut(), me, e.add.clone())
            .map_err(|x| at_feature(x, j))?;
        warnings.extend(v["warnings"].as_array().into_iter().flatten().cloned());
        if let Some(ops) = &e.ops {
            dispatch(
                session,
                live.as_deref_mut(),
                me,
                Request::SketchEdit {
                    part: ask.part.clone(),
                    sketch: e.id.to_string(),
                    ops: ops.clone(),
                    expected_revision: None,
                },
            )
            .map_err(|x| at_feature(x, j))?;
        }
    }
    if let Some(props) = &ask.props {
        dispatch(
            session,
            live,
            me,
            Request::PropsSet {
                part: ask.part.clone(),
                properties: props.clone(),
                expected_revision: None,
            },
        )
        .map_err(|e| e.with("pointer", "/props"))?;
    }
    let after = session.snapshot();
    let dir = format!("parts/{pid}/");
    let feature = |f: &Id| format!("{dir}features/{f}.json");
    let mut features = Map::new();
    for e in &declared {
        features.insert(
            e.id.to_string(),
            json!(status_of(&before, &after, &feature(&e.id))),
        );
    }
    for f in old.iter().filter(|f| !declared.iter().any(|e| e.id == **f)) {
        features.insert(f.to_string(), json!("deleted"));
    }
    let status = if !existed {
        "created"
    } else if before
        .keys()
        .chain(after.keys())
        .filter(|k| k.starts_with(&dir))
        .any(|k| status_of(&before, &after, k) != "unchanged")
    {
        "updated"
    } else {
        "unchanged"
    };
    let p = session.project().part(&pid)?;
    let mut v = json!({"part": pid, "kind": part_kind(p).as_str(), "status": status,
                       "revision": p.revision, "features": features});
    if ask.params.is_some() {
        v["params"] = json!(status_of(
            &before,
            &after,
            &format!("{dir}{}", crate::doc::project::PARAMETERS)
        ));
    }
    if ask.props.is_some() {
        v["props"] = json!(status_of(&before, &after, &format!("{dir}properties.json")));
    }
    if !warnings.is_empty() {
        v["warnings"] = json!(warnings);
    }
    Ok(v)
}

/// What became of file `rel` from `before` to `after`.
fn status_of(before: &Files, after: &Files, rel: &str) -> &'static str {
    match (before.get(rel), after.get(rel)) {
        (None, Some(_)) => "created",
        (Some(_), None) => "deleted",
        (Some(a), Some(b)) if a[..] != b[..] => "updated",
        _ => "unchanged",
    }
}

/// How many of `old` (the part's list) the declaration keeps as they are:
/// the longest prefix declared in that order whose files `feature.add`
/// (and, for a sketch, its ops and solve) would write again byte for
/// byte. One that doesn't build ends it: adding it again says why.
fn kept(session: &mut Session, pid: &Id, old: &[Id], declared: &[Entry]) -> usize {
    let mut k = 0;
    while k < old.len() && k < declared.len() && old[k] == declared[k].id {
        let Some(f) = predicted(session, pid, &old[..k], &declared[k]) else {
            break;
        };
        let same = session
            .project()
            .part(pid)
            .ok()
            .and_then(|p| p.features.get(&old[k]))
            .and_then(|g| g.as_ref().ok())
            .is_some_and(|g| g.to_json() == f.to_json());
        if !same {
            break;
        }
        k += 1;
    }
    k
}

/// The feature `e` would be written as, after `prefix`: what `feature.add`
/// builds, and a sketch's ops applied and solved as `sketch.edit` would.
fn predicted(session: &mut Session, pid: &Id, prefix: &[Id], e: &Entry) -> Option<Feature> {
    let Request::FeatureAdd {
        ty,
        params,
        placement,
        fields,
        id,
        ..
    } = e.add.clone()
    else {
        unreachable!("an entry is a feature add")
    };
    let given = crate::feature::Given {
        params,
        placement,
        fields,
    };
    let (f, _) = super::feature::built(
        session,
        pid,
        &ty,
        given,
        id.as_deref(),
        prefix,
        prefix.len(),
    )
    .ok()?;
    let (Some(ops), Feature::Sketch(mut s)) = (&e.ops, f.clone()) else {
        return Some(f);
    };
    let unit = session.project().manifest.default_unit;
    let env = session.project().part(pid).ok()?.env.clone();
    let applied = crate::sketch::apply(&mut s, pid, ops, unit, &env).ok()?;
    let ctx = crate::sketch::context_now(session, pid, &e.id, &s).ok()?;
    let (solved, report) = crate::sketch::solve_edited(&e.id, &s, &applied.moved, &ctx).ok()?;
    (report.converged && report.state != ryusketch::State::Over).then_some(Feature::Sketch(solved))
}

/// Takes `pid`'s features other than `kept` out of its list and its
/// folder, unchecked (unlisted files too, so an id declared again is
/// free): what the declaration adds again is checked as it is added.
/// The assemblies over `pid` are solved again as after `feature.delete`.
fn clear(session: &mut Session, pid: &Id, kept: &[Id]) -> Result<()> {
    let p = session.project().part(pid)?;
    let gone: Vec<Id> = p
        .features
        .keys()
        .filter(|f| !kept.contains(f))
        .cloned()
        .collect();
    if gone.is_empty() && order(p)? == kept {
        return Ok(());
    }
    // What it takes out moves the part as `feature.delete` would, so the
    // assemblies over it are solved again as they would be: noted for a
    // batch's solve at its end, kept current otherwise.
    let assemblies = crate::mate::current(session, pid)?;
    // The list first: a crash then leaves unlisted files, never a listed
    // feature with no file (the batch writes lists after content anyway).
    crate::save::save(session, pid, &[], kept.to_vec())?;
    for f in &gone {
        let path = feature_path(session.root(), pid, f);
        session.remove_file(&path)?;
    }
    session.reload()?;
    crate::mate::keep_current(session, &assemblies)?;
    Ok(())
}

/// One declared feature: its id, its `feature.add`, and for a sketch the
/// `sketch.edit` ops that draw it.
#[derive(Clone, Debug)]
struct Entry {
    id: Id,
    add: Request,
    ops: Option<Vec<Value>>,
}

/// The keys a declared feature can't have: the list's order places it,
/// the ensure is checked as a whole, and the part is the ensure's.
const NOT_DECLARED: [&str; 5] = ["part", "after", "first", "expected_revision", "dry_run"];

/// `features` read as `feature.add`s on `part`, each naming its `id` (a
/// made-up one would differ between runs and rename its faces and edges),
/// none twice; a sketch's `ops` kept beside it.
fn entries(part: &str, features: &[Value]) -> Result<Vec<Entry>> {
    let mut seen: BTreeMap<Id, usize> = BTreeMap::new();
    let mut out = Vec::with_capacity(features.len());
    for (j, v) in features.iter().enumerate() {
        let at = |key: &str| format!("/features/{j}{key}");
        let Some(o) = v.as_object() else {
            return Err(Error::new(
                Code::InvalidParam,
                format!("feature {j} isn't an object of feature add's params"),
            )
            .with("pointer", at("")));
        };
        let mut o = o.clone();
        for key in NOT_DECLARED {
            if o.contains_key(key) {
                return Err(Error::new(
                    Code::InvalidParam,
                    format!(
                        "a declared feature has no {key}: the list's order places it and the ensure is checked as a whole"
                    ),
                )
                .with("pointer", at(&format!("/{key}"))));
            }
        }
        let is_sketch = o.get("type").and_then(Value::as_str) == Some("sketch");
        let ops = match o.remove("ops") {
            None => None,
            Some(Value::Array(ops)) if is_sketch => Some(ops),
            Some(_) => {
                return Err(Error::new(
                    Code::InvalidParam,
                    "only a sketch takes ops: sketch.edit's, as an array",
                )
                .with("pointer", at("/ops")));
            }
        };
        let Some(id) = o.get("id").and_then(Value::as_str) else {
            return Err(Error::new(
                Code::InvalidParam,
                "every declared feature names its id: a made-up one would differ between runs and rename its faces and edges",
            )
            .with("pointer", at("/id")));
        };
        let id = crate::part::feature_id(id).map_err(|e| e.with("pointer", at("/id")))?;
        if let Some(k) = seen.insert(id.clone(), j) {
            return Err(Error::new(
                Code::DuplicateId,
                format!("features {k} and {j} are both {id}"),
            )
            .with("feature", &id)
            .with("pointer", at("/id")));
        }
        o.insert("part".into(), json!(part));
        let add = Request::from_rpc("feature.add", Some(Value::Object(o)))
            .map_err(|e| at_feature(e, j))?;
        out.push(Entry { id, add, ops });
    }
    Ok(out)
}

/// `e`, refusing declared feature `j`: a pointer into its fields made one
/// into the declaration (`/features/<j>…`), none made `/features/<j>`; a
/// pointer into a file, beside its `path`, left alone.
fn at_feature(e: Error, j: usize) -> Error {
    let pointer = match (
        e.details.get("pointer").and_then(Value::as_str),
        e.details.get("path"),
    ) {
        (Some(p), None) => Some(format!("/features/{j}{p}")),
        (None, _) => Some(format!("/features/{j}")),
        (Some(_), Some(_)) => None,
    };
    match pointer {
        Some(p) => e.with("pointer", p),
        None => e,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::annotation::propose::tests::{on_disk, twin};
    use crate::testing::{call, call_live};

    fn pointer(e: &Error) -> &str {
        e.details["pointer"].as_str().unwrap()
    }

    /// The twin's plate as it is, declared.
    fn plate() -> Value {
        json!({"part": "plate", "params": {"w": "40 mm", "unused": "1 mm"}, "features": [
            {"id": "slab", "type": "box", "params": {"x": "w", "y": 20, "z": 5}},
            {"id": "extra", "type": "sphere", "params": {"radius": 2}, "placement": {"origin": [100, 0, 0]}},
            {"id": "s", "type": "sketch"}
        ]})
    }

    fn ensure(s: &mut Session, params: Value) -> Result<Value> {
        call(
            s,
            "batch",
            json!({"ops": [{"method": "part.ensure", "params": params}], "rebuild": false}),
        )
    }

    fn listed(s: &Session, part: &str) -> Vec<String> {
        let p = s
            .project()
            .part(&crate::part::part_id(part).unwrap())
            .unwrap();
        order(p).unwrap().iter().map(|f| f.to_string()).collect()
    }

    #[test]
    fn an_ensure_makes_the_part_as_declared() {
        let (_dir, mut s) = twin();
        // A new part: made, its parameters written, its features added.
        ensure(
            &mut s,
            json!({"part": "bolt", "params": {"r": "3 mm"}, "features": [
            {"id": "shank", "type": "cylinder", "params": {"radius": "r", "height": 20}},
            {"id": "head", "type": "box", "params": {"x": 8, "y": 8, "z": 3}}
        ], "props": {"name": "Bolt"}}),
        )
        .unwrap();
        assert_eq!(listed(&s, "bolt"), ["shank", "head"]);
        // Changed, reordered, one gone and one new: the list is the declaration.
        let mut d = plate();
        d["features"] = json!([
            {"id": "s", "type": "sketch"},
            {"id": "slab", "type": "box", "params": {"x": "w", "y": 30, "z": 5}},
            {"id": "knob", "type": "sphere", "params": {"radius": 1}}
        ]);
        ensure(&mut s, d).unwrap();
        assert_eq!(listed(&s, "plate"), ["s", "slab", "knob"]);
        let slab = call(
            &mut s,
            "feature.show",
            json!({"part": "plate", "id": "slab"}),
        )
        .unwrap();
        assert_eq!(slab["params"]["y"]["value"], "30 mm", "{slab}");
        // An assembly, by its kind.
        ensure(
            &mut s,
            json!({"part": "jig", "kind": "assembly", "features": [
                {"id": "b", "type": "instance", "of": "bolt", "placement": {"origin": [0, 0, 10]}}
            ]}),
        )
        .unwrap();
        assert_eq!(listed(&s, "jig"), ["b"]);
    }

    /// The end state is checked, not each step: a feature and the one
    /// that reads it go together, which `feature.delete` alone refuses.
    #[test]
    fn a_feature_and_its_reader_go_together() {
        let (_dir, mut s) = twin();
        let mut d = plate();
        d["features"].as_array_mut().unwrap().extend([
            json!({"id": "hole", "type": "cylinder", "params": {"radius": 1, "height": 20},
                   "placement": {"origin": [5, 5, -5]}}),
            json!({"id": "cut", "type": "boolean", "op": "subtract", "target": "slab", "tools": ["hole"]}),
        ]);
        ensure(&mut s, d).unwrap();
        let e = call(
            &mut s,
            "feature.delete",
            json!({"part": "plate", "id": "hole"}),
        )
        .unwrap_err();
        assert_eq!(e.code, Code::HasDependents);
        ensure(&mut s, plate()).unwrap();
        assert_eq!(listed(&s, "plate"), ["slab", "extra", "s"]);
    }

    #[test]
    fn an_ensure_refuses_with_its_own_pointers_and_writes_nothing() {
        let (dir, mut s) = twin();
        let before = on_disk(dir.path());
        let mut d = plate();
        d["features"][1]["params"]["radius"] = json!("nope");
        let e = ensure(&mut s, d).unwrap_err();
        assert!(
            e.details["pointer"]
                .as_str()
                .unwrap()
                .starts_with("/ops/0/params/features/1"),
            "{e:?}"
        );
        let e = ensure(
            &mut s,
            json!({"part": "pin", "kind": "assembly", "features": []}),
        )
        .unwrap_err();
        assert_eq!(
            (e.code, e.details["pointer"].clone()),
            (Code::InvalidParam, json!("/ops/0/params/kind"))
        );
        let e = ensure(
            &mut s,
            json!({"part": "nobody", "features": [], "expected_revision": "r"}),
        )
        .unwrap_err();
        assert_eq!(e.code, Code::Conflict);
        let mut d = plate();
        d["params"] = json!({"w": "40 mm"});
        d["features"][0]["params"]["x"] = json!("gone");
        let e = ensure(&mut s, d).unwrap_err();
        assert!(
            e.details["pointer"]
                .as_str()
                .unwrap()
                .starts_with("/ops/0/params/features/0"),
            "{e:?}"
        );
        assert_eq!(on_disk(dir.path()), before);
    }

    #[test]
    fn an_ensure_says_what_became_of_each_thing_from_the_bytes() {
        let (dir, mut s) = twin();
        let op = |v: &Value| v["ops"][0]["result"].clone();
        // As it is: nothing written.
        let before = on_disk(dir.path());
        let v = ensure(&mut s, plate()).unwrap();
        assert_eq!(v["written"], 0, "{v}");
        assert_eq!(on_disk(dir.path()), before);
        let r = op(&v);
        assert_eq!(r["status"], "unchanged", "{r}");
        assert_eq!(
            r["features"],
            json!({"slab": "unchanged", "extra": "unchanged", "s": "unchanged"})
        );
        assert_eq!(r["params"], "unchanged");
        assert!(r.get("props").is_none(), "not declared, not said");
        // One changed, one gone, one new.
        let mut d = plate();
        d["features"][0]["params"]["y"] = json!(30);
        d["features"][1] = json!({"id": "knob", "type": "sphere", "params": {"radius": 1}});
        let r = op(&ensure(&mut s, d).unwrap());
        assert_eq!(r["status"], "updated");
        assert_eq!(
            r["features"],
            json!({"slab": "updated", "knob": "created", "s": "unchanged", "extra": "deleted"})
        );
        // Reordered: every feature as it was, the part updated.
        let mut d = plate();
        d["features"][0]["params"]["y"] = json!(30);
        d["features"] = json!([d["features"][2], d["features"][0],
                               {"id": "knob", "type": "sphere", "params": {"radius": 1}}]);
        let r = op(&ensure(&mut s, d).unwrap());
        assert_eq!(r["status"], "updated");
        assert_eq!(
            r["features"],
            json!({"s": "unchanged", "slab": "unchanged", "knob": "unchanged"})
        );
        // A new part, its parameters and properties.
        let r = op(&ensure(&mut s, json!({"part": "bolt", "params": {"r": "3 mm"}, "props": {"name": "Bolt"},
            "features": [{"id": "shank", "type": "cylinder", "params": {"radius": "r", "height": 5}}]}))
        .unwrap());
        assert_eq!(
            (r["status"].clone(), r["params"].clone(), r["props"].clone()),
            (json!("created"), json!("created"), json!("created"))
        );
        assert_eq!(r["features"]["shank"], "created");
        // No parameters any more: the file goes.
        let r = op(&ensure(&mut s, json!({"part": "bolt", "params": {}, "props": {"name": "Bolt"},
            "features": [{"id": "shank", "type": "cylinder", "params": {"radius": 3, "height": 5}}]}))
        .unwrap());
        assert_eq!(
            (
                r["params"].clone(),
                r["props"].clone(),
                r["features"]["shank"].clone()
            ),
            (json!("deleted"), json!("unchanged"), json!("updated"))
        );
    }

    #[test]
    fn an_ensure_keeps_the_prefix_that_already_matches() {
        let (_dir, mut s) = twin();
        let pid = crate::part::part_id("plate").unwrap();
        let kept_of = |s: &mut Session, d: Value| {
            let old = order(s.project().part(&pid).unwrap()).unwrap();
            let es = entries("plate", d["features"].as_array().unwrap()).unwrap();
            kept(s, &pid, &old, &es)
        };
        assert_eq!(
            kept_of(&mut s, plate()),
            3,
            "all of it, the empty sketch too"
        );
        let mut d = plate();
        d["features"][1]["params"]["radius"] = json!(3);
        assert_eq!(kept_of(&mut s, d), 1);
        let mut d = plate();
        d["features"][0]["params"]["y"] = json!(21);
        assert_eq!(kept_of(&mut s, d), 0);
        let mut d = plate();
        d["features"] = json!([d["features"][1], d["features"][0], d["features"][2]]);
        assert_eq!(kept_of(&mut s, d), 0, "another order");
        // A sketch drawn by ops is predicted through its solve.
        let draw = json!([{"op": "add", "id": "a", "type": "point", "at": [0, 0]},
                          {"op": "add", "id": "b", "type": "point", "at": [5, 0]},
                          {"op": "add", "id": "l", "type": "line", "start": "a", "end": "b"}]);
        let mut d = plate();
        d["features"][2]["ops"] = draw.clone();
        ensure(&mut s, d.clone()).unwrap();
        assert_eq!(kept_of(&mut s, d.clone()), 3);
        let r = ensure(&mut s, d).unwrap();
        assert_eq!(r["written"], 0, "{r}");
    }

    #[test]
    fn an_ensure_alone_is_one_change() {
        let (dir, mut s) = twin();
        let mut live = Live::new();
        let before = on_disk(dir.path());
        let mut d = plate();
        d["features"][0]["params"]["y"] = json!(30);
        let v = call_live(&mut s, &mut live, None, "part.ensure", d).unwrap();
        assert_eq!(
            (v["status"].clone(), v["features"]["slab"].clone()),
            (json!("updated"), json!("updated")),
            "{v}"
        );
        assert_eq!(
            v["revision"],
            json!(
                s.project()
                    .part(&crate::part::part_id("plate").unwrap())
                    .unwrap()
                    .revision
            )
        );
        assert_eq!(v["rebuild"]["failed"], 0, "{v}");
        let log = call_live(&mut s, &mut live, None, "activity.log", json!({})).unwrap();
        assert_eq!(log["entries"].as_array().unwrap().len(), 1, "{log}");
        assert_eq!(log["entries"][0]["method"], "part.ensure");
        // Unchanged: no entry.
        let mut d = plate();
        d["features"][0]["params"]["y"] = json!(30);
        let v = call_live(&mut s, &mut live, None, "part.ensure", d).unwrap();
        assert_eq!(v["status"], "unchanged");
        let log = call_live(&mut s, &mut live, None, "activity.log", json!({})).unwrap();
        assert_eq!(log["entries"].as_array().unwrap().len(), 1);
        call_live(&mut s, &mut live, None, "undo", json!({})).unwrap();
        assert_eq!(on_disk(dir.path()), before);
        // A refusal reads as its own.
        let e = call(
            &mut s,
            "part.ensure",
            json!({"part": "plate", "features": [{"type": "box"}]}),
        )
        .unwrap_err();
        assert_eq!(e.details["pointer"], "/features/0/id", "{e:?}");
        assert!(e.details.get("op").is_none());
    }

    #[test]
    fn a_sketch_is_drawn_by_its_ops() {
        let (_dir, mut s) = twin();
        ensure(
            &mut s,
            json!({"part": "pin", "features": [
                {"id": "shank", "type": "cylinder", "params": {"radius": 2, "height": 10}},
                {"id": "k", "type": "sketch", "ops": [
                    {"op": "add", "id": "a", "type": "point", "at": [0, 0]},
                    {"op": "add", "id": "b", "type": "point", "at": [5, 0]},
                    {"op": "add", "id": "l", "type": "line", "start": "a", "end": "b"}
                ]}
            ]}),
        )
        .unwrap();
        let k = call(&mut s, "sketch.get", json!({"part": "pin", "sketch": "k"})).unwrap();
        assert!(k.to_string().contains("\"l\""), "{k}");
    }

    #[test]
    fn a_declaration_is_feature_adds_with_their_ids() {
        let es = entries(
            "p",
            &[
                json!({"id": "b", "type": "box", "params": {"x": 1, "y": 1, "z": 1}}),
                json!({"id": "s", "type": "sketch", "ops": [{"op": "add", "id": "q", "type": "point", "at": [0, 0]}]}),
            ],
        )
        .unwrap();
        assert_eq!(
            es.iter().map(|e| e.id.as_str()).collect::<Vec<_>>(),
            ["b", "s"]
        );
        assert_eq!(es[0].add.method(), "feature.add");
        assert_eq!(es[0].add.params()["part"], "p");
        assert!(es[0].ops.is_none());
        assert_eq!(es[1].ops.as_ref().unwrap().len(), 1);
        assert!(es[1].add.params().get("ops").is_none());
    }

    #[test]
    fn a_declaration_refuses_what_it_cant_mean() {
        let refused = |fs: Value| entries("p", fs.as_array().unwrap()).unwrap_err();
        let e = refused(json!([{"id": "b", "type": "box"}, {"type": "box"}]));
        assert_eq!(
            (e.code, pointer(&e)),
            (Code::InvalidParam, "/features/1/id")
        );
        let e = refused(json!([{"id": "b", "type": "box"}, {"id": "b", "type": "sphere"}]));
        assert_eq!((e.code, pointer(&e)), (Code::DuplicateId, "/features/1/id"));
        for key in ["after", "first", "expected_revision", "dry_run", "part"] {
            let mut f = json!({"id": "b", "type": "box"});
            f[key] = json!(true);
            let e = refused(json!([f]));
            assert_eq!(
                (e.code, pointer(&e)),
                (Code::InvalidParam, format!("/features/0/{key}").as_str())
            );
        }
        let e = refused(json!([{"id": "b", "type": "box", "ops": []}]));
        assert_eq!(
            (e.code, pointer(&e)),
            (Code::InvalidParam, "/features/0/ops")
        );
        let e = refused(json!([{"id": "b", "type": "box", "bogus": 1}]));
        assert_eq!(e.code, Code::InvalidDocument);
        assert!(pointer(&e).starts_with("/features/0"), "{e:?}");
        let e = refused(json!([7]));
        assert_eq!((e.code, pointer(&e)), (Code::InvalidParam, "/features/0"));
    }

    #[test]
    fn a_pointer_into_a_feature_becomes_one_into_the_declaration() {
        let e = at_feature(
            Error::new(Code::InvalidParam, "x").with("pointer", "/params/x"),
            2,
        );
        assert_eq!(pointer(&e), "/features/2/params/x");
        let e = at_feature(Error::new(Code::NotFound, "x"), 1);
        assert_eq!(pointer(&e), "/features/1");
        let e = at_feature(
            Error::new(Code::InvalidDocument, "x")
                .with("path", "parts/p/part.json")
                .with("pointer", "/features"),
            0,
        );
        assert_eq!(pointer(&e), "/features");
    }
}
