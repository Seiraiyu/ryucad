//! Three-way merges of ryucad's files, value by value, so that Git's line
//! merge never splices two people's JSON together. Each file is walked in
//! base, ours and theirs at once, by key: a key one side changed takes that
//! side, a key both changed alike takes it, and a key changed differently
//! is a conflict. "Changed" includes added and removed. Which keys there
//! are depends on the file's kind; see [`merge`].
//!
//! What can't be merged stays in the file as a conflict record: ours' value
//! in place at each conflict, and every conflict listed under a top-level
//! `conflicts` key by JSON pointer. The file is still canonical JSON, so
//! everything that reads ryucad's files can report on it; loading refuses
//! it (`merge_conflict`) until it is resolved.
//!
//! Nothing here knows of Git, other files or the kernel: the merge of three
//! given files is the same bytes on every platform.

use crate::doc::project::CONFLICTS;
use serde_json::{Map, Value, json};
use std::collections::{BTreeMap, BTreeSet};

/// What kind of conflict a file holds. The strings are stable.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum Kind {
    /// A value changed differently on both sides, or changed on one and
    /// removed on the other.
    Value,
    /// A sketch entity or constraint removed on one side and changed, or
    /// used by something new, on the other.
    DeleteEdit,
    /// A feature placed differently in a part's list by each side, or
    /// moved on one and removed on the other.
    Order,
    /// Two different things added under one ID: a feature file, a sketch
    /// entity or constraint.
    IdCollision,
}

impl Kind {
    pub const ALL: [Kind; 4] = [
        Kind::Value,
        Kind::DeleteEdit,
        Kind::Order,
        Kind::IdCollision,
    ];

    pub fn as_str(self) -> &'static str {
        match self {
            Kind::Value => "value",
            Kind::DeleteEdit => "delete_edit",
            Kind::Order => "order",
            Kind::IdCollision => "id_collision",
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
fn child(at: &str, key: &str) -> String {
    format!("{at}/{}", escape(key))
}

/// A reference to another feature's face, edge or vertex: one value,
/// never merged field by field.
fn is_reference(v: &Value) -> bool {
    v.as_object().is_some_and(|o| {
        o.len() == 2 && o.get("feature").is_some_and(Value::is_string) && o.contains_key("name")
    })
}

/// The walk over one file: the conflicts found so far.
#[derive(Default)]
struct Walk {
    conflicts: BTreeMap<String, Conflict>,
}

impl Walk {
    /// A conflict at `at`; the result there is ours'.
    fn conflict(
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
    /// agree, else per key where all present sides are objects, else a
    /// conflict.
    fn value(
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
        let split = |v: Option<&Value>| v.is_none_or(|v| v.is_object() && !is_reference(v));
        match (ours, theirs) {
            (Some(o), Some(t))
                if o.is_object()
                    && t.is_object()
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
    fn object(
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
    fn whole(&mut self, base: Option<&Value>, ours: &Value, theirs: &Value) -> Value {
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

/// The newest format of each file kind this build reads; kinds it doesn't
/// model (ryupdm's) are merged by the generic rule at any format.
pub fn newest(kind: &str) -> Option<u64> {
    use crate::doc::files::{
        FEATURE_FORMAT, FEATURE_KIND, FORMAT, PARAMETERS_KIND, PART_KIND, PROJECT_KIND,
    };
    match kind {
        PROJECT_KIND | PART_KIND | PARAMETERS_KIND => Some(FORMAT),
        FEATURE_KIND => Some(FEATURE_FORMAT),
        _ => None,
    }
}

/// `v` without its `format`, the one key the merge doesn't walk.
fn unformatted(v: &Value) -> Value {
    let mut v = v.clone();
    if let Some(o) = v.as_object_mut() {
        o.remove("format");
    }
    v
}

/// Merges three versions of one file of a kind ryucad writes: `base` is
/// `None` when both sides added it. All three are JSON objects of one kind.
///
/// - A feature whose `type` changes on either side is one value; a sketch
///   merges per entity and constraint (see [`sketch`]); any other feature
///   per JSON pointer to each leaf, a reference being one value.
/// - Release and obsolete records are one value each: two different
///   records at one path are one conflict.
/// - Every other kind, and every key not named above, merges per key, an
///   array being one value.
///
/// The result is written at the lowest format that holds it, so merging
/// never churns formats.
pub fn merge(base: Option<&Value>, ours: &Value, theirs: &Value) -> Merged {
    let kind = ours["kind"].as_str().unwrap_or_default().to_owned();
    let (b, o, t) = (
        base.map(unformatted),
        unformatted(ours),
        unformatted(theirs),
    );
    let mut walk = Walk::default();
    let mut value = match kind.as_str() {
        "ryupdm.release" | "ryupdm.obsolete" => walk.whole(b.as_ref(), &o, &t),
        crate::doc::files::FEATURE_KIND => feature(&mut walk, b.as_ref(), &o, &t),
        _ => walk.value("", b.as_ref(), Some(&o), Some(&t)).unwrap_or(o),
    };
    let format = match kind.as_str() {
        crate::doc::files::FEATURE_KIND => json!(crate::doc::files::feature_format(&value)),
        _ => {
            let f = |v: &Value| v["format"].as_u64().unwrap_or(1);
            json!(f(ours).max(f(theirs)))
        }
    };
    if let Some(m) = value.as_object_mut() {
        m.insert("format".to_owned(), format);
    }
    Merged {
        value,
        conflicts: walk.conflicts,
    }
}

/// A feature file: one value when its type changes (or both sides added
/// it differently), else per key.
fn feature(walk: &mut Walk, base: Option<&Value>, ours: &Value, theirs: &Value) -> Value {
    let ty = |v: &Value| v.get("type").cloned();
    let same_type = match base {
        Some(b) => ty(b) == ty(ours) && ty(b) == ty(theirs),
        None => false,
    };
    if !same_type {
        return walk.whole(base, ours, theirs);
    }
    walk.value("", base, Some(ours), Some(theirs))
        .unwrap_or_else(|| ours.clone())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn feature_json(radius: &str, x: [i64; 3]) -> Value {
        json!({
            "format": 1, "kind": "ryucad.feature", "type": "cylinder",
            "params": {"height": "40 mm", "radius": radius},
            "placement": {"origin": ["0 mm", "0 mm", "0 mm"], "x": x, "z": [0, 0, 1]},
        })
    }

    #[test]
    fn values_one_side_changed_merge() {
        let base = feature_json("10 mm", [1, 0, 0]);
        let ours = feature_json("12 mm", [1, 0, 0]);
        let theirs = feature_json("10 mm", [0, 1, 0]);
        let m = merge(Some(&base), &ours, &theirs);
        assert!(m.is_clean(), "{:?}", m.conflicts);
        assert_eq!(m.value, feature_json("12 mm", [0, 1, 0]));
        // No change on one side gives the other.
        assert_eq!(merge(Some(&base), &base, &theirs).value, theirs);
        assert_eq!(merge(Some(&base), &ours, &base).value, ours);
        // The same change on both sides is that change.
        assert_eq!(merge(Some(&base), &ours, &ours).value, ours);
    }

    #[test]
    fn values_changed_differently_conflict_with_ours_in_place() {
        let base = feature_json("10 mm", [1, 0, 0]);
        let ours = feature_json("12 mm", [1, 0, 0]);
        let theirs = feature_json("15 mm", [1, 0, 0]);
        let m = merge(Some(&base), &ours, &theirs);
        assert_eq!(m.value, ours);
        assert_eq!(
            m.to_json()[CONFLICTS],
            json!({"/params/radius": {"base": "10 mm", "kind": "value",
                                      "ours": "12 mm", "theirs": "15 mm"}})
        );
        // An array is one value.
        let theirs = feature_json("10 mm", [0, 1, 0]);
        let ours = feature_json("10 mm", [0, 0, 1]);
        let m = merge(Some(&base), &ours, &theirs);
        assert_eq!(m.conflicts.keys().collect::<Vec<_>>(), ["/placement/x"]);
        // Removed on one side, changed on the other: the removing side has
        // no value.
        let mut gone = base.clone();
        gone["params"].as_object_mut().unwrap().remove("radius");
        let changed = feature_json("11 mm", [1, 0, 0]);
        let m = merge(Some(&base), &gone, &changed);
        let c = &m.conflicts["/params/radius"];
        assert_eq!((c.kind, c.ours.as_ref()), (Kind::Value, None));
        assert!(m.value["params"].get("radius").is_none());
        assert_eq!(record(&m.to_json()), m.conflicts);
    }

    #[test]
    fn references_are_one_value() {
        let at = |name: &str| {
            json!({
                "format": 2, "kind": "ryucad.feature", "type": "extrude", "sketch": "s1",
                "extent": {"type": "up_to", "target": {"feature": "box1", "name": name}},
            })
        };
        let a = "0".repeat(32);
        let (b, c) = ("1".repeat(32), "2".repeat(32));
        let m = merge(Some(&at(&a)), &at(&b), &at(&c));
        assert_eq!(m.conflicts.keys().collect::<Vec<_>>(), ["/extent/target"]);
    }

    #[test]
    fn a_type_change_makes_the_file_one_value() {
        let base = feature_json("10 mm", [1, 0, 0]);
        let mut sphere = json!({
            "format": 1, "kind": "ryucad.feature", "type": "sphere",
            "params": {"radius": "10 mm"},
            "placement": base["placement"].clone(),
        });
        let theirs = feature_json("15 mm", [1, 0, 0]);
        let m = merge(Some(&base), &sphere, &theirs);
        assert_eq!(m.conflicts.keys().collect::<Vec<_>>(), [""]);
        assert_eq!(m.value, sphere);
        // Changed on one side only, it is that side.
        assert!(merge(Some(&base), &sphere, &base).is_clean());
        // Two files added at one path: an ID collision, unless the same.
        sphere["params"]["radius"] = json!("11 mm");
        let m = merge(None, &sphere, &theirs);
        assert_eq!(m.conflicts[""].kind, Kind::IdCollision);
        assert_eq!(m.conflicts[""].base, None);
        assert!(merge(None, &theirs, &theirs).is_clean());
    }

    #[test]
    fn other_kinds_merge_per_key() {
        let params = |p: Value| json!({"format": 1, "kind": "ryucad.parameters", "parameters": p});
        let base = params(json!({"wall": "3 mm", "hole": "wall * 2"}));
        let ours = params(json!({"wall": "4 mm", "hole": "wall * 2", "gap": "1 mm"}));
        let theirs = params(json!({"wall": "3 mm", "slot": "2 mm"}));
        let m = merge(Some(&base), &ours, &theirs);
        assert!(m.is_clean());
        assert_eq!(
            m.value["parameters"],
            json!({"wall": "4 mm", "gap": "1 mm", "slot": "2 mm"})
        );
        // Both adding one name differently is a value conflict.
        let a = params(json!({"wall": "3 mm", "hole": "wall * 2", "gap": "1 mm"}));
        let b = params(json!({"wall": "3 mm", "hole": "wall * 2", "gap": "2 mm"}));
        let m = merge(Some(&base), &a, &b);
        let c = &m.conflicts["/parameters/gap"];
        assert_eq!((c.kind, c.base.as_ref()), (Kind::Value, None));
        let unit = |u: &str| json!({"default_unit": u, "format": 1, "kind": "ryucad.project"});
        let m = merge(Some(&unit("mm")), &unit("in"), &unit("cm"));
        assert_eq!(m.conflicts.keys().collect::<Vec<_>>(), ["/default_unit"]);
        // Keys with `/` and `~` are escaped in pointers.
        let odd = |v: &str| json!({"format": 1, "kind": "ryupdm.config", "a/b~c": v});
        let m = merge(Some(&odd("1")), &odd("2"), &odd("3"));
        assert_eq!(m.conflicts.keys().collect::<Vec<_>>(), ["/a~1b~0c"]);
        assert_eq!(unescape("a~1b~0c"), "a/b~c");
    }

    #[test]
    fn release_records_are_whole_files() {
        let rec = |hash: &str| json!({"format": 1, "kind": "ryupdm.release", "label": "A", "content": hash});
        let m = merge(None, &rec("aa"), &rec("bb"));
        assert_eq!(m.conflicts[""].kind, Kind::IdCollision);
        assert!(merge(None, &rec("aa"), &rec("aa")).is_clean());
    }

    #[test]
    fn the_format_is_the_lowest_that_holds_it() {
        // A literal on one side and an expression on the other: the merge
        // holds the expression, so format 2; undoing it goes back to 1.
        let base = feature_json("10 mm", [1, 0, 0]);
        let mut ours = feature_json("10 mm", [0, 1, 0]);
        let mut theirs = feature_json("wall * 2", [1, 0, 0]);
        theirs["format"] = json!(2);
        let m = merge(Some(&base), &ours, &theirs);
        assert_eq!(m.value["format"], 2);
        ours["params"]["radius"] = json!("wall * 2");
        ours["format"] = json!(2);
        let lit = feature_json("10 mm", [0, 1, 0]);
        let m = merge(Some(&ours), &lit, &ours);
        assert_eq!(m.value["format"], 1);
        assert!(m.is_clean());
    }
}
