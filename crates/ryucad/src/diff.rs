//! What changed between two versions of a project: [`textconv`] renders
//! one file as lines for Git's own diff to compare; [`structural`] compares
//! two versions' files as ryucad reads them, per part, feature, parameter
//! and sketch entity, and what changed in PDM (properties, release records
//! and release states); [`model`] adds what that did to the rebuilt bodies.

use crate::activity::Files;
use ryupdm::history::Releases;
use serde_json::{Map, Value, json};
use std::collections::{BTreeMap, BTreeSet};

/// A file as one line per value, in a fixed order, so that a line diff of
/// two renderings reads as the values that changed: `git diff` through
/// `ryucad git textconv`. A header names the file's kind (and a feature's
/// type) and format; a part lists its features one per line; a sketch gives
/// each entity and constraint a line, and its solved positions last, apart
/// from the intent. ryupdm's files: a part's properties one per line, each
/// declared property in the settings on one line, and a release record's
/// author on one line and its rebuilt results last, apart. Anything that
/// isn't JSON comes back as it is.
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
    // Lines sorted together, and a sketch's positions or a record's
    // results after them.
    let mut lines = Vec::new();
    let mut last = Vec::new();
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
                    last.extend(position(id, e));
                }
            }
            if let Some(Value::Object(constraints)) = rest.remove("constraints") {
                for (id, c) in &constraints {
                    lines.push(format!("constraints.{id} = {}", constraint(c)));
                }
            }
        }
        // An instance's revision in words, its pose last: solver state.
        Some("ryucad.feature") if o.get("type").and_then(Value::as_str) == Some("instance") => {
            if let Some(rev) = rest.remove("rev") {
                let rev = match rev.get("label").and_then(Value::as_str) {
                    Some(l) => format!("label {l}"),
                    None => scalar(&rev),
                };
                lines.push(format!("rev = {rev}"));
            }
            if let Some(Value::Object(pose)) = rest.remove("pose") {
                for (k, x) in &pose {
                    let mut flat = Vec::new();
                    flatten(&format!("pose.{k}"), x, &mut flat);
                    last.extend(flat);
                }
            }
        }
        Some(ryupdm::config::KIND) => {
            if let Some(Value::Object(decls)) = rest.remove("properties") {
                for (name, d) in &decls {
                    lines.push(format!("properties.{name} = {}", declaration(d)));
                }
            }
        }
        Some(ryupdm::record::RELEASE_KIND | ryupdm::record::OBSOLETE_KIND) => {
            if let Some(by) = rest.remove("released_by") {
                lines.push(format!("released_by = {}", released_by(&by)));
            }
            if let Some(Value::Object(results)) = rest.remove("results") {
                for (f, hash) in &results {
                    last.push(format!("results.{f} = {}", scalar(hash)));
                }
            }
        }
        _ => {}
    }
    flatten("", &Value::Object(rest), &mut lines);
    lines.sort();
    out.extend(lines);
    if !last.is_empty() {
        out.push(String::new());
        out.extend(last);
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
/// leaves or of references on one line, other arrays by index.
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
        // A two-way pattern's skips, one value as a merge sees them.
        Value::Array(a)
            if !a.is_empty()
                && a.iter().all(|x| {
                    x.as_array()
                        .is_some_and(|i| i.iter().all(|y| !y.is_object() && !y.is_array()))
                }) =>
        {
            let items: Vec<String> = a
                .iter()
                .map(|x| {
                    let inner: Vec<String> =
                        x.as_array().expect("an array").iter().map(scalar).collect();
                    format!("[{}]", inner.join(", "))
                })
                .collect();
            out.push(format!("{path} = [{}]", items.join(", ")));
        }
        // A fillet's or chamfer's edges, one value as a merge sees them.
        Value::Array(a) if !a.is_empty() && a.iter().all(|x| reference(x).is_some()) => {
            let items: Vec<String> = a.iter().filter_map(reference).collect();
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

/// A declared property: `enum anodized, bare (required, revising)`,
/// `quantity length`, `string`.
fn declaration(d: &Value) -> String {
    let mut s = d.get("type").map(scalar).unwrap_or_default();
    if let Some(Value::Array(values)) = d.get("values") {
        let list: Vec<String> = values.iter().map(scalar).collect();
        s.push_str(&format!(" {}", list.join(", ")));
    }
    if let Some(dim) = d.get("dim") {
        s.push_str(&format!(" {}", scalar(dim)));
    }
    let flags: Vec<&str> = ["required", "revising"]
        .into_iter()
        .filter(|f| d.get(*f) == Some(&Value::Bool(true)))
        .collect();
    if !flags.is_empty() {
        s.push_str(&format!(" ({})", flags.join(", ")));
    }
    if let Some(o) = d.as_object() {
        for (k, v) in o {
            if !matches!(
                k.as_str(),
                "type" | "values" | "dim" | "required" | "revising"
            ) {
                s.push_str(&format!(" {k}={}", scalar(v)));
            }
        }
    }
    s
}

/// Who wrote a record: `Ada <ada@example.com> as agent`, each part only
/// when it is there.
fn released_by(by: &Value) -> String {
    let mut parts = Vec::new();
    if let Some(n) = by.get("name") {
        parts.push(scalar(n));
    }
    if let Some(e) = by.get("email") {
        parts.push(format!("<{}>", scalar(e)));
    }
    if let Some(p) = by.get("participant") {
        parts.push(format!("as {}", scalar(p)));
    }
    parts.join(" ")
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
    let instance = |v: &Value| v["type"] == "instance";
    if instance(a) && instance(b) {
        // A pose is solver state, summarised as a sketch's positions are.
        let mut moved = Vec::new();
        for k in ["origin_m", "z", "x"] {
            let (p, q) = (&a["pose"][k], &b["pose"][k]);
            if p != q {
                moved.push(json!({"field": k, "from": p, "to": q}));
            }
        }
        out["pose"] = if opts.positions {
            json!(moved)
        } else {
            json!(if moved.is_empty() { "same" } else { "changed" })
        };
        let rest = |v: &Value| {
            let mut v = v.clone();
            v.as_object_mut()
                .expect("an instance is an object")
                .remove("pose");
            v
        };
        let mut c = Vec::new();
        changes("", Some(&rest(a)), Some(&rest(b)), &mut c);
        out["changes"] = json!(c);
        return out;
    }
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
    let moved = moved_features(a, b);
    let moved: Vec<&String> = b.iter().filter(|f| moved.contains(*f)).collect();
    json!({"added": added, "removed": removed, "moved": moved})
}

/// The features `a` and `b` both list that `b` moved: those not in the
/// longest run both keep in order, the fewest moves that explain `b`.
pub(crate) fn moved_features(a: &[String], b: &[String]) -> BTreeSet<String> {
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

/// The parts some release record names: folders under `releases/`.
fn recorded(files: &Files) -> BTreeSet<String> {
    files
        .keys()
        .filter_map(|p| {
            let mut seg = p.split('/');
            match (seg.next(), seg.next(), seg.next()) {
                (Some(crate::doc::project::RELEASES), Some(part), Some(_)) => Some(part.to_owned()),
                _ => None,
            }
        })
        .collect()
}

/// What changed in `part`'s PDM from `from` to `to`, `None` when nothing
/// did: each property changed (`from` and `to` as written, absent sides
/// left out, and whether the settings make it revising), each record
/// `added`, `removed` or `changed` (`{label, kind}`), and the part's
/// release state on each side when they differ, both read against
/// `releases`, the release branch as it is now.
fn pdm(from: &Files, to: &Files, part: &str, releases: &Releases) -> Option<Value> {
    let path = format!("parts/{part}/{}", crate::doc::project::PROPERTIES);
    let props = |files: &Files| -> Map<String, Value> {
        let mut o = json_at(files, &path)
            .and_then(|v| v.as_object().cloned())
            .unwrap_or_default();
        o.remove("format");
        o.remove("kind");
        o
    };
    let (pa, pb) = (props(from), props(to));
    let config = ryupdm::config::Config::read(to)
        .or_else(|_| ryupdm::config::Config::read(from))
        .unwrap_or_default();
    let mut properties = Map::new();
    for k in pa.keys().chain(pb.keys()).collect::<BTreeSet<_>>() {
        let (a, b) = (pa.get(k), pb.get(k));
        if a == b {
            continue;
        }
        let mut c = json!({"revising": config.decl(k).is_some_and(|d| d.revising)});
        if let Some(a) = a {
            c["from"] = a.clone();
        }
        if let Some(b) = b {
            c["to"] = b.clone();
        }
        properties.insert(k.clone(), c);
    }
    let records = |files: &Files| -> BTreeMap<String, (Value, Vec<u8>)> {
        ryupdm::record::Records::read(files)
            .unwrap_or_default()
            .entries
            .into_iter()
            .filter(|e| e.part.as_str() == part)
            .map(|e| {
                let what = json!({"label": e.label, "kind": e.kind.as_str()});
                (e.path, (what, e.bytes))
            })
            .collect()
    };
    let (ra, rb) = (records(from), records(to));
    let (mut added, mut removed, mut changed) = (Vec::new(), Vec::new(), Vec::new());
    for path in ra.keys().chain(rb.keys()).collect::<BTreeSet<_>>() {
        match (ra.get(path), rb.get(path)) {
            (None, Some((w, _))) => added.push(w.clone()),
            (Some((w, _)), None) => removed.push(w.clone()),
            (Some((w, a)), Some((_, b))) if a != b => changed.push(w.clone()),
            _ => {}
        }
    }
    let state = crate::doc::id::Id::parse(part, "part").ok().and_then(|id| {
        let a = ryupdm::state::derive(from, &id, releases).text();
        let b = ryupdm::state::derive(to, &id, releases).text();
        (a != b).then(|| json!({"from": a, "to": b}))
    });
    if properties.is_empty()
        && added.is_empty()
        && removed.is_empty()
        && changed.is_empty()
        && state.is_none()
    {
        return None;
    }
    let mut v = json!({
        "properties": properties,
        "records": {"added": added, "removed": removed, "changed": changed},
    });
    if let Some(s) = state {
        v["state"] = s;
    }
    Some(v)
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
/// constraint, its solved positions apart). A part whose PDM differs has
/// `pdm` too (see [`pdm`]), its states read against `releases`, the
/// release branch as read; a change to `ryupdm.json` is `pdm` at the top,
/// by pointer, as the manifest's is (settings that read with their
/// defaults filled in, so no file is the defaults).
pub fn structural(from: &Files, to: &Files, opts: &Options, releases: &Releases) -> Value {
    let mut manifest = Vec::new();
    changes(
        "",
        json_at(from, crate::doc::project::MANIFEST).as_ref(),
        json_at(to, crate::doc::project::MANIFEST).as_ref(),
        &mut manifest,
    );
    let (pa, pb) = (parts(from), parts(to));
    let (ra, rb) = (recorded(from), recorded(to));
    let names: BTreeSet<&String> = pa
        .keys()
        .chain(pb.keys())
        .chain(ra.iter())
        .chain(rb.iter())
        .collect();
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
        let prefixes = [
            format!("parts/{part}/"),
            format!("{}/{part}/", crate::doc::project::RELEASES),
        ];
        let mine = |files: &Files| -> Files {
            files
                .iter()
                .filter(|(p, _)| prefixes.iter().any(|x| p.starts_with(x.as_str())))
                .map(|(p, b)| (p.clone(), b.clone()))
                .collect()
        };
        let pdm = pdm(from, to, part, releases);
        // A part is there with its folder or a record of it.
        let changed_part = mine(from) != mine(to) || pdm.is_some();
        let st = status(
            fa.is_some() || ra.contains(part),
            fb.is_some() || rb.contains(part),
            changed_part,
        );
        if !opts.all && st == "same" {
            continue;
        }
        let mut v = json!({
            "status": st,
            "order": order,
            "parameters": {"added": added, "removed": removed, "changed": changed},
            "features": features,
        });
        if let Some(p) = pdm {
            v["pdm"] = p;
        }
        out.insert(part.clone(), v);
    }
    let mut v = json!({"manifest": {"changes": manifest}, "parts": out});
    // Settings as they read, defaults filled in (no file is the
    // defaults); as written when they don't read.
    let settings_of = |files: &Files| match ryupdm::config::Config::read(files) {
        Ok(c) => Some(c.to_json()),
        Err(_) => json_at(files, crate::doc::project::PDM_CONFIG),
    };
    let mut settings = Vec::new();
    changes(
        "",
        settings_of(from).as_ref(),
        settings_of(to).as_ref(),
        &mut settings,
    );
    if !settings.is_empty() {
        v["pdm"] = json!({ "changes": settings });
    }
    v
}

/// A part rebuilt on one side: its features' results in order, and its
/// solids by founder in list order, each as the model shows it (`None`
/// when not even its founder built).
#[derive(Clone, Debug, Default)]
pub struct Built {
    pub features: Vec<(
        crate::doc::id::Id,
        crate::error::Result<crate::rebuild::Output>,
    )>,
    pub solids: Vec<(
        crate::doc::id::Id,
        Option<std::sync::Arc<crate::rebuild::Built>>,
    )>,
    /// An assembly's occurrences.
    pub occurrences: Vec<crate::assembly::Occurrence>,
}

impl Built {
    /// A part's rebuild, with its solids as `part`'s list makes them.
    pub fn of(part: &crate::doc::project::Part, r: &crate::rebuild::PartResult) -> Built {
        let solids = crate::solid::Solids::of(part);
        let assembly = crate::server::part_kind(part) == crate::doc::files::PartKind::Assembly;
        Built {
            occurrences: match assembly {
                true => crate::assembly::Model::of_assembly(r).occurrences,
                false => Vec::new(),
            },
            features: r.features.clone(),
            solids: r
                .shown(&solids)
                .into_iter()
                .map(|s| {
                    (
                        s.solid.clone(),
                        s.state.map(|(_, b)| std::sync::Arc::clone(b)),
                    )
                })
                .collect(),
        }
    }
}

/// A feature's result as the model diff reports it.
fn result(r: Option<&crate::error::Result<crate::rebuild::Output>>) -> Value {
    match r {
        None => Value::Null,
        Some(Ok(o)) => json!({"ok": true, "hash": o.hash()}),
        Some(Err(e)) => json!({"ok": false, "error": e.code}),
    }
}

/// A point's bits, so that equal means identical.
fn bits(p: ryukei::geom::Point3) -> [u64; 3] {
    [p.x.to_bits(), p.y.to_bits(), p.z.to_bits()]
}

/// Each face, edge and vertex of a body by name, with what its geometry
/// is: a vertex's point, an edge's curve, range and end points, a face's
/// surface, sense and boundary.
struct Named<'a> {
    body: &'a ryukei::topo::Body,
    faces: BTreeMap<String, ryukei::topo::FaceId>,
    edges: BTreeMap<String, ryukei::topo::EdgeId>,
    vertices: BTreeMap<String, ryukei::topo::VertexId>,
}

impl<'a> Named<'a> {
    fn new(body: &'a ryukei::topo::Body) -> Named<'a> {
        let live = "live";
        Named {
            body,
            faces: body
                .faces()
                .map(|f| (body.face(f).expect(live).name.to_string(), f))
                .collect(),
            edges: body
                .edges()
                .map(|e| (body.edge(e).expect(live).name.to_string(), e))
                .collect(),
            vertices: body
                .vertices()
                .map(|v| (body.vertex(v).expect(live).name.to_string(), v))
                .collect(),
        }
    }

    fn point(&self, v: ryukei::topo::VertexId) -> [u64; 3] {
        bits(self.body.vertex(v).expect("live").point)
    }

    /// The names of a face's edges, in loop order.
    fn boundary(&self, f: ryukei::topo::FaceId) -> Vec<String> {
        let b = self.body;
        b.face_loops(f)
            .flat_map(|l| b.loop_fins(l).collect::<Vec<_>>())
            .map(|fin| {
                let e = b.fin(fin).expect("live").edge;
                b.edge(e).expect("live").name.to_string()
            })
            .collect()
    }
}

/// Whether the entity named `n` is the same on both sides, by its own
/// geometry and (for edges and faces) its boundary's.
fn same_vertex(a: &Named, b: &Named, n: &str) -> bool {
    a.point(a.vertices[n]) == b.point(b.vertices[n])
}

fn same_edge(a: &Named, b: &Named, n: &str) -> bool {
    let (x, y) = (
        a.body.edge(a.edges[n]).expect("live"),
        b.body.edge(b.edges[n]).expect("live"),
    );
    a.body.curve(x.curve) == b.body.curve(y.curve)
        && x.interval.map(f64::to_bits) == y.interval.map(f64::to_bits)
        && a.point(x.start) == b.point(y.start)
        && a.point(x.end) == b.point(y.end)
}

fn same_face(a: &Named, b: &Named, n: &str) -> bool {
    let (fa, fb) = (a.faces[n], b.faces[n]);
    let (x, y) = (
        a.body.face(fa).expect("live"),
        b.body.face(fb).expect("live"),
    );
    let (ea, eb) = (a.boundary(fa), b.boundary(fb));
    a.body.surface(x.surface) == b.body.surface(y.surface)
        && x.sense == y.sense
        && ea == eb
        && ea
            .iter()
            .all(|e| !b.edges.contains_key(e) || same_edge(a, b, e))
}

/// An edge's parameter range and end points, by bits.
type Ends = ([u64; 2], [u64; 3], [u64; 3]);

/// Pairs each entity of `a` with the one entity of `b` whose geometry is
/// the same, by `key` and then `same`; `None` unless every entity of each
/// has exactly one such partner.
fn pair<K: Ord>(
    a: &BTreeMap<String, K>,
    b: &BTreeMap<String, K>,
    same: impl Fn(&str, &str) -> bool,
) -> Option<BTreeMap<String, String>> {
    if a.len() != b.len() {
        return None;
    }
    let mut buckets: BTreeMap<&K, Vec<&String>> = BTreeMap::new();
    for (n, k) in b {
        buckets.entry(k).or_default().push(n);
    }
    let mut out = BTreeMap::new();
    let mut taken = BTreeSet::new();
    for (n, k) in a {
        let found: Vec<&&String> = buckets.get(k)?.iter().filter(|m| same(n, m)).collect();
        let [m] = found.as_slice() else {
            return None;
        };
        if !taken.insert((**m).clone()) {
            return None;
        }
        out.insert(n.clone(), (**m).clone());
    }
    Some(out)
}

/// What each face, edge and vertex of `a` is called in `b`, two builds of
/// one feature's inputs under different names, matched by their geometry
/// alone and compared by bits: a vertex by its point, an edge by its
/// curve, range and end points, a face by its surface, sense and the
/// edges it is bounded by. Names steer no geometry but a blend's rail or an
/// ordinal, so the match is exact or there is none: `None` unless every
/// entity of each matches exactly one of the other's.
pub fn match_names(
    a: &ryukei::topo::Body,
    b: &ryukei::topo::Body,
) -> Option<BTreeMap<String, String>> {
    let (x, y) = (Named::new(a), Named::new(b));
    let points = |n: &Named| -> BTreeMap<String, [u64; 3]> {
        n.vertices
            .iter()
            .map(|(k, v)| (k.clone(), n.point(*v)))
            .collect()
    };
    let vertices = pair(&points(&x), &points(&y), |_, _| true)?;
    let ends = |n: &Named| -> BTreeMap<String, Ends> {
        n.edges
            .iter()
            .map(|(k, e)| {
                let edge = n.body.edge(*e).expect("live");
                let key = (
                    edge.interval.map(f64::to_bits),
                    n.point(edge.start),
                    n.point(edge.end),
                );
                (k.clone(), key)
            })
            .collect()
    };
    let edges = pair(&ends(&x), &ends(&y), |p, q| {
        let (e, f) = (
            x.body.edge(x.edges[p]).expect("live"),
            y.body.edge(y.edges[q]).expect("live"),
        );
        x.body.curve(e.curve) == y.body.curve(f.curve)
    })?;
    let bounds =
        |n: &Named, rename: &dyn Fn(&str) -> String| -> BTreeMap<String, (bool, Vec<String>)> {
            n.faces
                .iter()
                .map(|(k, f)| {
                    let reversed =
                        n.body.face(*f).expect("live").sense == ryukei::topo::Sense::Reversed;
                    let mut edges: Vec<String> = n.boundary(*f).iter().map(|e| rename(e)).collect();
                    edges.sort();
                    (k.clone(), (reversed, edges))
                })
                .collect()
        };
    let faces = pair(
        &bounds(&x, &|e| edges[e].clone()),
        &bounds(&y, &|e| e.to_owned()),
        |p, q| {
            let (f, g) = (
                x.body.face(x.faces[p]).expect("live"),
                y.body.face(y.faces[q]).expect("live"),
            );
            x.body.surface(f.surface) == y.body.surface(g.surface)
        },
    )?;
    Some(faces.into_iter().chain(edges).chain(vertices).collect())
}

/// Names on one side only, and those on both whose geometry differs, for
/// one kind of entity.
fn names(
    a: &BTreeMap<String, impl Copy>,
    b: &BTreeMap<String, impl Copy>,
    same: impl Fn(&str) -> bool,
) -> Value {
    let gone: Vec<&String> = a.keys().filter(|n| !b.contains_key(*n)).collect();
    let new: Vec<&String> = b.keys().filter(|n| !a.contains_key(*n)).collect();
    let changed: Vec<&String> = a
        .keys()
        .filter(|n| b.contains_key(*n) && !same(n))
        .collect();
    json!({"gone": gone, "new": new, "changed": changed})
}

/// What changed between two builds of one feature's body. A face that was
/// renamed is one gone and one new: that is what a reference to it sees.
pub fn bodies(a: &ryukei::topo::Body, b: &ryukei::topo::Body) -> Value {
    let (x, y) = (Named::new(a), Named::new(b));
    let extent = |body| match crate::inspect::body_extent(body) {
        Some((min, max)) => json!({"min": min, "max": max}),
        None => Value::Null,
    };
    json!({
        "faces": names(&x.faces, &y.faces, |n| same_face(&x, &y, n)),
        "edges": names(&x.edges, &y.edges, |n| same_edge(&x, &y, n)),
        "vertices": names(&x.vertices, &y.vertices, |n| same_vertex(&x, &y, n)),
        "bounds": {"from": extent(a), "to": extent(b)},
    })
}

/// One feature's results on each side: whether each built, and, when the
/// results differ, what changed in its body or sketch.
fn feature_model(
    a: Option<&crate::error::Result<crate::rebuild::Output>>,
    b: Option<&crate::error::Result<crate::rebuild::Output>>,
) -> Value {
    let mut v = json!({"from": result(a), "to": result(b)});
    let ok = |r: Option<&crate::error::Result<crate::rebuild::Output>>| {
        r.and_then(|r| r.as_ref().ok()).cloned()
    };
    match (ok(a), ok(b)) {
        (Some(x), Some(y)) if x.hash() != y.hash() => match (x.body(), y.body()) {
            (Some(p), Some(q)) => v["body"] = bodies(&p.body, &q.body),
            _ => {
                if let (Some(p), Some(q)) = (x.sketch(), y.sketch()) {
                    let state = |s: &crate::sketch::Solved| json!({"state": s.report.state_str(), "dof": s.report.dof});
                    v["sketch"] = json!({"from": state(p), "to": state(q)});
                }
            }
        },
        _ => {}
    }
    v
}

/// A feature's result among one side's.
fn find<'a>(
    side: &'a Built,
    id: &crate::doc::id::Id,
) -> Option<&'a crate::error::Result<crate::rebuild::Output>> {
    side.features.iter().find(|(x, _)| x == id).map(|(_, r)| r)
}

/// Adds to a structural diff `v` what the change did to the model, from
/// both sides' rebuilt parts: under each feature whose result differs (or
/// every feature with `all`), `model` with its results on each side and,
/// for a body, the faces, edges and vertices gone, new and changed, and
/// its box; for a sketch, its state and freedom.
pub fn model(
    v: &mut Value,
    from: &BTreeMap<String, Built>,
    to: &BTreeMap<String, Built>,
    opts: &Options,
) {
    let parts: BTreeSet<&String> = from.keys().chain(to.keys()).collect();
    let none = Built::default();
    for part in parts {
        if opts.part.as_ref().is_some_and(|p| p != part) {
            continue;
        }
        let (a, b) = (
            from.get(part).unwrap_or(&none),
            to.get(part).unwrap_or(&none),
        );
        // Every feature built on either side, in the newer order first.
        let mut ids: Vec<&crate::doc::id::Id> = b.features.iter().map(|(id, _)| id).collect();
        ids.extend(
            a.features
                .iter()
                .map(|(id, _)| id)
                .filter(|id| !b.features.iter().any(|(x, _)| x == *id)),
        );
        for id in ids {
            let (ra, rb) = (find(a, id), find(b, id));
            let differs = result(ra) != result(rb);
            if !differs && !opts.all {
                continue;
            }
            let m = feature_model(ra, rb);
            let p = &mut v["parts"][part.as_str()];
            if p.is_null() {
                *p = json!({
                    "status": "same",
                    "order": {"added": [], "removed": [], "moved": []},
                    "parameters": {"added": [], "removed": [], "changed": []},
                    "features": {},
                });
            }
            let f = &mut p["features"][id.as_str()];
            if f.is_null() {
                *f = json!({"status": "same"});
            }
            f["model"] = m;
        }
        // Each solid as the model shows it, on either side, by founder:
        // those that differ (or every one with `all`).
        let shown = |side: &'_ Built, id: &crate::doc::id::Id| {
            side.solids
                .iter()
                .find(|(s, _)| s == id)
                .map(|(_, b)| b.clone())
        };
        let mut solids: Vec<&crate::doc::id::Id> = b.solids.iter().map(|(s, _)| s).collect();
        solids.extend(
            a.solids
                .iter()
                .map(|(s, _)| s)
                .filter(|s| !b.solids.iter().any(|(x, _)| x == *s)),
        );
        for id in solids {
            let (x, y) = (shown(a, id), shown(b, id));
            let hash = |s: &Option<Option<std::sync::Arc<crate::rebuild::Built>>>| match s {
                None => Value::Null,
                Some(None) => json!({"ok": false}),
                Some(Some(b)) => json!({"ok": true, "hash": b.hash}),
            };
            let (hx, hy) = (hash(&x), hash(&y));
            if hx == hy && !opts.all {
                continue;
            }
            let mut m = json!({"from": hx, "to": hy});
            if let (Some(Some(p)), Some(Some(q))) = (&x, &y)
                && p.hash != q.hash
            {
                m["body"] = bodies(&p.body, &q.body);
            }
            let p = &mut v["parts"][part.as_str()];
            if p.is_null() {
                *p = json!({
                    "status": "same",
                    "order": {"added": [], "removed": [], "moved": []},
                    "parameters": {"added": [], "removed": [], "changed": []},
                    "features": {},
                });
            }
            p["solids"][id.as_str()] = m;
        }
        if let Some(o) = occurrences(a, b, opts.all) {
            let p = &mut v["parts"][part.as_str()];
            if p.is_null() {
                *p = json!({
                    "status": "same",
                    "order": {"added": [], "removed": [], "moved": []},
                    "parameters": {"added": [], "removed": [], "changed": []},
                    "features": {},
                });
            }
            p["occurrences"] = o;
        }
    }
}

/// An assembly's occurrences on each side, by path: `added`, `removed`,
/// `moved` (its pose differs) and `changed` (its part's solids differ);
/// `None` when nothing differs, unless `all`.
fn occurrences(a: &Built, b: &Built, all: bool) -> Option<Value> {
    use crate::assembly::{Occurrence, path_text};
    if a.occurrences.is_empty() && b.occurrences.is_empty() {
        return None;
    }
    let by_path = |side: &'_ Built| -> BTreeMap<String, Occurrence> {
        side.occurrences
            .iter()
            .map(|o| (path_text(&o.path), o.clone()))
            .collect()
    };
    let (x, y) = (by_path(a), by_path(b));
    let added: Vec<&String> = y.keys().filter(|k| !x.contains_key(*k)).collect();
    let removed: Vec<&String> = x.keys().filter(|k| !y.contains_key(*k)).collect();
    let both: Vec<(&String, &Occurrence, &Occurrence)> = x
        .iter()
        .filter_map(|(k, o)| Some((k, o, y.get(k)?)))
        .collect();
    let moved: Vec<&String> = both
        .iter()
        .filter(|(_, p, q)| p.pose != q.pose)
        .map(|(k, _, _)| *k)
        .collect();
    let hashes = |o: &Occurrence| -> Vec<String> {
        o.solids
            .iter()
            .map(|s| format!("{} {}", s.founder, s.built.hash))
            .collect()
    };
    let changed: Vec<&String> = both
        .iter()
        .filter(|(_, p, q)| hashes(p) != hashes(q) || p.rev != q.rev)
        .map(|(k, _, _)| *k)
        .collect();
    let same = added.is_empty() && removed.is_empty() && moved.is_empty() && changed.is_empty();
    (!same || all)
        .then(|| json!({"added": added, "removed": removed, "moved": moved, "changed": changed}))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::session::Session;
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
        // What it does to solids, and a boolean of them.
        let cut = json!({
            "extent": {"both": true, "type": "through_all"},
            "format": 3, "kind": "ryucad.feature", "operation": "cut", "sketch": "s1",
            "targets": ["a", "b"], "type": "extrude",
        });
        assert_eq!(
            text(cut),
            "ryucad.feature extrude (format 3)\n\
             extent.both = true\n\
             extent.type = through_all\n\
             operation = cut\n\
             sketch = s1\n\
             targets = [a, b]\n"
        );
        let boolean = json!({
            "format": 3, "kind": "ryucad.feature", "op": "subtract", "target": "a",
            "tools": ["t", "u"], "type": "boolean",
        });
        assert_eq!(
            text(boolean),
            "ryucad.feature boolean (format 3)\n\
             op = subtract\n\
             target = a\n\
             tools = [t, u]\n"
        );
        // A fillet's groups by index, each group's edges on one line.
        let fillet = json!({
            "format": 3, "kind": "ryucad.feature", "propagate": false, "type": "fillet",
            "groups": [{"radius": "2 mm", "edges": [{"feature": "a", "name": "00aa"},
                                                   {"feature": "a", "name": "00bb"}]},
                       {"radius": "r", "edges": [{"feature": "a", "name": "00cc"}]}],
        });
        assert_eq!(
            text(fillet),
            "ryucad.feature fillet (format 3)\n\
             groups.0.edges = [a/00aa, a/00bb]\n\
             groups.0.radius = 2 mm\n\
             groups.1.edges = [a/00cc]\n\
             groups.1.radius = r\n\
             propagate = false\n"
        );
        let chamfer = json!({
            "format": 3, "kind": "ryucad.feature", "type": "chamfer",
            "groups": [{"form": "distances", "d1": "1 mm", "d2": "2 mm",
                        "face": {"feature": "a", "name": "00dd"},
                        "edges": [{"feature": "a", "name": "00aa"}]}],
        });
        assert_eq!(
            text(chamfer),
            "ryucad.feature chamfer (format 3)\n\
             groups.0.d1 = 1 mm\n\
             groups.0.d2 = 2 mm\n\
             groups.0.edges = [a/00aa]\n\
             groups.0.face = a/00dd\n\
             groups.0.form = distances\n"
        );
    }

    #[test]
    fn holes_and_patterns_are_one_line_per_value() {
        let hole = json!({
            "format": 3, "kind": "ryucad.feature", "type": "hole", "sketch": "s",
            "points": ["p1", "p2"], "form": "counterbore", "diameter": "6.6 mm",
            "counterbore": {"diameter": "11 mm", "depth": "6.8 mm"},
            "depth": {"up_to": {"target": {"feature": "b", "name": "00aa"}, "offset": "-1 mm"}},
            "bottom": {"drill": "118 deg"}, "targets": ["b"],
        });
        assert_eq!(
            text(hole),
            "ryucad.feature hole (format 3)\n\
             bottom.drill = 118 deg\n\
             counterbore.depth = 6.8 mm\n\
             counterbore.diameter = 11 mm\n\
             depth.up_to.offset = -1 mm\n\
             depth.up_to.target = b/00aa\n\
             diameter = 6.6 mm\n\
             form = counterbore\n\
             points = [p1, p2]\n\
             sketch = s\n\
             targets = [b]\n"
        );
        // Skips are one value, however many directions.
        let pattern = json!({
            "format": 3, "kind": "ryucad.feature", "type": "pattern", "form": "linear",
            "features": ["h", "c"], "direction": "x", "spacing": "10 mm", "count": "n",
            "direction2": {"feature": "b", "name": "00bb"}, "spacing2": "5 mm", "count2": "2",
            "skip": [[1, 1], [3, 0]], "targets": ["b"],
        });
        assert_eq!(
            text(pattern),
            "ryucad.feature pattern (format 3)\n\
             count = n\n\
             count2 = 2\n\
             direction = x\n\
             direction2 = b/00bb\n\
             features = [h, c]\n\
             form = linear\n\
             skip = [[1, 1], [3, 0]]\n\
             spacing = 10 mm\n\
             spacing2 = 5 mm\n\
             targets = [b]\n"
        );
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

    #[test]
    fn pdm_files_are_one_line_per_value() {
        let props = json!({"finish": "anodized", "format": 1, "kind": "ryupdm.properties",
                           "mass": "1.5 kg", "number": "100-0042"});
        assert_eq!(
            text(props),
            "ryupdm.properties (format 1)\n\
             finish = anodized\n\
             mass = 1.5 kg\n\
             number = 100-0042\n"
        );
        let config = json!({
            "format": 1, "kind": "ryupdm.config", "part_number": {"pattern": "^100-[0-9]{4}$"},
            "properties": {
                "finish": {"required": true, "revising": true, "type": "enum",
                           "values": ["anodized", "bare"]},
                "length": {"dim": "length", "required": false, "revising": false,
                           "type": "quantity"},
            },
            "release_branch": "main", "revisions": "letters",
        });
        assert_eq!(
            text(config),
            "ryupdm.config (format 1)\n\
             part_number.pattern = ^100-[0-9]{4}$\n\
             properties.finish = enum anodized, bare (required, revising)\n\
             properties.length = quantity length\n\
             release_branch = main\n\
             revisions = letters\n"
        );
        let record = json!({
            "built_with": {"commit": "abc", "ryucad": "0.1.0", "ryukei": "0.1.0"},
            "content": "c0ffee", "format": 1, "kind": "ryupdm.release", "label": "B",
            "note": "longer", "part": "p", "project": "0123456789abcdef0123456789abcdef",
            "properties": {"number": "100-0042"},
            "released_by": {"email": "ada@example.com", "name": "Ada", "participant": "agent"},
            "results": {"b": "1111", "a": "2222"}, "supersedes": "A",
        });
        assert_eq!(
            text(record),
            "ryupdm.release (format 1)\n\
             built_with.commit = abc\n\
             built_with.ryucad = 0.1.0\n\
             built_with.ryukei = 0.1.0\n\
             content = c0ffee\n\
             label = B\n\
             note = longer\n\
             part = p\n\
             project = 0123456789abcdef0123456789abcdef\n\
             properties.number = 100-0042\n\
             released_by = Ada <ada@example.com> as agent\n\
             supersedes = A\n\
             \n\
             results.a = 2222\n\
             results.b = 1111\n"
        );
        let obsolete = json!({
            "format": 1, "kind": "ryupdm.obsolete", "label": "A", "part": "p",
            "project": "0123456789abcdef0123456789abcdef",
            "released_by": {"participant": "agent"},
        });
        assert_eq!(
            text(obsolete),
            "ryupdm.obsolete (format 1)\n\
             label = A\n\
             part = p\n\
             project = 0123456789abcdef0123456789abcdef\n\
             released_by = as agent\n"
        );
    }

    /// A release of `part` as `files` have it.
    fn release(
        files: &Files,
        part: &str,
        label: &str,
        supersedes: Option<&str>,
    ) -> (String, Value) {
        let id = crate::doc::id::Id::parse(part, "part").unwrap();
        let config = ryupdm::config::Config::read(files).unwrap();
        let props = ryupdm::props::read(files, &id).unwrap();
        let r = ryupdm::record::Record::Release(ryupdm::record::Release {
            project: crate::doc::id::ProjectId::parse("0123456789abcdef0123456789abcdef").unwrap(),
            part: id.clone(),
            label: label.into(),
            content: ryupdm::content::hash(files, &id).unwrap(),
            properties: ryupdm::record::revising(&config, &props),
            results: BTreeMap::new(),
            built_with: ryupdm::record::BuiltWith {
                ryucad: "0".into(),
                ryukei: "0".into(),
                commit: "c".into(),
            },
            released_by: ryupdm::record::ReleasedBy {
                name: None,
                email: None,
                participant: "agent".into(),
            },
            supersedes: supersedes.map(str::to_owned),
            note: None,
        });
        (r.path(), r.to_json())
    }

    #[test]
    fn a_parts_pdm_changes_are_in_its_block() {
        let props = |v: Value| {
            let mut p = json!({"format": 1, "kind": "ryupdm.properties"});
            for (k, x) in v.as_object().unwrap() {
                p[k] = x.clone();
            }
            p
        };
        let common = [
            (
                "ryucad.json",
                json!({"default_unit": "mm", "format": 2, "kind": "ryucad.project",
                       "project": "0123456789abcdef0123456789abcdef"}),
            ),
            (
                "parts/p/part.json",
                json!({"features": ["a"], "format": 1, "kind": "ryucad.part"}),
            ),
            ("parts/p/features/a.json", box_x("10 mm")),
            (
                "parts/q/part.json",
                json!({"features": [], "format": 1, "kind": "ryucad.part"}),
            ),
        ];
        let mut from = files(&common);
        from.extend(files(&[(
            "parts/p/properties.json",
            props(json!({"number": "100"})),
        )]));
        let (a, ra) = release(&from, "p", "A", None);
        let (qa, rqa) = release(&from, "q", "A", None);
        from.extend(files(&[(a.as_str(), ra.clone()), (qa.as_str(), rqa)]));
        let mut to = files(&common);
        to.extend(files(&[
            (
                "parts/p/properties.json",
                props(json!({"number": "101", "name": "Pin"})),
            ),
            (
                "ryupdm.json",
                json!({"format": 1, "kind": "ryupdm.config",
                       "properties": {"finish": {"type": "string"}}}),
            ),
        ]));
        let (b, rb) = release(&to, "p", "B", Some("A"));
        to.extend(files(&[(a.as_str(), ra), (b.as_str(), rb)]));
        // p's A is official; q's A, a proposal, is withdrawn.
        let releases = Releases {
            official: [(
                a.clone(),
                ryupdm::history::Official {
                    commit: "c0ffee".into(),
                    blob: gix::ObjectId::null(gix::hash::Kind::Sha1),
                    shallow: false,
                },
            )]
            .into(),
            ..Releases::default()
        };
        let v = structural(&from, &to, &Options::default(), &releases);
        assert_eq!(
            v["parts"]["p"]["pdm"],
            json!({
                "properties": {
                    "name": {"revising": false, "to": "Pin"},
                    "number": {"from": "100", "revising": true, "to": "101"},
                },
                "records": {"added": [{"label": "B", "kind": "release"}],
                            "removed": [], "changed": []},
                "state": {"from": "released A", "to": "A modified, B proposed"},
            })
        );
        let q = &v["parts"]["q"];
        assert_eq!(q["status"], "changed");
        assert_eq!(
            q["pdm"],
            json!({"properties": {},
                   "records": {"added": [], "removed": [{"label": "A", "kind": "release"}],
                               "changed": []},
                   "state": {"from": "in work, A proposed", "to": "in work"}})
        );
        // No settings file was the defaults.
        assert_eq!(
            v["pdm"]["changes"],
            json!([{"at": "/properties",
                    "to": {"finish": {"required": false, "revising": true, "type": "string"}}}])
        );
        // A part whose PDM didn't change has no block; nor does a project
        // whose settings didn't.
        let v = structural(
            &to,
            &to,
            &Options {
                all: true,
                ..Options::default()
            },
            &releases,
        );
        assert!(v["parts"]["p"].get("pdm").is_none(), "{v}");
        assert!(v.get("pdm").is_none());
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
        let v = structural(&from, &to, &Options::default(), &Releases::default());
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
        let v = structural(&from, &to, &opts, &Releases::default());
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
            &Releases::default(),
        );
        assert_eq!(all["parts"]["same"]["status"], "same");
        assert_eq!(
            all["parts"]["p"]["features"]["a"],
            json!({"status": "same"})
        );
        // A file that doesn't read is no feature to compare.
        let mut broken = to.clone();
        broken.insert("parts/p/features/a.json".into(), b"{ nope".to_vec());
        let v = structural(&from, &broken, &Options::default(), &Releases::default());
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
    fn a_model_diff_says_what_the_bodies_did() {
        use crate::git::tests::git;
        use crate::server::tests::{call, fresh};
        let (dir, mut s) = fresh();
        let root = dir.path();
        git(root, &["init", "-q", "-b", "main"]);
        call(&mut s, "part.create", json!({"part": "p"})).unwrap();
        for f in [
            json!({"type": "box", "id": "b", "params": {"x": 10, "y": 20, "z": 5}}),
            json!({"type": "sphere", "id": "c", "params": {"radius": 1}}),
            json!({"type": "torus", "id": "t", "params": {"major_radius": 9, "minor_radius": 1}}),
            json!({"type": "sketch", "id": "s"}),
        ] {
            let mut f = f;
            f["part"] = json!("p");
            call(&mut s, "feature.add", f).unwrap();
        }
        call(
            &mut s,
            "sketch.edit",
            json!({"part": "p", "sketch": "s", "ops": crate::sketch::tests::rectangle_ops()}),
        )
        .unwrap();
        git(root, &["add", "."]);
        git(root, &["commit", "-q", "-m", "start"]);
        // The box grows along x, the sphere goes, the torus breaks and the
        // sketch is wider.
        let edit = |s: &mut Session, id: &str, params: Value| {
            call(
                s,
                "feature.edit",
                json!({"part": "p", "id": id, "params": params}),
            )
            .unwrap();
        };
        edit(&mut s, "b", json!({"x": 12}));
        edit(&mut s, "t", json!({"major_radius": 1}));
        call(&mut s, "feature.delete", json!({"part": "p", "id": "c"})).unwrap();
        call(
            &mut s,
            "sketch.edit",
            json!({"part": "p", "sketch": "s", "ops": [{"op": "set", "id": "width", "value": "30 mm"}]}),
        )
        .unwrap();
        let v = call(&mut s, "diff", json!({"model": true})).unwrap();
        let f = &v["parts"]["p"]["features"];
        let b = &f["b"]["model"];
        assert_eq!(
            (b["from"]["ok"].as_bool(), b["to"]["ok"].as_bool()),
            (Some(true), Some(true))
        );
        assert_ne!(b["from"]["hash"], b["to"]["hash"]);
        let count = |kind: &str, what: &str| b["body"][kind][what].as_array().unwrap().len();
        // The face at x = 0 and its four edges stay where they were.
        assert_eq!((count("faces", "gone"), count("faces", "new")), (0, 0));
        assert_eq!(count("faces", "changed"), 5);
        assert_eq!(count("edges", "changed"), 8);
        assert_eq!(count("vertices", "changed"), 4);
        assert_eq!(b["body"]["bounds"]["from"]["max"][0], 0.01);
        assert_eq!(b["body"]["bounds"]["to"]["max"][0], 0.012);
        assert_eq!(f["c"]["status"], "removed");
        assert_eq!(f["c"]["model"]["to"], Value::Null);
        assert_eq!(f["c"]["model"]["from"]["ok"], true);
        assert_eq!(f["t"]["model"]["to"]["ok"], false);
        assert_eq!(f["t"]["model"]["to"]["error"], "rebuild_failed");
        let sk = &f["s"]["model"]["sketch"];
        assert_eq!(sk["from"]["state"], sk["to"]["state"]);
        // With nothing changed there is no model to report, unless asked.
        let same = call(&mut s, "diff", json!({"to": "HEAD", "model": true})).unwrap();
        assert_eq!(same["parts"], json!({}));
        let all = call(
            &mut s,
            "diff",
            json!({"to": "HEAD", "model": true, "all": true}),
        )
        .unwrap();
        let c = &all["parts"]["p"]["features"]["c"];
        assert_eq!(c["status"], "same");
        assert_eq!(c["model"]["from"], c["model"]["to"]);
        assert!(c["model"].get("body").is_none());
    }

    #[test]
    fn a_model_diff_says_what_each_solid_did() {
        use crate::git::tests::git;
        use crate::server::tests::{call, fresh};
        let (dir, mut s) = fresh();
        let root = dir.path();
        git(root, &["init", "-q", "-b", "main"]);
        call(&mut s, "part.create", json!({"part": "p"})).unwrap();
        for f in [
            json!({"type": "box", "id": "a", "params": {"x": 10, "y": 10, "z": 10}}),
            json!({"type": "box", "id": "c", "operation": "cut",
                   "params": {"x": 4, "y": 20, "z": 20}, "placement": {"origin": [8, -5, -5]}}),
            json!({"type": "sphere", "id": "ball", "params": {"radius": 1},
                   "placement": {"origin": [30, 0, 0]}}),
        ] {
            let mut f = f;
            f["part"] = json!("p");
            call(&mut s, "feature.add", f).unwrap();
        }
        git(root, &["add", "."]);
        git(root, &["commit", "-q", "-m", "start"]);
        // A boss joined to a; the ball untouched.
        call(
            &mut s,
            "feature.add",
            json!({"part": "p", "type": "cylinder", "id": "boss", "operation": "join",
                   "targets": ["a"], "params": {"radius": 2, "height": 5},
                   "placement": {"origin": [4, 5, 10]}}),
        )
        .unwrap();
        let v = call(&mut s, "diff", json!({"model": true})).unwrap();
        let solids = &v["parts"]["p"]["solids"];
        let a = &solids["a"];
        assert_eq!(
            (&a["from"]["ok"], &a["to"]["ok"]),
            (&json!(true), &json!(true))
        );
        assert_ne!(a["from"]["hash"], a["to"]["hash"]);
        let new_faces = a["body"]["faces"]["new"].as_array().unwrap().len();
        assert_eq!(new_faces, 2, "{a}");
        assert!(solids.get("ball").is_none());
        let boss = &v["parts"]["p"]["features"]["boss"];
        assert_eq!(boss["status"], "added");
        assert_eq!(boss["model"]["to"]["ok"], true);
        let all = call(&mut s, "diff", json!({"model": true, "all": true})).unwrap();
        let ball = &all["parts"]["p"]["solids"]["ball"];
        assert_eq!(ball["from"], ball["to"]);
    }

    #[test]
    fn a_renamed_body_is_all_gone_and_new() {
        use crate::doc::files::{Feature, Placement, Primitive, PrimitiveType};
        use crate::units::{Quantity, Unit};
        let id = |s: &str| crate::doc::id::Id::parse(s, "feature").unwrap();
        let f = Feature::Primitive(Primitive {
            ty: PrimitiveType::Sphere,
            params: [("radius".to_owned(), Quantity::parse("1", Unit::Mm).unwrap())].into(),
            placement: Placement::identity(Unit::Mm),
            combine: crate::doc::solid::Combine::NEW,
        });
        let a = crate::rebuild::build_feature(&id("p"), &id("one"), &f).unwrap();
        let b = crate::rebuild::build_feature(&id("p"), &id("two"), &f).unwrap();
        let v = bodies(&a.body, &b.body);
        assert_eq!(v["faces"]["gone"].as_array().unwrap().len(), 1);
        assert_eq!(v["faces"]["new"].as_array().unwrap().len(), 1);
        assert_ne!(v["faces"]["gone"], v["faces"]["new"]);
        assert_eq!(v["faces"]["changed"], json!([]));
        let same = bodies(&a.body, &a.body);
        for kind in ["faces", "edges", "vertices"] {
            assert_eq!(same[kind], json!({"gone": [], "new": [], "changed": []}));
        }
    }

    #[test]
    fn two_builds_of_one_body_match_by_geometry() {
        use ryukei::name::OpKey;
        let at = |key: &[u8], z: f64| {
            let place = ryukei::geom::Rigid::translation(ryukei::geom::Vector3::new(0.0, 0.0, z));
            ryukei::primitive::block(OpKey::from_bytes(key), 0.01, 0.02, 0.03, &place)
                .unwrap()
                .0
        };
        let (a, b) = (at(b"one", 0.0), at(b"two", 0.0));
        let map = match_names(&a, &b).expect("the same box");
        assert_eq!(map.len(), 6 + 12 + 8);
        let named = |body: &ryukei::topo::Body| -> BTreeSet<String> {
            body.faces()
                .map(|f| body.face(f).unwrap().name.to_string())
                .collect()
        };
        assert!(named(&a).iter().all(|n| named(&b).contains(&map[n])));
        assert_ne!(named(&a), named(&b));
        // Moved, nothing matches.
        assert!(match_names(&a, &at(b"two", 0.001)).is_none());
    }

    #[test]
    fn what_isnt_json_comes_back_as_it_is() {
        for b in [&b"{ nope"[..], b"<<<<<<< ours\n", b"\xff\xfe", b"[1, 2]"] {
            assert_eq!(textconv(b), b);
        }
        assert_eq!(textconv(b"{\"a\": [{\"b\": 1}]}"), b"json\na.0.b = 1\n");
    }
}
