//! What changed between two versions of a project: [`textconv`] renders
//! one file as lines for Git's own diff to compare; [`structural`] compares
//! two versions' files as ryucad reads them, per part, feature, parameter
//! and sketch entity; [`model`] adds what that did to the rebuilt bodies.

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

/// A part's rebuilt features on one side, in order.
pub type Built = Vec<(
    crate::doc::id::Id,
    crate::error::Result<crate::rebuild::Output>,
)>;

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
    side.iter().find(|(x, _)| x == id).map(|(_, r)| r)
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
    let none = Built::new();
    for part in parts {
        if opts.part.as_ref().is_some_and(|p| p != part) {
            continue;
        }
        let (a, b) = (
            from.get(part).unwrap_or(&none),
            to.get(part).unwrap_or(&none),
        );
        // Every feature built on either side, in the newer order first.
        let mut ids: Vec<&crate::doc::id::Id> = b.iter().map(|(id, _)| id).collect();
        ids.extend(
            a.iter()
                .map(|(id, _)| id)
                .filter(|id| !b.iter().any(|(x, _)| x == *id)),
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
    }
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
    fn a_renamed_body_is_all_gone_and_new() {
        use crate::doc::files::{Feature, Placement, Primitive, PrimitiveType};
        use crate::units::{Quantity, Unit};
        let id = |s: &str| crate::doc::id::Id::parse(s, "feature").unwrap();
        let f = Feature::Primitive(Primitive {
            ty: PrimitiveType::Sphere,
            params: [("radius".to_owned(), Quantity::parse("1", Unit::Mm).unwrap())].into(),
            placement: Placement::identity(Unit::Mm),
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
