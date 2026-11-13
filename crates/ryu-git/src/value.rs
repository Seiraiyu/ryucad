//! Three-way merges of a product's JSON files, value by value, so that
//! Git's line merge never splices two people's JSON together: the walk
//! ryucad and ryueda both merge with, list order by identity, the
//! conflict record a file keeps what a merge couldn't settle in, and the
//! line merge for what isn't JSON. Each file is walked in base, ours and
//! theirs at once, by key: a key one side changed takes that side, a key
//! both changed alike takes it, and a key changed differently is a
//! conflict. "Changed" includes added and removed.
//!
//! What a product decides is its [`Rules`] (which pointers, and which
//! values, merge as one value) and its own rules for members and lists,
//! which call [`Walk::value`], [`Walk::object`], [`Walk::conflict`] and
//! [`order`]. Nothing here knows of Git, other files or a product: the
//! merge of three given values is the same bytes on every platform.

use serde_json::{Map, Value, json};
use std::collections::{BTreeMap, BTreeSet};

/// The member under which a file lists the conflicts a merge left in it,
/// by JSON pointer. No file kind of any product has it otherwise.
pub const CONFLICTS: &str = "conflicts";

/// What kind of conflict a file holds. The strings are stable.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum Kind {
    /// A value changed differently on both sides, or changed on one and
    /// removed on the other.
    Value,
    /// Something removed on one side and changed, or used by something
    /// new, on the other.
    DeleteEdit,
    /// Something placed differently in a list by each side, or moved on
    /// one and removed on the other.
    Order,
    /// Two different things added under one ID: a file, a member.
    IdCollision,
    /// A ryueda layout both sides changed: no driver can merge it without
    /// the merged connectivity, so `ryueda merge status` does (design
    /// 518). Always a whole file's entry.
    LayoutPending,
}

impl Kind {
    pub const ALL: [Kind; 5] = [
        Kind::Value,
        Kind::DeleteEdit,
        Kind::Order,
        Kind::IdCollision,
        Kind::LayoutPending,
    ];

    pub fn as_str(self) -> &'static str {
        match self {
            Kind::Value => "value",
            Kind::DeleteEdit => "delete_edit",
            Kind::Order => "order",
            Kind::IdCollision => "id_collision",
            Kind::LayoutPending => "layout_pending",
        }
    }

    pub fn parse(s: &str) -> Option<Kind> {
        Kind::ALL.into_iter().find(|k| k.as_str() == s)
    }
}

/// One conflict: its kind and each side's value, `None` where the side
/// doesn't have one (it removed it, or never had it).
#[derive(Clone, Debug, PartialEq)]
pub struct Conflict {
    pub kind: Kind,
    pub base: Option<Value>,
    pub ours: Option<Value>,
    pub theirs: Option<Value>,
}

impl Conflict {
    /// As the record lists it: a side without a value is left out.
    pub fn to_json(&self) -> Value {
        let mut v = json!({"kind": self.kind.as_str()});
        for (k, x) in [
            ("base", &self.base),
            ("ours", &self.ours),
            ("theirs", &self.theirs),
        ] {
            if let Some(x) = x {
                v[k] = x.clone();
            }
        }
        v
    }

    /// Reads a record's entry; `None` if it isn't one.
    pub fn from_json(v: &Value) -> Option<Conflict> {
        let o = v.as_object()?;
        Some(Conflict {
            kind: Kind::parse(o.get("kind")?.as_str()?)?,
            base: o.get("base").cloned(),
            ours: o.get("ours").cloned(),
            theirs: o.get("theirs").cloned(),
        })
    }
}

/// A merged file: its value, ours' at each conflict, and the conflicts by
/// JSON pointer.
#[derive(Clone, Debug, PartialEq)]
pub struct Merged {
    pub value: Value,
    pub conflicts: BTreeMap<String, Conflict>,
}

impl Merged {
    pub fn is_clean(&self) -> bool {
        self.conflicts.is_empty()
    }

    /// The file as written: the value, with the conflict record when there
    /// is one.
    pub fn to_json(&self) -> Value {
        let mut v = self.value.clone();
        if !self.conflicts.is_empty()
            && let Some(o) = v.as_object_mut()
        {
            let record: Map<String, Value> = self
                .conflicts
                .iter()
                .map(|(p, c)| (p.clone(), c.to_json()))
                .collect();
            o.insert(CONFLICTS.to_owned(), Value::Object(record));
        }
        v
    }
}

/// A file's conflict record, by pointer; empty when it has none. Entries
/// that don't read as conflicts are left out.
pub fn record(v: &Value) -> BTreeMap<String, Conflict> {
    v.get(CONFLICTS)
        .and_then(Value::as_object)
        .map(|o| {
            o.iter()
                .filter_map(|(p, c)| Some((p.clone(), Conflict::from_json(c)?)))
                .collect()
        })
        .unwrap_or_default()
}

/// `key` as one segment of a JSON pointer.
pub fn escape(key: &str) -> String {
    key.replace('~', "~0").replace('/', "~1")
}

/// A JSON pointer's segment back as a key.
pub fn unescape(segment: &str) -> String {
    segment.replace("~1", "/").replace("~0", "~")
}

/// `at` with `key` appended.
pub fn child(at: &str, key: &str) -> String {
    format!("{at}/{}", escape(key))
}

/// What a product merges as one value, never key by key: an object at a
/// pointer `atom` names (a manifest's dependency, a pin's node), and any
/// object `whole` recognises wherever it sits (ryucad's references).
#[derive(Clone, Copy)]
pub struct Rules {
    pub atom: fn(&str) -> bool,
    pub whole: fn(&Value) -> bool,
}

fn no_pointer(_: &str) -> bool {
    false
}

fn no_value(_: &Value) -> bool {
    false
}

impl Default for Rules {
    /// Every object merges key by key.
    fn default() -> Rules {
        Rules {
            atom: no_pointer,
            whole: no_value,
        }
    }
}

/// The walk over one file: its rules and the conflicts found so far.
#[derive(Default)]
pub struct Walk {
    /// The conflicts found so far, by pointer.
    pub conflicts: BTreeMap<String, Conflict>,
    rules: Rules,
}

impl Walk {
    pub fn new(rules: Rules) -> Walk {
        Walk {
            conflicts: BTreeMap::new(),
            rules,
        }
    }

    /// A conflict at `at`; the result there is ours'.
    pub fn conflict(
        &mut self,
        at: &str,
        kind: Kind,
        base: Option<&Value>,
        ours: Option<&Value>,
        theirs: Option<&Value>,
    ) -> Option<Value> {
        self.conflicts.insert(
            at.to_owned(),
            Conflict {
                kind,
                base: base.cloned(),
                ours: ours.cloned(),
                theirs: theirs.cloned(),
            },
        );
        ours.cloned()
    }

    /// The value at `at`: one side's if only it changed, either if they
    /// agree, else per key where all present sides are objects the rules
    /// don't keep whole, else a conflict.
    pub fn value(
        &mut self,
        at: &str,
        base: Option<&Value>,
        ours: Option<&Value>,
        theirs: Option<&Value>,
    ) -> Option<Value> {
        if ours == theirs || theirs == base {
            return ours.cloned();
        }
        if ours == base {
            return theirs.cloned();
        }
        let whole = self.rules.whole;
        let split = |v: Option<&Value>| v.is_none_or(|v| v.is_object() && !whole(v));
        let atom = (self.rules.atom)(at);
        match (ours, theirs) {
            (Some(o), Some(t))
                if o.is_object()
                    && t.is_object()
                    && !atom
                    && split(Some(o))
                    && split(base)
                    && split(Some(t)) =>
            {
                let empty = Map::new();
                let b = base.and_then(Value::as_object).unwrap_or(&empty);
                Some(Value::Object(self.object(
                    at,
                    b,
                    o.as_object().expect("an object"),
                    t.as_object().expect("an object"),
                    &[],
                )))
            }
            _ => self.conflict(at, Kind::Value, base, ours, theirs),
        }
    }

    /// Three objects merged key by key; `skip` keys are left out.
    pub fn object(
        &mut self,
        at: &str,
        base: &Map<String, Value>,
        ours: &Map<String, Value>,
        theirs: &Map<String, Value>,
        skip: &[&str],
    ) -> Map<String, Value> {
        let keys: BTreeSet<&String> = base
            .keys()
            .chain(ours.keys())
            .chain(theirs.keys())
            .collect();
        let mut out = Map::new();
        for k in keys {
            if skip.contains(&k.as_str()) {
                continue;
            }
            if let Some(v) = self.value(&child(at, k), base.get(k), ours.get(k), theirs.get(k)) {
                out.insert(k.clone(), v);
            }
        }
        out
    }

    /// The whole file as one value: one side's, or a conflict at the root.
    /// Two different files added under one name are an ID collision.
    pub fn whole(&mut self, base: Option<&Value>, ours: &Value, theirs: &Value) -> Value {
        if ours == theirs || Some(theirs) == base {
            return ours.clone();
        }
        if Some(ours) == base {
            return theirs.clone();
        }
        let kind = if base.is_none() {
            Kind::IdCollision
        } else {
            Kind::Value
        };
        self.conflict("", kind, base, Some(ours), Some(theirs));
        ours.clone()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn atom(at: &str) -> bool {
        at == "/node"
    }

    fn whole(v: &Value) -> bool {
        v.get("ref").is_some()
    }

    #[test]
    fn one_side_both_alike_and_differently() {
        let base = json!({"a": 1, "b": 1, "c": 1});
        let ours = json!({"a": 2, "b": 3, "c": 1});
        let theirs = json!({"a": 1, "b": 4, "c": 5});
        let mut w = Walk::default();
        let v = w
            .value("", Some(&base), Some(&ours), Some(&theirs))
            .unwrap();
        // a: ours alone; c: theirs alone; b: both differently, ours kept.
        assert_eq!(v, json!({"a": 2, "b": 3, "c": 5}));
        assert_eq!(w.conflicts.keys().collect::<Vec<_>>(), ["/b"]);
        assert_eq!(
            w.conflicts["/b"],
            Conflict {
                kind: Kind::Value,
                base: Some(json!(1)),
                ours: Some(json!(3)),
                theirs: Some(json!(4)),
            }
        );
        // Removed on one side, unchanged on the other: removed.
        let mut w = Walk::default();
        let v = w
            .value(
                "",
                Some(&json!({"x": 1, "y": 1})),
                Some(&json!({"y": 1})),
                Some(&json!({"x": 1, "y": 2})),
            )
            .unwrap();
        assert_eq!(v, json!({"y": 2}));
        assert!(w.conflicts.is_empty());
    }

    #[test]
    fn atoms_and_wholes_are_one_value() {
        let base =
            json!({"node": {"a": 1, "b": 1}, "r": {"ref": 1, "n": 1}, "o": {"p": 1, "q": 1}});
        let ours =
            json!({"node": {"a": 2, "b": 1}, "r": {"ref": 2, "n": 1}, "o": {"p": 2, "q": 1}});
        let theirs =
            json!({"node": {"a": 1, "b": 2}, "r": {"ref": 1, "n": 2}, "o": {"p": 1, "q": 2}});
        let mut w = Walk::new(Rules { atom, whole });
        let v = w
            .value("", Some(&base), Some(&ours), Some(&theirs))
            .unwrap();
        // An ordinary object merges per key; the atom and the whole don't.
        assert_eq!(v["o"], json!({"p": 2, "q": 2}));
        assert_eq!(v["node"], json!({"a": 2, "b": 1}));
        assert_eq!(w.conflicts.keys().collect::<Vec<_>>(), ["/node", "/r"]);
        // Without the rules the node would mix both sides' halves.
        let mut w = Walk::default();
        let v = w
            .value("", Some(&base), Some(&ours), Some(&theirs))
            .unwrap();
        assert_eq!(v["node"], json!({"a": 2, "b": 2}));
    }

    #[test]
    fn a_whole_file_added_twice_is_an_id_collision() {
        let mut w = Walk::default();
        let v = w.whole(None, &json!({"a": 1}), &json!({"a": 2}));
        assert_eq!(v, json!({"a": 1}));
        assert_eq!(w.conflicts[""].kind, Kind::IdCollision);
        let mut w = Walk::default();
        assert_eq!(w.whole(None, &json!(1), &json!(1)), json!(1));
        assert!(w.conflicts.is_empty());
    }

    #[test]
    fn records_round_trip() {
        let mut conflicts = BTreeMap::new();
        conflicts.insert(
            "/a~1b".to_owned(),
            Conflict {
                kind: Kind::LayoutPending,
                base: None,
                ours: Some(json!(1)),
                theirs: Some(json!(2)),
            },
        );
        let m = Merged {
            value: json!({"kind": "k", "a/b": 1}),
            conflicts,
        };
        let v = m.to_json();
        assert_eq!(
            v[CONFLICTS]["/a~1b"],
            json!({"kind": "layout_pending", "ours": 1, "theirs": 2})
        );
        assert_eq!(record(&v), m.conflicts);
        assert_eq!(escape("a/b~c"), "a~1b~0c");
        assert_eq!(unescape("a~1b~0c"), "a/b~c");
        assert_eq!(child("/x", "a/b"), "/x/a~1b");
        for k in Kind::ALL {
            assert_eq!(Kind::parse(k.as_str()), Some(k));
        }
        assert!(
            Merged {
                value: json!({}),
                conflicts: BTreeMap::new()
            }
            .is_clean()
        );
    }
}
