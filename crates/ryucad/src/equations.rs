//! `equations.list`: one table of every parameter of the project and of
//! a part, and every expression field of the part's features (a sketch's
//! dimensions among them): what each says, its value or why it has none,
//! and what it reads. Each row says where it lives, so whoever edits it
//! writes it where it is: a parameter through `param.set`, a feature's
//! field through `feature.edit` by its pointer, a sketch's dimension
//! through `sketch.edit`, each against the revision the table gives.

use crate::doc::files::expression_fields;
use crate::doc::id::Id;
use crate::doc::project::{Part, feature_path, relative};
use crate::error::Result;
use crate::expr::{Expr, Val};
use crate::server::part_id;
use crate::session::Session;
use serde_json::{Map, Value, json};
use std::collections::BTreeSet;

/// What `equations.list` asks: a part's (else every part's), and with
/// `all` the feature fields that are plain values too.
#[derive(Clone, Debug, Default)]
pub struct Ask {
    pub part: Option<String>,
    pub all: bool,
}

/// A row's text, value and reads: `value` (`{kind, m|rad|n|si}`) or
/// `error`, and `reads`, the parameters it names.
fn evaluated(text: &str, value: Result<Val>) -> Value {
    let reads: BTreeSet<String> = Expr::parse(text).map(|e| e.names()).unwrap_or_default();
    let mut v = json!({"text": text, "reads": reads});
    match value {
        Ok(x) => v["value"] = x.report(),
        Err(e) => v["error"] = json!(e),
    }
    v
}

/// `equations.list`: the project's parameters, then each part's (the one
/// asked for, else every part in ID order) parameters and, for each
/// listed feature in order, its expression fields in pointer order (with
/// `all`, plain values too). Its answer gives the project's revision and
/// each listed part's, which writes from the table are guarded by.
pub fn list(session: &mut Session, ask: &Ask, rev: Option<&str>) -> Result<Value> {
    let (commit, session) = session.at(rev)?;
    let project = session.project();
    let parts: Vec<&Part> = match &ask.part {
        Some(p) => vec![project.part(&part_id(p)?)?],
        None => project.parts.values().collect(),
    };
    let mut rows = Vec::new();
    if let Some(Ok(f)) = &project.parameters {
        for (name, e) in &f.parameters {
            let mut v = evaluated(e.text(), project.env.get(name));
            v["scope"] = json!("project");
            v["what"] = json!("parameter");
            v["name"] = json!(name);
            rows.push(v);
        }
    }
    let mut revisions = Map::new();
    for part in &parts {
        revisions.insert(part.id.to_string(), json!(part.revision));
        rows.extend(part_rows(session, part, ask.all));
    }
    let mut v = json!({
        "rows": rows,
        "revisions": {
            "project": crate::config::project_revision(session.root())?,
            "parts": revisions,
        },
    });
    if let Some(e) = project.parameters.as_ref().and_then(|p| p.as_ref().err()) {
        v["project_error"] = json!(e);
    }
    if let Some(c) = commit {
        v["commit"] = json!(c);
    }
    Ok(v)
}

/// A part's rows: its parameters, then its listed features' fields, each
/// read from the feature's file, so a feature that fails says why.
fn part_rows(session: &Session, part: &Part, all: bool) -> Vec<Value> {
    let mut rows = Vec::new();
    let on = |mut v: Value| {
        v["scope"] = json!("part");
        v["part"] = json!(part.id);
        v
    };
    if let Some(Ok(f)) = &part.parameters {
        for (name, e) in &f.parameters {
            let mut v = on(evaluated(e.text(), part.env.get(name)));
            v["what"] = json!("parameter");
            v["name"] = json!(name);
            rows.push(v);
        }
    }
    for fid in part.order() {
        let Some(json) = feature_json(session, part, fid) else {
            continue;
        };
        let ty = json["type"].as_str().unwrap_or_default().to_owned();
        for (pointer, text) in expression_fields(&json) {
            let expr = Expr::parse(&text);
            if !all && expr.as_ref().is_ok_and(Expr::is_literal) {
                continue;
            }
            let value = expr.and_then(|e| e.eval(&|n| part.env.get(n)));
            let mut v = on(evaluated(&text, value));
            v["feature"] = json!(fid);
            v["type"] = json!(ty);
            v["pointer"] = json!(pointer);
            // A sketch's dimension is a constraint's value.
            let constraint = pointer
                .strip_prefix("/constraints/")
                .and_then(|r| r.strip_suffix("/value"))
                .filter(|_| ty == "sketch");
            match constraint {
                Some(c) => {
                    v["what"] = json!("dimension");
                    v["constraint"] = json!(c);
                }
                None => v["what"] = json!("field"),
            }
            rows.push(v);
        }
    }
    rows
}

/// A listed feature's JSON as its file has it, if it reads.
fn feature_json(session: &Session, part: &Part, fid: &Id) -> Option<Value> {
    use crate::doc::source::Source;
    let path = feature_path(session.root(), &part.id, fid);
    let at = relative(session.root(), &path);
    let bytes = session.read(&at).ok()??;
    let v = crate::doc::canonical::parse(std::str::from_utf8(&bytes).ok()?).ok()?;
    Some(crate::doc::project::without_conflicts(&v))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::error::Code;
    use crate::server::tests::{call, fresh};

    /// Project parameter `wall`; part `p` with parameter `r` (`wall / 2`),
    /// a ball of radius `r` and a box of plain sizes but one; part `q`.
    fn project() -> (tempfile::TempDir, Session) {
        let (dir, mut s) = fresh();
        let mut ok = |m: &str, v: Value| {
            call(&mut s, m, v).unwrap_or_else(|e| panic!("{m}: {e}"));
        };
        ok(
            "param.set",
            json!({"scope": "project", "parameters": {"wall": "4 mm"}}),
        );
        for p in ["p", "q"] {
            ok("part.create", json!({"part": p}));
        }
        ok(
            "param.set",
            json!({"part": "p", "parameters": {"r": "wall / 2"}}),
        );
        ok(
            "feature.add",
            json!({"part": "p", "type": "sphere", "id": "ball", "params": {"radius": "r"}}),
        );
        ok(
            "feature.add",
            json!({"part": "p", "type": "box", "id": "b",
                   "params": {"x": "wall * 3", "y": 10, "z": 5}}),
        );
        (dir, s)
    }

    #[test]
    fn every_parameter_and_expression_in_one_table() {
        let (_dir, mut s) = project();
        let v = list(
            &mut s,
            &Ask {
                part: Some("p".into()),
                all: false,
            },
            None,
        )
        .unwrap();
        assert_eq!(
            v["rows"],
            json!([
                {"scope": "project", "what": "parameter", "name": "wall", "text": "4 mm",
                 "value": {"kind": "length", "m": 0.004}, "reads": []},
                {"scope": "part", "part": "p", "what": "parameter", "name": "r",
                 "text": "wall / 2", "value": {"kind": "length", "m": 0.002},
                 "reads": ["wall"]},
                {"scope": "part", "part": "p", "what": "field", "feature": "ball",
                 "type": "sphere", "pointer": "/params/radius", "text": "r",
                 "value": {"kind": "length", "m": 0.002}, "reads": ["r"]},
                {"scope": "part", "part": "p", "what": "field", "feature": "b",
                 "type": "box", "pointer": "/params/x", "text": "wall * 3",
                 "value": {"kind": "length", "m": 0.012}, "reads": ["wall"]},
            ])
        );
        let p = s.project().part(&part_id("p").unwrap()).unwrap();
        assert_eq!(v["revisions"]["parts"]["p"], json!(p.revision));
        assert_eq!(
            v["revisions"]["project"],
            json!(crate::config::project_revision(s.root()).unwrap())
        );
        // With `all`, plain values too.
        let all = list(
            &mut s,
            &Ask {
                part: Some("p".into()),
                all: true,
            },
            None,
        )
        .unwrap();
        let pointers: Vec<&str> = all["rows"]
            .as_array()
            .unwrap()
            .iter()
            .filter(|r| r["feature"] == "b")
            .map(|r| r["pointer"].as_str().unwrap())
            .collect();
        assert!(pointers.contains(&"/params/y"), "{pointers:?}");
        // Every part's, with no part named.
        let every = list(&mut s, &Ask::default(), None).unwrap();
        assert_eq!(
            every["revisions"]["parts"]
                .as_object()
                .unwrap()
                .keys()
                .collect::<Vec<_>>(),
            ["p", "q"]
        );
        let e = list(
            &mut s,
            &Ask {
                part: Some("nope".into()),
                all: false,
            },
            None,
        )
        .unwrap_err();
        assert_eq!(e.code, Code::NotFound);
    }

    #[test]
    fn what_fails_says_why_and_a_sketchs_dimensions_are_marked() {
        use crate::server::tests::with_sketch;
        let (dir, mut s) = with_sketch();
        s.refresh().unwrap();
        let v = list(&mut s, &Ask::default(), None).unwrap();
        let dim = v["rows"]
            .as_array()
            .unwrap()
            .iter()
            .find(|r| r["what"] == "dimension")
            .unwrap()
            .clone();
        assert_eq!(dim["feature"], "s");
        assert_eq!(dim["type"], "sketch");
        assert!(dim["constraint"].is_string(), "{dim}");
        assert_eq!(dim["reads"], json!(["width"]));
        // The parameter it reads gone (by hand): its row says why.
        std::fs::remove_file(dir.path().join("parts/p/parameters.json")).unwrap();
        s.refresh().unwrap();
        let v = list(&mut s, &Ask::default(), None).unwrap();
        let dim = v["rows"]
            .as_array()
            .unwrap()
            .iter()
            .find(|r| r["what"] == "dimension")
            .unwrap()
            .clone();
        assert_eq!(dim["error"]["code"], "unknown_parameter");
        assert!(dim.get("value").is_none());
    }

    #[test]
    fn a_feature_field_is_edited_by_its_pointer() {
        let (_dir, mut s) = project();
        let v = call(
            &mut s,
            "feature.edit",
            json!({"part": "p", "id": "b", "expressions": {"/params/x": "wall * 2"}}),
        )
        .unwrap();
        assert_eq!(v["feature"]["params"]["x"]["m"], 0.008);
        // Only an expression field it has, given alone.
        for (args, code) in [
            (
                json!({"part": "p", "id": "b", "expressions": {"/params/w": "1 mm"}}),
                Code::InvalidParam,
            ),
            (
                json!({"part": "p", "id": "b", "expressions": {"/params/x": "1 mm"},
                       "params": {"y": "2 mm"}}),
                Code::InvalidParam,
            ),
            (
                json!({"part": "p", "id": "b", "expressions": {"/params/x": "gap"}}),
                Code::UnknownParameter,
            ),
        ] {
            let e = call(&mut s, "feature.edit", args.clone()).unwrap_err();
            assert_eq!(e.code, code, "{args}");
        }
    }
}
