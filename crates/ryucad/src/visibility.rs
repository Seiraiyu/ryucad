//! What a participant's window hides: session state, like selection and
//! camera, never in Git.
//!
//! A participant hides occurrences of assemblies and solids of parts. A
//! reference to hide or show may name a whole part or assembly
//! (`<part>`), an instance at any depth (`<asm>/<inst>[/<inst>…]`,
//! which reaches every occurrence under it, a pattern's copy
//! `<asm>/<pattern>:<i>/<seed>` included) or a part's solid by its
//! founder (`<part>/<solid>`). The hidden set is kept as what those reach
//! at the leaves: each occurrence (`<asm>/<path>`) or solid
//! (`<part>/<solid>`), so hiding a subassembly and showing one of its
//! bolts leaves the rest of it hidden. Faces, edges and vertices can't be
//! hidden on their own.

use crate::doc::assembly::Rev;
use crate::doc::files::PartKind;
use crate::doc::id::Id;
use crate::entity::EntityRef;
use crate::error::{Code, Error, Result};
use crate::session::Session;
use std::collections::BTreeSet;

/// Everything of `part` that can be hidden: an assembly's occurrences, a
/// part's solids as the model shows them.
pub fn leaves(session: &mut Session, part: &Id) -> Result<Vec<String>> {
    let p = session.project().part(part)?;
    if crate::server::part_kind(p) == PartKind::Assembly {
        let model = session.model(part, &Rev::Working)?;
        return Ok(model
            .occurrences
            .iter()
            .map(|o| format!("{part}/{}", crate::assembly::path_text(&o.path)))
            .collect());
    }
    let solids = crate::solid::Solids::of(p);
    let built = session.rebuilt(part)?;
    Ok(built
        .shown(&solids)
        .iter()
        .map(|s| format!("{part}/{}", s.solid))
        .collect())
}

/// What `reference` reaches at the leaves; `not_found` if nothing, and
/// `invalid_param` for a face, edge or vertex.
pub fn reach(session: &mut Session, reference: &str) -> Result<Vec<String>> {
    let (part, rest) = match reference.split_once('/') {
        Some((p, r)) => (Id::parse(p, "part")?, Some(r)),
        None => (Id::parse(reference, "part")?, None),
    };
    let all = leaves(session, &part)?;
    let Some(rest) = rest else {
        return Ok(all);
    };
    let assembly = crate::server::part_kind(session.project().part(&part)?) == PartKind::Assembly;
    let prefix = if assembly {
        let r = EntityRef::parse(reference)?.in_assembly();
        if !r.is_occurrence() {
            return Err(Error::new(
                Code::InvalidParam,
                format!("{reference} is a face, edge or vertex; hide occurrences or solids"),
            )
            .with("entity", reference));
        }
        format!("{part}/{}", crate::assembly::path_text(&r.path))
    } else {
        if rest.contains('/') {
            return Err(Error::new(
                Code::InvalidParam,
                format!("{reference} is a face, edge or vertex; hide occurrences or solids"),
            )
            .with("entity", reference));
        }
        format!("{part}/{rest}")
    };
    let reached: Vec<String> = all
        .into_iter()
        .filter(|l| *l == prefix || l.starts_with(&format!("{prefix}/")))
        .collect();
    if reached.is_empty() {
        return Err(Error::new(
            Code::NotFound,
            format!("{reference} reaches nothing that can be hidden"),
        )
        .with("entity", reference));
    }
    Ok(reached)
}

/// What every reference reaches, together.
pub fn reach_all(session: &mut Session, references: &[String]) -> Result<BTreeSet<String>> {
    let mut out = BTreeSet::new();
    for r in references {
        out.extend(reach(session, r)?);
    }
    Ok(out)
}

/// A change to a hidden set: everything shown first (`all`), then
/// `isolate` (everything else in their parts hidden), then `show`, then
/// `hide`.
#[derive(Clone, Debug, Default)]
pub struct Change {
    pub all: bool,
    pub isolate: Vec<String>,
    pub show: Vec<String>,
    pub hide: Vec<String>,
}

/// `hidden` after `change`. Nothing changes unless every reference
/// reaches something.
pub fn apply(
    session: &mut Session,
    hidden: &BTreeSet<String>,
    change: &Change,
) -> Result<BTreeSet<String>> {
    let isolated = reach_all(session, &change.isolate)?;
    let shown = reach_all(session, &change.show)?;
    let hide = reach_all(session, &change.hide)?;
    let mut out = if change.all {
        BTreeSet::new()
    } else {
        hidden.clone()
    };
    let mut parts: BTreeSet<Id> = BTreeSet::new();
    for r in &change.isolate {
        let p = r.split('/').next().expect("split gives one");
        parts.insert(Id::parse(p, "part")?);
    }
    for p in parts {
        for l in leaves(session, &p)? {
            if isolated.contains(&l) {
                out.remove(&l);
            } else {
                out.insert(l);
            }
        }
    }
    for l in &shown {
        out.remove(l);
    }
    out.extend(hide);
    Ok(out)
}

/// Whether `leaf` (an occurrence or a solid, as [`leaves`] writes it) is
/// hidden.
pub fn is_hidden(hidden: &BTreeSet<String>, leaf: &str) -> bool {
    hidden.contains(leaf)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::server::tests::{call, fresh};
    use serde_json::json;

    /// Part `bolt` (a shank), part `plate` (a base and a boss), assembly
    /// `pair` (two bolts) and assembly `frame` (the plate and two pairs).
    fn project() -> (tempfile::TempDir, Session) {
        let (dir, mut s) = fresh();
        let mut ok = |m: &str, v: serde_json::Value| {
            call(&mut s, m, v).unwrap_or_else(|e| panic!("{m}: {e}"));
        };
        ok("part.create", json!({"part": "bolt"}));
        ok(
            "feature.add",
            json!({"part": "bolt", "type": "cylinder", "id": "shank",
                   "params": {"radius": 2, "height": 10}}),
        );
        ok("part.create", json!({"part": "plate"}));
        ok(
            "feature.add",
            json!({"part": "plate", "type": "box", "id": "base",
                   "params": {"x": 60, "y": 30, "z": 6}}),
        );
        ok(
            "feature.add",
            json!({"part": "plate", "type": "cylinder", "id": "boss",
                   "params": {"radius": 5, "height": 4}, "placement": {"origin": [0, 0, 6]}}),
        );
        for a in ["pair", "frame"] {
            ok("part.create", json!({"part": a, "kind": "assembly"}));
        }
        for (id, x) in [("left", -8), ("right", 8)] {
            ok(
                "feature.add",
                json!({"part": "pair", "type": "instance", "id": id, "of": "bolt",
                       "placement": {"origin": [x, 0, 0]}}),
            );
        }
        ok(
            "feature.add",
            json!({"part": "frame", "type": "instance", "id": "base", "of": "plate"}),
        );
        for (id, x) in [("p1", 15), ("p2", 45)] {
            ok(
                "feature.add",
                json!({"part": "frame", "type": "instance", "id": id, "of": "pair",
                       "placement": {"origin": [x, 15, 6]}}),
            );
        }
        (dir, s)
    }

    fn set(xs: &[&str]) -> BTreeSet<String> {
        xs.iter().map(|x| x.to_string()).collect()
    }

    #[test]
    fn references_reach_their_leaves() {
        let (_dir, mut s) = project();
        assert_eq!(
            leaves(&mut s, &Id::parse("frame", "part").unwrap()).unwrap(),
            [
                "frame/base",
                "frame/p1/left",
                "frame/p1/right",
                "frame/p2/left",
                "frame/p2/right"
            ]
        );
        assert_eq!(
            reach(&mut s, "frame/p1").unwrap(),
            ["frame/p1/left", "frame/p1/right"]
        );
        assert_eq!(reach(&mut s, "frame/p2/right").unwrap(), ["frame/p2/right"]);
        assert_eq!(reach(&mut s, "plate/boss").unwrap(), ["plate/boss"]);
        assert_eq!(
            reach(&mut s, "plate").unwrap(),
            ["plate/base", "plate/boss"]
        );
        // Nothing there, a face, a part's face.
        assert_eq!(reach(&mut s, "frame/p3").unwrap_err().code, Code::NotFound);
        assert_eq!(
            reach(&mut s, "plate/nope").unwrap_err().code,
            Code::NotFound
        );
        let face = format!("plate/base/{}", "0".repeat(32));
        assert_eq!(reach(&mut s, &face).unwrap_err().code, Code::InvalidParam);
        let deep = format!("frame/base/base/{}", "0".repeat(32));
        assert_eq!(reach(&mut s, &deep).unwrap_err().code, Code::InvalidParam);
        assert_eq!(reach(&mut s, "nopart").unwrap_err().code, Code::NotFound);
    }

    #[test]
    fn hide_show_and_isolate() {
        let (_dir, mut s) = project();
        let change = |hide: &[&str], show: &[&str], isolate: &[&str], all| Change {
            all,
            isolate: isolate.iter().map(|x| x.to_string()).collect(),
            show: show.iter().map(|x| x.to_string()).collect(),
            hide: hide.iter().map(|x| x.to_string()).collect(),
        };
        let h = apply(
            &mut s,
            &BTreeSet::new(),
            &change(&["frame/p1"], &[], &[], false),
        )
        .unwrap();
        assert_eq!(h, set(&["frame/p1/left", "frame/p1/right"]));
        // Showing one bolt of a hidden pair leaves the other hidden.
        let h = apply(&mut s, &h, &change(&[], &["frame/p1/left"], &[], false)).unwrap();
        assert_eq!(h, set(&["frame/p1/right"]));
        // Isolating hides the rest of that assembly, and keeps what other
        // parts hide.
        let h = apply(&mut s, &h, &change(&["plate/boss"], &[], &[], false)).unwrap();
        let h = apply(&mut s, &h, &change(&[], &[], &["frame/p2"], false)).unwrap();
        assert_eq!(
            h,
            set(&[
                "frame/base",
                "frame/p1/left",
                "frame/p1/right",
                "plate/boss"
            ])
        );
        // A reference that reaches nothing changes nothing.
        let bad = apply(&mut s, &h, &change(&["frame/p9"], &[], &[], false));
        assert_eq!(bad.unwrap_err().code, Code::NotFound);
        let h = apply(&mut s, &h, &change(&[], &[], &[], true)).unwrap();
        assert!(h.is_empty());
        assert!(!is_hidden(&h, "frame/base"));
    }

    #[test]
    fn a_participants_window_hides_what_it_is_told() {
        use crate::live::{Kind, Live};
        use crate::server::tests::call_live;
        let (_dir, mut s) = project();
        let mut live = Live::new();
        let vis = |s: &mut Session, live: &mut Live, v: serde_json::Value| {
            call_live(s, live, None, "visibility.set", v)
        };
        // With no human, the caller's own window.
        let v = vis(&mut s, &mut live, json!({"hide": ["frame/p1"]})).unwrap();
        assert_eq!(v["hidden"], json!(["frame/p1/left", "frame/p1/right"]));
        let agent = v["participant"].as_str().unwrap().to_owned();
        // With one, the human's: an agent shows a person something.
        let human = live.hello(Kind::Human, Some("ann"), None).unwrap();
        let v = vis(&mut s, &mut live, json!({"isolate": ["frame/base"]})).unwrap();
        assert_eq!(v["participant"], json!(human));
        assert_eq!(
            live.get(&human).unwrap().to_json()["hidden"],
            json!([
                "frame/p1/left",
                "frame/p1/right",
                "frame/p2/left",
                "frame/p2/right"
            ])
        );
        let v = vis(
            &mut s,
            &mut live,
            json!({"show": ["frame/p2"], "participant": agent}),
        )
        .unwrap();
        assert_eq!(
            v["hidden"],
            json!(["frame/p1/left", "frame/p1/right"]),
            "the agent's own"
        );
        let v = vis(&mut s, &mut live, json!({"all": true})).unwrap();
        assert_eq!(v["hidden"], json!([]));
        let e = vis(&mut s, &mut live, json!({"hide": ["frame/nothing"]})).unwrap_err();
        assert_eq!(e.code, Code::NotFound);
        let e = call(&mut s, "visibility.set", json!({"hide": ["frame/p1"]})).unwrap_err();
        assert_eq!(e.code, Code::NoLiveSession);
    }

    #[test]
    fn renders_leave_out_what_is_hidden() {
        use crate::live::{Kind, Live};
        use crate::server::tests::render_pixels;
        let (dir, mut s) = project();
        let r = dir.path();
        let small = json!({"part": "frame", "width": 64, "height": 48});
        let mut without = small.clone();
        without["hidden"] = json!(["frame/p1", "frame/p2"]);
        let plate_only = render_pixels(r, Session::open(r).unwrap(), without);
        let all = render_pixels(r, Session::open(r).unwrap(), small.clone());
        assert_ne!(plate_only, all);
        // The same as an assembly that never placed the pairs.
        for p in ["p1", "p2"] {
            call(&mut s, "feature.delete", json!({"part": "frame", "id": p})).unwrap();
        }
        drop(s);
        assert_eq!(
            render_pixels(r, Session::open(r).unwrap(), small.clone()),
            plate_only
        );
        // A render leaves out what the caller's window hides.
        let mut live = Live::new();
        let human = live.hello(Kind::Human, Some("ann"), None).unwrap();
        let mut slot = Some(Session::open(r).unwrap());
        let mut ask = |me: Option<&str>, m: &str, v: serde_json::Value| {
            let req = crate::proto::Request::from_rpc(m, Some(v)).unwrap();
            crate::server::handle_safely(&mut slot, Some(&mut live), me, r, req).unwrap()
        };
        ask(None, "visibility.set", json!({"hide": ["frame/base"]}));
        let a = ask(Some(&human), "render", small.clone());
        let mut empty = small;
        empty["hidden"] = json!(["frame"]);
        let b = ask(None, "render", empty);
        assert_eq!(a.binary, b.binary, "nothing left to draw either way");
    }
}
