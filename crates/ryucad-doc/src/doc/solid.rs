//! What a body-making feature does with the body it builds: found a solid
//! of its own (`new`), or join it to, cut it from or intersect it with
//! the solids it names. A solid is named by the feature that founded it.

use super::files::{Obj, invalid};
use super::id::Id;
use crate::error::Result;
use serde_json::{Value, json};

/// A body-making feature's operation.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Operation {
    New,
    Join,
    Cut,
    Intersect,
}

impl Operation {
    pub const ALL: [Operation; 4] = [
        Operation::New,
        Operation::Join,
        Operation::Cut,
        Operation::Intersect,
    ];

    pub fn as_str(self) -> &'static str {
        match self {
            Operation::New => "new",
            Operation::Join => "join",
            Operation::Cut => "cut",
            Operation::Intersect => "intersect",
        }
    }

    pub fn parse(s: &str) -> Option<Operation> {
        Operation::ALL.into_iter().find(|o| o.as_str() == s)
    }
}

/// A feature's operation and the solids it acts on, by their founders:
/// none for `new`, at least one, each once, for the others.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Combine {
    pub operation: Operation,
    pub targets: Vec<Id>,
}

impl Combine {
    /// A feature that founds its own solid.
    pub const NEW: Combine = Combine {
        operation: Operation::New,
        targets: Vec::new(),
    };

    pub fn is_new(&self) -> bool {
        self.operation == Operation::New
    }

    /// Writes `operation` and `targets` into a feature's fields; `new`
    /// writes nothing, so files from before solids don't change.
    pub fn write(&self, v: &mut Value) {
        if !self.is_new() {
            v["operation"] = json!(self.operation.as_str());
            v["targets"] = json!(self.targets);
        }
    }

    /// Reads the optional `operation` and `targets` fields.
    pub(crate) fn from_fields(o: &mut Obj) -> Result<Combine> {
        let operation = match o.take_opt("operation") {
            None => Operation::New,
            Some(v) => v.as_str().and_then(Operation::parse).ok_or_else(|| {
                invalid(
                    "/operation",
                    "operation is one of new, join, cut, intersect",
                )
            })?,
        };
        let targets = match o.take_opt("targets") {
            None => Vec::new(),
            Some(v) => targets_from_json(&v, "/targets")?,
        };
        match (operation, targets.is_empty()) {
            (Operation::New, false) => Err(invalid(
                "/targets",
                "a feature that founds a new solid has no targets",
            )),
            (Operation::New, true) => Ok(Combine::NEW),
            (_, true) => Err(invalid(
                "/targets",
                &format!(
                    "a {} names the solids it acts on: targets is required",
                    operation.as_str()
                ),
            )),
            (_, false) => Ok(Combine { operation, targets }),
        }
    }
}

/// Reads an array of solids by their founders' IDs: at least one, each
/// once. `at` is its JSON pointer.
pub(crate) fn targets_from_json(v: &Value, at: &str) -> Result<Vec<Id>> {
    let a = v
        .as_array()
        .filter(|a| !a.is_empty())
        .ok_or_else(|| invalid(at, "expected a non-empty array of solids' feature IDs"))?;
    let mut out: Vec<Id> = Vec::with_capacity(a.len());
    for (i, x) in a.iter().enumerate() {
        let here = format!("{at}/{i}");
        let s = x
            .as_str()
            .ok_or_else(|| invalid(&here, "expected a feature's ID"))?;
        let id = Id::parse(s, "feature").map_err(|e| e.with("pointer", &here))?;
        if out.contains(&id) {
            return Err(invalid(&here, &format!("{id} is named twice")));
        }
        out.push(id);
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::error::Code;

    fn read(v: Value) -> Result<Combine> {
        let mut o = Obj::new(&v, "")?;
        let c = Combine::from_fields(&mut o)?;
        o.finish()?;
        Ok(c)
    }

    #[test]
    fn operations_round_trip() {
        assert_eq!(read(json!({})).unwrap(), Combine::NEW);
        assert_eq!(read(json!({"operation": "new"})).unwrap(), Combine::NEW);
        let c = read(json!({"operation": "cut", "targets": ["b", "a"]})).unwrap();
        assert_eq!(c.operation, Operation::Cut);
        assert_eq!(
            c.targets.iter().map(Id::as_str).collect::<Vec<_>>(),
            ["b", "a"]
        );
        let mut v = json!({});
        c.write(&mut v);
        assert_eq!(v, json!({"operation": "cut", "targets": ["b", "a"]}));
        assert_eq!(read(v).unwrap(), c);
        let mut v = json!({});
        Combine::NEW.write(&mut v);
        assert_eq!(v, json!({}));
    }

    #[test]
    fn operations_are_checked() {
        for (v, pointer) in [
            (json!({"operation": "glue", "targets": ["a"]}), "/operation"),
            (json!({"operation": 1}), "/operation"),
            (json!({"operation": "join"}), "/targets"),
            (json!({"operation": "cut", "targets": []}), "/targets"),
            (json!({"operation": "cut", "targets": "a"}), "/targets"),
            (
                json!({"operation": "cut", "targets": ["a", 2]}),
                "/targets/1",
            ),
            (
                json!({"operation": "cut", "targets": ["a", "a"]}),
                "/targets/1",
            ),
            (json!({"targets": ["a"]}), "/targets"),
            (json!({"operation": "new", "targets": ["a"]}), "/targets"),
        ] {
            let e = read(v.clone()).unwrap_err();
            assert_eq!(e.code, Code::InvalidDocument, "{v}");
            assert_eq!(e.details["pointer"], pointer, "{v}");
        }
        let e = read(json!({"operation": "cut", "targets": ["A"]})).unwrap_err();
        assert_eq!(
            (e.code, e.details["pointer"].as_str()),
            (Code::InvalidId, Some("/targets/0"))
        );
    }
}
