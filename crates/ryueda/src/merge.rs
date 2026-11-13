//! Three-way merges of ryueda's files, value by value (design 518), over
//! `ryu_git::value`'s walk: the rules of each file kind, what Git's merge
//! driver writes ([`bytes`]), and one conflict of a record settled
//! ([`settle`]).
//!
//! - A component's `of`, `footprint` and each pin's node are one value
//!   each, so one side connecting pin 3 while the other leaves it alone
//!   takes it, and both connecting it differently conflict at `/pins/3`.
//!   Its `parameters` merge by name; its `tags` (and nets' and rails',
//!   and `style.json`'s `locks`) as a set: added by either, removed by
//!   either.
//! - A net's `pair`, an instance's `ports/<p>` and `refdes`, `erc.json`'s
//!   `waivers/<id>`, `style.json`'s `palette/<name>`, a symbol's
//!   `pins/<n>` and `graphics/<id>`, a footprint's `pads/<id>`,
//!   `graphics/<id>` and `keepouts/<id>`, a library component's `symbol`
//!   and `category` are one value each.
//! - A library component's `footprints` merge by footprint, in order
//!   ([`ryu_git::value::order`] at `/footprints`), each one's pin map by
//!   pin.
//! - Every other key merges per key, an array being one value (a bus's
//!   `nets`, `style.json`'s `rules`, `offers`, `alternates`, `models`).
//! - A file added on both sides is one value (`id_collision` when they
//!   differ); a file whose `kind` changed is one value.
//!
//! The result is written at the lowest format that holds it.

use crate::conn;
use ryu_git::value::{self, Conflict, Kind, Merged, Rules, Walk};
use std::collections::BTreeMap;

pub use ryu_git::value::Outcome;
use serde_json::{Map, Value, json};

/// The segments of a JSON pointer, as written.
fn segs(at: &str) -> Vec<&str> {
    at.split('/').skip(1).collect()
}

fn component_atom(at: &str) -> bool {
    matches!(segs(at).as_slice(), ["of"] | ["footprint"] | ["pins", _])
}

fn net_atom(at: &str) -> bool {
    matches!(segs(at).as_slice(), ["pair"])
}

fn instance_atom(at: &str) -> bool {
    matches!(segs(at).as_slice(), ["ports", _] | ["refdes"])
}

fn erc_atom(at: &str) -> bool {
    matches!(segs(at).as_slice(), ["waivers", _])
}

fn style_atom(at: &str) -> bool {
    matches!(segs(at).as_slice(), ["palette", _])
}

fn symbol_atom(at: &str) -> bool {
    matches!(segs(at).as_slice(), ["pins", _] | ["graphics", _])
}

fn footprint_atom(at: &str) -> bool {
    matches!(
        segs(at).as_slice(),
        ["pads", _] | ["graphics", _] | ["keepouts", _]
    )
}

fn libcomp_atom(at: &str) -> bool {
    matches!(
        segs(at).as_slice(),
        ["symbol"] | ["category"] | ["footprints", _, "footprint"]
    )
}

fn no_atom(_: &str) -> bool {
    false
}

/// The pointers a file of `kind` merges as one value.
fn atoms(kind: &str) -> fn(&str) -> bool {
    match kind {
        conn::COMPONENT_KIND => component_atom,
        conn::NET_KIND => net_atom,
        conn::INSTANCE_KIND => instance_atom,
        crate::erc::KIND => erc_atom,
        crate::style::KIND => style_atom,
        crate::symbol::KIND => symbol_atom,
        crate::footprint::KIND => footprint_atom,
        crate::libcomp::KIND => libcomp_atom,
        _ => no_atom,
    }
}

/// The members a file of `kind` keeps as sorted sets of strings.
fn sets(kind: &str) -> &'static [&'static str] {
    match kind {
        conn::COMPONENT_KIND | conn::NET_KIND | conn::RAIL_KIND => &["tags"],
        crate::style::KIND => &["locks"],
        _ => &[],
    }
}

/// `v` with each of `sets` that is an array of strings as an object of
/// its items, so the walk merges item by item.
fn set_objects(v: &Value, sets: &[&str]) -> Value {
    let mut v = v.clone();
    for s in sets {
        if let Some(items) = v.get(*s).and_then(Value::as_array)
            && items.iter().all(Value::is_string)
        {
            let o: Map<String, Value> = items
                .iter()
                .filter_map(|i| Some((i.as_str()?.to_owned(), Value::Bool(true))))
                .collect();
            v[*s] = Value::Object(o);
        }
    }
    v
}

/// [`set_objects`] undone: each set's items sorted, an empty set left out
/// (as every writer skips it).
fn set_arrays(v: &mut Value, sets: &[&str]) {
    for s in sets {
        let Some(items) = v
            .get(*s)
            .and_then(Value::as_object)
            .map(|o| o.keys().map(|k| json!(k)).collect::<Vec<_>>())
        else {
            continue;
        };
        if items.is_empty() {
            if let Some(o) = v.as_object_mut() {
                o.remove(*s);
            }
        } else {
            v[*s] = Value::Array(items);
        }
    }
}

/// The text naming a library reference: `<library>/<id>`, or `<id>` in
/// the file's own library.
fn ref_key(r: &Value) -> Option<String> {
    let id = r.get("id")?.as_str()?;
    Some(match r.get("library").and_then(Value::as_str) {
        Some(l) => format!("{l}/{id}"),
        None => id.to_owned(),
    })
}

/// A library component's `footprints` as an object by footprint and
/// their order; `None` unless the list is of distinct footprints. No
/// list is an empty one.
fn keyed(list: Option<&Value>) -> Option<(Map<String, Value>, Vec<String>)> {
    let mut map = Map::new();
    let mut order = Vec::new();
    let Some(list) = list else {
        return Some((map, order));
    };
    for u in list.as_array()? {
        let k = ref_key(u.get("footprint")?)?;
        if map.insert(k.clone(), u.clone()).is_some() {
            return None;
        }
        order.push(k);
    }
    Some((map, order))
}

/// `footprints` again from [`keyed`]'s form: the entries `map` has, in
/// `order` and then any `order` doesn't name; `None` for none.
fn unkeyed(map: &Map<String, Value>, order: &[String]) -> Option<Value> {
    let mut order = order.to_vec();
    for k in map.keys() {
        if !order.contains(k) {
            order.push(k.clone());
        }
    }
    let list: Vec<Value> = order.iter().filter_map(|k| map.get(k).cloned()).collect();
    (!list.is_empty()).then_some(Value::Array(list))
}

/// A library component: per key, its `footprints` by footprint in order.
fn libcomp(walk: &mut Walk, base: &Value, ours: &Value, theirs: &Value) -> Value {
    let obj = |v: &Value| v.as_object().cloned().unwrap_or_default();
    let (b, o, t) = (obj(base), obj(ours), obj(theirs));
    let mut out = walk.object("", &b, &o, &t, &["footprints"]);
    let lists = (
        keyed(b.get("footprints")),
        keyed(o.get("footprints")),
        keyed(t.get("footprints")),
    );
    let footprints = match lists {
        (Some((bm, bl)), Some((om, ol)), Some((tm, tl))) => {
            let list = value::order(walk, "/footprints", &bl, &ol, &tl);
            let map = walk
                .value(
                    "/footprints",
                    Some(&Value::Object(bm)),
                    Some(&Value::Object(om)),
                    Some(&Value::Object(tm)),
                )
                .and_then(|m| m.as_object().cloned())
                .unwrap_or_default();
            unkeyed(&map, &list)
        }
        // A list that isn't one of distinct footprints is one value.
        _ => walk.value(
            "/footprints",
            b.get("footprints"),
            o.get("footprints"),
            t.get("footprints"),
        ),
    };
    if let Some(f) = footprints {
        out.insert("footprints".to_owned(), f);
    }
    Value::Object(out)
}

/// The lowest format of `kind` that holds `v`, as each writer chooses it;
/// the greater of the sides' for a kind with no such rule.
fn format_of(kind: &str, v: &Value, ours: &Value, theirs: &Value) -> u64 {
    let f = |x: &Value| x["format"].as_u64().unwrap_or(1);
    match kind {
        conn::DESIGN_KIND
        | conn::BLOCK_KIND
        | conn::COMPONENT_KIND
        | conn::NET_KIND
        | conn::INSTANCE_KIND
        | conn::RAIL_KIND
        | conn::CLASS_KIND => conn::format_of(kind, v),
        crate::symbol::KIND => {
            if v.get("class").is_some() {
                2
            } else {
                1
            }
        }
        crate::libcomp::KIND => {
            if v.get("category").is_some()
                || v["alternates"].as_array().is_some_and(|a| !a.is_empty())
            {
                2
            } else {
                1
            }
        }
        crate::project::KIND => {
            if v["properties"].as_object().is_some_and(|o| !o.is_empty())
                || v.get("schematic").is_some()
            {
                crate::project::FORMAT_2
            } else {
                crate::project::FORMAT
            }
        }
        _ => f(ours).max(f(theirs)),
    }
}

/// Merges three versions of one ryueda file of one kind (not a layout:
/// see [`bytes`]); `base` is `None` when both sides added it.
pub fn merge(base: Option<&Value>, ours: &Value, theirs: &Value) -> Merged {
    let kind = ours["kind"].as_str().unwrap_or_default().to_owned();
    let sets = sets(&kind);
    let prep = |v: &Value| {
        let mut v = set_objects(v, sets);
        if let Some(o) = v.as_object_mut() {
            o.remove("format");
        }
        v
    };
    let (b, o, t) = (base.map(prep), prep(ours), prep(theirs));
    let mut walk = Walk::new(Rules {
        atom: atoms(&kind),
        ..Rules::default()
    });
    let mut value = match &b {
        None => walk.whole(None, &o, &t),
        Some(b) if kind == crate::libcomp::KIND => libcomp(&mut walk, b, &o, &t),
        Some(b) => walk
            .value("", Some(b), Some(&o), Some(&t))
            .unwrap_or_else(|| o.clone()),
    };
    set_arrays(&mut value, sets);
    let format = format_of(&kind, &value, ours, theirs);
    if let Some(m) = value.as_object_mut() {
        m.insert("format".to_owned(), json!(format));
    }
    // A whole file's sides are whole files, as written.
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

/// The newest format of each file kind this build reads; kinds it
/// doesn't know merge by the generic rule at any format.
pub fn newest(kind: &str) -> Option<u64> {
    Some(match kind {
        conn::DESIGN_KIND
        | conn::BLOCK_KIND
        | conn::COMPONENT_KIND
        | conn::NET_KIND
        | conn::INSTANCE_KIND
        | conn::RAIL_KIND
        | conn::CLASS_KIND => conn::newest(kind),
        crate::project::KIND => crate::project::FORMAT_2,
        crate::erc::KIND => crate::erc::FORMAT,
        crate::lint::KIND => crate::lint::FORMAT,
        crate::style::KIND => crate::style::FORMAT,
        crate::standards::KIND => crate::standards::FORMAT,
        crate::sourcing::KIND => crate::sourcing::FORMAT,
        crate::symbol::KIND => crate::symbol::FORMAT,
        crate::footprint::KIND => crate::footprint::FORMAT,
        crate::libcomp::KIND => crate::libcomp::FORMAT,
        crate::schem::LAYOUT_KIND => crate::schem::LAYOUT_FORMAT,
        _ => return None,
    })
}

/// A file's JSON if ryueda can merge it by value: UTF-8 JSON, an object
/// with a `kind` and a `format`, no conflict record, no format newer than
/// this build reads. It needn't be canonical: the merge compares values
/// and writes the result canonical.
fn readable(bytes: &[u8]) -> Option<Value> {
    let v = crate::canonical::parse(std::str::from_utf8(bytes).ok()?).ok()?;
    if v.get(value::CONFLICTS).is_some() {
        return None;
    }
    let kind = v.get("kind")?.as_str()?;
    let format = v.get("format")?.as_u64()?;
    if newest(kind).is_some_and(|n| format > n) {
        return None;
    }
    Some(v)
}

/// A layout, which only `merge status` can merge (it needs the merged
/// connectivity): one side's when only it changed, or both alike; else
/// ours' with a whole-file `layout_pending` entry holding all three.
fn layout(base: Option<&Value>, ours: &Value, theirs: &Value) -> Merged {
    let one = if ours == theirs || Some(theirs) == base {
        Some(ours)
    } else if Some(ours) == base {
        Some(theirs)
    } else {
        None
    };
    let mut conflicts = BTreeMap::new();
    if one.is_none() {
        conflicts.insert(
            String::new(),
            Conflict {
                kind: Kind::LayoutPending,
                base: base.cloned(),
                ours: Some(ours.clone()),
                theirs: Some(theirs.clone()),
            },
        );
    }
    Merged {
        value: one.unwrap_or(ours).clone(),
        conflicts,
    }
}

/// Merges three versions of a file as Git hands them to the driver; an
/// empty base means both sides added it. Files ryueda reads merge by
/// value and come out canonical: by [`merge`] when all are of one kind,
/// as one value when a side changed the kind, by [`layout`] when any is a
/// layout. If any version can't be read they merge as lines, as `git
/// merge-file` does.
pub fn bytes(base: &[u8], ours: &[u8], theirs: &[u8]) -> (Vec<u8>, Outcome) {
    let b = if base.is_empty() {
        Some(None)
    } else {
        readable(base).map(Some)
    };
    if let (Some(b), Some(o), Some(t)) = (b, readable(ours), readable(theirs)) {
        let kinds = [Some(&o), Some(&t), b.as_ref()].map(|v| v.map(|v| v["kind"].clone()));
        let is_layout = kinds
            .iter()
            .any(|k| k.as_ref().is_some_and(|k| *k == crate::schem::LAYOUT_KIND));
        let one_kind = kinds.iter().flatten().all(|k| *k == o["kind"]);
        let m = if is_layout && one_kind {
            layout(b.as_ref(), &o, &t)
        } else if !one_kind {
            let mut walk = Walk::default();
            let value = walk.whole(b.as_ref(), &o, &t);
            Merged {
                value,
                conflicts: walk.conflicts,
            }
        } else {
            merge(b.as_ref(), &o, &t)
        };
        let outcome = match m.conflicts.len() {
            0 => Outcome::Clean,
            n => Outcome::Conflicts(n),
        };
        return (
            crate::canonical::to_string(&m.to_json()).into_bytes(),
            outcome,
        );
    }
    let (out, n) = value::text(base, ours, theirs);
    (out, Outcome::Text(n))
}

/// `v` (a file without its record) with the conflict at `pointer`
/// settled by `x` (`None` removes what is there): a library component's
/// footprint by footprint, an `order` conflict by the place `x` gives,
/// anything else by pointer; then at the lowest format that holds it.
pub fn settle(v: &Value, pointer: &str, kind: Kind, x: Option<Value>) -> Value {
    let mut v = v.clone();
    let listed = v["kind"] == crate::libcomp::KIND && pointer.starts_with("/footprints/");
    match keyed(v.get("footprints")).filter(|_| listed) {
        Some((map, mut list)) => {
            let rest = &pointer["/footprints".len()..];
            let mut m = Value::Object(map);
            if kind == Kind::Order {
                let id = value::unescape(rest.trim_start_matches('/'));
                value::put(&mut list, &id, x.as_ref().filter(|p| !p.is_null()));
            } else {
                value::set_at(&mut m, rest, x);
            }
            let map = m.as_object().cloned().unwrap_or_default();
            match unkeyed(&map, &list) {
                Some(f) => v["footprints"] = f,
                None => {
                    if let Some(o) = v.as_object_mut() {
                        o.remove("footprints");
                    }
                }
            }
        }
        None => value::set_at(&mut v, pointer, x),
    }
    let kind = v["kind"].as_str().unwrap_or_default().to_owned();
    let format = format_of(&kind, &v, &v, &v);
    if let Some(o) = v.as_object_mut() {
        o.insert("format".to_owned(), json!(format));
    }
    v
}

#[cfg(test)]
mod tests {
    use super::*;

    fn comp(extra: Value) -> Value {
        let mut v = json!({"kind": "ryueda.component", "format": 1, "of": {"id": "r_0402"},
                           "refdes": "R1", "pins": {"1": "n1", "2": "n2"}});
        for (k, x) in extra.as_object().unwrap() {
            v[k] = x.clone();
        }
        v
    }

    #[test]
    fn a_components_pins_merge_by_pin_and_its_tags_as_a_set() {
        let base = comp(json!({"tags": ["a", "b"]}));
        let ours =
            comp(json!({"pins": {"1": "n1", "2": "n2", "3": "n1"}, "tags": ["a", "b", "c"]}));
        let theirs =
            comp(json!({"pins": {"1": "n1", "2": {"rail": "gnd"}}, "tags": ["b"], "value": "10k"}));
        let m = merge(Some(&base), &ours, &theirs);
        assert!(m.is_clean(), "{:?}", m.conflicts);
        assert_eq!(
            m.value["pins"],
            json!({"1": "n1", "2": {"rail": "gnd"}, "3": "n1"})
        );
        // Added by either, removed by either.
        assert_eq!(m.value["tags"], json!(["b", "c"]));
        assert_eq!(
            (m.value["value"].clone(), m.value["format"].clone()),
            (json!("10k"), json!(2))
        );
        // One pin connected differently on each side: one conflict there.
        let ours = comp(json!({"pins": {"1": "n1", "2": "n2", "3": "n1"}}));
        let theirs = comp(json!({"pins": {"1": "n1", "2": "n2", "3": {"port": "p1"}}}));
        let m = merge(Some(&comp(json!({}))), &ours, &theirs);
        assert_eq!(m.conflicts.keys().collect::<Vec<_>>(), ["/pins/3"]);
        assert_eq!(m.value["pins"]["3"], "n1");
        assert_eq!(m.value["format"], 1);
        // A reference is one value: never one side's library with the other's ID.
        let ours = comp(json!({"of": {"library": "lib", "id": "r_0402"}}));
        let theirs = comp(json!({"of": {"id": "r_0603"}}));
        let m = merge(Some(&comp(json!({}))), &ours, &theirs);
        assert_eq!(m.conflicts.keys().collect::<Vec<_>>(), ["/of"]);
    }

    #[test]
    fn nodes_pairs_waivers_palettes_and_symbol_pins_are_one_value() {
        let cases = [
            (
                "ryueda.instance",
                "/ports/p1",
                json!({"ports": {"p1": {"rail": "a"}}}),
                json!({"ports": {"p1": {"rail": "b"}}}),
                json!({"ports": {"p1": "n2"}}),
            ),
            (
                "ryueda.net",
                "/pair",
                json!({"pair": {"with": "x", "polarity": "p"}}),
                json!({"pair": {"with": "y", "polarity": "p"}}),
                json!({"pair": {"with": "x", "polarity": "n"}}),
            ),
            (
                "ryueda.erc",
                "/waivers/f1",
                json!({"waivers": {"f1": {"reason": "r", "by": "a"}}}),
                json!({"waivers": {"f1": {"reason": "s", "by": "a"}}}),
                json!({"waivers": {"f1": {"reason": "r", "by": "b"}}}),
            ),
            (
                "ryueda.style",
                "/palette/accent",
                json!({"palette": {"accent": {"light": "#000000", "dark": "#ffffff"}}}),
                json!({"palette": {"accent": {"light": "#111111", "dark": "#ffffff"}}}),
                json!({"palette": {"accent": {"light": "#000000", "dark": "#eeeeee"}}}),
            ),
            (
                "ryueda.symbol",
                "/pins/1",
                json!({"pins": {"1": {"name": "A", "at": [0, 0]}}}),
                json!({"pins": {"1": {"name": "B", "at": [0, 0]}}}),
                json!({"pins": {"1": {"name": "A", "at": [1, 0]}}}),
            ),
            (
                "ryueda.footprint",
                "/pads/p1",
                json!({"pads": {"p1": {"at": [0, 0], "size": [1, 1]}}}),
                json!({"pads": {"p1": {"at": [1, 0], "size": [1, 1]}}}),
                json!({"pads": {"p1": {"at": [0, 0], "size": [2, 2]}}}),
            ),
        ];
        for (kind, at, base, ours, theirs) in cases {
            let file = |x: Value| {
                let mut v = x;
                v["kind"] = json!(kind);
                v["format"] = json!(1);
                v
            };
            let m = merge(Some(&file(base)), &file(ours), &file(theirs));
            assert_eq!(m.conflicts.keys().collect::<Vec<_>>(), [at], "{kind}");
        }
        // A block's ports merge per field; a bus's nets are one value.
        let block = |x: Value| {
            let mut v = json!({"kind": "ryueda.block", "format": 1, "name": "B"});
            for (k, y) in x.as_object().unwrap() {
                v[k] = y.clone();
            }
            v
        };
        let m = merge(
            Some(&block(
                json!({"ports": {"p": {"name": "SDA", "direction": "in"}}, "buses": {"b": {"name": "I2C", "nets": ["x"]}}}),
            )),
            &block(
                json!({"ports": {"p": {"name": "SDA2", "direction": "in"}}, "buses": {"b": {"name": "I2C", "nets": ["x", "y"]}}}),
            ),
            &block(
                json!({"ports": {"p": {"name": "SDA", "direction": "out"}}, "buses": {"b": {"name": "I2C", "nets": ["z"]}}}),
            ),
        );
        assert_eq!(m.conflicts.keys().collect::<Vec<_>>(), ["/buses/b/nets"]);
        assert_eq!(
            m.value["ports"]["p"],
            json!({"name": "SDA2", "direction": "out"})
        );
    }

    fn libcomp(footprints: Value) -> Value {
        json!({"kind": "ryueda.libcomp", "format": 1, "symbol": {"id": "r"}, "prefix": "R",
               "footprints": footprints})
    }

    #[test]
    fn a_library_components_footprints_merge_by_footprint_in_order() {
        let base = libcomp(json!([{"footprint": {"id": "a"}}, {"footprint": {"id": "b"}}]));
        let ours = libcomp(
            json!([{"footprint": {"id": "a"}, "pins": {"1": ["2"]}}, {"footprint": {"id": "b"}}]),
        );
        let theirs = libcomp(
            json!([{"footprint": {"id": "a"}}, {"footprint": {"id": "b"}}, {"footprint": {"library": "l", "id": "c"}}]),
        );
        let m = merge(Some(&base), &ours, &theirs);
        assert!(m.is_clean(), "{:?}", m.conflicts);
        assert_eq!(
            m.value["footprints"],
            json!([{"footprint": {"id": "a"}, "pins": {"1": ["2"]}}, {"footprint": {"id": "b"}},
                   {"footprint": {"library": "l", "id": "c"}}])
        );
        // One pin map changed differently on each side: a conflict at that pin.
        let theirs = libcomp(
            json!([{"footprint": {"id": "a"}, "pins": {"1": ["3"]}}, {"footprint": {"id": "b"}}]),
        );
        let m = merge(Some(&base), &ours, &theirs);
        assert_eq!(
            m.conflicts.keys().collect::<Vec<_>>(),
            ["/footprints/a/pins/1"]
        );
        // Settled by a value, the list keeps its order and the record its rest.
        let plain = m.value.clone();
        let settled = settle(
            &plain,
            "/footprints/a/pins/1",
            Kind::Value,
            Some(json!(["4"])),
        );
        assert_eq!(
            settled["footprints"][0],
            json!({"footprint": {"id": "a"}, "pins": {"1": ["4"]}})
        );
        // The default footprint moved differently by each side: an order conflict.
        let ours = libcomp(json!([{"footprint": {"id": "b"}}, {"footprint": {"id": "a"}}]));
        let theirs = libcomp(
            json!([{"footprint": {"id": "a"}}, {"footprint": {"id": "b"}}, {"footprint": {"id": "d"}}]),
        );
        let m = merge(Some(&base), &ours, &theirs);
        assert!(m.is_clean(), "{:?}", m.conflicts);
        assert_eq!(
            m.value["footprints"],
            json!([{"footprint": {"id": "b"}}, {"footprint": {"id": "a"}}, {"footprint": {"id": "d"}}])
        );
        let settled = settle(
            &m.value,
            "/footprints/d",
            Kind::Order,
            Some(json!({"first": true})),
        );
        assert_eq!(settled["footprints"][0], json!({"footprint": {"id": "d"}}));
    }

    #[test]
    fn style_locks_are_a_set_and_rules_one_value() {
        let style = |locks: Value, rules: Value| json!({"kind": "ryueda.style", "format": 1, "locks": locks, "rules": rules});
        let m = merge(
            Some(&style(
                json!(["colour"]),
                json!([{"select": {}, "set": {"emphasis": true}}]),
            )),
            &style(json!(["colour", "dash"]), json!([])),
            &style(
                json!([]),
                json!([{"select": {}, "set": {"emphasis": false}}]),
            ),
        );
        assert_eq!(m.value["locks"], json!(["dash"]));
        assert_eq!(m.conflicts.keys().collect::<Vec<_>>(), ["/rules"]);
    }

    #[test]
    fn files_added_twice_and_formats() {
        let m = merge(None, &comp(json!({})), &comp(json!({"refdes": "R2"})));
        assert_eq!(m.conflicts[""].kind, Kind::IdCollision);
        assert_eq!(m.conflicts[""].theirs, Some(comp(json!({"refdes": "R2"}))));
        let m = merge(None, &comp(json!({})), &comp(json!({})));
        assert!(m.is_clean());
        // The lowest format that holds the result: a tag dropped on one
        // side, the only thing format 2 held, gives format 1.
        let m = merge(
            Some(&comp(json!({"format": 2, "tags": ["a"]}))),
            &comp(json!({"format": 2, "tags": ["a"], "value": "1k"})),
            &comp(json!({})),
        );
        assert!(m.is_clean(), "{:?}", m.conflicts);
        assert_eq!(
            (m.value.get("tags"), m.value["format"].clone()),
            (None, json!(1))
        );
        let manifest = |p: Value| json!({"kind": "ryueda.project", "format": 2, "project": "x", "default_unit": "mm", "properties": p});
        let m = merge(
            Some(&manifest(json!({"a": "1"}))),
            &manifest(json!({"a": "1", "b": "2"})),
            &manifest(json!({"a": "1"})),
        );
        assert_eq!(
            m.value["format"], 2,
            "properties: {}",
            m.value["properties"]
        );
    }

    fn text(v: &Value) -> Vec<u8> {
        crate::canonical::to_string(v).into_bytes()
    }

    #[test]
    fn layouts_both_sides_changed_wait_for_the_status() {
        let layout = |x: i64| json!({"kind": "ryueda.layout", "format": 1, "placed": {"r1": {"sheet": "s1", "at": [x, 0]}}});
        let (out, outcome) = bytes(&text(&layout(0)), &text(&layout(1)), &text(&layout(2)));
        assert_eq!(outcome, Outcome::Conflicts(1));
        let v: Value = serde_json::from_slice(&out).unwrap();
        let record = value::record(&v);
        assert_eq!(record[""].kind, Kind::LayoutPending);
        assert_eq!(record[""].theirs, Some(layout(2)));
        assert_eq!(v["placed"], layout(1)["placed"]);
        // Only one side changed it: that side, canonical.
        let (out, outcome) = bytes(&text(&layout(0)), &text(&layout(0)), &text(&layout(2)));
        assert_eq!((out, outcome), (text(&layout(2)), Outcome::Clean));
        // A file that loads refuses the record until it is settled.
        let e = crate::schem::read_layout(
            &bytes(&text(&layout(0)), &text(&layout(1)), &text(&layout(2))).0,
            "blocks/b/layout.json",
        )
        .unwrap_err();
        assert_eq!(e.code, crate::error::Code::MergeConflict);
    }

    #[test]
    fn what_isnt_a_file_this_build_reads_merges_as_lines() {
        let c = comp(json!({}));
        let newer = comp(json!({"format": 99}));
        let (out, outcome) = bytes(
            &text(&c),
            &text(&comp(json!({"refdes": "R2"}))),
            &text(&newer),
        );
        // Merged as lines: the two changes are lines apart, so both stay.
        assert_eq!(outcome, Outcome::Text(0));
        let out = String::from_utf8(out).unwrap();
        assert!(
            out.contains("\"format\": 99") && out.contains("\"R2\""),
            "{out}"
        );
        // One written by hand on one line still merges by value, canonical.
        let one_line = serde_json::to_vec(&comp(json!({"refdes": "R2"}))).unwrap();
        let (out, outcome) = bytes(&text(&c), &one_line, &text(&comp(json!({"value": "1k"}))));
        assert_eq!(outcome, Outcome::Clean);
        assert_eq!(out, text(&comp(json!({"refdes": "R2", "value": "1k"}))));
        // A kind changed on a side is one value.
        let net = json!({"kind": "ryueda.net", "format": 1, "name": "X"});
        let (out, outcome) = bytes(&text(&c), &text(&net), &text(&comp(json!({"value": "1k"}))));
        assert_eq!(outcome, Outcome::Conflicts(1));
        let v: Value = serde_json::from_slice(&out).unwrap();
        assert_eq!(value::record(&v)[""].kind, Kind::Value);
        // Both sides added one file differently: an ID collision.
        let (_, outcome) = bytes(b"", &text(&c), &text(&comp(json!({"refdes": "R9"}))));
        assert_eq!(outcome, Outcome::Conflicts(1));
    }
}
