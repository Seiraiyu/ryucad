//! The repair for an ID collision: two different things added under one
//! ID on the two sides of a merge. Ours keeps the ID; theirs' thing takes
//! the next free one, and every use of it that came from theirs follows: a
//! feature's file, its place in the part's list, the sweeps and holes of
//! a sketch, the features that act on a solid it founds, the patterns
//! that repeat it, and the references to its faces; a sketch entity's
//! key, the entities and constraints built on it, and the sweeps and
//! holes that name it.
//!
//! Names hash IDs, so renumbering renames what theirs built from it. Which
//! name became which is found by building theirs as committed and theirs
//! renumbered, and pairing each body's faces, edges and vertices by exact
//! geometry ([`crate::diff::match_names`]); theirs' references are
//! rewritten through that. When a pairing isn't one to one, no repair is
//! planned and the collision is the agent's to settle. This maps two
//! builds of one input; a reference itself never resolves by geometry.

use crate::activity::Files;
use crate::doc::id::Id;
use crate::doc::project::Project;
use crate::merge::{Conflict, NAMES};
use serde_json::{Value, json};
use std::collections::BTreeMap;
use std::path::Path;

/// What is renumbered: a feature of a part, or an entity or constraint of
/// one of its sketches.
#[derive(Clone, Debug, PartialEq)]
pub enum Rename {
    Feature {
        part: String,
        from: String,
        to: String,
    },
    Entity {
        part: String,
        sketch: String,
        from: String,
        to: String,
    },
}

impl Rename {
    /// The part it is in.
    pub fn part(&self) -> &str {
        match self {
            Rename::Feature { part, .. } | Rename::Entity { part, .. } => part,
        }
    }

    /// The new ID.
    pub fn to(&self) -> &str {
        match self {
            Rename::Feature { to, .. } | Rename::Entity { to, .. } => to,
        }
    }
}

fn json_of(files: &Files, path: &str) -> Option<Value> {
    serde_json::from_slice(files.get(path)?).ok()
}

fn bytes(v: &Value) -> Vec<u8> {
    crate::doc::canonical::to_string(v).into_bytes()
}

/// The feature IDs a set of files has in `part`, as files or listed.
fn features(files: &Files, part: &str) -> Vec<String> {
    let dir = format!("parts/{part}/features/");
    let mut out: Vec<String> = files
        .keys()
        .filter_map(|p| p.strip_prefix(&dir)?.strip_suffix(".json"))
        .map(str::to_owned)
        .collect();
    if let Some(v) = json_of(files, &format!("parts/{part}/part.json")) {
        out.extend(
            v["features"]
                .as_array()
                .into_iter()
                .flatten()
                .filter_map(|f| f.as_str().map(str::to_owned)),
        );
    }
    out
}

/// The ID after the largest of `prefix` followed by digits among `ids`.
fn after(prefix: &str, ids: impl Iterator<Item = String>) -> String {
    let n = ids
        .filter_map(|k| k.strip_prefix(prefix)?.parse::<u64>().ok())
        .max()
        .unwrap_or(0);
    format!("{prefix}{}", n + 1)
}

/// The renumbering for the collision at `pointer` of the file at `path`,
/// to an ID neither the merge nor either side uses: a feature takes
/// `<id>-<n>` (its own `-<n>` dropped first), the smallest free; a sketch
/// entity or constraint one past the largest with its prefix.
pub fn rename(path: &str, pointer: &str, sides: [&Files; 3]) -> Option<Rename> {
    let seg: Vec<&str> = path.split('/').collect();
    let ["parts", part, "features", file] = seg.as_slice() else {
        return None;
    };
    let feature = file.strip_suffix(".json")?;
    if pointer.is_empty() {
        let stem = match feature.rsplit_once('-') {
            Some((s, n)) if !s.is_empty() && n.bytes().all(|c| c.is_ascii_digit()) => s,
            _ => feature,
        };
        let taken: Vec<String> = sides.iter().flat_map(|f| features(f, part)).collect();
        let to = Id::generate(stem, |s| taken.iter().any(|t| t == s)).to_string();
        return Some(Rename::Feature {
            part: (*part).to_owned(),
            from: feature.to_owned(),
            to,
        });
    }
    let p: Vec<String> = pointer
        .split('/')
        .skip(1)
        .map(crate::merge::unescape)
        .collect();
    let [group, id] = p.as_slice() else {
        return None;
    };
    if group != "entities" && group != "constraints" {
        return None;
    }
    let prefix: String = id.chars().take_while(|c| !c.is_ascii_digit()).collect();
    let ids = sides.into_iter().flat_map(|f| {
        let v = json_of(f, path).unwrap_or_default();
        ["entities", "constraints"]
            .into_iter()
            .flat_map(move |g| {
                v[g].as_object()
                    .map(|o| o.keys().cloned().collect::<Vec<_>>())
                    .unwrap_or_default()
            })
            .collect::<Vec<_>>()
    });
    Some(Rename::Entity {
        part: (*part).to_owned(),
        sketch: feature.to_owned(),
        from: id.clone(),
        to: after(&prefix, ids),
    })
}

/// Whether `v` is a reference to another feature's entity.
fn is_reference(v: &Value) -> bool {
    v.as_object().is_some_and(|o| {
        o.len() == 2 && o.get("feature").is_some_and(Value::is_string) && o.contains_key("name")
    })
}

/// What `node` at `pointer` of feature file `file` (of the renamed part)
/// becomes under `r`, with references' names mapped through `names` (by
/// the feature they reference, on theirs); `None` when it stays.
fn transform(
    r: &Rename,
    names: &BTreeMap<String, BTreeMap<String, String>>,
    file: &Value,
    path_feature: &str,
    pointer: &str,
    node: &Value,
) -> Option<Value> {
    // A connector's entity is a solid of the part its instance places,
    // not of this one: renumbering here touches only its path's first
    // step, an instance or a pattern's copy (`<pattern>:<i>`).
    if pointer.ends_with("/at") && is_reference(node) {
        return None;
    }
    if let Rename::Feature { from, to, .. } = r
        && pointer.ends_with("/instance")
        && let Some(path) = node.as_array()
    {
        let first = path.first()?.as_str()?;
        let step = match first.split_once(':') {
            Some((p, copy)) if p == from => format!("{to}:{copy}"),
            None if first == from => to.clone(),
            _ => return None,
        };
        let mut path = path.clone();
        path[0] = json!(step);
        return Some(json!(path));
    }
    if is_reference(node) {
        let f = node["feature"].as_str()?;
        let n = node["name"].as_str()?;
        let mut feature = f.to_owned();
        if let Rename::Feature { from, to, .. } = r
            && f == from
        {
            feature = to.clone();
        }
        let name = names
            .get(f)
            .and_then(|m| m.get(n))
            .cloned()
            .unwrap_or_else(|| n.to_owned());
        let new = json!({"feature": feature, "name": name});
        return (new != *node).then_some(new);
    }
    match r {
        Rename::Feature { from, to, .. } => match pointer {
            "/sketch" | "/target" => (node.as_str() == Some(from)).then(|| json!(to)),
            // The solids an operation or a boolean acts on, what a
            // pattern repeats, and the instances an instance pattern
            // copies.
            "/targets" | "/tools" | "/features" | "/instances" => {
                let list: Vec<Value> = node
                    .as_array()?
                    .iter()
                    .map(|x| {
                        if x.as_str() == Some(from) {
                            json!(to)
                        } else {
                            x.clone()
                        }
                    })
                    .collect();
                let new = json!(list);
                (new != *node).then_some(new)
            }
            _ => None,
        },
        Rename::Entity {
            sketch, from, to, ..
        } => {
            let seg: Vec<&str> = pointer.split('/').skip(1).collect();
            let in_sketch = path_feature == sketch
                && matches!(seg.as_slice(), [g, _, f] if (*g == "entities" || *g == "constraints") && NAMES.contains(f));
            let sweeps_it = file["sketch"].as_str() == Some(sketch);
            if in_sketch && node.as_str() == Some(from) {
                return Some(json!(to));
            }
            if sweeps_it && pointer == "/axis" && node.as_str() == Some(from) {
                return Some(json!(to));
            }
            if sweeps_it && (pointer == "/loops" || pointer == "/points") {
                let list: Vec<Value> = node
                    .as_array()?
                    .iter()
                    .map(|x| {
                        if x.as_str() == Some(from) {
                            json!(to)
                        } else {
                            x.clone()
                        }
                    })
                    .collect();
                let new = json!(list);
                return (new != *node).then_some(new);
            }
            None
        }
    }
}

/// `v` with every node `change` gives a new value replaced by it; objects
/// and arrays (a fillet's groups, their edges) are walked into, anything
/// else is a leaf.
fn rewrite(
    v: &Value,
    pointer: &str,
    change: &mut dyn FnMut(&str, &Value) -> Option<Value>,
) -> Value {
    if let Some(new) = change(pointer, v) {
        return new;
    }
    match v {
        Value::Object(o) => Value::Object(
            o.iter()
                .map(|(k, x)| {
                    let at = format!("{pointer}/{}", crate::merge::escape(k));
                    (k.clone(), rewrite(x, &at, change))
                })
                .collect(),
        ),
        Value::Array(a) => Value::Array(
            a.iter()
                .enumerate()
                .map(|(i, x)| rewrite(x, &format!("{pointer}/{i}"), change))
                .collect(),
        ),
        _ => v.clone(),
    }
}

/// The feature a file of the part is, if it is one.
fn feature_of<'a>(path: &'a str, part: &str) -> Option<&'a str> {
    path.strip_prefix(&format!("parts/{part}/features/"))?
        .strip_suffix(".json")
}

/// Theirs' files with `r` applied everywhere, references' names mapped
/// through `names`: what theirs would have been had it used the new ID.
fn renumbered(
    files: &Files,
    r: &Rename,
    names: &BTreeMap<String, BTreeMap<String, String>>,
) -> Files {
    let part = r.part();
    let mut out = Files::new();
    for (path, b) in files {
        let Some(f) = feature_of(path, part) else {
            out.insert(path.clone(), b.clone());
            continue;
        };
        let Ok(v) = serde_json::from_slice::<Value>(b) else {
            out.insert(path.clone(), b.clone());
            continue;
        };
        let mut v = rewrite(&v, "", &mut |p, n| transform(r, names, &v, f, p, n));
        let mut path = path.clone();
        match r {
            Rename::Feature { from, to, .. } if f == from => {
                path = format!("parts/{part}/features/{to}.json");
            }
            Rename::Entity {
                sketch, from, to, ..
            } if f == sketch => {
                for g in ["entities", "constraints"] {
                    if let Some(o) = v[g].as_object_mut()
                        && let Some(x) = o.remove(from)
                    {
                        o.insert(to.clone(), x);
                    }
                }
            }
            _ => {}
        }
        out.insert(path, bytes(&v));
    }
    if let Rename::Feature { from, to, .. } = r {
        let index = format!("parts/{part}/part.json");
        if let Some(mut v) = json_of(files, &index) {
            if let Some(list) = v["features"].as_array_mut() {
                for f in list.iter_mut() {
                    if f.as_str() == Some(from) {
                        *f = json!(to);
                    }
                }
            }
            out.insert(index, bytes(&v));
        }
    }
    out
}

/// Every state of every solid of `part` built from `files`, by solid and
/// the feature that left it so.
fn bodies(
    root: &Path,
    files: &Files,
    part: &str,
) -> Option<BTreeMap<(String, String), ryukei::topo::Body>> {
    let project = Project::load_from(root, files).ok()?;
    let p = project.parts.get(&Id::parse(part, "part").ok()?)?;
    let built = crate::rebuild::rebuild_part(p);
    Some(
        built
            .features
            .into_iter()
            .filter_map(|(id, r)| Some((id, r.ok()?)))
            .flat_map(|(id, o)| {
                o.solids()
                    .iter()
                    .map(|(s, b)| ((s.to_string(), id.to_string()), b.body.clone()))
                    .collect::<Vec<_>>()
            })
            .collect(),
    )
}

/// Which name of each of theirs' solids became which under `r`, by the
/// solid as theirs calls it, across every state of it a reference may
/// read; `None` when some state's pairing isn't one to one, a name would
/// map two ways, or a state never builds. A state that reads names of a
/// renamed one (a fillet's edges) builds once those are mapped: each
/// round maps what built and builds again with it.
fn names(
    root: &Path,
    theirs: &Files,
    r: &Rename,
) -> Option<BTreeMap<String, BTreeMap<String, String>>> {
    let part = r.part();
    let before = bodies(root, theirs, part)?;
    let renamed = |f: &String| match r {
        Rename::Feature { from, to, .. } if f == from => to.clone(),
        _ => f.clone(),
    };
    let mut out: BTreeMap<String, BTreeMap<String, String>> = BTreeMap::new();
    loop {
        let after = bodies(root, &renumbered(theirs, r, &out), part)?;
        let mut next: BTreeMap<String, BTreeMap<String, String>> = BTreeMap::new();
        let mut unbuilt = false;
        for ((solid, f), body) in &before {
            let Some(other) = after.get(&(renamed(solid), renamed(f))) else {
                unbuilt = true;
                continue;
            };
            let map = crate::diff::match_names(body, other)?;
            let names = next.entry(solid.clone()).or_default();
            for (a, b) in map {
                if names.get(&a).is_some_and(|was| *was != b) {
                    return None;
                }
                names.insert(a, b);
            }
        }
        for m in next.values_mut() {
            m.retain(|a, b| a != b);
        }
        next.retain(|_, m| !m.is_empty());
        if !unbuilt {
            return Some(next);
        }
        if next == out {
            return None;
        }
        out = next;
    }
}

/// The renumbering and its file changes for the collision `c` at
/// `pointer` of `path`, in the working files, from the three sides' files;
/// `None` when it can't be planned.
pub fn plan(
    root: &Path,
    working: &Files,
    ours: &Files,
    theirs: &Files,
    path: &str,
    pointer: &str,
    c: &Conflict,
) -> Option<(Rename, Vec<crate::doctor::Change>)> {
    let r = rename(path, pointer, [working, ours, theirs])?;
    let names = names(root, theirs, &r)?;
    let part = r.part().to_owned();
    let mut files = working.clone();
    // Theirs' uses of it, and of what it built, follow it: every node of a
    // file in the part that the merge has from theirs, not ours.
    for (p, b) in working {
        let Some(f) = feature_of(p, &part) else {
            continue;
        };
        let (Ok(v), t, o) = (
            serde_json::from_slice::<Value>(b),
            json_of(theirs, p),
            json_of(ours, p),
        ) else {
            continue;
        };
        let from_theirs = |pointer: &str, node: &Value| {
            let at = |s: &Option<Value>| s.as_ref().and_then(|s| s.pointer(pointer)).cloned();
            at(&t).as_ref() == Some(node) && at(&o).as_ref() != Some(node)
        };
        let new = rewrite(&v, "", &mut |pt, n| {
            from_theirs(pt, n)
                .then(|| transform(&r, &names, &v, f, pt, n))
                .flatten()
        });
        if new != v {
            files.insert(p.clone(), bytes(&new));
        }
    }
    match &r {
        Rename::Feature { from, to, .. } => {
            // Ours' file as it is, theirs' under the new ID.
            let ours_file = c.ours.clone()?;
            files.insert(path.to_owned(), bytes(&ours_file));
            let theirs_file = c.theirs.clone()?;
            let moved = rewrite(&theirs_file, "", &mut |pt, n| {
                transform(&r, &names, &theirs_file, from, pt, n)
            });
            files.insert(format!("parts/{part}/features/{to}.json"), bytes(&moved));
            // Listed where theirs lists it; where ours lists it, it stays.
            let index = format!("parts/{part}/part.json");
            let mut v = json_of(&files, &index)?;
            let mut record = crate::merge::record(&v);
            record.remove(&format!("/features/{}", crate::merge::escape(from)));
            let mut plain = crate::doc::project::without_conflicts(&v);
            let mut list: Vec<String> = serde_json::from_value(plain["features"].clone()).ok()?;
            let theirs_list: Vec<String> = json_of(theirs, &index)
                .and_then(|t| serde_json::from_value(t["features"].clone()).ok())
                .unwrap_or_default();
            let ours_list: Vec<String> = json_of(ours, &index)
                .and_then(|t| serde_json::from_value(t["features"].clone()).ok())
                .unwrap_or_default();
            // Where both put it, theirs' goes after ours', as the merge
            // puts ours first.
            let theirs_place = crate::merge::place(&theirs_list, from);
            let place = if theirs_place == crate::merge::place(&ours_list, from) {
                json!({ "after": from })
            } else {
                theirs_place.unwrap_or(json!({}))
            };
            if !list.contains(from) {
                list.push(from.clone());
            }
            crate::validate::put(&mut list, to, Some(&place));
            plain["features"] = json!(list);
            v = crate::merge::Merged {
                value: plain,
                conflicts: record,
            }
            .to_json();
            files.insert(index, bytes(&v));
        }
        Rename::Entity { from, to, .. } => {
            let mut v = json_of(&files, path)?;
            let mut record = crate::merge::record(&v);
            record.remove(pointer);
            let group = if pointer.starts_with("/entities/") {
                "entities"
            } else {
                "constraints"
            };
            let mut plain = crate::doc::project::without_conflicts(&v);
            plain[group][to] = c.theirs.clone()?;
            // Theirs' sides of the other collisions that name it follow too.
            for x in record.values_mut() {
                if let Some(t) = &x.theirs {
                    x.theirs = Some(rewrite(t, "", &mut |_, n| {
                        n.as_object().map(|o| {
                            let mut o = o.clone();
                            for f in NAMES {
                                if o.get(f).and_then(Value::as_str) == Some(from) {
                                    o.insert(f.to_owned(), json!(to));
                                }
                            }
                            Value::Object(o)
                        })
                    }));
                }
            }
            if plain["kind"] == crate::doc::files::FEATURE_KIND {
                plain["format"] = json!(crate::doc::files::feature_format(&plain));
            }
            v = crate::merge::Merged {
                value: plain,
                conflicts: record,
            }
            .to_json();
            files.insert(path.to_owned(), bytes(&v));
        }
    }
    let change = |path: &String, content: Option<&Vec<u8>>| crate::doctor::Change {
        path: path.clone(),
        content: content.cloned(),
    };
    let mut changes: Vec<crate::doctor::Change> = files
        .iter()
        .filter(|(p, b)| working.get(*p) != Some(*b))
        .map(|(p, b)| change(p, Some(b)))
        .collect();
    changes.extend(
        working
            .keys()
            .filter(|p| !files.contains_key(*p))
            .map(|p| change(p, None)),
    );
    Some((r, changes))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn files(list: &[(&str, Value)]) -> Files {
        list.iter()
            .map(|(p, v)| ((*p).to_owned(), bytes(v)))
            .collect()
    }

    #[test]
    fn a_renumbered_instance_or_pattern_is_renamed_in_connectors_and_patterns() {
        // Theirs' `pin` collided and is renumbered `pin-1`. Its connectors
        // and patterns follow; a connector's solid, which is the bolt's,
        // doesn't, even where it shares the ID.
        let mate = json!({
            "format": 4, "kind": "ryucad.feature", "type": "mate", "form": "ball",
            "a": {"instance": ["pin"], "at": {"feature": "pin", "name": "0".repeat(32)}},
            "b": {"instance": ["ring:2", "pin"], "at": "origin"},
        });
        let ring = json!({
            "format": 4, "kind": "ryucad.feature", "type": "instance_pattern",
            "form": "circular", "instances": ["pin", "nut"], "count": "3", "angle": "360 deg",
            "axis": {"instance": ["pin"], "at": "origin"},
        });
        let f = files(&[
            ("parts/a/features/m1.json", mate),
            ("parts/a/features/ring.json", ring),
        ]);
        let r = Rename::Feature {
            part: "a".into(),
            from: "pin".into(),
            to: "pin-1".into(),
        };
        let out = renumbered(&f, &r, &BTreeMap::new());
        let read = |p: &str| serde_json::from_slice::<Value>(&out[p]).unwrap();
        let m = read("parts/a/features/m1.json");
        assert_eq!(m["a"]["instance"], json!(["pin-1"]));
        assert_eq!(m["a"]["at"]["feature"], "pin");
        // A copy's seed is the rest of its path: renumbering the pattern
        // would rename its first step.
        assert_eq!(m["b"]["instance"], json!(["ring:2", "pin"]));
        let p = read("parts/a/features/ring.json");
        assert_eq!(p["instances"], json!(["pin-1", "nut"]));
        assert_eq!(p["axis"]["instance"], json!(["pin-1"]));
        let r = Rename::Feature {
            part: "a".into(),
            from: "ring".into(),
            to: "ring-1".into(),
        };
        let m = serde_json::from_slice::<Value>(
            &renumbered(&f, &r, &BTreeMap::new())["parts/a/features/m1.json"],
        )
        .unwrap();
        assert_eq!(m["b"]["instance"], json!(["ring-1:2", "pin"]));
    }

    #[test]
    fn new_ids_are_free_everywhere() {
        let part = |f: &[&str]| json!({"features": f, "format": 1, "kind": "ryucad.part"});
        let a = files(&[("parts/p/part.json", part(&["box-1", "box-2"]))]);
        let b = files(&[("parts/p/part.json", part(&["box-1", "box-3"]))]);
        let r = rename("parts/p/features/box-1.json", "", [&a, &b, &Files::new()]).unwrap();
        assert_eq!(
            r,
            Rename::Feature {
                part: "p".into(),
                from: "box-1".into(),
                to: "box-4".into()
            }
        );
        let r = rename("parts/p/features/boss.json", "", [&a, &b, &Files::new()]).unwrap();
        assert_eq!(r.to(), "boss-1");
        let sketch = |ids: &[&str]| {
            let e: serde_json::Map<String, Value> = ids
                .iter()
                .map(|i| ((*i).to_owned(), json!({"type": "point", "m": [0, 0]})))
                .collect();
            json!({"type": "sketch", "entities": e, "constraints": {"d4": {}}})
        };
        let a = files(&[("parts/p/features/s.json", sketch(&["p1", "p7"]))]);
        let b = files(&[("parts/p/features/s.json", sketch(&["p1", "p7", "p8"]))]);
        let r = rename(
            "parts/p/features/s.json",
            "/entities/p7",
            [&a, &b, &Files::new()],
        )
        .unwrap();
        assert_eq!(r.to(), "p9");
        let r = rename(
            "parts/p/features/s.json",
            "/constraints/d4",
            [&a, &b, &Files::new()],
        )
        .unwrap();
        assert_eq!(r.to(), "d5");
        assert!(rename("parts/p/part.json", "/features/x", [&a, &b, &a]).is_none());
    }
}
