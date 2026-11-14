//! What reading any product's files shares: a file's header (`kind` and
//! `format`), its fields taken one by one so leftovers are errors,
//! `invalid_document` at a JSON pointer, and the conflicts a merge left
//! in a file. `ryucad-doc` re-exports these at the paths ryucad has
//! always used, naming ryucad.

use crate::error::{Code, Error, Result};
use serde_json::Value;
use std::collections::BTreeMap;

/// The key under which a file a merge couldn't settle lists its conflicts,
/// by JSON pointer. No file kind uses it otherwise.
pub const CONFLICTS: &str = "conflicts";

/// `invalid_document` at the JSON pointer `at`.
pub fn invalid(at: &str, msg: &str) -> Error {
    Error::new(Code::InvalidDocument, msg).with("pointer", at)
}

/// Checks `kind` and `format` (at most `newest`, which `product` reads),
/// and returns the remaining fields.
pub fn header_in<'a>(v: &'a Value, kind: &str, newest: u64, product: &str) -> Result<Obj<'a>> {
    let mut o = Obj::new(v, "")?;
    let k = o.take("kind")?;
    if k.as_str() != Some(kind) {
        return Err(invalid(
            "/kind",
            &format!("expected kind {kind:?}, found {k}"),
        ));
    }
    let f = o.take("format")?;
    let format = f
        .as_u64()
        .filter(|f| *f >= 1)
        .ok_or_else(|| invalid("/format", "format must be a positive integer"))?;
    if format > newest {
        return Err(Error::new(
            Code::FormatTooNew,
            format!(
                "this file has format {format}; this {product} reads up to {newest}. Upgrade {product}"
            ),
        )
        .with("format", format)
        .with("supported", newest));
    }
    Ok(o)
}

/// The fields of a JSON object, taken one by one so leftovers are errors.
pub struct Obj<'a> {
    map: BTreeMap<&'a str, &'a Value>,
    at: String,
}

impl<'a> Obj<'a> {
    pub fn new(v: &'a Value, at: &str) -> Result<Obj<'a>> {
        let map = v
            .as_object()
            .ok_or_else(|| invalid(at, "expected a JSON object"))?;
        Ok(Obj {
            map: map.iter().map(|(k, v)| (k.as_str(), v)).collect(),
            at: at.to_owned(),
        })
    }

    /// Whether a field is there, not yet taken.
    pub fn has(&self, key: &str) -> bool {
        self.map.contains_key(key)
    }

    /// A field that may be left out.
    pub fn take_opt(&mut self, key: &str) -> Option<Value> {
        self.map.remove(key).cloned()
    }

    pub fn take(&mut self, key: &str) -> Result<Value> {
        self.map.remove(key).cloned().ok_or_else(|| {
            invalid(
                &format!("{}/{key}", self.at),
                &format!("missing field {key:?}"),
            )
        })
    }

    pub fn finish(self) -> Result<()> {
        match self.map.keys().next() {
            None => Ok(()),
            Some(k) => Err(invalid(
                &format!("{}/{k}", self.at),
                &format!("unexpected field {k:?}"),
            )),
        }
    }
}

/// `merge_conflict` for the file at `at` if its JSON lists merge
/// conflicts, naming their pointers and `product`'s `merge status`, and a
/// `<product>.feature`'s `type` where no conflict is about it, so a
/// failed feature can still say what it is.
pub fn conflicted_in(v: &Value, at: &str, product: &str) -> Option<Error> {
    let listed = v.as_object()?.get(CONFLICTS)?;
    let pointers: Vec<&str> = listed
        .as_object()
        .map(|o| o.keys().map(String::as_str).collect())
        .unwrap_or_default();
    let at_list = if pointers.is_empty() {
        String::new()
    } else {
        format!(" at {}", pointers.join(", "))
    };
    let feature = format!("{product}.feature");
    let ty = v
        .get("type")
        .and_then(Value::as_str)
        .filter(|_| v.get("kind").and_then(Value::as_str) == Some(feature.as_str()))
        .filter(|_| !pointers.iter().any(|p| p.is_empty() || *p == "/type"));
    let e = Error::new(
        Code::MergeConflict,
        format!("{at} has unresolved merge conflicts{at_list}; see `{product} merge status`"),
    )
    .with("path", at)
    .with("pointers", &pointers);
    Some(match ty {
        Some(t) => e.with("type", t),
        None => e,
    })
}

/// `v` without its merge conflicts: the values the merge left in place.
pub fn without_conflicts(v: &Value) -> Value {
    let mut v = v.clone();
    if let Some(o) = v.as_object_mut() {
        o.remove(CONFLICTS);
    }
    v
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn a_header_names_the_product_that_reads_it() {
        let v = json!({"format": 3, "kind": "toy.thing", "x": 1});
        let e = header_in(&v, "toy.thing", 2, "ryueda").err().unwrap();
        assert_eq!(e.code, Code::FormatTooNew);
        assert_eq!(
            e.message,
            "this file has format 3; this ryueda reads up to 2. Upgrade ryueda"
        );
        let ok = json!({"format": 1, "kind": "toy.thing", "x": 1});
        let mut o = header_in(&ok, "toy.thing", 2, "ryueda").unwrap();
        assert_eq!(o.take("x").unwrap(), json!(1));
        o.finish().unwrap();
        let e = header_in(&json!({"format": 1, "kind": "other"}), "toy.thing", 2, "ryueda")
            .err()
            .unwrap();
        assert_eq!(e.code, Code::InvalidDocument);
        assert_eq!(e.details["pointer"], json!("/kind"));
    }

    #[test]
    fn conflicts_name_the_product_and_a_features_type() {
        let v = json!({"conflicts": {"/x": {"kind": "value"}}, "kind": "ryucad.feature", "type": "fillet"});
        let e = conflicted_in(&v, "parts/p/features/f.json", "ryucad").unwrap();
        assert_eq!(e.code, Code::MergeConflict);
        assert_eq!(
            e.message,
            "parts/p/features/f.json has unresolved merge conflicts at /x; see `ryucad merge status`"
        );
        assert_eq!(e.details["type"], json!("fillet"));
        let e = conflicted_in(&v, "a.json", "ryueda").unwrap();
        assert!(e.message.ends_with("see `ryueda merge status`"));
        assert!(e.details.get("type").is_none());
        assert!(conflicted_in(&json!({"x": 1}), "a.json", "ryueda").is_none());
        assert_eq!(
            without_conflicts(&v),
            json!({"kind": "ryucad.feature", "type": "fillet"})
        );
    }
}
