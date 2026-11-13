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

/// The IDs `a` and `b` both list that `b` moved: those not in the longest
/// run both keep in order, the fewest moves that explain `b`.
pub fn moved(a: &[String], b: &[String]) -> BTreeSet<String> {
    let x: Vec<&String> = a.iter().filter(|f| b.contains(f)).collect();
    let y: Vec<&String> = b.iter().filter(|f| a.contains(f)).collect();
    // The longest common subsequence, by dynamic programming.
    let (n, m) = (x.len(), y.len());
    let mut l = vec![vec![0usize; m + 1]; n + 1];
    for i in (0..n).rev() {
        for j in (0..m).rev() {
            l[i][j] = if x[i] == y[j] {
                l[i + 1][j + 1] + 1
            } else {
                l[i + 1][j].max(l[i][j + 1])
            };
        }
    }
    let mut kept = BTreeSet::new();
    let (mut i, mut j) = (0, 0);
    while i < n && j < m {
        if x[i] == y[j] {
            kept.insert(x[i]);
            i += 1;
            j += 1;
        } else if l[i + 1][j] >= l[i][j + 1] {
            i += 1;
        } else {
            j += 1;
        }
    }
    y.into_iter()
        .filter(|f| !kept.contains(*f))
        .cloned()
        .collect()
}

/// Where `id` sits in `list`: `{"after": <the ID before it>}`,
/// `{"first": true}`, or `None` when the list doesn't have it.
pub fn place(list: &[String], id: &str) -> Option<Value> {
    let i = list.iter().position(|f| f == id)?;
    Some(match i {
        0 => json!({"first": true}),
        i => json!({"after": list[i - 1]}),
    })
}

/// Which side an ID's place in the merged list comes from.
#[derive(Clone, Copy, PartialEq, Eq)]
enum From {
    Ours,
    Theirs,
}

/// Inserts `id` into `out` after every ID before it in `side` that `out`
/// has (first if none), and past any of `skip` that follow the last of
/// those: whatever it could read on its side still comes before it.
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

/// A list of distinct IDs at pointer `at` (a part's `/features`, a
/// library component's `/footprints`), merged by identity. The result keeps base's
/// IDs that neither side removed, in base's order; an ID one side
/// moved takes that side's place; each side's additions go after the
/// ID they follow on that side (after all that preceded them there,
/// so nothing they could read comes after them), ours first where both
/// add at one spot. An ID moved differently by both sides, moved on one and
/// removed on the other, moved where the other side's moves leave it out
/// of its side's order, or added by both at different places is an
/// `order` conflict at `<at>/<id>`, its sides' places as
/// [`place`] gives them.
pub fn order(
    walk: &mut Walk,
    at: &str,
    base: &[String],
    ours: &[String],
    theirs: &[String],
) -> Vec<String> {
    let has = |l: &[String], id: &str| l.iter().any(|f| f == id);
    let (mo, mt) = (moved(base, ours), moved(base, theirs));
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
                &child(at, id),
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
    let pos = |l: &[String], id: &str| l.iter().position(|f| f == id);
    for (side, moved) in [(ours, &mo), (theirs, &mt)] {
        for id in moved {
            let key = child(at, id);
            let (Some(si), Some(oi)) = (pos(side, id), pos(&out, id)) else {
                continue;
            };
            if walk.conflicts.contains_key(&key) {
                continue;
            }
            let broken = side
                .iter()
                .enumerate()
                .any(|(j, f)| f != id && pos(&out, f).is_some_and(|k| (j < si) != (k < oi)));
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

/// Puts `x` at `pointer` in `v`, making the objects on the way; `None`
/// removes what is there.
pub fn set_at(v: &mut Value, pointer: &str, x: Option<Value>) {
    if pointer.is_empty() {
        if let Some(x) = x {
            *v = x;
        }
        return;
    }
    let mut seg: Vec<String> = pointer.split('/').skip(1).map(unescape).collect();
    let last = seg.pop().expect("a pointer below the root has a segment");
    let mut at = v;
    for k in seg {
        if !at.get(&k).is_some_and(Value::is_object) {
            at[&k] = json!({});
        }
        at = at.get_mut(&k).expect("just made");
    }
    let Some(o) = at.as_object_mut() else { return };
    match x {
        Some(x) => {
            o.insert(last, x);
        }
        None => {
            o.remove(&last);
        }
    }
}

/// Puts `id` in `list` where `place` says ([`place`]'s form: first, or
/// after an ID), or takes it out for `None`. After an ID the list doesn't
/// have, or anywhere else, it goes last.
pub fn put(list: &mut Vec<String>, id: &str, place: Option<&Value>) {
    list.retain(|f| f != id);
    let Some(p) = place else { return };
    let at = match (p.get("first"), p.get("after").and_then(Value::as_str)) {
        (Some(Value::Bool(true)), _) => 0,
        (_, Some(after)) => list
            .iter()
            .position(|f| f == after)
            .map_or(list.len(), |i| i + 1),
        _ => list.len(),
    };
    list.insert(at, id.to_owned());
}

/// How a file's merge came out.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Outcome {
    /// Merged, nothing left to resolve.
    Clean,
    /// Merged, with this many conflicts in the file's record.
    Conflicts(usize),
    /// A version isn't a file this build can merge: merged as
    /// lines, with this many conflicts marked as Git marks them.
    Text(usize),
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

    fn s(l: &[&str]) -> Vec<String> {
        l.iter().map(|x| x.to_string()).collect()
    }

    #[test]
    fn lists_merge_by_identity_at_their_pointer() {
        let mut w = Walk::default();
        let out = order(
            &mut w,
            "/footprints",
            &s(&["a", "b", "c"]),
            &s(&["a", "x", "b", "c"]),
            &s(&["a", "b", "c", "y"]),
        );
        assert_eq!(out, s(&["a", "x", "b", "c", "y"]));
        assert!(w.conflicts.is_empty());
        // Both add one ID at different places: an order conflict there.
        let mut w = Walk::default();
        order(
            &mut w,
            "/footprints",
            &s(&["a", "b"]),
            &s(&["n", "a", "b"]),
            &s(&["a", "b", "n"]),
        );
        assert_eq!(w.conflicts["/footprints/n"].kind, Kind::Order);
        assert_eq!(
            w.conflicts["/footprints/n"].ours,
            Some(json!({"first": true}))
        );
        assert_eq!(
            w.conflicts["/footprints/n"].theirs,
            Some(json!({"after": "b"}))
        );
        assert_eq!(
            moved(&s(&["a", "b", "c"]), &s(&["c", "a", "b"])),
            BTreeSet::from(["c".to_owned()])
        );
        assert_eq!(place(&s(&["a", "b"]), "z"), None);
    }

    #[test]
    fn settling_puts_a_value_or_a_place() {
        let mut v = json!({"a": {"b": 1}});
        set_at(&mut v, "/a/c~1d", Some(json!(2)));
        set_at(&mut v, "/a/b", None);
        set_at(&mut v, "/x/y", Some(json!(3)));
        assert_eq!(v, json!({"a": {"c/d": 2}, "x": {"y": 3}}));
        set_at(&mut v, "", Some(json!(7)));
        assert_eq!(v, json!(7));
        let mut l = s(&["a", "b", "c"]);
        put(&mut l, "c", Some(&json!({"first": true})));
        assert_eq!(l, s(&["c", "a", "b"]));
        put(&mut l, "c", Some(&json!({"after": "a"})));
        assert_eq!(l, s(&["a", "c", "b"]));
        put(&mut l, "c", None);
        assert_eq!(l, s(&["a", "b"]));
    }

    #[test]
    fn lines_merge_as_git_merges_them() {
        let (out, n) = text(b"a\nb\nc\n", b"A\nb\nc\n", b"a\nb\nC\n");
        assert_eq!((out, n), (b"A\nb\nC\n".to_vec(), 0));
        let (out, n) = text(b"a\nb\nc\n", b"a\nB\nc\n", b"a\nX\nc\n");
        assert_eq!(n, 1);
        assert_eq!(
            String::from_utf8(out).unwrap(),
            "a\n<<<<<<< ours\nB\n=======\nX\n>>>>>>> theirs\nc\n"
        );
    }
}
