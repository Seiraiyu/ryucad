//! The repair for an ID collision: two different things added under one
//! ID on the two sides of a merge. Ours keeps the ID; theirs' thing takes
//! the next free one, and every use of it that came from theirs follows: a
//! feature's file, its place in the part's list, the sweeps and holes of
//! a sketch, the features that act on a solid it founds, the patterns
//! that repeat it, and the references to its faces, the connectors of
//! assemblies that place its part included; a sketch entity's
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

fn bytes(v: &Value) -> crate::activity::Bytes {
    crate::doc::canonical::to_string(v).into_bytes().into()
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
        // A path's propagated edge says so beside its reference.
        let grown = usize::from(o.get("propagate").is_some_and(Value::is_boolean));
        // An inserted solid's entity names its solid too (design 521).
        let solid = usize::from(o.get("solid").is_some_and(Value::is_string));
        o.len() == 2 + grown + solid
            && o.get("feature").is_some_and(Value::is_string)
            && o.contains_key("name")
    })
}

/// Whether `v` is a geometric reference to a whole feature (`{feature}`)
/// or a sketch's entity (`{feature, entity}`).
fn is_geometry(v: &Value) -> bool {
    v.as_object().is_some_and(|o| {
        o.get("feature").is_some_and(Value::is_string)
            && (o.len() == 1 || (o.len() == 2 && o.get("entity").is_some_and(Value::is_string)))
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
    // A following pattern's `on` names a feature of the part its path
    // reaches, not of this one: only its path's steps renumber
    // (`/on/instance`).
    if pointer == "/on" {
        return None;
    }
    // A connector's entity is a solid of the part its instance places,
    // not of this one: renumbering here touches only its path's first
    // step, an instance or a pattern's copy (`<pattern>:<i>`).
    // (A datum's own `at`, at the file's top, is this part's.)
    if pointer.ends_with("/at") && pointer != "/at" && (is_reference(node) || is_geometry(node)) {
        return None;
    }
    // A whole feature (`{feature}`) or a sketch's entity
    // (`{feature, entity}`) follows its feature, and the entity its sketch's.
    if is_geometry(node) {
        let f = node["feature"].as_str()?;
        let mut new = node.clone();
        match r {
            Rename::Feature { from, to, .. } if f == from => new["feature"] = json!(to),
            Rename::Entity {
                sketch, from, to, ..
            } if f == sketch && node["entity"].as_str() == Some(from) => {
                new["entity"] = json!(to);
            }
            _ => {}
        }
        return (new != *node).then_some(new);
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
        // An inserted solid's names are its source's, which renumbering
        // this part never changes.
        let name = match node.get("solid") {
            Some(_) => n.to_owned(),
            None => names
                .get(f)
                .and_then(|m| m.get(n))
                .cloned()
                .unwrap_or_else(|| n.to_owned()),
        };
        let mut new = node.clone();
        new["feature"] = json!(feature);
        new["name"] = json!(name);
        return (new != *node).then_some(new);
    }
    // A sweep's profile or a loft's section: `(where it is, the field
    // under it)`.
    let section = section_of(pointer);
    let holder = section.and_then(|(at, _)| file.pointer(at));
    // A face profile or section names its loops by the face's solid's
    // names: they map as its reference's do.
    if let (Some((_, "loops")), Some(h)) = (section, holder)
        && let Some(f) = h["face"]["feature"].as_str()
    {
        let m = names.get(f)?;
        let list: Vec<Value> = node
            .as_array()?
            .iter()
            .map(|x| {
                x.as_str()
                    .and_then(|n| m.get(n))
                    .map_or_else(|| x.clone(), |n| json!(n))
            })
            .collect();
        let new = json!(list);
        return (new != *node).then_some(new);
    }
    match r {
        Rename::Feature { from, to, .. } if matches!(section, Some((_, "sketch"))) => {
            (node.as_str() == Some(from)).then(|| json!(to))
        }
        Rename::Feature { from, to, .. } => {
            // A solid's key is its founder's ID, or `<insert>/<source key>`:
            // either follows a renamed founder or insert (design 521).
            let follow = |s: &str, paths: bool| -> Option<String> {
                if s == from.as_str() {
                    Some(to.to_string())
                } else if paths {
                    s.strip_prefix(&format!("{from}/"))
                        .map(|rest| format!("{to}/{rest}"))
                } else {
                    None
                }
            };
            match pointer {
                // A refold's `unfold` names its unfold as `target` names a
                // solid's founder; a split's tool body likewise.
                "/sketch" | "/unfold" | "/from/sheet" => {
                    (node.as_str() == Some(from)).then(|| json!(to))
                }
                "/target" | "/solid" | "/tool/body" => {
                    follow(node.as_str()?, true).map(|s| json!(s))
                }
                // The solids an operation or a boolean acts on, the sheets a
                // sew sews, what a pattern repeats, the instances an
                // instance pattern copies, and the solids a delete deletes.
                "/targets" | "/tools" | "/features" | "/instances" | "/sheets" | "/solids" => {
                    let paths = matches!(pointer, "/targets" | "/tools" | "/solids");
                    let list: Vec<Value> = node
                        .as_array()?
                        .iter()
                        .map(|x| {
                            x.as_str()
                                .and_then(|s| follow(s, paths))
                                .map_or_else(|| x.clone(), |s| json!(s))
                        })
                        .collect();
                    let new = json!(list);
                    (new != *node).then_some(new)
                }
                _ => None,
            }
        }
        Rename::Entity {
            sketch, from, to, ..
        } => {
            let seg: Vec<&str> = pointer.split('/').skip(1).collect();
            let in_sketch = path_feature == sketch
                && matches!(seg.as_slice(), [g, _, f] if (*g == "entities" || *g == "constraints") && NAMES.contains(f));
            let sweeps_it = file["sketch"].as_str() == Some(sketch);
            // A profile's or section's loops on the sketch.
            if let (Some((_, "loops")), Some(h)) = (section, holder)
                && h["sketch"].as_str() == Some(sketch)
            {
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
            if in_sketch && node.as_str() == Some(from) {
                return Some(json!(to));
            }
            if sweeps_it && pointer == "/axis" && node.as_str() == Some(from) {
                return Some(json!(to));
            }
            if sweeps_it && matches!(pointer, "/loops" | "/points") {
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

/// Where a pointer lies under a sweep's profile or a loft's section:
/// `("/profile", field)` or `("/sections/<i>", field)`.
fn section_of(pointer: &str) -> Option<(&str, &str)> {
    if let Some(rest) = pointer.strip_prefix("/profile/") {
        return Some((&pointer[.."/profile".len()], rest));
    }
    let rest = pointer.strip_prefix("/sections/")?;
    let (i, field) = rest.split_once('/')?;
    i.parse::<usize>().ok()?;
    Some((&pointer[.."/sections/".len() + i.len()], field))
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

/// The part a connector's instance path, from assembly `asm`, ends in, as
/// `files` have it, when every instance along it places its part at
/// `working`: a label reads a release's files, which no merge renumbers.
/// A copy (`<pattern>:<i>`) is followed by its seed.
fn path_part(files: &Files, asm: &str, path: &[Value]) -> Option<String> {
    let mut part = asm.to_owned();
    let mut steps = path.iter().map(Value::as_str);
    while let Some(step) = steps.next() {
        let step = step?;
        let inst = if step.contains(':') {
            steps.next()??
        } else {
            step
        };
        let v = json_of(files, &format!("parts/{part}/features/{inst}.json"))?;
        if v["type"] != "instance" || v["rev"] != "working" {
            return None;
        }
        part = v["part"].as_str()?.to_owned();
    }
    Some(part)
}

/// The part a drawing's node at `pt` names faces of: the source of the
/// view it is under (its base view's `of`, else `owner`, the drawing's own
/// part).
fn drawing_source<'a>(v: &'a Value, pt: &str, owner: &'a str) -> &'a str {
    let mut k = match pt.strip_prefix("/views/").and_then(|r| r.split('/').next()) {
        Some(k) => k,
        None => return owner,
    };
    for _ in 0..=v["views"].as_object().map_or(0, |m| m.len()) {
        let w = &v["views"][k];
        match w["from"].as_str() {
            Some(up) => k = up,
            None => return w["of"].as_str().unwrap_or(owner),
        }
    }
    owner
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
    // With no children, an insert fails here, and so does every feature
    // reading its keys. That is harmless: failed states are left out on
    // both sides alike, so only the inserted readers' name maps are lost.
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

/// An instance path through instance (or pattern) `from`, through `to`
/// instead: `from/rest` and `from:<i>/rest` renamed; `None` for one that
/// doesn't start there.
fn renamed_path(k: &str, from: &str, to: &str) -> Option<String> {
    let (first, rest) = match k.split_once('/') {
        Some((f, r)) => (f, Some(r)),
        None => (k, None),
    };
    let step = match first.split_once(':') {
        Some((pt, copy)) if pt == from => format!("{to}:{copy}"),
        None if first == from => to.to_owned(),
        _ => return None,
    };
    Some(match rest {
        Some(r) => format!("{step}/{r}"),
        None => step,
    })
}

/// The pointers under `/annotations` of a drawing file `v` that hold an
/// entity (`<solid>/<name>`, a datum's ID, or an occurrence's
/// `<instance>/…/<solid>/<name>`), and those that hold a balloon's
/// occurrence path. A file that doesn't read names none.
fn annotation_pointers(
    v: &Value,
) -> (
    std::collections::BTreeSet<String>,
    std::collections::BTreeSet<String>,
) {
    use crate::doc::drawing_annotation::Kind;
    let Ok(file) = crate::doc::drawing::DrawingFile::from_json(v) else {
        return Default::default();
    };
    let entities = file
        .annotation_entities()
        .into_iter()
        .map(|(p, _)| p)
        .collect();
    let occurrences = file
        .annotations
        .iter()
        .filter(|(_, a)| matches!(a.kind, Kind::Balloon { .. }))
        .map(|(k, _)| format!("/annotations/{k}/of"))
        .collect();
    (entities, occurrences)
}

/// What an annotation's entity `text`, in a view of `source`, becomes when
/// feature `from` of `part` is renumbered `to`; `None` when it stays.
fn annotation_entity(
    theirs: &Files,
    names: &BTreeMap<String, BTreeMap<String, String>>,
    text: &str,
    source: &str,
    part: &str,
    from: &str,
    to: &str,
) -> Option<String> {
    let steps: Vec<&str> = text.split('/').collect();
    match steps.as_slice() {
        // A datum of the view's source.
        [datum] => (source == part && *datum == from).then(|| to.to_owned()),
        [path @ .., solid, name] => {
            // The instance path through the source (an assembly), whose
            // first step may be the renumbered instance or pattern.
            let path_text = path.join("/");
            let new_path = if path.is_empty() || source != part {
                path_text.clone()
            } else {
                renamed_path(&path_text, from, to).unwrap_or_else(|| path_text.clone())
            };
            // The part the path ends in names the solid and its face.
            let placed = if path.is_empty() {
                Some(source.to_owned())
            } else {
                let steps: Vec<Value> = path.iter().map(|s| json!(s)).collect();
                path_part(theirs, source, &steps)
            };
            let on_part = placed.as_deref() == Some(part);
            let name = if on_part {
                names
                    .get(*solid)
                    .and_then(|m| m.get(*name))
                    .map_or(*name, String::as_str)
            } else {
                name
            };
            let solid = if on_part && *solid == from { to } else { solid };
            Some(if new_path.is_empty() {
                format!("{solid}/{name}")
            } else {
                format!("{new_path}/{solid}/{name}")
            })
        }
        [] => None,
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
    // Connectors in other parts' files that theirs aimed at what it built
    // (issue #1145): an assembly's, whose instance path reaches this part
    // at `working`, naming the renumbered solid.
    if let Rename::Feature { from, to, .. } = &r {
        for (p, b) in working {
            let Some((asm, _)) = p
                .strip_prefix("parts/")
                .and_then(|x| x.split_once("/features/"))
            else {
                continue;
            };
            let (Ok(v), Some(t), o) = (
                serde_json::from_slice::<Value>(b),
                json_of(theirs, p),
                json_of(ours, p),
            ) else {
                continue;
            };
            if asm == part {
                continue;
            }
            let new = rewrite(&v, "", &mut |pt, n| {
                let at = pt.strip_suffix("/at")?;
                if !is_reference(n) || n["feature"].as_str() != Some(from) {
                    return None;
                }
                let connector = t.pointer(at)?;
                let mine = o.as_ref().and_then(|o| o.pointer(at));
                if v.pointer(at) != Some(connector) || mine == Some(connector) {
                    return None;
                }
                let path = connector["instance"].as_array()?;
                if path_part(theirs, asm, path)? != part {
                    return None;
                }
                let name = n["name"].as_str()?;
                let name = names
                    .get(from)
                    .and_then(|m| m.get(name))
                    .map_or(name, String::as_str);
                Some(json!({"feature": to, "name": name}))
            });
            if new != v {
                files.insert(p.clone(), bytes(&new));
            }
        }
    }
    // Appearance entries theirs keyed by what it renumbers (issue: plan
    // 10.C): the part's own solids and faces, an assembly's occurrences
    // through a renumbered instance, and other assemblies' entries for
    // the solid where their paths reach this part at `working`.
    if let Rename::Feature { from, to, .. } = &r {
        let file = crate::doc::appearance::APPEARANCE;
        for (p, b) in working {
            let Some(owner) = p
                .strip_prefix("parts/")
                .and_then(|x| x.strip_suffix(&format!("/{file}")))
            else {
                continue;
            };
            let (Ok(v), Some(t), o) = (
                serde_json::from_slice::<Value>(b),
                json_of(theirs, p),
                json_of(ours, p),
            ) else {
                continue;
            };
            let key_of = |group: &str, k: &str| -> Option<String> {
                if owner == part {
                    match group {
                        "solids" if k == from => Some(to.clone()),
                        "faces" => {
                            let (solid, name) = k.split_once('/')?;
                            if solid != from {
                                return None;
                            }
                            let name = names
                                .get(from)
                                .and_then(|m| m.get(name))
                                .map_or(name, String::as_str);
                            Some(format!("{to}/{name}"))
                        }
                        "occurrences" => renamed_path(k, from, to),
                        _ => None,
                    }
                } else {
                    // `<path>/<solid>`, the path ending in this part.
                    let (path, solid) = k.rsplit_once('/')?;
                    if group != "occurrences" || solid != from {
                        return None;
                    }
                    let steps: Vec<Value> = path.split('/').map(|s| json!(s)).collect();
                    (path_part(theirs, owner, &steps)? == part).then(|| format!("{path}/{to}"))
                }
            };
            let mut new = v.clone();
            for group in ["solids", "faces", "occurrences"] {
                let Some(entries) = v[group].as_object() else {
                    continue;
                };
                for (k, look) in entries {
                    let theirs_has = t[group].get(k) == Some(look);
                    let ours_has = o.as_ref().and_then(|o| o[group].get(k)) == Some(look);
                    let Some(nk) = key_of(group, k).filter(|_| theirs_has && !ours_has) else {
                        continue;
                    };
                    let m = new[group].as_object_mut().expect("an object above");
                    m.remove(k);
                    m.insert(nk, look.clone());
                }
            }
            if new != v {
                files.insert(p.clone(), bytes(&new));
            }
        }
    }
    // References theirs' drawings of the part make (plan 9.A): a node the
    // merge has from theirs, not ours, mapped as a feature's references
    // are. Only references: a drawing's other strings are its own keys
    // (`s1`, `v2`) and part IDs, which a feature's renumbering never names.
    // Every part's drawings: a view names faces of its source part (a
    // base view's `of`, else the drawing's own part), so only views whose
    // source is the renumbered part are mapped.
    if let Rename::Feature { .. } = &r {
        let sub = format!("/{}/", crate::doc::drawing::DRAWINGS);
        for (p, b) in working {
            let Some(owner) = p
                .strip_prefix("parts/")
                .and_then(|rest| rest.split_once(sub.as_str()))
                .map(|(o, _)| o)
            else {
                continue;
            };
            let (Ok(v), Some(t), o) = (
                serde_json::from_slice::<Value>(b),
                json_of(theirs, p),
                json_of(ours, p),
            ) else {
                continue;
            };
            // An annotation's entities (plan 9.C) are text, and so are a
            // balloon's occurrence and a note's words: only the pointers
            // the file's annotations name entities at are followed.
            let (entities, occurrences) = annotation_pointers(&v);
            let new = rewrite(&v, "", &mut |pt, n| {
                if let Some(rest) = pt.strip_prefix("/annotations/") {
                    let entity = entities.contains(pt);
                    if !entity && !occurrences.contains(pt) {
                        return None;
                    }
                    let text = n.as_str()?;
                    let view = rest
                        .split('/')
                        .next()
                        .and_then(|k| v["annotations"][k]["view"].as_str())
                        .map(|w| format!("/views/{w}"))?;
                    let source = drawing_source(&v, &view, owner);
                    let mine = o.as_ref().and_then(|o| o.pointer(pt));
                    if t.pointer(pt) != Some(n) || mine == Some(n) {
                        return None;
                    }
                    let Rename::Feature { from, to, .. } = &r else {
                        return None;
                    };
                    let out = if entity {
                        annotation_entity(theirs, &names, text, source, &part, from, to)?
                    } else if source == part {
                        renamed_path(text, from, to)?
                    } else {
                        return None;
                    };
                    let out = json!(out);
                    return (out != *n).then_some(out);
                }
                if !is_reference(n) || drawing_source(&v, pt, owner) != part {
                    return None;
                }
                let mine = o.as_ref().and_then(|o| o.pointer(pt));
                if t.pointer(pt) != Some(n) || mine == Some(n) {
                    return None;
                }
                transform(&r, &names, &v, "", pt, n)
            });
            if new != v {
                files.insert(p.clone(), bytes(&new));
            }
        }
    }
    // Explode steps theirs wrote (plan 10.D): the assembly's instance
    // paths through a renumbered instance or pattern. A file's steps are
    // one value, as the merge takes them.
    if let Rename::Feature { from, to, .. } = &r {
        let dir = format!("parts/{part}/{}/", crate::doc::explode::EXPLODES);
        for (p, b) in working {
            if !p.starts_with(&dir) {
                continue;
            }
            let (Ok(v), Some(t), o) = (
                serde_json::from_slice::<Value>(b),
                json_of(theirs, p),
                json_of(ours, p),
            ) else {
                continue;
            };
            if v["steps"] != t["steps"] || o.as_ref().is_some_and(|o| o["steps"] == v["steps"]) {
                continue;
            }
            let mut new = v.clone();
            for step in new["steps"].as_array_mut().into_iter().flatten() {
                for i in step["instances"].as_array_mut().into_iter().flatten() {
                    if let Some(n) = i.as_str().and_then(|k| renamed_path(k, from, to)) {
                        *i = json!(n);
                    }
                }
            }
            if new != v {
                files.insert(p.clone(), bytes(&new));
            }
        }
    }
    // Motions theirs wrote (plan 26.B): the instances and the mate
    // their drives name, through a renumbered feature. A file's drives
    // are one value, as the merge takes them.
    if let Rename::Feature { from, to, .. } = &r {
        let dir = format!("parts/{part}/{}/", crate::doc::motion::MOTIONS);
        for (p, b) in working {
            if !p.starts_with(&dir) {
                continue;
            }
            let (Ok(v), Some(t), o) = (
                serde_json::from_slice::<Value>(b),
                json_of(theirs, p),
                json_of(ours, p),
            ) else {
                continue;
            };
            if v["drives"] != t["drives"] || o.as_ref().is_some_and(|o| o["drives"] == v["drives"])
            {
                continue;
            }
            let mut new = v.clone();
            for d in new["drives"].as_array_mut().into_iter().flatten() {
                if d.get("mate").and_then(Value::as_str) == Some(from.as_str()) {
                    d["mate"] = json!(to);
                }
                if let Some(list) = d.get_mut("instances").and_then(Value::as_array_mut) {
                    for i in list {
                        if i.as_str() == Some(from.as_str()) {
                            *i = json!(to);
                        }
                    }
                }
            }
            if new != v {
                files.insert(p.clone(), bytes(&new));
            }
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
    let change = |path: &String, content: Option<&crate::activity::Bytes>| crate::doctor::Change {
        path: path.clone(),
        content: content.map(|b| b.to_vec()),
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

    #[test]
    fn drafts_follow_their_target_faces_parting_line_and_flips() {
        let old = |k: u8| format!("{k:032x}");
        let new = |k: u8| format!("{:032x}", k + 100);
        let r = |k: u8| json!({"feature": "b", "name": old(k)});
        let d = json!({"format": crate::doc::tweak::DRAFT_FORMAT, "kind": "ryucad.feature",
            "type": "draft", "target": "b", "angle": "2 deg", "parting": [r(3)], "pull": "z",
            "faces": [r(1)], "groups": [{"angle": "5 deg", "faces": [r(2)]}], "flip": [r(2)]});
        let n = json!({"format": crate::doc::tweak::DRAFT_FORMAT, "kind": "ryucad.feature",
            "type": "draft", "target": "b", "angle": "2 deg", "neutral": r(4), "faces": [r(1)]});
        let f = files(&[
            ("parts/a/features/d.json", d),
            ("parts/a/features/n.json", n),
        ]);
        let rename = Rename::Feature {
            part: "a".into(),
            from: "b".into(),
            to: "b-1".into(),
        };
        let names = BTreeMap::from([(
            "b".to_owned(),
            (1..=4)
                .map(|k| (old(k), new(k)))
                .collect::<BTreeMap<_, _>>(),
        )]);
        let out = renumbered(&f, &rename, &names);
        let read = |p: &str| serde_json::from_slice::<Value>(&out[p]).unwrap();
        let to = |k: u8| json!({"feature": "b-1", "name": new(k)});
        let d = read("parts/a/features/d.json");
        assert_eq!(
            (&d["target"], &d["faces"][0], &d["parting"][0]),
            (&json!("b-1"), &to(1), &to(3))
        );
        assert_eq!(
            (&d["groups"][0]["faces"][0], &d["flip"][0]),
            (&to(2), &to(2))
        );
        assert_eq!(read("parts/a/features/n.json")["neutral"], to(4));
    }

    #[test]
    fn only_annotations_entities_and_occurrences_are_renumbered() {
        let h = |k: u8| format!("{k:032x}");
        let v = json!({"format": 1, "kind": "ryucad.drawing", "name": "D",
        "sheets": {"s1": {"name": "Sheet 1", "size": "iso_a3"}},
        "views": {"v1": {"type": "base", "sheet": "s1", "orient": "front"}},
        "annotations": {
            "d1": {"type": "linear", "view": "v1", "a": format!("b/{}", h(1)), "b": "q",
                   "along": "horizontal", "prefix": "b/x"},
            "n1": {"type": "note", "view": "v1", "text": "b/x"},
            "b1": {"type": "balloon", "view": "v1", "of": "b"}
        }});
        let (entities, occurrences) = annotation_pointers(&v);
        assert!(entities.contains("/annotations/d1/a") && entities.contains("/annotations/d1/b"));
        assert!(!entities.contains("/annotations/d1/prefix"));
        assert!(!entities.contains("/annotations/n1/text"));
        assert!(occurrences.contains("/annotations/b1/of"));
        let names = BTreeMap::from([("b".to_owned(), BTreeMap::from([(h(1), h(101))]))]);
        let none = files(&[]);
        let e =
            |t: &str, source: &str| annotation_entity(&none, &names, t, source, "p", "b", "b-1");
        // A solid's face, a datum, and nothing of another part.
        assert_eq!(
            e(&format!("b/{}", h(1)), "p").unwrap(),
            format!("b-1/{}", h(101))
        );
        assert_eq!(e("b", "p").unwrap(), "b-1");
        assert!(e("q", "p").is_none());
        assert_eq!(
            e(&format!("b/{}", h(1)), "other").unwrap(),
            format!("b/{}", h(1))
        );
        // An occurrence's entity in an assembly drawing: the instance
        // renamed, and the solid left to the part it places.
        assert_eq!(
            e(&format!("b/s/{}", h(1)), "p").unwrap(),
            format!("b-1/s/{}", h(1))
        );
    }

    #[test]
    fn a_drawings_node_names_faces_of_its_views_source() {
        let v = json!({"views": {
            "v1": {"type": "base", "of": "q"},
            "v2": {"type": "projected", "from": "v1"},
            "v3": {"type": "base"}
        }});
        assert_eq!(drawing_source(&v, "/views/v2/normal_to", "p"), "q");
        assert_eq!(drawing_source(&v, "/views/v1/orient/normal_to", "p"), "q");
        assert_eq!(drawing_source(&v, "/views/v3/orient/normal_to", "p"), "p");
        assert_eq!(drawing_source(&v, "/sheets/s1", "p"), "p");
    }

    #[test]
    fn a_delete_follows_its_target_faces_and_seeds() {
        let old = |k: u8| format!("{k:032x}");
        let new = |k: u8| format!("{:032x}", k + 100);
        let r = |k: u8| json!({"feature": "b", "name": old(k)});
        let del = json!({"format": crate::doc::edit::FORMAT, "kind": "ryucad.feature",
            "type": "delete_face", "target": "b", "faces": [r(1), r(2)], "seeds": [r(3)],
            "heal": "cap"});
        let f = files(&[("parts/a/features/d.json", del)]);
        let rename = Rename::Feature {
            part: "a".into(),
            from: "b".into(),
            to: "b-1".into(),
        };
        let names = BTreeMap::from([(
            "b".to_owned(),
            (1..=3)
                .map(|k| (old(k), new(k)))
                .collect::<BTreeMap<_, _>>(),
        )]);
        let out = renumbered(&f, &rename, &names);
        let d = serde_json::from_slice::<Value>(&out["parts/a/features/d.json"]).unwrap();
        let to = |k: u8| json!({"feature": "b-1", "name": new(k)});
        assert_eq!(
            (&d["target"], &d["faces"], &d["seeds"], &d["heal"]),
            (
                &json!("b-1"),
                &json!([to(1), to(2)]),
                &json!([to(3)]),
                &json!("cap")
            )
        );
    }

    #[test]
    fn tweaks_follow_their_target_faces_and_axis() {
        let old = |k: u8| format!("{k:032x}");
        let new = |k: u8| format!("{:032x}", k + 100);
        let r = |k: u8| json!({"feature": "b", "name": old(k)});
        let mv = json!({"format": crate::doc::tweak::FORMAT, "kind": "ryucad.feature",
            "type": "move_face", "target": "b", "by": "rotate", "axis": r(3), "angle": "3 deg",
            "faces": [r(1)]});
        let off = json!({"format": crate::doc::tweak::FORMAT, "kind": "ryucad.feature",
            "type": "move_face", "target": "b", "by": "offset", "distance": "0 mm",
            "groups": [{"distance": "1 mm", "faces": [r(2)]}]});
        let rp = json!({"format": crate::doc::tweak::FORMAT, "kind": "ryucad.feature",
            "type": "replace_face", "target": "b", "groups": [{"with": r(2), "faces": [r(1)]}]});
        let f = files(&[
            ("parts/a/features/m.json", mv),
            ("parts/a/features/o.json", off),
            ("parts/a/features/r.json", rp),
        ]);
        let rename = Rename::Feature {
            part: "a".into(),
            from: "b".into(),
            to: "b-1".into(),
        };
        let names = BTreeMap::from([(
            "b".to_owned(),
            (1..=3)
                .map(|k| (old(k), new(k)))
                .collect::<BTreeMap<_, _>>(),
        )]);
        let out = renumbered(&f, &rename, &names);
        let read = |p: &str| serde_json::from_slice::<Value>(&out[p]).unwrap();
        let to = |k: u8| json!({"feature": "b-1", "name": new(k)});
        let m = read("parts/a/features/m.json");
        assert_eq!(
            (&m["target"], &m["faces"][0], &m["axis"]),
            (&json!("b-1"), &to(1), &to(3))
        );
        let o = read("parts/a/features/o.json");
        assert_eq!(o["groups"][0]["faces"][0], to(2));
        let x = read("parts/a/features/r.json");
        assert_eq!(
            (&x["groups"][0]["with"], &x["groups"][0]["faces"][0]),
            (&to(2), &to(1))
        );
    }

    #[test]
    fn shells_and_offsets_follow_their_target() {
        let old = "0123456789abcdef0123456789abcdef";
        let new = "fedcba9876543210fedcba9876543210";
        let shell = json!({
            "format": 9, "kind": "ryucad.feature", "type": "shell", "target": "b",
            "thickness": "2 mm", "open": [{"feature": "b", "name": old}],
        });
        let offset = json!({
            "format": 9, "kind": "ryucad.feature", "type": "offset", "target": "b",
            "distance": "0 mm",
            "faces": [{"distance": "1 mm", "faces": [{"feature": "b", "name": old}]}],
        });
        let sew = json!({"format": crate::doc::surface::FORMAT, "kind": "ryucad.feature", "type": "sew",
            "sheets": ["b", "c"], "tolerance": "0.01 mm", "solid": true});
        let th = json!({"format": crate::doc::surface::FORMAT, "kind": "ryucad.feature", "type": "thicken",
            "from": {"sheet": "b"}, "front": "1 mm", "back": "0 mm", "operation": "new"});
        let thf = json!({"format": crate::doc::surface::FORMAT, "kind": "ryucad.feature", "type": "thicken",
            "from": {"faces": [{"feature": "b", "name": old}]}, "front": "1 mm", "back": "0 mm",
            "operation": "new"});
        let fl = json!({"format": crate::doc::surface::FORMAT, "kind": "ryucad.feature", "type": "fill",
            "sides": [{"edge": {"feature": "b", "name": old}}, {"curve": "x"}, {"curve": "y"}],
            "tolerance": "0.001 mm", "angle": "1 deg"});
        let f = files(&[
            ("parts/a/features/sh.json", shell),
            ("parts/a/features/o.json", offset),
            ("parts/a/features/sw.json", sew),
            ("parts/a/features/th.json", th),
            ("parts/a/features/thf.json", thf),
            ("parts/a/features/fl.json", fl),
        ]);
        let r = Rename::Feature {
            part: "a".into(),
            from: "b".into(),
            to: "b-1".into(),
        };
        let names = BTreeMap::from([(
            "b".to_owned(),
            BTreeMap::from([(old.to_owned(), new.to_owned())]),
        )]);
        let out = renumbered(&f, &r, &names);
        let read = |p: &str| serde_json::from_slice::<Value>(&out[p]).unwrap();
        let sh = read("parts/a/features/sh.json");
        assert_eq!(sh["target"], "b-1");
        assert_eq!(sh["open"][0], json!({"feature": "b-1", "name": new}));
        let o = read("parts/a/features/o.json");
        assert_eq!(o["target"], "b-1");
        assert_eq!(
            o["faces"][0]["faces"][0],
            json!({"feature": "b-1", "name": new})
        );
        let sw = read("parts/a/features/sw.json");
        assert_eq!(sw["sheets"], json!(["b-1", "c"]));
        let th = read("parts/a/features/th.json");
        assert_eq!(th["from"], json!({"sheet": "b-1"}));
        let thf = read("parts/a/features/thf.json");
        assert_eq!(
            thf["from"]["faces"][0],
            json!({"feature": "b-1", "name": new})
        );
        let fl = read("parts/a/features/fl.json");
        assert_eq!(
            fl["sides"][0]["edge"],
            json!({"feature": "b-1", "name": new})
        );
    }

    #[test]
    fn base_flanges_and_flanges_follow_their_sketch_and_solid() {
        let old = |k: u8| format!("{k:032x}");
        let new = |k: u8| format!("{:032x}", k + 100);
        let base = json!({
            "format": 11, "kind": "ryucad.feature", "type": "base_flange", "sketch": "s",
            "thickness": "1.5 mm", "radius": "1 mm", "allowance": {"k_factor": "0.44"},
        });
        let r = |k: u8| json!({"feature": "plate", "name": old(k)});
        let flange = json!({
            "format": 11, "kind": "ryucad.feature", "type": "flange", "target": "plate",
            "edges": [r(1), {"edge": r(2), "side": r(3)}], "angle": "90 deg", "length": "20 mm",
            "ends": [{"vertex": r(4), "setback": "5 mm"}],
        });
        let f = files(&[
            ("parts/a/features/plate.json", base),
            ("parts/a/features/f.json", flange),
        ]);
        let none = BTreeMap::new();
        let out = renumbered(
            &f,
            &Rename::Feature {
                part: "a".into(),
                from: "s".into(),
                to: "s-1".into(),
            },
            &none,
        );
        let read = |out: &Files, p: &str| serde_json::from_slice::<Value>(&out[p]).unwrap();
        assert_eq!(read(&out, "parts/a/features/plate.json")["sketch"], "s-1");
        let names = BTreeMap::from([(
            "plate".to_owned(),
            (1..=4)
                .map(|k| (old(k), new(k)))
                .collect::<BTreeMap<_, _>>(),
        )]);
        let out = renumbered(
            &f,
            &Rename::Feature {
                part: "a".into(),
                from: "plate".into(),
                to: "plate-1".into(),
            },
            &names,
        );
        let x = read(&out, "parts/a/features/f.json");
        let to = |k: u8| json!({"feature": "plate-1", "name": new(k)});
        assert_eq!(x["target"], "plate-1");
        assert_eq!(x["edges"][0], to(1));
        assert_eq!(x["edges"][1], json!({"edge": to(2), "side": to(3)}));
        assert_eq!(x["ends"][0]["vertex"], to(4));
    }

    #[test]
    fn unfolds_and_refolds_follow_their_solid_their_unfold_and_their_bends() {
        let old = |k: u8| format!("{k:032x}");
        let new = |k: u8| format!("{:032x}", k + 100);
        let r = |f: &str, k: u8| json!({"feature": f, "name": old(k)});
        let unfold = json!({
            "format": 14, "kind": "ryucad.feature", "type": "unfold", "target": "plate",
            "fixed": r("plate", 1), "only": [r("f", 2)],
            "bends": [{"bend": r("f", 2), "deduction": "2 mm"}],
        });
        let refold = json!({
            "format": 14, "kind": "ryucad.feature", "type": "refold", "target": "plate",
            "unfold": "u", "only": [r("f", 2)],
        });
        let f = files(&[
            ("parts/a/features/u.json", unfold),
            ("parts/a/features/rf.json", refold),
        ]);
        let read = |out: &Files, p: &str| serde_json::from_slice::<Value>(&out[p]).unwrap();
        // The unfold renumbered: the refold follows it.
        let out = renumbered(
            &f,
            &Rename::Feature {
                part: "a".into(),
                from: "u".into(),
                to: "u-1".into(),
            },
            &BTreeMap::new(),
        );
        assert_eq!(read(&out, "parts/a/features/rf.json")["unfold"], "u-1");
        // The solid and the flange renumbered, names mapped: both follow.
        let names = BTreeMap::from([
            ("plate".to_owned(), BTreeMap::from([(old(1), new(1))])),
            ("f".to_owned(), BTreeMap::from([(old(2), new(2))])),
        ]);
        let out = renumbered(
            &f,
            &Rename::Feature {
                part: "a".into(),
                from: "plate".into(),
                to: "plate-1".into(),
            },
            &names,
        );
        let u = read(&out, "parts/a/features/u.json");
        assert_eq!(u["target"], "plate-1");
        assert_eq!(u["fixed"], json!({"feature": "plate-1", "name": new(1)}));
        assert_eq!(u["only"][0], json!({"feature": "f", "name": new(2)}));
        assert_eq!(
            u["bends"][0]["bend"],
            json!({"feature": "f", "name": new(2)})
        );
        let rf = read(&out, "parts/a/features/rf.json");
        assert_eq!(rf["target"], "plate-1");
        assert_eq!(rf["only"][0], json!({"feature": "f", "name": new(2)}));
    }

    #[test]
    fn mirrors_and_splits_follow_their_solids_and_planes() {
        let old = "0123456789abcdef0123456789abcdef";
        let new = "fedcba9876543210fedcba9876543210";
        let mirror = json!({
            "format": 10, "kind": "ryucad.feature", "type": "mirror", "solid": "b",
            "plane": {"feature": "b", "name": old}, "operation": "join", "targets": ["b"],
        });
        let split = json!({
            "format": 10, "kind": "ryucad.feature", "type": "split", "target": "b",
            "plane": {"feature": "d"},
        });
        // What reads the side a renumbered split founds.
        let on = json!({
            "format": 3, "kind": "ryucad.feature", "type": "box", "operation": "join",
            "targets": ["s"], "params": {"x": "1 mm", "y": "1 mm", "z": "1 mm"},
            "placement": {"origin": ["0 mm", "0 mm", "0 mm"], "x": [1, 0, 0], "z": [0, 0, 1]},
        });
        let f = files(&[
            ("parts/a/features/m.json", mirror),
            ("parts/a/features/s.json", split),
            ("parts/a/features/on.json", on),
        ]);
        let names = BTreeMap::from([(
            "b".to_owned(),
            BTreeMap::from([(old.to_owned(), new.to_owned())]),
        )]);
        let out = renumbered(
            &f,
            &Rename::Feature {
                part: "a".into(),
                from: "b".into(),
                to: "b-1".into(),
            },
            &names,
        );
        let read = |out: &Files, p: &str| serde_json::from_slice::<Value>(&out[p]).unwrap();
        let m = read(&out, "parts/a/features/m.json");
        assert_eq!(
            (&m["solid"], &m["targets"]),
            (&json!("b-1"), &json!(["b-1"]))
        );
        assert_eq!(m["plane"], json!({"feature": "b-1", "name": new}));
        assert_eq!(read(&out, "parts/a/features/s.json")["target"], "b-1");
        let out = renumbered(
            &f,
            &Rename::Feature {
                part: "a".into(),
                from: "s".into(),
                to: "s-1".into(),
            },
            &BTreeMap::new(),
        );
        assert!(out.contains_key("parts/a/features/s-1.json"));
        assert_eq!(
            read(&out, "parts/a/features/on.json")["targets"],
            json!(["s-1"])
        );
    }

    #[test]
    fn a_splits_tool_follows_its_body_and_faces() {
        let old = "0123456789abcdef0123456789abcdef";
        let new = "fedcba9876543210fedcba9876543210";
        let by_body = json!({
            "format": 19, "kind": "ryucad.feature", "type": "split", "target": "h",
            "tool": {"body": "parting"},
        });
        let by_faces = json!({
            "format": 19, "kind": "ryucad.feature", "type": "split", "target": "h",
            "tool": {"faces": [{"feature": "parting", "name": old}]},
        });
        let f = files(&[
            ("parts/a/features/s.json", by_body),
            ("parts/a/features/t.json", by_faces),
        ]);
        let names = BTreeMap::from([(
            "parting".to_owned(),
            BTreeMap::from([(old.to_owned(), new.to_owned())]),
        )]);
        let out = renumbered(
            &f,
            &Rename::Feature {
                part: "a".into(),
                from: "parting".into(),
                to: "parting-2".into(),
            },
            &names,
        );
        let read = |p: &str| serde_json::from_slice::<Value>(&out[p]).unwrap();
        assert_eq!(read("parts/a/features/s.json")["tool"]["body"], "parting-2");
        assert_eq!(
            read("parts/a/features/t.json")["tool"]["faces"][0],
            json!({"feature": "parting-2", "name": new})
        );
    }

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
    fn a_following_patterns_path_renumbers_and_its_feature_never() {
        // Theirs' `holes` (an instance here) collided and is `holes-1`:
        // `on`'s path follows; its feature is the lid part's `holes`, not
        // this assembly's, and stays.
        let screws = json!({
            "format": 25, "kind": "ryucad.feature", "type": "instance_pattern",
            "form": "feature", "instances": ["s0"],
            "on": {"instance": ["holes"], "feature": "holes"},
        });
        let mate = json!({
            "format": 4, "kind": "ryucad.feature", "type": "mate", "form": "fastened",
            "a": {"instance": ["screws:p7", "s0"], "at": "origin"},
            "b": {"instance": ["base"], "at": "origin"},
        });
        let f = files(&[
            ("parts/a/features/screws.json", screws),
            ("parts/a/features/m1.json", mate),
        ]);
        let r = Rename::Feature {
            part: "a".into(),
            from: "holes".into(),
            to: "holes-1".into(),
        };
        let out = renumbered(&f, &r, &BTreeMap::new());
        let read = |o: &Files, p: &str| serde_json::from_slice::<Value>(&o[p]).unwrap();
        assert_eq!(
            read(&out, "parts/a/features/screws.json")["on"],
            json!({"instance": ["holes-1"], "feature": "holes"})
        );
        // A copy step keyed by a point renumbers its pattern only.
        let r = Rename::Feature {
            part: "a".into(),
            from: "screws".into(),
            to: "screws-1".into(),
        };
        let out = renumbered(&f, &r, &BTreeMap::new());
        assert_eq!(
            read(&out, "parts/a/features/m1.json")["a"]["instance"],
            json!(["screws-1:p7", "s0"])
        );
    }

    #[test]
    fn a_planar_follows_its_sketch_and_loops() {
        let planar = json!({
            "format": 12, "kind": "ryucad.feature", "type": "planar",
            "profile": {"sketch": "s", "loops": ["l1"]},
        });
        let f = files(&[("parts/a/features/f.json", planar)]);
        let read =
            |o: &Files| serde_json::from_slice::<Value>(&o["parts/a/features/f.json"]).unwrap();
        let r = Rename::Feature {
            part: "a".into(),
            from: "s".into(),
            to: "s-2".into(),
        };
        assert_eq!(
            read(&renumbered(&f, &r, &BTreeMap::new()))["profile"]["sketch"],
            "s-2"
        );
        let r = Rename::Entity {
            part: "a".into(),
            sketch: "s".into(),
            from: "l1".into(),
            to: "l9".into(),
        };
        assert_eq!(
            read(&renumbered(&f, &r, &BTreeMap::new()))["profile"]["loops"],
            json!(["l9"])
        );
    }

    #[test]
    fn sweeps_follow_what_they_read() {
        let old = "0123456789abcdef0123456789abcdef";
        let new = "fedcba9876543210fedcba9876543210";
        let sweep = json!({
            "format": 8, "kind": "ryucad.feature", "type": "sweep",
            "profile": {"sketch": "s", "loops": ["l1"]},
            "path": {"feature": "s", "entity": "l2"},
            "law": "binormal", "direction": {"feature": "h1"},
        });
        let face = json!({
            "format": 8, "kind": "ryucad.feature", "type": "sweep",
            "profile": {"face": {"feature": "b", "name": old}, "loops": [old]},
            "path": {"feature": "b", "name": old}, "law": "minimal",
        });
        let f = files(&[
            ("parts/a/features/w.json", sweep),
            ("parts/a/features/v.json", face),
        ]);
        let read = |o: &Files, k: &str| {
            serde_json::from_slice::<Value>(&o[&format!("parts/a/features/{k}.json")]).unwrap()
        };
        // Theirs' sketch is `s-1` now: the profile, and the path on it,
        // follow.
        let r = Rename::Feature {
            part: "a".into(),
            from: "s".into(),
            to: "s-1".into(),
        };
        let out = renumbered(&f, &r, &BTreeMap::new());
        let w = read(&out, "w");
        assert_eq!(w["profile"]["sketch"], "s-1");
        assert_eq!(w["path"], json!({"feature": "s-1", "entity": "l2"}));
        assert_eq!(w["direction"], json!({"feature": "h1"}));
        // A sketch's entity renumbered: the loop it names follows.
        let r = Rename::Entity {
            part: "a".into(),
            sketch: "s".into(),
            from: "l1".into(),
            to: "l9".into(),
        };
        let out = renumbered(&f, &r, &BTreeMap::new());
        assert_eq!(read(&out, "w")["profile"]["loops"], json!(["l9"]));
        // The solid renumbered, its names mapped: the face, its loops and
        // the edge path follow.
        let r = Rename::Feature {
            part: "a".into(),
            from: "b".into(),
            to: "b-1".into(),
        };
        let names = BTreeMap::from([(
            "b".to_owned(),
            BTreeMap::from([(old.to_owned(), new.to_owned())]),
        )]);
        let out = renumbered(&f, &r, &names);
        let v = read(&out, "v");
        assert_eq!(v["profile"]["face"], json!({"feature": "b-1", "name": new}));
        assert_eq!(v["profile"]["loops"], json!([new]));
        assert_eq!(v["path"], json!({"feature": "b-1", "name": new}));
    }

    #[test]
    fn lofts_follow_what_they_read() {
        let old = "0123456789abcdef0123456789abcdef";
        let new = "fedcba9876543210fedcba9876543210";
        let loft = json!({
            "format": 8, "kind": "ryucad.feature", "type": "loft",
            "sections": [{"sketch": "s", "loops": ["p1"]}, {"sketch": "t"},
                         {"face": {"feature": "b", "name": old}, "loops": [old]}],
            "start_tangent": {"direction": {"feature": "s", "entity": "l2"}},
        });
        let f = files(&[("parts/a/features/l.json", loft)]);
        let read =
            |o: &Files| serde_json::from_slice::<Value>(&o["parts/a/features/l.json"]).unwrap();
        // Theirs' sketch is `s-1` now: its section, and the tangent on it,
        // follow; the other sketch stays.
        let r = Rename::Feature {
            part: "a".into(),
            from: "s".into(),
            to: "s-1".into(),
        };
        let l = read(&renumbered(&f, &r, &BTreeMap::new()));
        assert_eq!(l["sections"][0]["sketch"], "s-1");
        assert_eq!(l["sections"][1]["sketch"], "t");
        assert_eq!(
            l["start_tangent"]["direction"],
            json!({"feature": "s-1", "entity": "l2"})
        );
        // A point renumbered: the loop entry naming it follows.
        let r = Rename::Entity {
            part: "a".into(),
            sketch: "s".into(),
            from: "p1".into(),
            to: "p9".into(),
        };
        let l = read(&renumbered(&f, &r, &BTreeMap::new()));
        assert_eq!(l["sections"][0]["loops"], json!(["p9"]));
        assert_eq!(l["sections"][2]["loops"], json!([old]));
        // The solid renumbered, its names mapped: the face and its loops
        // follow.
        let r = Rename::Feature {
            part: "a".into(),
            from: "b".into(),
            to: "b-1".into(),
        };
        let names = BTreeMap::from([(
            "b".to_owned(),
            BTreeMap::from([(old.to_owned(), new.to_owned())]),
        )]);
        let l = read(&renumbered(&f, &r, &names));
        assert_eq!(
            l["sections"][2]["face"],
            json!({"feature": "b-1", "name": new})
        );
        assert_eq!(l["sections"][2]["loops"], json!([new]));
    }

    #[test]
    fn paths_follow_what_they_read() {
        let old = "0123456789abcdef0123456789abcdef";
        let new = "fedcba9876543210fedcba9876543210";
        let path = json!({
            "format": 7, "kind": "ryucad.feature", "type": "path",
            "segments": [{"feature": "b", "name": old, "propagate": true},
                         {"feature": "s", "entity": "l1"}, {"feature": "h1"}],
        });
        let f = files(&[("parts/a/features/w.json", path)]);
        let read =
            |o: &Files| serde_json::from_slice::<Value>(&o["parts/a/features/w.json"]).unwrap();
        // Theirs' sketch is `s-1` now: the path's curve on it follows.
        let r = Rename::Feature {
            part: "a".into(),
            from: "s".into(),
            to: "s-1".into(),
        };
        let out = renumbered(&f, &r, &BTreeMap::new());
        assert_eq!(read(&out)["segments"][1]["feature"], "s-1");
        // The solid renumbered, its names mapped: the propagated edge
        // keeps saying so.
        let r = Rename::Feature {
            part: "a".into(),
            from: "b".into(),
            to: "b-1".into(),
        };
        let names = BTreeMap::from([(
            "b".to_owned(),
            BTreeMap::from([(old.to_owned(), new.to_owned())]),
        )]);
        let out = renumbered(&f, &r, &names);
        assert_eq!(
            read(&out)["segments"][0],
            json!({"feature": "b-1", "name": new, "propagate": true})
        );
        assert_eq!(read(&out)["segments"][2], json!({"feature": "h1"}));
    }

    #[test]
    fn datums_follow_what_they_read() {
        // Theirs' sketch `s` collided and is `s-1` now, then its point p3
        // is p4: a plane through them and a point on it follow.
        let plane = json!({
            "format": 7, "kind": "ryucad.feature", "type": "plane", "by": "three_points",
            "points": [{"feature": "s", "entity": "p3"}, {"feature": "q1"}, "origin"],
        });
        let point = json!({
            "format": 7, "kind": "ryucad.feature", "type": "point", "by": "of",
            "point": {"feature": "s", "entity": "p1"},
        });
        let f = files(&[
            ("parts/a/features/d1.json", plane),
            ("parts/a/features/q2.json", point),
        ]);
        let r = Rename::Feature {
            part: "a".into(),
            from: "s".into(),
            to: "s-1".into(),
        };
        let out = renumbered(&f, &r, &BTreeMap::new());
        let read = |o: &Files, p: &str| serde_json::from_slice::<Value>(&o[p]).unwrap();
        let d = read(&out, "parts/a/features/d1.json");
        assert_eq!(d["points"][0], json!({"feature": "s-1", "entity": "p3"}));
        assert_eq!(d["points"][1], json!({"feature": "q1"}));
        assert_eq!(
            read(&out, "parts/a/features/q2.json")["point"]["feature"],
            "s-1"
        );
        let r = Rename::Entity {
            part: "a".into(),
            sketch: "s".into(),
            from: "p3".into(),
            to: "p4".into(),
        };
        let out = renumbered(&f, &r, &BTreeMap::new());
        let d = read(&out, "parts/a/features/d1.json");
        assert_eq!(d["points"][0], json!({"feature": "s", "entity": "p4"}));
        // A point that names another entity stays.
        assert!(
            !out.contains_key("parts/a/features/q2.json")
                || read(&out, "parts/a/features/q2.json")["point"]["entity"] == "p1"
        );
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

    #[test]
    fn blend_edits_follow_their_target_faces_and_chamfer_faces() {
        let old = |k: u8| format!("{k:032x}");
        let new = |k: u8| format!("{:032x}", k + 100);
        let r = |k: u8| json!({"feature": "b", "name": old(k)});
        let del = json!({"format": crate::doc::edit::FORMAT, "kind": "ryucad.feature",
            "type": "delete_blend", "target": "b", "below": "2 mm", "faces": [r(1)]});
        let rz = json!({"format": crate::doc::edit::FORMAT, "kind": "ryucad.feature",
            "type": "resize_blend", "target": "b", "groups": [
                {"form": "distances", "d1": "1 mm", "d2": "2 mm", "face": r(2), "faces": [r(3)]}]});
        let f = files(&[
            ("parts/a/features/d.json", del),
            ("parts/a/features/z.json", rz),
        ]);
        let rename = Rename::Feature {
            part: "a".into(),
            from: "b".into(),
            to: "b-1".into(),
        };
        let names = BTreeMap::from([(
            "b".to_owned(),
            (1..=3)
                .map(|k| (old(k), new(k)))
                .collect::<BTreeMap<_, _>>(),
        )]);
        let out = renumbered(&f, &rename, &names);
        let to = |k: u8| json!({"feature": "b-1", "name": new(k)});
        let d = serde_json::from_slice::<Value>(&out["parts/a/features/d.json"]).unwrap();
        assert_eq!(
            (&d["target"], &d["faces"]),
            (&json!("b-1"), &json!([to(1)]))
        );
        let z = serde_json::from_slice::<Value>(&out["parts/a/features/z.json"]).unwrap();
        assert_eq!(
            (
                &z["target"],
                &z["groups"][0]["face"],
                &z["groups"][0]["faces"]
            ),
            (&json!("b-1"), &to(2), &json!([to(3)]))
        );
    }

    #[test]
    fn an_inserts_keys_and_references_follow_it_when_it_is_renumbered() {
        let n = "0123456789abcdef0123456789abcdef";
        let head = |mut v: Value| {
            v["format"] = json!(21);
            v["kind"] = json!("ryucad.feature");
            v
        };
        let f = files(&[
            (
                "parts/enc/features/cut.json",
                head(
                    json!({"type": "boolean", "op": "subtract", "target": "shell",
                            "tools": ["board/u1"]}),
                ),
            ),
            (
                "parts/enc/features/del.json",
                head(json!({"type": "delete_body", "solids": ["board", "board/u2"]})),
            ),
            (
                "parts/enc/features/fil.json",
                head(json!({"type": "fillet",
                            "groups": [{"radius": "1 mm",
                                        "edges": [{"feature": "board", "solid": "u1", "name": n}]}]})),
            ),
            (
                "parts/enc/features/m.json",
                head(json!({"type": "mirror", "solid": "board/u1", "plane": "xy"})),
            ),
        ]);
        let out = renumbered(
            &f,
            &Rename::Feature {
                part: "enc".into(),
                from: "board".into(),
                to: "board-2".into(),
            },
            &BTreeMap::new(),
        );
        let read = |p: &str| serde_json::from_slice::<Value>(&out[p]).unwrap();
        assert_eq!(
            read("parts/enc/features/cut.json")["tools"],
            json!(["board-2/u1"])
        );
        assert_eq!(
            read("parts/enc/features/del.json")["solids"],
            json!(["board-2", "board-2/u2"])
        );
        assert_eq!(
            read("parts/enc/features/fil.json")["groups"][0]["edges"][0],
            json!({"feature": "board-2", "solid": "u1", "name": n})
        );
        assert_eq!(read("parts/enc/features/m.json")["solid"], "board-2/u1");
    }

    #[test]
    fn a_cosmetic_thread_follows_its_faces_and_start() {
        let old = |k: u8| format!("{k:032x}");
        let new = |k: u8| format!("{:032x}", k + 100);
        let r = |k: u8| json!({"feature": "shaft", "name": old(k)});
        let t = json!({"format": 25, "kind": "ryucad.feature", "type": "cosmetic_thread",
            "faces": [r(1)], "from": r(2), "depth": "12 mm",
            "thread": {"standard": "iso_metric", "size": "M10"}});
        let f = files(&[("parts/a/features/t.json", t)]);
        let rename = Rename::Feature {
            part: "a".into(),
            from: "shaft".into(),
            to: "shaft2".into(),
        };
        let names = BTreeMap::from([(
            "shaft".to_owned(),
            (1..=2)
                .map(|k| (old(k), new(k)))
                .collect::<BTreeMap<_, _>>(),
        )]);
        let out = renumbered(&f, &rename, &names);
        let d = serde_json::from_slice::<Value>(&out["parts/a/features/t.json"]).unwrap();
        let to = |k: u8| json!({"feature": "shaft2", "name": new(k)});
        assert_eq!((&d["faces"], &d["from"]), (&json!([to(1)]), &to(2)));
    }
}
