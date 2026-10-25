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
pub struct Walk {
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
        FEATURE_FORMAT, FEATURE_KIND, FORMAT, MANIFEST_FORMAT, PARAMETERS_KIND, PART_FORMAT,
        PART_KIND, PROJECT_KIND,
    };
    match kind {
        PROJECT_KIND => Some(MANIFEST_FORMAT),
        PART_KIND => Some(PART_FORMAT),
        PARAMETERS_KIND => Some(FORMAT),
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
/// - A part's `features` list merges as an ordered list (see [`order`]).
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
        crate::doc::files::PART_KIND => part(&mut walk, b.as_ref(), &o, &t),
        _ => walk.value("", b.as_ref(), Some(&o), Some(&t)).unwrap_or(o),
    };
    let format = match kind.as_str() {
        crate::doc::files::FEATURE_KIND => json!(crate::doc::files::feature_format(&value)),
        crate::doc::files::PROJECT_KIND => json!(crate::doc::files::manifest_format(&value)),
        crate::doc::files::PART_KIND => json!(crate::doc::files::part_format(&value)),
        _ => {
            let f = |v: &Value| v["format"].as_u64().unwrap_or(1);
            json!(f(ours).max(f(theirs)))
        }
    };
    if let Some(m) = value.as_object_mut() {
        m.insert("format".to_owned(), format);
    }
    // A whole file's sides are whole files, formats and all.
    if let Some(c) = walk.conflicts.get_mut("") {
        c.base = base.cloned();
        c.ours = Some(ours.clone());
        c.theirs = Some(theirs.clone());
    }
    Merged {
        value,
        conflicts: walk.conflicts,
    }
}

/// A feature file: one value when its type changes (or both sides added
/// it differently), a sketch per member, else per key.
fn feature(walk: &mut Walk, base: Option<&Value>, ours: &Value, theirs: &Value) -> Value {
    let ty = |v: &Value| v.get("type").cloned();
    let same_type = match base {
        Some(b) => ty(b) == ty(ours) && ty(b) == ty(theirs),
        None => false,
    };
    match base {
        Some(b) if same_type && ty(b) == Some(json!("sketch")) => sketch(walk, b, ours, theirs),
        Some(b) if same_type && ty(b) == Some(json!("instance")) => instance(walk, b, ours, theirs),
        Some(b) if same_type => walk
            .value("", Some(b), Some(ours), Some(theirs))
            .unwrap_or_else(|| ours.clone()),
        _ => walk.whole(base, ours, theirs),
    }
}

/// An instance: per key, but its revision is one value, and its pose is
/// solver state: the side that moved it, ours if both, never a conflict.
fn instance(walk: &mut Walk, base: &Value, ours: &Value, theirs: &Value) -> Value {
    let obj = |v: &Value| v.as_object().cloned().unwrap_or_default();
    let (b, o, t) = (obj(base), obj(ours), obj(theirs));
    let mut out = walk.object("", &b, &o, &t, &["pose", "rev"]);
    let (rb, ro, rt) = (b.get("rev"), o.get("rev"), t.get("rev"));
    let rev = if ro == rt || rt == rb {
        ro.cloned()
    } else if ro == rb {
        rt.cloned()
    } else {
        walk.conflict("/rev", Kind::Value, rb, ro, rt)
    };
    if let Some(rev) = rev {
        out.insert("rev".to_owned(), rev);
    }
    let pose = match o.get("pose") == b.get("pose") {
        true => t.get("pose"),
        false => o.get("pose"),
    };
    if let Some(pose) = pose.or(o.get("pose")) {
        out.insert("pose".to_owned(), pose.clone());
    }
    Value::Object(out)
}

/// The fields of a sketch entity or constraint that name another entity.
pub const NAMES: [&str; 6] = ["start", "end", "center", "a", "b", "about"];

/// The fields that are solver state: a point's position, a circle's
/// radius.
pub const POSITIONS: [&str; 2] = ["m", "r_m"];

/// The entities a sketch member names.
pub fn names_in(member: &Value) -> Vec<&str> {
    NAMES
        .iter()
        .filter_map(|f| member.get(*f).and_then(Value::as_str))
        .collect()
}

/// A sketch: its entities and constraints per ID and, within one, per
/// field (see [`members`]), the rest per key. Then an entity one side
/// removed that something left in the merge still names (a constraint the
/// other side added on it, a line it changed to end there) is a
/// `delete_edit` conflict: ours' value in place, so it comes back when
/// theirs removed it.
fn sketch(walk: &mut Walk, base: &Value, ours: &Value, theirs: &Value) -> Value {
    let obj = |v: &Value| v.as_object().cloned().unwrap_or_default();
    let (b, o, t) = (obj(base), obj(ours), obj(theirs));
    let mut out = walk.object("", &b, &o, &t, &["entities", "constraints"]);
    let groups = ["entities", "constraints"];
    for g in groups {
        let at = child("", g);
        let v = match (b.get(g), o.get(g), t.get(g)) {
            (Some(Value::Object(x)), Some(Value::Object(y)), Some(Value::Object(z))) => {
                Some(Value::Object(members(walk, &at, x, y, z)))
            }
            (x, y, z) => walk.value(&at, x, y, z),
        };
        if let Some(v) = v {
            out.insert(g.to_owned(), v);
        }
    }
    let (Some(Value::Object(bases)), Some(Value::Object(ourses)), Some(Value::Object(theirses))) =
        (b.get("entities"), o.get("entities"), t.get("entities"))
    else {
        return Value::Object(out);
    };
    // Entities brought back are named again in turn: until nothing changes.
    loop {
        let present: BTreeSet<String> = out
            .get("entities")
            .and_then(Value::as_object)
            .map(|e| e.keys().cloned().collect())
            .unwrap_or_default();
        let named: BTreeSet<String> = groups
            .iter()
            .filter_map(|g| out.get(*g).and_then(Value::as_object))
            .flat_map(|m| m.values())
            .flat_map(|m| names_in(m).into_iter().map(str::to_owned))
            .collect();
        let mut back = Vec::new();
        for id in named.iter().filter(|n| !present.contains(*n)) {
            let at = child("/entities", id);
            let Some(bv) = bases.get(id) else { continue };
            if walk.conflicts.contains_key(&at) {
                continue;
            }
            let (ov, tv) = (ourses.get(id), theirses.get(id));
            if let Some(v) = walk.conflict(&at, Kind::DeleteEdit, Some(bv), ov, tv) {
                back.push((id.clone(), v));
            }
        }
        if back.is_empty() {
            break;
        }
        let entities = out
            .entry("entities")
            .or_insert_with(|| json!({}))
            .as_object_mut()
            .expect("entities are an object");
        entities.extend(back);
    }
    Value::Object(out)
}

/// A sketch's entities or constraints, merged by ID. One added by one side
/// is taken, two different ones added under one ID are an `id_collision`
/// (ours' kept), one removed on one side and changed on the other (beyond
/// its positions) is a `delete_edit`; one changed on both merges per field, positions
/// ([`POSITIONS`]) never conflicting: each takes the side that moved it,
/// ours if both did.
fn members(
    walk: &mut Walk,
    at: &str,
    base: &Map<String, Value>,
    ours: &Map<String, Value>,
    theirs: &Map<String, Value>,
) -> Map<String, Value> {
    let ids: BTreeSet<&String> = base
        .keys()
        .chain(ours.keys())
        .chain(theirs.keys())
        .collect();
    let mut out = Map::new();
    for id in ids {
        let at = child(at, id);
        let (b, o, t) = (base.get(id), ours.get(id), theirs.get(id));
        let v = match (b, o, t) {
            _ if o == t => o.cloned(),
            (None, Some(_), Some(_)) => walk.conflict(&at, Kind::IdCollision, b, o, t),
            (None, x, y) => x.or(y).cloned(),
            (Some(_), None, x) | (Some(_), x, None) if fixed(x) == fixed(b) => None,
            (Some(_), None, _) | (Some(_), _, None) => {
                walk.conflict(&at, Kind::DeleteEdit, b, o, t)
            }
            (Some(b), Some(o), Some(t)) => member(walk, &at, b, o, t),
        };
        if let Some(v) = v {
            out.insert(id.clone(), v);
        }
    }
    out
}

/// One entity or constraint changed on both sides: per field, positions
/// from the side that moved them. Its `type` changing makes the rest of it
/// one value.
fn member(walk: &mut Walk, at: &str, base: &Value, ours: &Value, theirs: &Value) -> Option<Value> {
    let (Some(b), Some(o), Some(t)) = (base.as_object(), ours.as_object(), theirs.as_object())
    else {
        return walk.value(at, Some(base), Some(ours), Some(theirs));
    };
    let retyped = b.get("type") != o.get("type") || b.get("type") != t.get("type");
    let mut out = if retyped {
        // One value apart from positions: the side that changed it.
        let (fb, fo, ft) = (fixed(Some(base)), fixed(Some(ours)), fixed(Some(theirs)));
        let shape = if fo == fb {
            t
        } else if ft == fb || fo == ft {
            o
        } else {
            return walk.conflict(at, Kind::Value, Some(base), Some(ours), Some(theirs));
        };
        let mut out = shape.clone();
        out.retain(|k, _| !POSITIONS.contains(&k.as_str()));
        out
    } else {
        walk.object(at, b, o, t, &POSITIONS)
    };
    let shape = if retyped && fixed(Some(ours)) == fixed(Some(base)) {
        t
    } else {
        o
    };
    for k in POSITIONS {
        let v = if o.get(k) != b.get(k) {
            o.get(k)
        } else {
            t.get(k)
        };
        // A retyped member keeps only the positions its new type has.
        if let Some(v) = v.filter(|_| !retyped || shape.contains_key(k)) {
            out.insert(k.to_owned(), v.clone());
        }
    }
    Some(Value::Object(out))
}

/// A member without its positions: what it is apart from solver state.
fn fixed(v: Option<&Value>) -> Option<Value> {
    let mut v = v?.clone();
    if let Some(o) = v.as_object_mut() {
        o.retain(|k, _| !POSITIONS.contains(&k.as_str()));
    }
    Some(v)
}

/// A list of distinct IDs, if `v` is one.
fn ids(v: Option<&Value>) -> Option<Vec<String>> {
    let list: Vec<String> = v?
        .as_array()?
        .iter()
        .map(|x| x.as_str().map(str::to_owned))
        .collect::<Option<_>>()?;
    let distinct: BTreeSet<&String> = list.iter().collect();
    (distinct.len() == list.len()).then_some(list)
}

/// A part file: its feature list by [`order`], the rest per key.
fn part(walk: &mut Walk, base: Option<&Value>, ours: &Value, theirs: &Value) -> Value {
    let empty = Map::new();
    let b = base.and_then(Value::as_object).unwrap_or(&empty);
    let (o, t) = (
        ours.as_object().expect("an object"),
        theirs.as_object().expect("an object"),
    );
    let mut out = walk.object("", b, o, t, &["features"]);
    let lists = (
        ids(b.get("features")).or(base.is_none().then(Vec::new)),
        ids(o.get("features")),
        ids(t.get("features")),
    );
    let features = match lists {
        (Some(bl), Some(ol), Some(tl)) => Some(json!(order(walk, &bl, &ol, &tl))),
        // A list that isn't one is one value.
        _ => walk.value(
            "/features",
            b.get("features"),
            o.get("features"),
            t.get("features"),
        ),
    };
    if let Some(f) = features {
        out.insert("features".to_owned(), f);
    }
    Value::Object(out)
}

/// Where `id` sits in `list`: `{"after": <the feature before it>}`,
/// `{"first": true}`, or `None` when the list doesn't have it.
pub fn place(list: &[String], id: &str) -> Option<Value> {
    let i = list.iter().position(|f| f == id)?;
    Some(match i {
        0 => json!({"first": true}),
        i => json!({"after": list[i - 1]}),
    })
}

/// Which side a feature's place in the merged list comes from.
#[derive(Clone, Copy, PartialEq, Eq)]
enum From {
    Ours,
    Theirs,
}

/// Inserts `id` into `out` after every feature before it in `side` that
/// `out` has (first if none), and past any of `skip` that follow the last
/// of those: whatever it could read on its side still comes before it.
fn insert(out: &mut Vec<String>, side: &[String], id: &str, skip: &BTreeSet<String>) {
    let i = side.iter().position(|f| f == id).expect("listed");
    let mut at = side[..i]
        .iter()
        .filter_map(|p| out.iter().position(|f| f == p))
        .max()
        .map_or(0, |p| p + 1);
    while at < out.len() && skip.contains(&out[at]) {
        at += 1;
    }
    out.insert(at, id.to_owned());
}

/// A part's feature list, merged by identity. The result keeps base's
/// features that neither side removed, in base's order; a feature one side
/// moved takes that side's place; each side's additions go after the
/// feature they follow on that side (after all that preceded them there,
/// so nothing they could read comes after them), ours first where both
/// add at one spot. A feature moved differently by both sides, moved on one and
/// removed on the other, moved where the other side's moves leave it out
/// of its side's order, or added by both at different places is an
/// `order` conflict at `/features/<id>`, its sides' places as
/// [`place`] gives them.
pub fn order(walk: &mut Walk, base: &[String], ours: &[String], theirs: &[String]) -> Vec<String> {
    let has = |l: &[String], id: &str| l.iter().any(|f| f == id);
    let (mo, mt) = (
        crate::diff::moved_features(base, ours),
        crate::diff::moved_features(base, theirs),
    );
    let mut out: Vec<String> = base
        .iter()
        .filter(|f| has(ours, f) && has(theirs, f) && !mo.contains(*f) && !mt.contains(*f))
        .cloned()
        .collect();
    let all: BTreeSet<&String> = base.iter().chain(ours).chain(theirs).collect();
    let mut from: BTreeMap<&String, From> = BTreeMap::new();
    for id in all {
        if out.contains(id) {
            continue;
        }
        let (b, o, t) = (has(base, id), has(ours, id), has(theirs, id));
        let (pb, po, pt) = (place(base, id), place(ours, id), place(theirs, id));
        let conflict = |walk: &mut Walk| {
            walk.conflict(
                &child("/features", id),
                Kind::Order,
                pb.as_ref(),
                po.as_ref(),
                pt.as_ref(),
            );
        };
        let side = match (b, o, t) {
            (true, true, true) => match (mo.contains(id), mt.contains(id)) {
                (true, true) if po != pt => {
                    conflict(walk);
                    Some(From::Ours)
                }
                (_, true) if !mo.contains(id) => Some(From::Theirs),
                _ => Some(From::Ours),
            },
            // Moved on one side, removed on the other.
            (true, true, false) if mo.contains(id) => {
                conflict(walk);
                Some(From::Ours)
            }
            (true, false, true) if mt.contains(id) => {
                conflict(walk);
                None
            }
            (true, _, _) => None,
            (false, true, true) => {
                if po != pt {
                    conflict(walk);
                }
                Some(From::Ours)
            }
            (false, true, false) => Some(From::Ours),
            (false, false, true) => Some(From::Theirs),
            (false, false, false) => None,
        };
        if let Some(s) = side {
            from.insert(id, s);
        }
    }
    let none = BTreeSet::new();
    let mut ours_placed = BTreeSet::new();
    for id in ours {
        if from.get(id) == Some(&From::Ours) {
            insert(&mut out, ours, id, &none);
            ours_placed.insert(id.clone());
        }
    }
    for id in theirs {
        if from.get(id) == Some(&From::Theirs) {
            insert(&mut out, theirs, id, &ours_placed);
        }
    }
    // Each side's moves keep that side's order against what it lists: two
    // moves can pull against each other, and placing each alone would
    // undo one of them silently.
    let at = |l: &[String], id: &str| l.iter().position(|f| f == id);
    for (side, moved) in [(ours, &mo), (theirs, &mt)] {
        for id in moved {
            let key = child("/features", id);
            let (Some(si), Some(oi)) = (at(side, id), at(&out, id)) else {
                continue;
            };
            if walk.conflicts.contains_key(&key) {
                continue;
            }
            let broken = side
                .iter()
                .enumerate()
                .any(|(j, f)| f != id && at(&out, f).is_some_and(|k| (j < si) != (k < oi)));
            if broken {
                walk.conflict(
                    &key,
                    Kind::Order,
                    place(base, id).as_ref(),
                    place(ours, id).as_ref(),
                    place(theirs, id).as_ref(),
                );
            }
        }
    }
    out
}

/// How a file's merge came out.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Outcome {
    /// Merged, nothing left to resolve.
    Clean,
    /// Merged, with this many conflicts in the file's record.
    Conflicts(usize),
    /// A version isn't a ryucad file this build can merge: merged as
    /// lines, with this many conflicts marked as Git marks them.
    Text(usize),
}

/// A file's JSON if ryucad can merge it: UTF-8 JSON, an object with a
/// `kind`, no conflict record, and no format newer than this build reads.
/// It needn't be canonical: an agent that writes a file itself (on one
/// line, say) merges as the CLI's writes do, since the merge compares
/// values and writes the result canonical.
fn readable(bytes: &[u8]) -> Option<Value> {
    let text = std::str::from_utf8(bytes).ok()?;
    let v = crate::doc::canonical::parse(text).ok()?;
    if v.get(CONFLICTS).is_some() {
        return None;
    }
    let kind = v.get("kind")?.as_str()?;
    let format = v.get("format")?.as_u64()?;
    if newest(kind).is_some_and(|n| format > n) {
        return None;
    }
    Some(v)
}

/// Merges three versions of a file as Git hands them to a merge driver;
/// an empty base means both sides added the file. Files ryucad can read
/// merge by value (see [`merge`]) and come out canonical; if any version
/// can't be read, or they aren't of one kind, they merge as lines
/// instead, as `git merge-file` does.
pub fn bytes(base: &[u8], ours: &[u8], theirs: &[u8]) -> (Vec<u8>, Outcome) {
    let b = if base.is_empty() {
        Some(None)
    } else {
        readable(base).map(Some)
    };
    if let (Some(b), Some(o), Some(t)) = (b, readable(ours), readable(theirs)) {
        let kind = |v: &Value| v["kind"].clone();
        if kind(&o) == kind(&t) && b.as_ref().is_none_or(|b| kind(b) == kind(&o)) {
            let m = merge(b.as_ref(), &o, &t);
            let outcome = match m.conflicts.len() {
                0 => Outcome::Clean,
                n => Outcome::Conflicts(n),
            };
            return (
                crate::doc::canonical::to_string(&m.to_json()).into_bytes(),
                outcome,
            );
        }
    }
    let (out, n) = text(base, ours, theirs);
    (out, Outcome::Text(n))
}

/// Lines with their endings.
fn lines(b: &[u8]) -> Vec<&[u8]> {
    b.split_inclusive(|c| *c == b'\n').collect()
}

/// The most cells the line matcher fills; past it the changed middle of
/// two files is one change.
const MATCH_CELLS: usize = 1 << 22;

/// For each line of `a`, the line of `b` it is matched with in a longest
/// common subsequence: common ends first, then the middle by dynamic
/// programming when it is small enough.
fn matches(a: &[&[u8]], b: &[&[u8]]) -> Vec<Option<usize>> {
    let mut out = vec![None; a.len()];
    let pre = a.iter().zip(b).take_while(|(x, y)| x == y).count();
    for (i, m) in out.iter_mut().enumerate().take(pre) {
        *m = Some(i);
    }
    let suf = a[pre..]
        .iter()
        .rev()
        .zip(b[pre..].iter().rev())
        .take_while(|(x, y)| x == y)
        .count();
    for i in 0..suf {
        out[a.len() - 1 - i] = Some(b.len() - 1 - i);
    }
    let (x, y) = (&a[pre..a.len() - suf], &b[pre..b.len() - suf]);
    let (n, m) = (x.len(), y.len());
    if n == 0 || m == 0 || (n + 1) * (m + 1) > MATCH_CELLS {
        return out;
    }
    let mut l = vec![0u32; (n + 1) * (m + 1)];
    let at = |i: usize, j: usize| i * (m + 1) + j;
    for i in (0..n).rev() {
        for j in (0..m).rev() {
            l[at(i, j)] = if x[i] == y[j] {
                l[at(i + 1, j + 1)] + 1
            } else {
                l[at(i + 1, j)].max(l[at(i, j + 1)])
            };
        }
    }
    let (mut i, mut j) = (0, 0);
    while i < n && j < m {
        if x[i] == y[j] {
            out[pre + i] = Some(pre + j);
            i += 1;
            j += 1;
        } else if l[at(i + 1, j)] >= l[at(i, j + 1)] {
            i += 1;
        } else {
            j += 1;
        }
    }
    out
}

/// A three-way merge of lines, as `git merge-file` makes one: between the
/// lines all three versions keep, a stretch only one side changed takes
/// that side, one both changed alike takes it, and any other is marked
/// `<<<<<<< ours`, `=======`, `>>>>>>> theirs`. The merged bytes and the
/// number of conflicts.
pub fn text(base: &[u8], ours: &[u8], theirs: &[u8]) -> (Vec<u8>, usize) {
    let (b, o, t) = (lines(base), lines(ours), lines(theirs));
    let (mo, mt) = (matches(&b, &o), matches(&b, &t));
    let mut out: Vec<u8> = Vec::new();
    let mut conflicts = 0;
    let (mut i, mut j, mut k) = (0, 0, 0);
    let emit = |out: &mut Vec<u8>, chunk: &[&[u8]]| {
        for l in chunk {
            out.extend_from_slice(l);
        }
    };
    let marker = |out: &mut Vec<u8>, m: &str| {
        if out.last().is_some_and(|c| *c != b'\n') {
            out.push(b'\n');
        }
        out.extend_from_slice(m.as_bytes());
    };
    loop {
        let next = (i..b.len()).find_map(|x| match (mo[x], mt[x]) {
            (Some(y), Some(z)) if y >= j && z >= k => Some((x, y, z)),
            _ => None,
        });
        let (bx, oy, tz) = next.unwrap_or((b.len(), o.len(), t.len()));
        let (cb, co, ct) = (&b[i..bx], &o[j..oy], &t[k..tz]);
        if co == cb {
            emit(&mut out, ct);
        } else if ct == cb || co == ct {
            emit(&mut out, co);
        } else {
            conflicts += 1;
            marker(&mut out, "<<<<<<< ours\n");
            emit(&mut out, co);
            marker(&mut out, "=======\n");
            emit(&mut out, ct);
            marker(&mut out, ">>>>>>> theirs\n");
        }
        match next {
            None => break,
            Some((x, y, z)) => {
                out.extend_from_slice(b[x]);
                (i, j, k) = (x + 1, y + 1, z + 1);
            }
        }
    }
    (out, conflicts)
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
        assert_eq!(m.conflicts[""].theirs.as_ref(), Some(&theirs));
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

    /// Merges three feature lists.
    fn lists(base: &[&str], ours: &[&str], theirs: &[&str]) -> (Vec<String>, Vec<String>) {
        let s = |l: &[&str]| -> Vec<String> { l.iter().map(|x| x.to_string()).collect() };
        let mut walk = Walk::default();
        let out = order(&mut walk, &s(base), &s(ours), &s(theirs));
        (out, walk.conflicts.keys().cloned().collect())
    }

    #[test]
    fn lists_merge_by_identity() {
        let ok = |b: &[&str], o: &[&str], t: &[&str], want: &[&str]| {
            let (out, conflicts) = lists(b, o, t);
            assert_eq!(out, want, "{b:?} {o:?} {t:?}");
            assert!(conflicts.is_empty(), "{conflicts:?}");
        };
        // Additions on both sides, after what they follow there.
        ok(
            &["a", "b"],
            &["a", "x", "b"],
            &["a", "b", "y"],
            &["a", "x", "b", "y"],
        );
        // At one spot: ours first, then theirs.
        ok(
            &["a", "b"],
            &["a", "x", "b"],
            &["a", "y", "b"],
            &["a", "x", "y", "b"],
        );
        ok(&["a"], &["x", "a"], &["y", "z", "a"], &["x", "y", "z", "a"]);
        // A chain of additions stays together.
        ok(
            &["a"],
            &["a", "x1", "x2"],
            &["a", "y"],
            &["a", "x1", "x2", "y"],
        );
        // Removals on either side.
        ok(
            &["a", "b", "c"],
            &["a", "c"],
            &["a", "b", "c", "d"],
            &["a", "c", "d"],
        );
        // A move on one side.
        ok(
            &["a", "b", "c"],
            &["c", "a", "b"],
            &["a", "b", "c", "d"],
            &["c", "a", "b", "d"],
        );
        ok(
            &["a", "b", "c"],
            &["a", "b", "c"],
            &["b", "a", "c"],
            &["b", "a", "c"],
        );
        // The same move, or the same addition at one place, on both.
        ok(
            &["a", "b", "c"],
            &["b", "c", "a"],
            &["b", "c", "a"],
            &["b", "c", "a"],
        );
        ok(&["a"], &["a", "x"], &["a", "x"], &["a", "x"]);
        // An addition after a feature the other side removed.
        ok(&["a", "b"], &["a", "b", "x"], &["a"], &["a", "x"]);
        // After a feature the other side moved earlier: still after all
        // that came before it.
        ok(
            &["a", "b", "c"],
            &["c", "a", "b"],
            &["a", "b", "c", "d"],
            &["c", "a", "b", "d"],
        );
        // Both new: a part both sides created.
        ok(&[], &["a"], &["b"], &["a", "b"]);
    }

    #[test]
    fn lists_conflict_on_order() {
        // Moved differently on both sides: ours' place, a conflict.
        let (out, c) = lists(&["a", "b", "c"], &["b", "a", "c"], &["b", "c", "a"]);
        assert_eq!(
            (out, c),
            (
                vec!["b".into(), "a".into(), "c".into()],
                vec!["/features/a".into()]
            )
        );
        // Moved on one side, removed on the other.
        let (out, c) = lists(&["a", "b", "c"], &["c", "a", "b"], &["a", "b"]);
        assert_eq!(
            (out, c),
            (
                vec!["c".into(), "a".into(), "b".into()],
                vec!["/features/c".into()]
            )
        );
        let (out, c) = lists(&["a", "b", "c"], &["a", "b"], &["c", "a", "b"]);
        assert_eq!(
            (out, c),
            (vec!["a".into(), "b".into()], vec!["/features/c".into()])
        );
        // Moves that pull against each other: ours puts b before a, theirs
        // c before b, and base keeps a before c. Placing each alone would
        // undo ours' move silently.
        let (out, c) = lists(&["a", "b", "c"], &["b", "a", "c"], &["a", "c", "b"]);
        assert_eq!(
            (out, c),
            (
                vec!["a".into(), "c".into(), "b".into()],
                vec!["/features/a".into()]
            )
        );
        // Added by both at different places.
        let (out, c) = lists(&["a", "b"], &["x", "a", "b"], &["a", "b", "x"]);
        assert_eq!(
            (out, c),
            (
                vec!["x".into(), "a".into(), "b".into()],
                vec!["/features/x".into()]
            )
        );
        let mut walk = Walk::default();
        let s = |l: &[&str]| -> Vec<String> { l.iter().map(|x| x.to_string()).collect() };
        order(
            &mut walk,
            &s(&["a", "b", "c"]),
            &s(&["b", "a", "c"]),
            &s(&["b", "c", "a"]),
        );
        assert_eq!(
            walk.conflicts["/features/a"].to_json(),
            json!({"kind": "order", "base": {"first": true}, "ours": {"after": "b"},
                   "theirs": {"after": "c"}})
        );
    }

    #[test]
    fn a_part_file_merges_its_list() {
        let part = |f: &[&str]| json!({"features": f, "format": 1, "kind": "ryucad.part"});
        let m = merge(Some(&part(&["a"])), &part(&["a", "x"]), &part(&["y", "a"]));
        assert!(m.is_clean());
        assert_eq!(m.value, part(&["y", "a", "x"]));
        // A list that isn't one is one value.
        let bad = json!({"features": ["a", "a"], "format": 1, "kind": "ryucad.part"});
        let m = merge(Some(&part(&["a"])), &bad, &part(&["b"]));
        assert_eq!(m.conflicts.keys().collect::<Vec<_>>(), ["/features"]);
    }

    #[test]
    fn a_part_kind_merges_by_key_and_sets_the_format() {
        let part = |f: &[&str]| json!({"features": f, "format": 1, "kind": "ryucad.part"});
        let assembly = |f: &[&str]| json!({"features": f, "format": 2, "kind": "ryucad.part", "part_kind": "assembly"});
        // One side makes it an assembly, the other adds a feature.
        let m = merge(Some(&part(&["a"])), &assembly(&["a"]), &part(&["a", "b"]));
        assert!(m.is_clean());
        assert_eq!(m.value, assembly(&["a", "b"]));
        // Theirs took it back to a part: format 1 again.
        let m = merge(
            Some(&assembly(&["a"])),
            &assembly(&["a", "b"]),
            &part(&["a"]),
        );
        assert!(m.is_clean());
        assert_eq!(m.value, part(&["a", "b"]));
    }

    /// A sketch on xy with the entities and constraints given.
    fn sketch_json(entities: Value, constraints: Value) -> Value {
        json!({"format": 2, "kind": "ryucad.feature", "type": "sketch", "plane": "xy",
               "entities": entities, "constraints": constraints})
    }

    /// Three points and a line from the first to the second, with a length.
    fn base_sketch() -> Value {
        sketch_json(
            json!({
                "p1": {"type": "point", "m": [0, 0]},
                "p2": {"type": "point", "m": [0.01, 0]},
                "p3": {"type": "point", "m": [0, 0.01]},
                "l1": {"type": "line", "start": "p1", "end": "p2"},
            }),
            json!({"d1": {"type": "length", "a": "l1", "value": "10 mm"}}),
        )
    }

    #[test]
    fn sketches_merge_per_entity_and_constraint() {
        let base = base_sketch();
        let mut ours = base.clone();
        ours["entities"]["p4"] = json!({"type": "point", "m": [0.02, 0]});
        ours["entities"]["l2"] = json!({"type": "line", "start": "p2", "end": "p4"});
        ours["constraints"]["c1"] = json!({"type": "horizontal", "a": "l2"});
        ours["entities"]["l1"]["construction"] = json!(true);
        let mut theirs = base.clone();
        theirs["entities"]["p5"] = json!({"type": "point", "m": [0, 0.02]});
        theirs["constraints"]["d1"]["value"] = json!("wall * 2");
        let m = merge(Some(&base), &ours, &theirs);
        assert!(m.is_clean(), "{:?}", m.conflicts);
        let e = &m.value["entities"];
        assert_eq!(e["p4"], ours["entities"]["p4"]);
        assert_eq!(e["p5"], theirs["entities"]["p5"]);
        assert_eq!(e["l1"]["construction"], true);
        assert_eq!(m.value["constraints"]["d1"]["value"], "wall * 2");
        assert_eq!(m.value["constraints"]["c1"]["a"], "l2");
        // A dimension changed differently on both sides conflicts.
        ours["constraints"]["d1"]["value"] = json!("12 mm");
        let m = merge(Some(&base), &ours, &theirs);
        assert_eq!(
            m.conflicts.keys().collect::<Vec<_>>(),
            ["/constraints/d1/value"]
        );
        assert_eq!(m.value["constraints"]["d1"]["value"], "12 mm");
        // So does the plane.
        let mut a = base.clone();
        a["plane"] = json!("yz");
        let mut b = base.clone();
        b["plane"] = json!("zx");
        assert_eq!(
            merge(Some(&base), &a, &b)
                .conflicts
                .keys()
                .collect::<Vec<_>>(),
            ["/plane"]
        );
    }

    #[test]
    fn positions_never_conflict() {
        let base = base_sketch();
        let mut ours = base.clone();
        let mut theirs = base.clone();
        ours["entities"]["p1"]["m"] = json!([0.001, 0]);
        theirs["entities"]["p1"]["m"] = json!([0.002, 0]);
        theirs["entities"]["p2"]["m"] = json!([0.011, 0]);
        let m = merge(Some(&base), &ours, &theirs);
        assert!(m.is_clean(), "{:?}", m.conflicts);
        // Ours where both moved it; theirs where only it did.
        assert_eq!(m.value["entities"]["p1"]["m"], json!([0.001, 0]));
        assert_eq!(m.value["entities"]["p2"]["m"], json!([0.011, 0]));
        // A point both moved and one side made construction: still per field.
        theirs["entities"]["p1"]["construction"] = json!(true);
        let m = merge(Some(&base), &ours, &theirs);
        assert!(m.is_clean());
        assert_eq!(
            m.value["entities"]["p1"],
            json!({"type": "point", "m": [0.001, 0], "construction": true})
        );
    }

    #[test]
    fn a_member_whose_type_changes_is_one_value() {
        let base = base_sketch();
        let mut ours = base.clone();
        ours["constraints"]["d1"]["type"] = json!("distance");
        let mut theirs = base.clone();
        theirs["constraints"]["d1"]["value"] = json!("20 mm");
        let m = merge(Some(&base), &ours, &theirs);
        assert_eq!(m.conflicts.keys().collect::<Vec<_>>(), ["/constraints/d1"]);
        assert_eq!(m.value["constraints"]["d1"], ours["constraints"]["d1"]);
        // A side that only moved it doesn't stand in the way.
        let mut ours = base.clone();
        ours["entities"]["p3"] = json!({"type": "circle", "m": [0, 0.01], "r_m": 0.002});
        let mut theirs = base.clone();
        theirs["entities"]["p3"]["m"] = json!([0, 0.02]);
        let m = merge(Some(&base), &ours, &theirs);
        assert!(m.is_clean(), "{:?}", m.conflicts);
        assert_eq!(
            m.value["entities"]["p3"],
            json!({"type": "circle", "m": [0, 0.02], "r_m": 0.002})
        );
    }

    #[test]
    fn a_member_only_moved_follows_its_removal() {
        let base = base_sketch();
        let mut ours = base.clone();
        ours["entities"].as_object_mut().unwrap().remove("p3");
        let mut theirs = base.clone();
        theirs["entities"]["p3"]["m"] = json!([0, 0.02]);
        theirs["constraints"]["d1"]["value"] = json!("12 mm");
        for (o, t) in [(&ours, &theirs), (&theirs, &ours)] {
            let m = merge(Some(&base), o, t);
            assert!(m.is_clean(), "{:?}", m.conflicts);
            assert!(m.value["entities"].get("p3").is_none());
            assert_eq!(m.value["constraints"]["d1"]["value"], "12 mm");
        }
    }

    #[test]
    fn removed_on_one_side_and_edited_or_used_on_the_other() {
        let base = base_sketch();
        // Ours removes the line and its length; theirs makes it construction.
        let mut ours = base.clone();
        ours["entities"].as_object_mut().unwrap().remove("l1");
        ours["constraints"].as_object_mut().unwrap().remove("d1");
        let mut theirs = base.clone();
        theirs["entities"]["l1"]["construction"] = json!(true);
        let m = merge(Some(&base), &ours, &theirs);
        assert_eq!(m.conflicts.keys().collect::<Vec<_>>(), ["/entities/l1"]);
        let c = &m.conflicts["/entities/l1"];
        assert_eq!((c.kind, c.ours.as_ref()), (Kind::DeleteEdit, None));
        assert!(m.value["entities"].get("l1").is_none());
        // Theirs removes a point that ours puts a new constraint on: it
        // comes back, as ours has it, with a conflict.
        let mut ours = base.clone();
        ours["constraints"]["c2"] = json!({"type": "fix", "a": "p3"});
        let mut theirs = base.clone();
        theirs["entities"].as_object_mut().unwrap().remove("p3");
        let m = merge(Some(&base), &ours, &theirs);
        assert_eq!(m.conflicts.keys().collect::<Vec<_>>(), ["/entities/p3"]);
        assert_eq!(m.conflicts["/entities/p3"].kind, Kind::DeleteEdit);
        assert_eq!(m.value["entities"]["p3"], base["entities"]["p3"]);
        // The other way round, ours' removal stays, and so does the use.
        let m = merge(Some(&base), &theirs, &ours);
        assert_eq!(m.conflicts.keys().collect::<Vec<_>>(), ["/entities/p3"]);
        assert!(m.value["entities"].get("p3").is_none());
        // Theirs removes a line with its points; ours constrains the line:
        // the line comes back, and the points it needs with it.
        let mut theirs = base.clone();
        for e in ["l1", "p1", "p2"] {
            theirs["entities"].as_object_mut().unwrap().remove(e);
        }
        theirs["constraints"].as_object_mut().unwrap().remove("d1");
        let mut ours = base.clone();
        ours["constraints"]["c3"] = json!({"type": "horizontal", "a": "l1"});
        let m = merge(Some(&base), &ours, &theirs);
        assert_eq!(
            m.conflicts.keys().collect::<Vec<_>>(),
            ["/entities/l1", "/entities/p1", "/entities/p2"]
        );
        for e in ["l1", "p1", "p2"] {
            assert_eq!(m.value["entities"][e], base["entities"][e], "{e}");
        }
        // Removed on both sides is simply gone.
        let m = merge(Some(&base), &theirs, &theirs);
        assert!(m.is_clean());
    }

    #[test]
    fn two_entities_added_under_one_id_collide() {
        let base = base_sketch();
        let mut ours = base.clone();
        ours["entities"]["p7"] = json!({"type": "point", "m": [0, 0]});
        let mut theirs = base.clone();
        theirs["entities"]["p7"] = json!({"type": "point", "m": [0.01, 0]});
        let m = merge(Some(&base), &ours, &theirs);
        assert_eq!(
            m.to_json()[CONFLICTS],
            json!({"/entities/p7": {"kind": "id_collision",
                                    "ours": {"m": [0, 0], "type": "point"},
                                    "theirs": {"m": [0.01, 0], "type": "point"}}})
        );
        assert_eq!(m.value["entities"]["p7"], ours["entities"]["p7"]);
        // The same addition on both sides is no conflict.
        assert!(merge(Some(&base), &ours, &ours).is_clean());
    }

    fn canonical(v: &Value) -> Vec<u8> {
        crate::doc::canonical::to_string(v).into_bytes()
    }

    #[test]
    fn files_merge_by_bytes() {
        let base = feature_json("10 mm", [1, 0, 0]);
        let ours = feature_json("12 mm", [1, 0, 0]);
        let theirs = feature_json("10 mm", [0, 1, 0]);
        let (out, o) = bytes(&canonical(&base), &canonical(&ours), &canonical(&theirs));
        assert_eq!(o, Outcome::Clean);
        assert_eq!(out, canonical(&feature_json("12 mm", [0, 1, 0])));
        let clash = feature_json("15 mm", [1, 0, 0]);
        let (out, o) = bytes(&canonical(&base), &canonical(&ours), &canonical(&clash));
        assert_eq!(o, Outcome::Conflicts(1));
        let v: Value = serde_json::from_slice(&out).unwrap();
        assert_eq!(out, canonical(&v));
        assert_eq!(record(&v).len(), 1);
        // An empty base: both added the file.
        let (_, o) = bytes(b"", &canonical(&ours), &canonical(&clash));
        assert_eq!(o, Outcome::Conflicts(1));
        assert_eq!(
            bytes(b"", &canonical(&ours), &canonical(&ours)).1,
            Outcome::Clean
        );
    }

    #[test]
    fn json_written_by_hand_merges_by_value() {
        // Issue #52: one side's file written compactly by an agent, the
        // other's by the CLI, changing different values.
        let base = canonical(&feature_json("10 mm", [1, 0, 0]));
        let ours = serde_json::to_vec(&feature_json("12 mm", [1, 0, 0])).unwrap();
        let theirs = canonical(&feature_json("10 mm", [0, 1, 0]));
        let (out, o) = bytes(&base, &ours, &theirs);
        assert_eq!(o, Outcome::Clean);
        assert_eq!(out, canonical(&feature_json("12 mm", [0, 1, 0])));
        // Spaced oddly, with CRLF, on every side.
        let odd = |v: &[u8]| {
            String::from_utf8(v.to_vec())
                .unwrap()
                .replace(": ", ":  ")
                .replace('\n', "\r\n")
                .into_bytes()
        };
        let (out2, o) = bytes(&odd(&base), &odd(&ours), &odd(&theirs));
        assert_eq!((out2, o), (out, Outcome::Clean));
    }

    #[test]
    fn what_ryucad_cant_read_merges_as_lines() {
        let base = canonical(&feature_json("10 mm", [1, 0, 0]));
        let ours = canonical(&feature_json("12 mm", [1, 0, 0]));
        // A newer format, another kind, a conflict record, not JSON: each
        // falls back to lines.
        let mut newer = feature_json("12 mm", [1, 0, 0]);
        newer["format"] = json!(9);
        let mut other = feature_json("12 mm", [1, 0, 0]);
        other["kind"] = json!("ryucad.part");
        let mut conflicted = feature_json("12 mm", [1, 0, 0]);
        conflicted[CONFLICTS] = json!({});
        for theirs in [
            canonical(&newer),
            canonical(&other),
            canonical(&conflicted),
            b"<<<<<<< x".to_vec(),
        ] {
            assert!(matches!(bytes(&base, &ours, &theirs).1, Outcome::Text(_)));
        }
    }

    #[test]
    fn lines_merge_as_git_merges_them() {
        let (out, n) = text(b"a\nb\nc\n", b"A\nb\nc\n", b"a\nb\nC\n");
        assert_eq!((out.as_slice(), n), (b"A\nb\nC\n".as_slice(), 0));
        let (out, n) = text(b"a\nb\nc\n", b"a\nB\nc\n", b"a\nX\nc\n");
        assert_eq!(
            (String::from_utf8(out).unwrap().as_str(), n),
            ("a\n<<<<<<< ours\nB\n=======\nX\n>>>>>>> theirs\nc\n", 1)
        );
        // The same change on both sides, and additions at the ends.
        let (out, n) = text(b"a\n", b"0\na\nb", b"0\na\n");
        assert_eq!((out.as_slice(), n), (b"0\na\nb".as_slice(), 0));
        // A last line without its newline still gets markers on their own
        // lines.
        let (out, n) = text(b"a", b"b", b"c");
        assert_eq!(
            (String::from_utf8(out).unwrap().as_str(), n),
            ("<<<<<<< ours\nb\n=======\nc\n>>>>>>> theirs\n", 1)
        );
        let (out, n) = text(b"", b"x\n", b"x\n");
        assert_eq!((out.as_slice(), n), (b"x\n".as_slice(), 0));
        // Doctor knows the markers.
        assert!(
            String::from_utf8(text(b"a", b"b", b"c").0)
                .unwrap()
                .contains(">>>>>>> ")
        );
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
        // A manifest is format 2 while it holds a project ID.
        let unit = |u: &str| json!({"default_unit": u, "format": 1, "kind": "ryucad.project"});
        let mut with_id = unit("mm");
        with_id["project"] = json!("ab".repeat(16));
        with_id["format"] = json!(2);
        let m = merge(Some(&unit("mm")), &unit("in"), &with_id);
        assert_eq!(
            m.value,
            json!({"default_unit": "in", "format": 2, "kind": "ryucad.project",
                   "project": "ab".repeat(16)})
        );
        assert!(readable(&crate::doc::canonical::to_string(&with_id).into_bytes()).is_some());
    }

    #[test]
    fn an_instances_pose_never_conflicts_and_its_revision_is_one_value() {
        let inst = |rev: Value, x: f64, fixed: bool| {
            json!({"format": 4, "kind": "ryucad.feature", "type": "instance", "part": "bolt",
                   "rev": rev, "fixed": fixed,
                   "pose": {"origin_m": [x, 0, 0], "z": [0, 0, 1], "x": [1, 0, 0]}})
        };
        let base = inst(json!("working"), 0.0, false);
        // Both moved it: ours. Theirs fixed it: that merges.
        let m = merge(
            Some(&base),
            &inst(json!("working"), 0.01, false),
            &inst(json!("working"), 0.02, true),
        );
        assert!(m.is_clean());
        assert_eq!(m.value, inst(json!("working"), 0.01, true));
        // Only theirs moved it: theirs.
        let m = merge(Some(&base), &base, &inst(json!("working"), 0.02, false));
        assert_eq!(m.value["pose"]["origin_m"], json!([0.02, 0, 0]));
        // Pinned to two different labels: one conflict at /rev, ours in place.
        let m = merge(
            Some(&base),
            &inst(json!({"label": "A"}), 0.0, false),
            &inst(json!({"label": "B"}), 0.0, false),
        );
        assert_eq!(m.conflicts.keys().collect::<Vec<_>>(), ["/rev"]);
        assert_eq!(m.value["rev"], json!({"label": "A"}));
        // A label on one side, working left on the other: the label.
        let m = merge(Some(&base), &base, &inst(json!({"label": "B"}), 0.0, false));
        assert!(m.is_clean());
        assert_eq!(m.value["rev"], json!({"label": "B"}));
        assert_eq!(m.value["format"], 4);
    }
}
