//! What changed between two versions of a project: [`textconv`] renders
//! one file as lines for Git's own diff to compare; [`structural`] compares
//! two versions' files as ryucad reads them, per part, feature, parameter
//! and sketch entity.

use crate::activity::Files;
use serde_json::{Map, Value, json};
use std::collections::{BTreeMap, BTreeSet};

/// A file as one line per value, in a fixed order, so that a line diff of
/// two renderings reads as the values that changed: `git diff` through
/// `ryucad git textconv`. A header names the file's kind (and a feature's
/// type) and format; a part lists its features one per line; a sketch gives
/// each entity and constraint a line, and its solved positions last, apart
/// from the intent. Anything that isn't JSON comes back as it is.
pub fn textconv(bytes: &[u8]) -> Vec<u8> {
    let Some(Value::Object(o)) = std::str::from_utf8(bytes)
        .ok()
        .and_then(|t| serde_json::from_str::<Value>(t).ok())
    else {
        return bytes.to_vec();
    };
    let kind = o.get("kind").and_then(Value::as_str);
    let mut header = kind.unwrap_or("json").to_owned();
    if let Some(t) = o.get("type").and_then(Value::as_str) {
        header = format!("{header} {t}");
    }
    if let Some(f) = o.get("format") {
        header = format!("{header} (format {f})");
    }
    let mut out = vec![header];
    let mut rest: Map<String, Value> = o
        .iter()
        .filter(|(k, _)| !matches!(k.as_str(), "kind" | "format" | "type"))
        .map(|(k, v)| (k.clone(), v.clone()))
        .collect();
    // Lines sorted together, and a sketch's positions after them.
    let mut lines = Vec::new();
    let mut positions = Vec::new();
    match kind {
        // A part's features keep their order.
        Some("ryucad.part") => {
            if let Some(Value::Array(list)) = rest.remove("features") {
                out.extend(list.iter().map(|f| format!("feature {}", scalar(f))));
            }
        }
        Some("ryucad.feature") if o.get("type").and_then(Value::as_str) == Some("sketch") => {
            if let Some(Value::Object(entities)) = rest.remove("entities") {
                for (id, e) in &entities {
                    lines.push(format!("entities.{id} = {}", entity(e)));
                    positions.extend(position(id, e));
                }
            }
            if let Some(Value::Object(constraints)) = rest.remove("constraints") {
                for (id, c) in &constraints {
                    lines.push(format!("constraints.{id} = {}", constraint(c)));
                }
            }
        }
        _ => {}
    }
    flatten("", &Value::Object(rest), &mut lines);
    lines.sort();
    out.extend(lines);
    if !positions.is_empty() {
        out.push(String::new());
        out.extend(positions);
    }
    let mut text = out.join("\n");
    text.push('\n');
    text.into_bytes()
}

/// A leaf value on its line: strings bare, everything else as JSON.
fn scalar(v: &Value) -> String {
    match v {
        Value::String(s) => s.clone(),
        v => v.to_string(),
    }
}

/// A reference to another feature's entity, as `<feature>/<name>`.
fn reference(v: &Value) -> Option<String> {
    let o = v.as_object()?;
    if o.len() != 2 {
        return None;
    }
    Some(format!(
        "{}/{}",
        o.get("feature")?.as_str()?,
        o.get("name")?.as_str()?
    ))
}

/// Every leaf of `v` as `path = value`, objects by key with `.`, arrays of
/// leaves on one line, other arrays by index.
fn flatten(path: &str, v: &Value, out: &mut Vec<String>) {
    let at = |key: &str| {
        if path.is_empty() {
            key.to_owned()
        } else {
            format!("{path}.{key}")
        }
    };
    if let Some(r) = reference(v) {
        out.push(format!("{path} = {r}"));
        return;
    }
    match v {
        Value::Object(o) if o.is_empty() && !path.is_empty() => out.push(format!("{path} = {{}}")),
        Value::Object(o) => {
            for (k, x) in o {
                flatten(&at(k), x, out);
            }
        }
        Value::Array(a) if a.iter().all(|x| !x.is_object() && !x.is_array()) => {
            let items: Vec<String> = a.iter().map(scalar).collect();
            out.push(format!("{path} = [{}]", items.join(", ")));
        }
        Value::Array(a) => {
            for (i, x) in a.iter().enumerate() {
                flatten(&at(&i.to_string()), x, out);
            }
        }
        v => out.push(format!("{path} = {}", scalar(v))),
    }
}

/// A sketch entity without its position: `line p1 → p2`, `arc p5: p2 → p3`,
/// `circle p5`, `point`, then its flags.
fn entity(e: &Value) -> String {
    let field = |k: &str| e.get(k).map(scalar).unwrap_or_default();
    let ty = field("type");
    let mut s = match ty.as_str() {
        "line" => format!("line {} → {}", field("start"), field("end")),
        "arc" => format!(
            "arc {}: {} → {}",
            field("center"),
            field("start"),
            field("end")
        ),
        "circle" => format!("circle {}", field("center")),
        t => t.to_owned(),
    };
    let known = ["type", "start", "end", "center", "m", "r_m"];
    if let Some(o) = e.as_object() {
        for (k, v) in o.iter().filter(|(k, _)| !known.contains(&k.as_str())) {
            match (k.as_str(), v) {
                ("construction", Value::Bool(true)) => s.push_str(" (construction)"),
                ("projected", r) => {
                    let r = reference(r).unwrap_or_else(|| r.to_string());
                    s.push_str(&format!(" projected from {r}"));
                }
                (k, v) => s.push_str(&format!(" {k}={}", scalar(v))),
            }
        }
    }
    s
}

/// A sketch entity's solved position: a point's `m`, a circle's `r_m`.
fn position(id: &str, e: &Value) -> Vec<String> {
    let mut out = Vec::new();
    if let Some(Value::Array(m)) = e.get("m") {
        let xy: Vec<String> = m.iter().map(scalar).collect();
        out.push(format!("positions.{id} = {}", xy.join(", ")));
    }
    if let Some(r) = e.get("r_m") {
        out.push(format!("positions.{id}.r = {}", scalar(r)));
    }
    out
}

/// A sketch constraint: `length l1 = width`, `parallel l1 l2`,
/// `symmetric p1 p2 about l3`.
fn constraint(c: &Value) -> String {
    let field = |k: &str| c.get(k).map(scalar);
    let mut s = field("type").unwrap_or_default();
    for k in ["a", "b"] {
        if let Some(x) = field(k) {
            s.push(' ');
            s.push_str(&x);
        }
    }
    if let Some(x) = field("about") {
        s.push_str(&format!(" about {x}"));
    }
    if let Some(o) = c.as_object() {
        for (k, v) in o {
            if !matches!(k.as_str(), "type" | "a" | "b" | "about" | "value") {
                s.push_str(&format!(" {k}={}", scalar(v)));
            }
        }
    }
    if let Some(x) = field("value") {
        s.push_str(&format!(" = {x}"));
    }
    s
}

/// What a structural diff includes.
#[derive(Clone, Debug, Default)]
pub struct Options {
    /// Only this part.
    pub part: Option<String>,
    /// List each sketch position that moved, not only that some did.
    pub positions: bool,
    /// Include parts and features that are the same.
    pub all: bool,
}

/// A file's JSON, or `None` when it is missing or unreadable.
fn json_at(files: &Files, path: &str) -> Option<Value> {
    let text = std::str::from_utf8(files.get(path)?).ok()?;
    serde_json::from_str(text).ok()
}

/// `added`, `removed`, `changed` or `same`, for something on either side.
fn status(from: bool, to: bool, changed: bool) -> &'static str {
    match (from, to) {
        (false, _) => "added",
        (_, false) => "removed",
        _ if changed => "changed",
        _ => "same",
    }
}

/// Every leaf that differs between `a` and `b`, by JSON pointer: objects
/// by key, anything else (arrays included) as one value. A side the leaf
/// is missing from is left out.
fn changes(at: &str, a: Option<&Value>, b: Option<&Value>, out: &mut Vec<Value>) {
    match (a, b) {
        (Some(Value::Object(x)), Some(Value::Object(y))) => {
            let keys: BTreeSet<&String> = x.keys().chain(y.keys()).collect();
            for k in keys {
                let at = format!("{at}/{}", k.replace('~', "~0").replace('/', "~1"));
                changes(&at, x.get(k), y.get(k), out);
            }
        }
        (a, b) if a == b => {}
        (a, b) => {
            let mut c = json!({ "at": at });
            if let Some(a) = a {
                c["from"] = a.clone();
            }
            if let Some(b) = b {
                c["to"] = b.clone();
            }
            out.push(c);
        }
    }
}

/// The members of a sketch's `entities` or `constraints`, added, removed
/// and changed by ID; `skip` fields don't count.
fn members(a: &Value, b: &Value, skip: &[&str]) -> Value {
    let empty = Map::new();
    let (x, y) = (
        a.as_object().unwrap_or(&empty),
        b.as_object().unwrap_or(&empty),
    );
    let strip = |v: &Value| -> Value {
        let mut v = v.clone();
        if let Some(o) = v.as_object_mut() {
            for k in skip {
                o.remove(*k);
            }
        }
        v
    };
    let added: Vec<&String> = y.keys().filter(|k| !x.contains_key(*k)).collect();
    let removed: Vec<&String> = x.keys().filter(|k| !y.contains_key(*k)).collect();
    let changed: Vec<&String> = x
        .iter()
        .filter(|(k, v)| y.get(*k).is_some_and(|w| strip(v) != strip(w)))
        .map(|(k, _)| k)
        .collect();
    json!({"added": added, "removed": removed, "changed": changed})
}

/// Each position that moved between two versions of a sketch: a point's
/// `m`, a circle's `r_m`.
fn moved(a: &Value, b: &Value) -> Vec<Value> {
    let mut out = Vec::new();
    let (Some(x), Some(y)) = (a.as_object(), b.as_object()) else {
        return out;
    };
    for (id, e) in x {
        let Some(f) = y.get(id) else { continue };
        for k in ["m", "r_m"] {
            if let (Some(p), Some(q)) = (e.get(k), f.get(k))
                && p != q
            {
                out.push(json!({"entity": id, "field": k, "from": p, "to": q}));
            }
        }
    }
    out
}

/// One feature file on each side.
fn feature(a: Option<&Value>, b: Option<&Value>, opts: &Options) -> Value {
    let mut out = json!({ "status": status(a.is_some(), b.is_some(), a != b) });
    let (Some(a), Some(b)) = (a, b) else {
        return out;
    };
    if a == b {
        return out;
    }
    let sketch = |v: &Value| v["type"] == "sketch";
    if sketch(a) && sketch(b) {
        let (ea, eb) = (&a["entities"], &b["entities"]);
        out["entities"] = members(ea, eb, &["m", "r_m"]);
        out["constraints"] = members(&a["constraints"], &b["constraints"], &[]);
        let positions = moved(ea, eb);
        out["positions"] = if opts.positions {
            json!(positions)
        } else {
            json!(if positions.is_empty() {
                "same"
            } else {
                "changed"
            })
        };
        let rest = |v: &Value| {
            let mut v = v.clone();
            let o = v.as_object_mut().expect("a sketch is an object");
            o.remove("entities");
            o.remove("constraints");
            v
        };
        let mut c = Vec::new();
        changes("", Some(&rest(a)), Some(&rest(b)), &mut c);
        out["changes"] = json!(c);
    } else {
        let mut c = Vec::new();
        changes("", Some(a), Some(b), &mut c);
        out["changes"] = json!(c);
    }
    out
}

/// The features `a` and `b` list, in order: what was added, removed, and
/// moved (those kept that aren't in the longest run both keep in order).
fn order(a: &[String], b: &[String]) -> Value {
    let added: Vec<&String> = b.iter().filter(|f| !a.contains(f)).collect();
    let removed: Vec<&String> = a.iter().filter(|f| !b.contains(f)).collect();
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
    let moved: Vec<&&String> = y.iter().filter(|f| !kept.contains(**f)).collect();
    json!({"added": added, "removed": removed, "moved": moved})
}

/// A part's listed features, if its `part.json` reads.
fn listed(files: &Files, part: &str) -> Vec<String> {
    json_at(files, &format!("parts/{part}/part.json"))
        .and_then(|v| serde_json::from_value(v["features"].clone()).ok())
        .unwrap_or_default()
}

/// The parts a set of files has, and each one's feature files by ID.
fn parts(files: &Files) -> BTreeMap<String, BTreeSet<String>> {
    let mut out: BTreeMap<String, BTreeSet<String>> = BTreeMap::new();
    for path in files.keys() {
        let mut seg = path.split('/');
        if let (Some("parts"), Some(part), Some(rest)) = (seg.next(), seg.next(), seg.next()) {
            let features = out.entry(part.to_owned()).or_default();
            if rest == "features"
                && let Some(f) = seg.next().and_then(|n| n.strip_suffix(".json"))
                && seg.next().is_none()
            {
                features.insert(f.to_owned());
            }
        }
    }
    out
}

/// A part's parameters as written, by name.
fn parameters(files: &Files, part: &str) -> Map<String, Value> {
    json_at(files, &format!("parts/{part}/parameters.json"))
        .and_then(|v| v["parameters"].as_object().cloned())
        .unwrap_or_default()
}

/// What changed between two versions of a project's files, `from` and
/// `to`: the manifest, and per part its feature order, its parameters as
/// written, and each feature by JSON pointer (a sketch by entity and
/// constraint, its solved positions apart).
pub fn structural(from: &Files, to: &Files, opts: &Options) -> Value {
    let mut manifest = Vec::new();
    changes(
        "",
        json_at(from, crate::doc::project::MANIFEST).as_ref(),
        json_at(to, crate::doc::project::MANIFEST).as_ref(),
        &mut manifest,
    );
    let (pa, pb) = (parts(from), parts(to));
    let names: BTreeSet<&String> = pa.keys().chain(pb.keys()).collect();
    let mut out = Map::new();
    for part in names {
        if opts.part.as_ref().is_some_and(|p| p != part) {
            continue;
        }
        let (fa, fb) = (pa.get(part), pb.get(part));
        let empty = BTreeSet::new();
        let ids: BTreeSet<&String> = fa
            .unwrap_or(&empty)
            .iter()
            .chain(fb.unwrap_or(&empty))
            .collect();
        let mut features = Map::new();
        for f in ids {
            let path = format!("parts/{part}/features/{f}.json");
            let (va, vb) = (json_at(from, &path), json_at(to, &path));
            // A file that is there but doesn't read is no feature to compare.
            let bad: Vec<&str> = [("from", from, &va), ("to", to, &vb)]
                .into_iter()
                .filter(|(_, files, v)| files.contains_key(&path) && v.is_none())
                .map(|(side, _, _)| side)
                .collect();
            let v = if bad.is_empty() {
                feature(va.as_ref(), vb.as_ref(), opts)
            } else {
                let st = status(
                    from.contains_key(&path),
                    to.contains_key(&path),
                    from.get(&path) != to.get(&path),
                );
                json!({"status": st, "invalid": bad})
            };
            if opts.all || v["status"] != "same" {
                features.insert(f.clone(), v);
            }
        }
        let order = order(&listed(from, part), &listed(to, part));
        let (qa, qb) = (parameters(from, part), parameters(to, part));
        let keys: BTreeSet<&String> = qa.keys().chain(qb.keys()).collect();
        let (mut added, mut removed, mut changed) = (Vec::new(), Vec::new(), Vec::new());
        for k in keys {
            match (qa.get(k), qb.get(k)) {
                (None, Some(t)) => added.push(json!({"name": k, "to": t})),
                (Some(f), None) => removed.push(json!({"name": k, "from": f})),
                (Some(f), Some(t)) if f != t => {
                    changed.push(json!({"name": k, "from": f, "to": t}));
                }
                _ => {}
            }
        }
        let prefix = format!("parts/{part}/");
        let mine = |files: &Files| -> Files {
            files
                .iter()
                .filter(|(p, _)| p.starts_with(&prefix))
                .map(|(p, b)| (p.clone(), b.clone()))
                .collect()
        };
        let changed_part = mine(from) != mine(to);
        let st = status(fa.is_some(), fb.is_some(), changed_part);
        if !opts.all && st == "same" {
            continue;
        }
        out.insert(
            part.clone(),
            json!({
                "status": st,
                "order": order,
                "parameters": {"added": added, "removed": removed, "changed": changed},
                "features": features,
            }),
        );
    }
    json!({"manifest": {"changes": manifest}, "parts": out})
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn text(v: Value) -> String {
        String::from_utf8(textconv(crate::doc::canonical::to_string(&v).as_bytes())).unwrap()
    }

    #[test]
    fn a_feature_is_one_line_per_value() {
        let v = json!({
            "extent": {"length": "15 mm", "type": "blind"},
            "format": 2, "kind": "ryucad.feature", "sketch": "s1", "type": "extrude",
            "loops": ["l1", "a2"],
        });
        assert_eq!(
            text(v),
            "ryucad.feature extrude (format 2)\n\
             extent.length = 15 mm\n\
             extent.type = blind\n\
             loops = [l1, a2]\n\
             sketch = s1\n"
        );
        let up_to = json!({
            "extent": {"target": {"feature": "box1", "name": "0011aabb"}, "type": "up_to"},
            "format": 2, "kind": "ryucad.feature", "sketch": "s1", "type": "extrude",
        });
        assert!(text(up_to).contains("\nextent.target = box1/0011aabb\n"));
    }

    #[test]
    fn a_part_lists_its_features() {
        let v = json!({"features": ["box1", "s1"], "format": 1, "kind": "ryucad.part"});
        assert_eq!(
            text(v),
            "ryucad.part (format 1)\nfeature box1\nfeature s1\n"
        );
        let m = json!({"default_unit": "mm", "format": 1, "kind": "ryucad.project"});
        assert_eq!(text(m), "ryucad.project (format 1)\ndefault_unit = mm\n");
        let p = json!({"format": 1, "kind": "ryucad.parameters",
                       "parameters": {"wall": "3 mm", "hole": "wall * 2"}});
        assert_eq!(
            text(p),
            "ryucad.parameters (format 1)\nparameters.hole = wall * 2\nparameters.wall = 3 mm\n"
        );
    }

    #[test]
    fn a_sketch_puts_its_positions_last() {
        let v = json!({
            "constraints": {
                "c1": {"a": "l1", "type": "horizontal"},
                "c2": {"a": "p1", "b": "p2", "about": "l2", "type": "symmetric"},
                "d1": {"a": "l1", "type": "length", "value": "width"},
            },
            "entities": {
                "a1": {"center": "p5", "end": "p3", "start": "p2", "type": "arc"},
                "ci1": {"center": "p5", "r_m": 0.005, "type": "circle"},
                "l1": {"end": "p2", "start": "p1", "type": "line"},
                "l2": {"construction": true, "end": "p4", "start": "p3", "type": "line"},
                "p1": {"m": [0, 0.01], "type": "point",
                       "projected": {"feature": "box1", "name": "77"}},
            },
            "format": 2, "kind": "ryucad.feature", "plane": "xy", "type": "sketch",
        });
        assert_eq!(
            text(v),
            "ryucad.feature sketch (format 2)\n\
             constraints.c1 = horizontal l1\n\
             constraints.c2 = symmetric p1 p2 about l2\n\
             constraints.d1 = length l1 = width\n\
             entities.a1 = arc p5: p2 → p3\n\
             entities.ci1 = circle p5\n\
             entities.l1 = line p1 → p2\n\
             entities.l2 = line p3 → p4 (construction)\n\
             entities.p1 = point projected from box1/77\n\
             plane = xy\n\
             \n\
             positions.ci1.r = 0.005\n\
             positions.p1 = 0, 0.01\n"
        );
    }

    fn files(list: &[(&str, Value)]) -> Files {
        list.iter()
            .map(|(p, v)| {
                let text = crate::doc::canonical::to_string(v);
                ((*p).to_owned(), text.into_bytes())
            })
            .collect()
    }

    fn box_x(x: &str) -> Value {
        json!({"format": 1, "kind": "ryucad.feature", "type": "box",
               "params": {"x": x, "y": "1 mm", "z": "1 mm"}})
    }

    fn sketch(p1: [f64; 2], d1: &str, plane: &str, more: bool) -> Value {
        let mut v = json!({
            "constraints": {"d1": {"a": "l1", "type": "length", "value": d1}},
            "entities": {
                "l1": {"end": "p0", "start": "p1", "type": "line"},
                "p0": {"m": [1, 0], "type": "point"},
                "p1": {"m": p1, "type": "point"},
            },
            "format": 2, "kind": "ryucad.feature", "plane": plane, "type": "sketch",
        });
        if more {
            v["entities"]["p2"] = json!({"m": [2, 2], "type": "point"});
            v["entities"]["l2"] = json!({"end": "p2", "start": "p1", "type": "line"});
        }
        v
    }

    #[test]
    fn a_structural_diff_names_what_changed() {
        let manifest = |u: &str| json!({"default_unit": u, "format": 1, "kind": "ryucad.project"});
        let part = |f: &[&str]| json!({"features": f, "format": 1, "kind": "ryucad.part"});
        let params = |p: Value| json!({"format": 1, "kind": "ryucad.parameters", "parameters": p});
        let from = files(&[
            ("ryucad.json", manifest("mm")),
            ("parts/p/part.json", part(&["a", "s", "b"])),
            (
                "parts/p/parameters.json",
                params(json!({"w": "3 mm", "h": "1 mm"})),
            ),
            ("parts/p/features/a.json", box_x("10 mm")),
            (
                "parts/p/features/s.json",
                sketch([0.0, 0.0], "w", "xy", false),
            ),
            ("parts/p/features/b.json", box_x("1 mm")),
            ("parts/r/part.json", part(&[])),
            ("parts/same/part.json", part(&[])),
        ]);
        let to = files(&[
            ("ryucad.json", manifest("in")),
            ("parts/p/part.json", part(&["s", "a", "c"])),
            (
                "parts/p/parameters.json",
                params(json!({"w": "4 mm", "k": "w * 2"})),
            ),
            ("parts/p/features/a.json", box_x("12 mm")),
            (
                "parts/p/features/s.json",
                sketch([0.0, 0.01], "w * 2", "zx", true),
            ),
            ("parts/p/features/c.json", box_x("1 mm")),
            ("parts/q/part.json", part(&[])),
            ("parts/same/part.json", part(&[])),
        ]);
        let v = structural(&from, &to, &Options::default());
        assert_eq!(
            v["manifest"]["changes"],
            json!([{"at": "/default_unit", "from": "mm", "to": "in"}])
        );
        let parts = v["parts"].as_object().unwrap();
        assert_eq!(parts.keys().collect::<Vec<_>>(), ["p", "q", "r"]);
        assert_eq!(parts["q"]["status"], "added");
        assert_eq!(parts["r"]["status"], "removed");
        let p = &parts["p"];
        assert_eq!(p["status"], "changed");
        assert_eq!(
            p["order"],
            json!({"added": ["c"], "removed": ["b"], "moved": ["a"]})
        );
        assert_eq!(
            p["parameters"],
            json!({"added": [{"name": "k", "to": "w * 2"}],
                   "removed": [{"name": "h", "from": "1 mm"}],
                   "changed": [{"name": "w", "from": "3 mm", "to": "4 mm"}]})
        );
        let f = &p["features"];
        assert_eq!(
            f["a"],
            json!({"status": "changed",
                   "changes": [{"at": "/params/x", "from": "10 mm", "to": "12 mm"}]})
        );
        assert_eq!(f["b"], json!({"status": "removed"}));
        assert_eq!(f["c"], json!({"status": "added"}));
        assert_eq!(
            f["s"],
            json!({"status": "changed",
                   "entities": {"added": ["l2", "p2"], "removed": [], "changed": []},
                   "constraints": {"added": [], "removed": [], "changed": ["d1"]},
                   "positions": "changed",
                   "changes": [{"at": "/plane", "from": "xy", "to": "zx"}]})
        );
        // Every position, on asking; everything, the same included.
        let opts = Options {
            positions: true,
            all: true,
            part: Some("p".into()),
        };
        let v = structural(&from, &to, &opts);
        assert_eq!(
            v["parts"]["p"]["features"]["s"]["positions"],
            json!([{"entity": "p1", "field": "m", "from": [0.0, 0.0], "to": [0.0, 0.01]}])
        );
        assert_eq!(v["parts"].as_object().unwrap().len(), 1);
        let all = structural(
            &from,
            &from,
            &Options {
                all: true,
                ..Options::default()
            },
        );
        assert_eq!(all["parts"]["same"]["status"], "same");
        assert_eq!(
            all["parts"]["p"]["features"]["a"],
            json!({"status": "same"})
        );
        // A file that doesn't read is no feature to compare.
        let mut broken = to.clone();
        broken.insert("parts/p/features/a.json".into(), b"{ nope".to_vec());
        let v = structural(&from, &broken, &Options::default());
        assert_eq!(
            v["parts"]["p"]["features"]["a"],
            json!({"status": "changed", "invalid": ["to"]})
        );
    }

    #[test]
    fn diff_compares_revisions_and_the_working_files() {
        use crate::git::tests::git;
        use crate::server::tests::{call, fresh};
        let (dir, mut s) = fresh();
        let root = dir.path();
        // Outside a repository there is no HEAD to compare with.
        let e = call(&mut s, "diff", json!({})).unwrap_err();
        assert_eq!(e.code, crate::error::Code::NotAGitRepo);
        git(root, &["init", "-q", "-b", "main"]);
        call(&mut s, "part.create", json!({"part": "p"})).unwrap();
        call(
            &mut s,
            "feature.add",
            json!({"part": "p", "type": "sphere", "id": "s", "params": {"radius": 1}}),
        )
        .unwrap();
        git(root, &["add", "."]);
        git(root, &["commit", "-q", "-m", "a sphere"]);
        let v = call(&mut s, "diff", json!({})).unwrap();
        assert_eq!(v["parts"], json!({}));
        assert_eq!(v["from"]["rev"], "HEAD");
        assert_eq!(v["from"]["commit"].as_str().unwrap().len(), 40);
        assert_eq!(v["to"], json!({"rev": "working"}));
        call(
            &mut s,
            "feature.edit",
            json!({"part": "p", "id": "s", "params": {"radius": 2}}),
        )
        .unwrap();
        let v = call(&mut s, "diff", json!({})).unwrap();
        assert_eq!(
            v["parts"]["p"]["features"]["s"]["changes"],
            json!([{"at": "/params/radius", "from": "1 mm", "to": "2 mm"}])
        );
        git(root, &["commit", "-q", "-am", "bigger"]);
        let v = call(&mut s, "diff", json!({"from": "HEAD~1", "to": "HEAD"})).unwrap();
        assert_eq!(v["parts"]["p"]["features"]["s"]["status"], "changed");
        let back = call(&mut s, "diff", json!({"from": "HEAD", "to": "HEAD~1"})).unwrap();
        assert_eq!(
            back["parts"]["p"]["features"]["s"]["changes"][0]["to"],
            "1 mm"
        );
        let e = call(&mut s, "diff", json!({"from": "nowhere"})).unwrap_err();
        assert_eq!(e.code, crate::error::Code::RevNotFound);
        let e = call(&mut s, "diff", json!({"part": "Bad Part"})).unwrap_err();
        assert_eq!(e.code, crate::error::Code::InvalidId);
    }

    #[test]
    fn what_isnt_json_comes_back_as_it_is() {
        for b in [&b"{ nope"[..], b"<<<<<<< ours\n", b"\xff\xfe", b"[1, 2]"] {
            assert_eq!(textconv(b), b);
        }
        assert_eq!(textconv(b"{\"a\": [{\"b\": 1}]}"), b"json\na.0.b = 1\n");
    }
}
