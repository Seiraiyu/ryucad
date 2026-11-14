//! `part.ensure` (design 603, plan 31.C): a part made exactly as a
//! script declares it, writing only what differs, with what became of
//! the part, each feature, its parameters and its properties.

use crate::doc::id::Id;
use crate::error::{Code, Error, Result};
use crate::live::Live;
use crate::proto::Request;
use crate::session::Session;
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

/// `part.ensure`.
pub(crate) fn run(
    _session: &mut Session,
    _live: Option<&mut Live>,
    _me: Option<&str>,
    _ask: Ask,
) -> Result<Value> {
    // Tasks 5-9 fill this in.
    Err(Error::new(Code::Usage, "part.ensure isn't answered yet"))
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

    fn pointer(e: &Error) -> &str {
        e.details["pointer"].as_str().unwrap()
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
