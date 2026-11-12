//! What changed between two versions of a project: [`textconv`] renders
//! one file as lines for Git's own diff to compare; [`structural`] compares
//! two versions' files as ryucad reads them, per part, feature, parameter
//! and sketch entity, and what changed in PDM (properties, release records
//! and release states); [`model`] adds what that did to the rebuilt bodies.

use crate::activity::{self, Files};
use crate::doc::id::Id;
use crate::doc::reads::{self, ReadBy};
use crate::error::Result;
use crate::part::part_id;
use crate::session::Session;
use ryu_session::job::Phase;
use ryupdm::history::Releases;
use serde_json::{Map, Value, json};
use std::collections::{BTreeMap, BTreeSet};

/// A file as one line per value, in a fixed order, so that a line diff of
/// two renderings reads as the values that changed: `git diff` through
/// `ryucad git textconv`. A header names the file's kind (and a feature's
/// type) and format; a part lists its features one per line, then an
/// assembly's items one per line in number order; a sketch gives
/// each entity and constraint a line, and its solved positions last, apart
/// from the intent. ryupdm's files: a part's properties one per line, each
/// declared property in the settings on one line, and a release record's
/// author on one line and its rebuilt results last, apart. Anything that
/// isn't JSON comes back as it is.
pub fn textconv(bytes: &[u8]) -> Vec<u8> {
    textconv_named(bytes, &BTreeMap::new())
}

/// The names other projects' IDs have in the manifest of the project at
/// or above `dir`, for [`textconv_named`]; none when there isn't one or it
/// doesn't read.
pub fn names_near(dir: &std::path::Path) -> BTreeMap<String, String> {
    let Some(root) = dir
        .ancestors()
        .find(|d| d.join(crate::doc::project::MANIFEST).is_file())
    else {
        return BTreeMap::new();
    };
    std::fs::read_to_string(root.join(crate::doc::project::MANIFEST))
        .ok()
        .and_then(|t| serde_json::from_str::<Value>(&t).ok())
        .map(|m| dependency_names(&m))
        .unwrap_or_default()
}

/// A manifest's dependency names by project ID.
fn dependency_names(manifest: &Value) -> BTreeMap<String, String> {
    manifest["dependencies"]
        .as_object()
        .map(|d| {
            d.iter()
                .filter_map(|(id, dep)| Some((id.clone(), dep["name"].as_str()?.to_owned())))
                .collect()
        })
        .unwrap_or_default()
}

/// A sweep's profile or a loft's section on one line: `sketch s3 (loops
/// p7, l3)`, `face base/<hex>`.
fn section_text(p: &Value) -> String {
    let mut line = match (p.get("sketch"), p.get("face")) {
        (Some(s), _) => format!("sketch {}", scalar(s)),
        (None, Some(f)) => format!("face {}", reference(f).unwrap_or_else(|| scalar(f))),
        _ => scalar(p),
    };
    if let Some(Value::Array(l)) = p.get("loops") {
        let l: Vec<String> = l.iter().map(scalar).collect();
        line.push_str(&format!(" (loops {})", l.join(", ")));
    }
    line
}

/// [`textconv`], other projects named by `names` (project ID to the name
/// their manifest gives them; a manifest's own names win), else by ID:
/// the manifest's dependencies one per line (`dependency fasteners =
/// <ID> <url> (path "")`) and where it was forked from, the lock's
/// projects and labels one per line (`fasteners m6-bolt/B = <commit>
/// content <hash>`), and an instance of another project's part as `rev =
/// fasteners:m6-bolt @ label B`.
pub fn textconv_named(bytes: &[u8], names: &BTreeMap<String, String>) -> Vec<u8> {
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
    let named = |id: &str| names.get(id).cloned().unwrap_or_else(|| id.to_owned());
    match kind {
        Some("ryucad.project") => {
            let own = dependency_names(&Value::Object(o.clone()));
            if let Some(Value::Object(deps)) = rest.remove("dependencies") {
                for (id, d) in &deps {
                    let name = own.get(id).cloned().unwrap_or_else(|| named(id));
                    lines.push(format!(
                        "dependency {name} = {id} {} (path {:?})",
                        d["url"].as_str().unwrap_or_default(),
                        d["path"].as_str().unwrap_or_default()
                    ));
                }
            }
            if let Some(f) = rest.remove("forked_from") {
                lines.push(format!(
                    "forked_from = {} at {}",
                    f["project"].as_str().unwrap_or_default(),
                    f["commit"].as_str().unwrap_or_default()
                ));
            }
        }
        Some(crate::doc::lock::KIND) => {
            if let Some(Value::Object(projects)) = rest.remove("projects") {
                for (id, p) in &projects {
                    let name = named(id);
                    let via: Vec<String> = p["via"]
                        .as_array()
                        .into_iter()
                        .flatten()
                        .filter_map(Value::as_str)
                        .map(named)
                        .collect();
                    let mut line = format!(
                        "project {name} = {id} {} (path {:?}) on {}",
                        p["url"].as_str().unwrap_or_default(),
                        p["path"].as_str().unwrap_or_default(),
                        p["release_branch"].as_str().unwrap_or_default()
                    );
                    if !via.is_empty() {
                        line.push_str(&format!(" via {}", via.join(" > ")));
                    }
                    lines.push(line);
                    for (label, l) in p["labels"].as_object().into_iter().flatten() {
                        lines.push(format!(
                            "{name} {label} = {} content {}",
                            l["commit"].as_str().unwrap_or_default(),
                            l["content"].as_str().unwrap_or_default()
                        ));
                    }
                }
            }
        }
        // A part's features keep their order; an assembly's items follow
        // in number order, a number shared by two parts on two lines.
        Some("ryucad.part") => {
            if let Some(Value::Array(list)) = rest.remove("features") {
                out.extend(list.iter().map(|f| format!("feature {}", scalar(f))));
            }
            if let Some(Value::Object(items)) = rest.remove("items") {
                let mut by: Vec<(String, &String)> =
                    items.iter().map(|(part, n)| (scalar(n), part)).collect();
                by.sort_by_key(|(n, part)| {
                    (n.parse::<u64>().unwrap_or(u64::MAX), n.clone(), *part)
                });
                out.extend(by.into_iter().map(|(n, part)| format!("item {n} = {part}")));
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
        // A mate's or an instance pattern's connectors, one line each: the
        // instance path, then the entity or `origin`, then any offset.
        Some("ryucad.feature")
            if matches!(
                o.get("type").and_then(Value::as_str),
                Some("mate" | "instance_pattern")
            ) =>
        {
            for k in ["a", "b", "axis", "direction", "direction2"] {
                if let Some(c) = rest.get(k).filter(|c| c.get("instance").is_some()).cloned() {
                    rest.remove(k);
                    lines.push(format!("{k} = {}", connector(&c)));
                }
            }
        }
        // A path's segments one line each, in order: the reference, and
        // whether it grows to its tangent chain.
        Some("ryucad.feature") if o.get("type").and_then(Value::as_str) == Some("path") => {
            if let Some(Value::Array(segments)) = rest.remove("segments") {
                for (i, s) in segments.iter().enumerate() {
                    let mut bare = s.clone();
                    let grown = bare
                        .as_object_mut()
                        .and_then(|m| m.remove("propagate"))
                        .is_some_and(|p| p == Value::Bool(true));
                    let text = reference(&bare).unwrap_or_else(|| scalar(&bare));
                    let tail = if grown { " (propagate)" } else { "" };
                    out.push(format!("segment {} = {text}{tail}", i + 1));
                }
            }
        }
        // A sweep's profile on one line (`sketch s3 (loops p7, l3)`, `face
        // base/<hex>`), and its law with its direction (`binormal z`).
        Some("ryucad.feature")
            if matches!(
                o.get("type").and_then(Value::as_str),
                Some("sweep" | "planar")
            ) =>
        {
            if let Some(p) = rest.remove("profile") {
                lines.push(format!("profile = {}", section_text(&p)));
            }
            if let Some(law) = rest.remove("law") {
                let mut line = format!("law = {}", scalar(&law));
                if let Some(d) = rest.remove("direction") {
                    line.push(' ');
                    line.push_str(&reference(&d).unwrap_or_else(|| scalar(&d)));
                }
                lines.push(line);
            }
        }
        // A loft's sections one line each, in order, as a sweep's profile
        // is written; each tangent on a line, its weight beside it.
        Some("ryucad.feature") if o.get("type").and_then(Value::as_str) == Some("loft") => {
            if let Some(Value::Array(sections)) = rest.remove("sections") {
                for (i, x) in sections.iter().enumerate() {
                    out.push(format!("section {} = {}", i + 1, section_text(x)));
                }
            }
            for k in ["start_tangent", "end_tangent"] {
                if let Some(Value::Object(t)) = rest.remove(k) {
                    let mut line = match t.get("direction") {
                        Some(d) => format!("{k} = {}", reference(d).unwrap_or_else(|| scalar(d))),
                        None => format!("{k} = {}", scalar(&Value::Object(t.clone()))),
                    };
                    if let Some(w) = t.get("weight") {
                        line.push_str(&format!(" (weight {})", scalar(w)));
                    }
                    lines.push(line);
                }
            }
        }
        // A sheet metal feature's allowance and relief each on a line
        // (`allowance = k_factor 0.44`, `relief = obround 1 mm × 1.5 mm`), a
        // flange's edges on one line, each end on its own
        // (`ends[plate/<hex>] = setback 5 mm, relief tear 0.5 mm`), an
        // unfold's overrides each on a line keyed by its bend
        // (`bends[f1/<hex>] = deduction 2.2 mm`), and an unfold's or
        // refold's `only` on one line.
        Some("ryucad.feature")
            if matches!(
                o.get("type").and_then(Value::as_str),
                Some("base_flange" | "flange" | "unfold" | "refold")
            ) =>
        {
            fn relief_text(r: &Value) -> String {
                let kind = r.get("kind").and_then(Value::as_str).unwrap_or_default();
                match (r.get("width"), r.get("depth")) {
                    (Some(w), Some(d)) => format!("{kind} {} × {}", scalar(w), scalar(d)),
                    (Some(w), None) => format!("{kind} {}", scalar(w)),
                    _ => kind.to_owned(),
                }
            }
            if let Some(Value::Object(a)) = rest.remove("allowance") {
                let text: Vec<String> = a
                    .iter()
                    .map(|(k, x)| format!("{k} {}", scalar(x)))
                    .collect();
                lines.push(format!("allowance = {}", text.join(", ")));
            }
            if let Some(r) = rest.remove("relief") {
                lines.push(format!("relief = {}", relief_text(&r)));
            }
            if let Some(Value::Array(edges)) = rest.remove("edges") {
                let text: Vec<String> = edges
                    .iter()
                    .map(|e| match e.get("edge") {
                        Some(x) => format!(
                            "{} (side {})",
                            reference(x).unwrap_or_else(|| scalar(x)),
                            reference(&e["side"]).unwrap_or_else(|| scalar(&e["side"]))
                        ),
                        None => reference(e).unwrap_or_else(|| scalar(e)),
                    })
                    .collect();
                lines.push(format!("edges = {}", text.join(", ")));
            }
            if let Some(Value::Array(ends)) = rest.remove("ends") {
                for e in &ends {
                    let v = reference(&e["vertex"]).unwrap_or_else(|| scalar(&e["vertex"]));
                    let mut parts = Vec::new();
                    if let Some(s) = e.get("setback") {
                        parts.push(format!("setback {}", scalar(s)));
                    }
                    if let Some(r) = e.get("relief") {
                        parts.push(format!("relief {}", relief_text(r)));
                    }
                    lines.push(format!("ends[{v}] = {}", parts.join(", ")));
                }
            }
            if let Some(Value::Array(only)) = rest.remove("only") {
                let text: Vec<String> = only
                    .iter()
                    .map(|r| reference(r).unwrap_or_else(|| scalar(r)))
                    .collect();
                lines.push(format!("only = {}", text.join(", ")));
            }
            if let Some(Value::Array(bends)) = rest.remove("bends") {
                for b in &bends {
                    let bend = reference(&b["bend"]).unwrap_or_else(|| scalar(&b["bend"]));
                    let text: Vec<String> = b
                        .as_object()
                        .into_iter()
                        .flatten()
                        .filter(|(k, _)| *k != "bend")
                        .map(|(k, x)| format!("{k} {}", scalar(x)))
                        .collect();
                    lines.push(format!("bends[{bend}] = {}", text.join(", ")));
                }
            }
        }
        // A fill's sides, one line each in order: `side 1 = wall/<hex>
        // (tangent)`, `side 2 = s2/a1`.
        Some("ryucad.feature") if o.get("type").and_then(Value::as_str) == Some("fill") => {
            if let Some(Value::Array(sides)) = rest.remove("sides") {
                for (i, s) in sides.iter().enumerate() {
                    let (r, tangent) = match (s.get("edge"), s.get("curve")) {
                        (Some(e), _) => (e, s.get("tangent") == Some(&Value::Bool(true))),
                        (None, Some(c)) => (c, false),
                        _ => (s, false),
                    };
                    let r = reference(r).unwrap_or_else(|| scalar(r));
                    let t = if tangent { " (tangent)" } else { "" };
                    lines.push(format!("side {} = {r}{t}", i + 1));
                }
            }
        }
        // A split's tool on one line: `tool = body parting`, or
        // `tool = faces k/<hex>, k/<hex>`.
        Some("ryucad.feature") if o.get("type").and_then(Value::as_str) == Some("split") => {
            if let Some(tool) = rest.remove("tool") {
                let line = match (tool.get("body"), tool.get("faces")) {
                    (Some(b), _) => format!("tool = body {}", scalar(b)),
                    (None, Some(f)) => {
                        let refs: Vec<String> = f
                            .as_array()
                            .into_iter()
                            .flatten()
                            .map(|r| reference(r).unwrap_or_else(|| scalar(r)))
                            .collect();
                        format!("tool = faces {}", refs.join(", "))
                    }
                    _ => format!("tool = {}", scalar(&tool)),
                };
                lines.push(line);
            }
        }
        // A thicken's source on one line, its groups one line each:
        // `from = sheet skin`, `faces[0] = 3 mm, 0 mm: skin/<hex>`.
        Some("ryucad.feature") if o.get("type").and_then(Value::as_str) == Some("thicken") => {
            if let Some(from) = rest.remove("from") {
                let refs = |v: &Value| -> String {
                    v.as_array()
                        .into_iter()
                        .flatten()
                        .map(|f| reference(f).unwrap_or_else(|| scalar(f)))
                        .collect::<Vec<_>>()
                        .join(", ")
                };
                let line = match (from.get("sheet"), from.get("faces")) {
                    (Some(s), _) => format!("from = sheet {}", scalar(s)),
                    (None, Some(f)) => format!("from = faces {}", refs(f)),
                    _ => format!("from = {}", scalar(&from)),
                };
                lines.push(line);
            }
            if let Some(Value::Array(groups)) = rest.remove("faces") {
                for (i, g) in groups.iter().enumerate() {
                    let faces: Vec<String> = g["faces"]
                        .as_array()
                        .into_iter()
                        .flatten()
                        .map(|f| reference(f).unwrap_or_else(|| scalar(f)))
                        .collect();
                    lines.push(format!(
                        "faces[{i}] = {}, {}: {}",
                        scalar(&g["front"]),
                        scalar(&g["back"]),
                        faces.join(", ")
                    ));
                }
            }
        }
        // A tweak's groups, each on one line: a move's
        // `groups[0] = 1 mm: base/<hex>, base/<hex>`, a replacement's
        // `groups[0] = lid/<hex>: base/<hex>` (its surface first), a
        // draft's `groups[0] = 5 deg: base/<hex>`, a resize's
        // `groups[0] = 3 mm: base/<hex>` or
        // `groups[1] = distances 1 mm, 2 mm from base/<hex>: base/<hex>`.
        Some("ryucad.feature")
            if matches!(
                o.get("type").and_then(Value::as_str),
                Some("move_face" | "replace_face" | "draft" | "resize_blend")
            ) =>
        {
            if let Some(Value::Array(groups)) = rest.remove("groups") {
                for (i, g) in groups.iter().enumerate() {
                    let head = if o.get("type").and_then(Value::as_str) == Some("resize_blend") {
                        resize_head(g)
                    } else {
                        g.get("distance")
                            .or_else(|| g.get("angle"))
                            .or_else(|| g.get("with"))
                            .map(|x| reference(x).unwrap_or_else(|| scalar(x)))
                            .unwrap_or_default()
                    };
                    let faces: Vec<String> = g["faces"]
                        .as_array()
                        .into_iter()
                        .flatten()
                        .map(|f| reference(f).unwrap_or_else(|| scalar(f)))
                        .collect();
                    lines.push(format!("groups[{i}] = {head}: {}", faces.join(", ")));
                }
            }
        }
        // A shell's or offset's face groups, each on one line:
        // `faces[0] = 4 mm: base/<hex>, base/<hex>`.
        Some("ryucad.feature")
            if matches!(
                o.get("type").and_then(Value::as_str),
                Some("shell" | "offset")
            ) =>
        {
            if let Some(Value::Array(groups)) = rest.remove("faces") {
                for (i, g) in groups.iter().enumerate() {
                    let q = g
                        .get("thickness")
                        .or_else(|| g.get("distance"))
                        .map(scalar)
                        .unwrap_or_default();
                    let faces: Vec<String> = g["faces"]
                        .as_array()
                        .into_iter()
                        .flatten()
                        .map(|f| reference(f).unwrap_or_else(|| scalar(f)))
                        .collect();
                    lines.push(format!("faces[{i}] = {q}: {}", faces.join(", ")));
                }
            }
        }
        // A flattened import's place on one line: the usages from a root
        // down (each with its ordinal past the first, and the product it
        // places), then the pose, in meters as written.
        Some("ryucad.feature") if o.get("type").and_then(Value::as_str) == Some("import") => {
            if let Some(p) = rest.remove("place") {
                let via: Vec<String> = p["via"]
                    .as_array()
                    .into_iter()
                    .flatten()
                    .map(|u| {
                        let mut t = u["usage"].as_str().unwrap_or_default().to_owned();
                        if let Some(n) = u.get("ordinal") {
                            t.push_str(&format!("#{n}"));
                        }
                        format!("{t} of {}", u["product"]["id"].as_str().unwrap_or_default())
                    })
                    .collect();
                let list = |k: &str| {
                    let v: Vec<String> = p["pose"][k]
                        .as_array()
                        .into_iter()
                        .flatten()
                        .map(scalar)
                        .collect();
                    v.join(", ")
                };
                lines.push(format!(
                    "place = {} at ({}) m, z [{}], x [{}]",
                    via.join(" > "),
                    list("origin_m"),
                    list("z"),
                    list("x")
                ));
            }
        }
        // An instance's revision in words, its pose last: solver state.
        Some("ryucad.feature") if o.get("type").and_then(Value::as_str) == Some("instance") => {
            let project = rest.remove("project");
            if let Some(rev) = rest.remove("rev") {
                let rev = match (rev.get("label").and_then(Value::as_str), &project) {
                    (Some(l), Some(p)) => format!(
                        "{}:{} @ label {l}",
                        named(p.as_str().unwrap_or_default()),
                        o.get("part").map(scalar).unwrap_or_default()
                    ),
                    (Some(l), None) => format!("label {l}"),
                    _ => scalar(&rev),
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
        Some(crate::doc::materials::KIND) => {
            if let Some(Value::Object(materials)) = rest.remove("materials") {
                for (name, m) in &materials {
                    let field = |k: &str| m.get(k).map(scalar);
                    let mut line = format!(
                        "materials.{name} = density {}",
                        field("density").unwrap_or_default()
                    );
                    if let Some(c) = field("colour") {
                        line.push_str(&format!(", colour {c}"));
                    }
                    lines.push(line);
                }
            }
        }
        // Each entry on one line: what it sets, in a fixed order.
        Some(crate::doc::appearance::KIND) => {
            if let Some(p) = rest.remove("part") {
                lines.push(format!("part = {}", look(&p)));
            }
            for key in ["solids", "faces", "occurrences"] {
                if let Some(Value::Object(entries)) = rest.remove(key) {
                    for (k, l) in &entries {
                        lines.push(format!("{key}.{k} = {}", look(l)));
                    }
                }
            }
        }
        // Each sheet and view on one line: `views.v2 = projected from v1,
        // top, offset 70 mm, sheet s1`.
        Some(crate::doc::drawing::KIND) => {
            for key in ["sheets", "views", "annotations"] {
                if let Some(Value::Object(members)) = rest.remove(key) {
                    let mut members: Vec<(&String, &Value)> = members.iter().collect();
                    members.sort_by_key(|(k, _)| member_order(k));
                    for (k, m) in members {
                        let line = match key {
                            "sheets" => drawing_sheet(m),
                            "views" => drawing_view(m),
                            _ => drawing_annotation(m),
                        };
                        out.push(format!("{key}.{k} = {line}"));
                    }
                }
            }
        }
        // Each step on one line, in order: what it moves, then how.
        Some(crate::doc::explode::KIND) => {
            if let Some(Value::Array(steps)) = rest.remove("steps") {
                for (i, s) in steps.iter().enumerate() {
                    out.push(format!("step {} = {}", i + 1, explode_step(s)));
                }
            }
        }
        Some(ryupdm::config::KIND) => {
            if let Some(Value::Object(decls)) = rest.remove("properties") {
                for (name, d) in &decls {
                    lines.push(format!("properties.{name} = {}", declaration(d)));
                }
            }
            if let Some(Value::Object(mut a)) = rest.remove("approvals") {
                if let Some(Value::Array(rules)) = a.remove("rules") {
                    for (i, r) in rules.iter().enumerate() {
                        let line = ryupdm::approvals::Rule::from_json(r, "")
                            .map_or_else(|_| r.to_string(), |r| r.text());
                        lines.push(format!("approvals.rules.{i} = {line}"));
                    }
                }
                if let Some(Value::Object(keys)) = a.remove("keys") {
                    for (id, k) in &keys {
                        // A key, or `{key, revoked}` for one revoked on a day.
                        let line = match (k.as_str(), &k["key"], &k["revoked"]) {
                            (Some(hex), _, _) => hex.to_owned(),
                            (None, Value::String(hex), Value::String(day)) => {
                                format!("{hex} (revoked {day})")
                            }
                            _ => k.to_string(),
                        };
                        lines.push(format!("approvals.keys.{id} = {line}"));
                    }
                }
                // Anything else a newer file holds, field by field.
                if !a.is_empty() {
                    flatten("approvals", &Value::Object(a), &mut lines);
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
        Some(crate::doc::annotation::KIND) => {
            let who = |b: &Value| {
                format!(
                    "{} ({})",
                    b["participant"].as_str().unwrap_or_default(),
                    b["kind"].as_str().unwrap_or_default()
                )
            };
            for key in ["by", "claimed_by"] {
                match rest.remove(key) {
                    Some(b @ Value::Object(_)) => lines.push(format!("{key} = {}", who(&b))),
                    Some(Value::Null) => lines.push(format!("{key} = none")),
                    _ => {}
                }
            }
            if let Some(Value::Array(pins)) = rest.remove("pins") {
                for (i, p) in pins.iter().enumerate() {
                    let at = p["point_m"]
                        .as_array()
                        .map(|a| a.iter().map(scalar).collect::<Vec<_>>().join(", "));
                    let what = p["ref"].as_str().or(p["part"].as_str()).unwrap_or_default();
                    lines.push(match at {
                        Some(at) => format!("pin {} = {what} at {at}", i + 1),
                        None => format!("pin {} = {what}", i + 1),
                    });
                }
            }
            if let Some(Value::Object(marks)) = rest.remove("marks") {
                let mm = |x: &Value| crate::measure::number(x.as_f64().unwrap_or(0.0) * 1000.0);
                let pt = |p: &Value| {
                    let c: Vec<String> = p.as_array().into_iter().flatten().map(mm).collect();
                    format!("({})", c.join(", "))
                };
                // A stroke's box: each coordinate's least and greatest.
                let span = |pts: &[Value]| {
                    let n = pts.first().and_then(Value::as_array).map_or(0, Vec::len);
                    let (lo, hi): (Vec<f64>, Vec<f64>) = (0..n)
                        .map(|k| {
                            let xs = pts.iter().filter_map(|p| p[k].as_f64());
                            let lo = xs.clone().fold(f64::INFINITY, f64::min);
                            (lo, xs.fold(f64::NEG_INFINITY, f64::max))
                        })
                        .unzip();
                    (pt(&json!(lo)), pt(&json!(hi)))
                };
                if let Some(p) = marks.get("part") {
                    lines.push(format!("marks.part = {}", scalar(p)));
                }
                for (i, m) in marks
                    .get("list")
                    .and_then(Value::as_array)
                    .into_iter()
                    .flatten()
                    .enumerate()
                {
                    let kind = m["kind"].as_str().unwrap_or_default();
                    let pts: Vec<Value> = m["xy_m"]
                        .as_array()
                        .or(m["points_m"].as_array())
                        .cloned()
                        .unwrap_or_default();
                    let mut line = match (kind, m["in"].as_str()) {
                        ("text", _) => format!(
                            "text in view {} mm: {}",
                            pts.first().map(pt).unwrap_or_default(),
                            m["text"].as_str().unwrap_or_default()
                        ),
                        ("stroke", Some("face")) => {
                            let (lo, hi) = span(&pts);
                            format!(
                                "stroke on {}, {} points, {lo} mm to {hi} mm",
                                m["on"].as_str().unwrap_or_default(),
                                pts.len()
                            )
                        }
                        ("stroke", _) => {
                            let (lo, hi) = span(&pts);
                            format!("stroke in view, {} points, {lo} mm to {hi} mm", pts.len())
                        }
                        ("line" | "arrow", _) if pts.len() == 2 => {
                            format!("{kind} in view {} mm -> {} mm", pt(&pts[0]), pt(&pts[1]))
                        }
                        _ if pts.len() == 2 => {
                            format!("{kind} in view {} mm to {} mm", pt(&pts[0]), pt(&pts[1]))
                        }
                        _ => format!("{kind}, {} points", pts.len()),
                    };
                    let touches: Vec<String> = m["touches"]
                        .as_array()
                        .into_iter()
                        .flatten()
                        .map(|t| {
                            format!(
                                "{} {}",
                                t["at"].as_str().unwrap_or_default(),
                                t["ref"].as_str().unwrap_or_default()
                            )
                        })
                        .collect();
                    if !touches.is_empty() {
                        line.push_str(&format!("; {}", touches.join(", ")));
                    }
                    lines.push(format!("marks[{i}] = {line}"));
                }
            }
            match rest.remove("change") {
                Some(Value::Object(c)) => {
                    let mut line = format!(
                        "change = {}",
                        c.get("request")
                            .and_then(|r| r["method"].as_str())
                            .unwrap_or_default()
                    );
                    if c.get("rough") == Some(&Value::Bool(true)) {
                        line.push_str(" (rough)");
                    }
                    let after: Vec<String> = c
                        .get("after")
                        .and_then(Value::as_array)
                        .into_iter()
                        .flatten()
                        .map(scalar)
                        .collect();
                    if !after.is_empty() {
                        line.push_str(&format!(", after {}", after.join(", ")));
                    }
                    lines.push(line);
                    if let Some(Value::Object(w)) = c.get("writes") {
                        let paths: Vec<&str> = w.keys().map(String::as_str).collect();
                        lines.push(format!("change.writes = {}", paths.join(", ")));
                    }
                    if let Some(r) = c.get("request") {
                        last.push(format!(
                            "change.request = {}",
                            crate::doc::canonical::to_string(r)
                        ));
                    }
                }
                Some(Value::Null) => lines.push("change = none".to_owned()),
                _ => {}
            }
            if let Some(Value::Object(replies)) = rest.remove("replies") {
                for (id, r) in &replies {
                    lines.push(format!(
                        "reply {id} = {} {}: {}",
                        who(&r["by"]),
                        r["at"].as_str().unwrap_or_default(),
                        r["text"].as_str().unwrap_or_default()
                    ));
                }
            }
            if let Some(r) = rest.remove("resolution") {
                lines.push(match r {
                    Value::Null => "resolution = none".to_owned(),
                    r => format!(
                        "resolution = {} {}{}{}",
                        who(&r["by"]),
                        r["at"].as_str().unwrap_or_default(),
                        if r["applied"] == true {
                            ", applied"
                        } else {
                            ""
                        },
                        r["note"]
                            .as_str()
                            .map(|n| format!(": {n}"))
                            .unwrap_or_default()
                    ),
                });
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

/// An appearance entry on one line: `colour #d03030, opacity 0.4`, or
/// `material colour`.
fn look(v: &Value) -> String {
    let mut parts = Vec::new();
    if let Some(c) = v.get("colour") {
        parts.push(format!("colour {}", scalar(c)));
    }
    if v.get("material") == Some(&Value::Bool(true)) {
        parts.push("material colour".to_owned());
    }
    if let Some(o) = v.get("opacity") {
        parts.push(format!("opacity {}", scalar(o)));
    }
    let known = ["colour", "material", "opacity"];
    if let Some(o) = v.as_object() {
        for (k, x) in o.iter().filter(|(k, _)| !known.contains(&k.as_str())) {
            parts.push(format!("{k} {}", scalar(x)));
        }
    }
    parts.join(", ")
}

/// A drawing member's key in its natural order: `v2` before `v10`.
fn member_order(k: &str) -> (String, u64) {
    let digits = k.trim_start_matches(|c: char| c.is_ascii_alphabetic());
    (
        k[..k.len() - digits.len()].to_owned(),
        digits.parse().unwrap_or(u64::MAX),
    )
}

/// The fields of a drawing member not yet written, `name value`, in key
/// order.
fn rest_of(m: &Value, done: &[&str]) -> Vec<String> {
    m.as_object()
        .into_iter()
        .flatten()
        .filter(|(k, _)| !done.contains(&k.as_str()))
        .map(|(k, v)| {
            let text = match v.as_array() {
                Some(a) => a.iter().map(scalar).collect::<Vec<_>>().join(", "),
                None => reference(v).unwrap_or_else(|| scalar(v)),
            };
            format!("{k} {text}")
        })
        .collect()
}

/// A drawing's annotation on one line: its type, its view or sheet and
/// its entities (`linear v1 b/<hex> to b/<hex>`), then the rest of its
/// fields in key order, a tolerance as its form and values.
fn drawing_annotation(m: &Value) -> String {
    let ty = m.get("type").and_then(Value::as_str).unwrap_or_default();
    let on = m
        .get("view")
        .or_else(|| m.get("sheet"))
        .map(scalar)
        .unwrap_or_default();
    let ends: Vec<String> = ["a", "b", "datum", "of", "on", "to", "vertex", "leader"]
        .iter()
        .filter_map(|k| m.get(*k).map(|v| format!("{k} {}", scalar(v))))
        .collect();
    let mut parts = vec![format!("{ty} {on}").trim().to_owned()];
    parts.extend(ends);
    if let Some(t) = m.get("tolerance").and_then(Value::as_object) {
        for (k, v) in t {
            let v = match v.as_array() {
                Some(a) => a.iter().map(scalar).collect::<Vec<_>>().join(" "),
                None => scalar(v),
            };
            parts.push(format!("tolerance {k} {v}"));
        }
    }
    if let Some(Value::Array(rows)) = m.get("rows") {
        for (i, r) in rows.iter().enumerate() {
            parts.push(format!("row {} {}", i + 1, r));
        }
    }
    parts.extend(rest_of(
        m,
        &[
            "type",
            "view",
            "sheet",
            "a",
            "b",
            "datum",
            "of",
            "on",
            "to",
            "vertex",
            "leader",
            "tolerance",
            "rows",
        ],
    ));
    parts.join(", ")
}

/// A drawing's sheet on one line: `Sheet 1, ansi_b, scale 1:2`.
fn drawing_sheet(m: &Value) -> String {
    let mut parts = vec![
        m.get("name").map(scalar).unwrap_or_default(),
        m.get("size").map(scalar).unwrap_or_default(),
    ];
    parts.extend(rest_of(m, &["name", "size"]));
    parts.join(", ")
}

/// A drawing's view on one line: what it is and where from, then the
/// rest of its fields in key order.
fn drawing_view(m: &Value) -> String {
    let ty = m.get("type").and_then(Value::as_str).unwrap_or_default();
    let (head, done): (String, &[&str]) = match ty {
        "base" => {
            let orient = match m.get("orient") {
                Some(Value::Object(o)) => o
                    .get("normal_to")
                    .map(|r| format!("normal to {}", reference(r).unwrap_or_else(|| scalar(r))))
                    .unwrap_or_default(),
                Some(o) => scalar(o),
                None => String::new(),
            };
            let of = match (m.get("of"), m.get("rev").and_then(|r| r.get("label"))) {
                (Some(p), Some(l)) => format!(" of {} @ label {}", scalar(p), scalar(l)),
                (Some(p), None) => format!(" of {}", scalar(p)),
                (None, Some(l)) => format!(" @ label {}", scalar(l)),
                (None, None) => String::new(),
            };
            (
                format!("base {orient}{of}"),
                &["type", "orient", "of", "rev"],
            )
        }
        "projected" | "auxiliary" => {
            let from = m.get("from").map(scalar).unwrap_or_default();
            let how = m
                .get("side")
                .map(scalar)
                .or_else(|| {
                    m.get("normal_to")
                        .map(|r| format!("normal to {}", reference(r).unwrap_or_else(|| scalar(r))))
                })
                .unwrap_or_default();
            (
                format!("{ty} from {from}, {how}"),
                &["type", "from", "side", "normal_to"],
            )
        }
        "section" => {
            let from = m.get("from").map(scalar).unwrap_or_default();
            let letter = m.get("label").map(scalar).unwrap_or_default();
            let point = |p: &Value| match p {
                Value::Array(a) => a.iter().map(scalar).collect::<Vec<_>>().join(", "),
                p => reference(p).unwrap_or_else(|| scalar(p)),
            };
            let cut = match m.get("cut") {
                Some(c) if c.get("plane").is_some() => {
                    let mut t = format!("on {}", point(&c["plane"]));
                    if let Some(o) = c.get("offset") {
                        t.push_str(&format!(" offset {}", scalar(o)));
                    }
                    t
                }
                Some(c) if c.get("through").is_some() => format!(
                    "through {}",
                    c["through"]
                        .as_array()
                        .into_iter()
                        .flatten()
                        .map(|p| format!("({})", point(p)))
                        .collect::<Vec<_>>()
                        .join(" ")
                ),
                Some(c) => format!(
                    "at {} normal {}",
                    point(c.get("origin").unwrap_or(&Value::Null)),
                    point(c.get("normal").unwrap_or(&Value::Null))
                ),
                None => String::new(),
            };
            (
                format!("section {letter} from {from}, {cut}"),
                &["type", "from", "label", "cut"],
            )
        }
        "detail" => {
            let from = m.get("from").map(scalar).unwrap_or_default();
            let letter = m.get("label").map(scalar).unwrap_or_default();
            let centre = match m.get("centre") {
                Some(Value::Array(a)) => a.iter().map(scalar).collect::<Vec<_>>().join(", "),
                Some(p) => reference(p).unwrap_or_else(|| scalar(p)),
                None => String::new(),
            };
            let r = m.get("radius").map(scalar).unwrap_or_default();
            (
                format!("detail {letter} from {from}, about {centre} radius {r}"),
                &["type", "from", "label", "centre", "radius"],
            )
        }
        "flat" => {
            let of = m
                .get("of")
                .map(|p| format!(" of {}", scalar(p)))
                .unwrap_or_default();
            let solid = m
                .get("solid")
                .map(|p| format!(" solid {}", scalar(p)))
                .unwrap_or_default();
            (format!("flat{of}{solid}"), &["type", "of", "solid"])
        }
        "exploded" => {
            let e = m.get("explode").map(scalar).unwrap_or_default();
            let of = m
                .get("of")
                .map(|p| format!(" of {}", scalar(p)))
                .unwrap_or_default();
            let orient = match m.get("orient") {
                Some(Value::Object(o)) => o
                    .get("normal_to")
                    .map(|r| format!("normal to {}", reference(r).unwrap_or_else(|| scalar(r))))
                    .unwrap_or_default(),
                Some(o) => scalar(o),
                None => String::new(),
            };
            (
                format!("exploded {e}{of}, {orient}"),
                &["type", "explode", "of", "orient"],
            )
        }
        _ => (scalar(&json!(ty)), &["type"]),
    };
    let mut parts = vec![head];
    parts.extend(rest_of(m, done));
    parts.join(", ")
}

/// An explode's step on one line: `lid, p1/left: translate 0 mm, 0 mm,
/// 40 mm; rotate 90 deg about [0, 0, 1]`.
fn explode_step(v: &Value) -> String {
    let list = |v: Option<&Value>| match v {
        Some(Value::Array(a)) => a.iter().map(scalar).collect::<Vec<_>>().join(", "),
        Some(v) => scalar(v),
        None => String::new(),
    };
    let mut how = Vec::new();
    if let Some(t) = v.get("translate") {
        how.push(format!("translate {}", list(Some(t))));
    }
    if let Some(r) = v.get("rotate") {
        let angle = r.get("angle").map(scalar).unwrap_or_default();
        let axis = r.get("axis").map(|a| a.to_string()).unwrap_or_default();
        how.push(format!("rotate {angle} about {axis}"));
    }
    let known = ["instances", "translate", "rotate"];
    if let Some(o) = v.as_object() {
        for (k, x) in o.iter().filter(|(k, _)| !known.contains(&k.as_str())) {
            how.push(format!("{k} {}", scalar(x)));
        }
    }
    format!("{}: {}", list(v.get("instances")), how.join("; "))
}

/// A leaf value on its line: strings bare, everything else as JSON.
/// A resize group's size as one text: a fillet's `3 mm`, a chamfer's
/// `equal 1 mm`, `distances 1 mm, 2 mm from base/<hex>` or
/// `distance_angle 1 mm, 30 deg from base/<hex>`.
fn resize_head(g: &Value) -> String {
    if let Some(r) = g.get("radius") {
        return scalar(r);
    }
    let form = g.get("form").map(scalar).unwrap_or_default();
    let sizes: Vec<String> = ["d", "d1", "d2", "angle"]
        .iter()
        .filter_map(|k| g.get(*k))
        .map(scalar)
        .collect();
    let mut head = format!("{form} {}", sizes.join(", "));
    if let Some(f) = g.get("face") {
        head.push_str(&format!(
            " from {}",
            reference(f).unwrap_or_else(|| scalar(f))
        ));
    }
    head
}

fn scalar(v: &Value) -> String {
    match v {
        Value::String(s) => s.clone(),
        v => v.to_string(),
    }
}

/// A reference to another feature's entity, as `<feature>/<name>`; a
/// whole feature (a datum) as `<feature>`, a sketch's entity as
/// `<sketch>/<entity>`.
fn reference(v: &Value) -> Option<String> {
    let o = v.as_object()?;
    let f = o.get("feature")?.as_str()?;
    match o.len() {
        1 => Some(f.to_owned()),
        2 => {
            let n = o.get("name").or_else(|| o.get("entity"))?.as_str()?;
            Some(format!("{f}/{n}"))
        }
        _ => None,
    }
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
        // A fillet's or chamfer's edges, or a datum's points, one value
        // as a merge sees them.
        Value::Array(a)
            if !a.is_empty()
                && a.iter()
                    .all(|x| reference(x).is_some() || (!x.is_object() && !x.is_array())) =>
        {
            let items: Vec<String> = a
                .iter()
                .map(|x| reference(x).unwrap_or_else(|| scalar(x)))
                .collect();
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

/// A connector as one line: `base : slab/<name>`, `pin : origin`, then
/// `offset origin 0 mm, 0 mm, 2 mm rotate 90 deg flip` for what it has.
fn connector(c: &Value) -> String {
    let path: Vec<&str> = c["instance"]
        .as_array()
        .map(|a| a.iter().filter_map(Value::as_str).collect())
        .unwrap_or_default();
    let at = match &c["at"] {
        Value::String(s) => s.clone(),
        r => format!(
            "{}/{}",
            r["feature"].as_str().unwrap_or_default(),
            r["name"].as_str().unwrap_or_default()
        ),
    };
    let mut out = format!("{} : {at}", path.join("/"));
    if let Some(o) = c.get("offset") {
        out.push_str(" offset");
        if let Some(a) = o["origin"].as_array() {
            let parts: Vec<String> = a.iter().map(scalar).collect();
            out.push_str(&format!(" origin {}", parts.join(", ")));
        }
        if let Some(r) = o.get("rotate") {
            out.push_str(&format!(" rotate {}", scalar(r)));
        }
        if o["flip"] == true {
            out.push_str(" flip");
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
/// An assembly's item numbers by part, as written.
fn items(files: &Files, part: &str) -> Map<String, Value> {
    json_at(files, &format!("parts/{part}/part.json"))
        .and_then(|v| v["items"].as_object().cloned())
        .unwrap_or_default()
}

/// A part's named views as written, by ID: the files of
/// `parts/<part>/views/` that read as JSON.
fn views(files: &Files, part: &str) -> BTreeMap<String, Value> {
    folder(files, part, crate::doc::views::VIEWS)
}

/// The files of a part's folder `dir` that read as JSON, by their names
/// less `.json`.
fn folder(files: &Files, part: &str, dir: &str) -> BTreeMap<String, Value> {
    let dir = format!("parts/{part}/{dir}/");
    files
        .keys()
        .filter_map(|p| {
            let id = p.strip_prefix(&dir)?.strip_suffix(".json")?;
            (!id.contains('/')).then(|| Some((id.to_owned(), json_at(files, p)?)))?
        })
        .collect()
}

/// Files of one folder, each `added`, `removed` or `changed` by pointer
/// (named by `what`); `None` when neither side has any.
fn folder_diff(
    a: &BTreeMap<String, Value>,
    b: &BTreeMap<String, Value>,
    what: &str,
) -> Option<Value> {
    if a.is_empty() && b.is_empty() {
        return None;
    }
    let keys: BTreeSet<&String> = a.keys().chain(b.keys()).collect();
    let (mut added, mut removed, mut changed) = (Vec::new(), Vec::new(), Vec::new());
    for k in keys {
        match (a.get(k), b.get(k)) {
            (None, Some(_)) => added.push(json!(k)),
            (Some(_), None) => removed.push(json!(k)),
            (Some(f), Some(t)) if f != t => {
                let mut list = Vec::new();
                changes("", Some(f), Some(t), &mut list);
                changed.push(json!({what: k, "changes": list}));
            }
            _ => {}
        }
    }
    Some(json!({"added": added, "removed": removed, "changed": changed}))
}

fn parameters(files: &Files, part: &str) -> Map<String, Value> {
    parameters_at(files, &format!("parts/{part}/parameters.json"))
}

/// The parameters of the file at `at` as written, by name.
fn parameters_at(files: &Files, at: &str) -> Map<String, Value> {
    json_at(files, at)
        .and_then(|v| v["parameters"].as_object().cloned())
        .unwrap_or_default()
}

/// Two sides' parameters as written: `added`, `removed` and `changed`,
/// by name.
fn parameter_changes(qa: &Map<String, Value>, qb: &Map<String, Value>) -> Value {
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
    json!({"added": added, "removed": removed, "changed": changed})
}

/// Gives each entry of a `parameters` block `read_by: {from, to}`: who
/// reaches the parameter on each side (features, explodes as
/// `explodes/<id>`, or for the project's, parts), through other
/// parameters too (`doc::reads`). `unused: true` when neither side has
/// any, so a reviewer sees the change moves nothing.
fn mark_read_by(block: &mut Value, from: &ReadBy, to: &ReadBy) {
    for k in ["added", "removed", "changed"] {
        for e in block[k].as_array_mut().into_iter().flatten() {
            let name = e["name"].as_str().unwrap_or_default().to_owned();
            let side = |by: &ReadBy| -> Vec<String> {
                by.get(&name)
                    .map(|s| s.iter().cloned().collect())
                    .unwrap_or_default()
            };
            let (f, t) = (side(from), side(to));
            if f.is_empty() && t.is_empty() {
                e["unused"] = json!(true);
            }
            e["read_by"] = json!({"from": f, "to": t});
        }
    }
}

/// Whether a `parameters` block lists anything.
fn lists_any(block: &Value) -> bool {
    ["added", "removed", "changed"]
        .iter()
        .any(|k| block[k].as_array().is_some_and(|a| !a.is_empty()))
}

/// Who reaches each of `part`'s own parameters in `files`.
fn part_read_by(files: &Files, part: &crate::doc::id::Id) -> ReadBy {
    let project = reads::parameters_in(files, crate::doc::project::PARAMETERS);
    let (own, readers) = reads::readers_in(files, part);
    reads::reach_part(&own, &project, &readers).0
}

/// What changed between two versions of a project's files, `from` and
/// `to`: the manifest, and per part its feature order, its parameters as
/// written, each with who reads it on each side (`read_by`, and
/// `unused`), an assembly's item numbers by part (`items`, when either side
/// has any), its named views (`views`, when either side has any: each
/// view added, removed or changed by pointer), and each feature by JSON
/// pointer (a sketch by entity and constraint, its solved positions
/// apart). A part whose PDM differs has
/// `pdm` too (see [`pdm`]), its states read against `releases`, the
/// release branch as read; a change to `ryupdm.json` is `pdm` at the top,
/// by pointer, as the manifest's is (settings that read with their
/// defaults filled in, so no file is the defaults). A lock whose labels
/// differ gives `deps` at the top ([`lock_changes`]); the manifest's
/// dependencies are its changes by pointer.
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
        let mut parameters = parameter_changes(&parameters(from, part), &parameters(to, part));
        if lists_any(&parameters)
            && let Ok(id) = crate::doc::id::Id::parse(part, "part")
        {
            mark_read_by(
                &mut parameters,
                &part_read_by(from, &id),
                &part_read_by(to, &id),
            );
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
            "parameters": parameters,
            "features": features,
        });
        let (ia, ib) = (items(from, part), items(to, part));
        if !ia.is_empty() || !ib.is_empty() {
            let keys: BTreeSet<&String> = ia.keys().chain(ib.keys()).collect();
            let (mut added, mut removed, mut changed) = (Vec::new(), Vec::new(), Vec::new());
            for k in keys {
                match (ia.get(k), ib.get(k)) {
                    (None, Some(t)) => added.push(json!({"part": k, "to": t})),
                    (Some(f), None) => removed.push(json!({"part": k, "from": f})),
                    (Some(f), Some(t)) if f != t => {
                        changed.push(json!({"part": k, "from": f, "to": t}));
                    }
                    _ => {}
                }
            }
            v["items"] = json!({"added": added, "removed": removed, "changed": changed});
        }
        // Named views and explodes, each by ID, when either side has
        // any: no result changes, so never under `--model`.
        let explodes = folder_diff(
            &folder(from, part, crate::doc::explode::EXPLODES),
            &folder(to, part, crate::doc::explode::EXPLODES),
            "explode",
        );
        if let Some(d) = folder_diff(&views(from, part), &views(to, part), "view") {
            v["views"] = d;
        }
        if let Some(d) = explodes {
            v["explodes"] = d;
        }
        // Its drawings, each by ID, likewise.
        if let Some(d) = folder_diff(
            &folder(from, part, crate::doc::drawing::DRAWINGS),
            &folder(to, part, crate::doc::drawing::DRAWINGS),
            "drawing",
        ) {
            v["drawings"] = d;
        }
        // Its appearance by pointer, when either side has one: no result
        // changes, so never under `--model`.
        let at = format!("parts/{part}/{}", crate::doc::appearance::APPEARANCE);
        let (aa, ab) = (json_at(from, &at), json_at(to, &at));
        if aa.is_some() || ab.is_some() {
            let mut list = Vec::new();
            changes("", aa.as_ref(), ab.as_ref(), &mut list);
            v["appearance"] = json!({ "changes": list });
        }
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
    // The project's parameters by name, when they differ, with the parts
    // that read any that changed on either side: what they rebuild. Each
    // entry says who reads it on each side (`read_by`), and `unused` when
    // nothing does: `parts` is what rebuilds (its revision hashes the
    // names), `read_by` what the model reads.
    let (qa, qb) = (
        parameters_at(from, crate::doc::project::PARAMETERS),
        parameters_at(to, crate::doc::project::PARAMETERS),
    );
    if qa != qb {
        let mut p = parameter_changes(&qa, &qb);
        let moved: BTreeSet<&String> = qa
            .keys()
            .chain(qb.keys())
            .filter(|k| qa.get(*k) != qb.get(*k))
            .collect();
        let reading: BTreeSet<&String> = pa
            .keys()
            .chain(pb.keys())
            .filter(|part| {
                let Ok(id) = crate::doc::id::Id::parse(part, "part") else {
                    return false;
                };
                [from, to].iter().any(|files| {
                    crate::doc::project::reads_in(*files, &id)
                        .iter()
                        .any(|n| moved.contains(n))
                })
            })
            .collect();
        p["parts"] = json!(reading);
        mark_read_by(
            &mut p,
            &reads::in_source(from).project,
            &reads::in_source(to).project,
        );
        v["parameters"] = p;
    }
    let mut materials = Vec::new();
    let at = |files: &Files| json_at(files, crate::doc::materials::MATERIALS);
    changes("", at(from).as_ref(), at(to).as_ref(), &mut materials);
    if !materials.is_empty() {
        v["materials"] = json!({ "changes": materials });
    }
    let imports = imports(from, to);
    if !imports.is_empty() {
        v["imports"] = json!(imports);
    }
    if let Some(d) = lock_changes(from, to) {
        v["deps"] = d;
    }
    if let Some(a) = annotation_changes(from, to) {
        v["annotations"] = a;
    }
    // The project's drawing templates, by ID, when either side has any.
    let templates = |files: &Files| -> BTreeMap<String, Value> {
        let dir = format!("{}/", crate::doc::drawing_template::TEMPLATES);
        files
            .keys()
            .filter_map(|p| {
                let id = p.strip_prefix(&dir)?.strip_suffix(".json")?;
                (!id.contains('/')).then(|| Some((id.to_owned(), json_at(files, p)?)))?
            })
            .collect()
    };
    if let Some(d) = folder_diff(&templates(from), &templates(to), "template") {
        v["templates"] = d;
    }
    v
}

/// Annotations added, removed and changed (by JSON pointer), by ID;
/// `None` when none differs. A file that doesn't read is compared as
/// its bytes' JSON, or not at all.
fn annotation_changes(from: &Files, to: &Files) -> Option<Value> {
    let of = |files: &Files| -> BTreeMap<String, Option<Value>> {
        files
            .iter()
            .filter(|(p, _)| crate::doc::annotation::is_annotation_path(p))
            .map(|(p, _)| {
                let id = p["annotations/".len()..p.len() - ".json".len()].to_owned();
                (id, json_at(files, p))
            })
            .collect()
    };
    let (a, b) = (of(from), of(to));
    let added: Vec<&String> = b.keys().filter(|k| !a.contains_key(*k)).collect();
    let removed: Vec<&String> = a.keys().filter(|k| !b.contains_key(*k)).collect();
    let mut changed = Map::new();
    for (id, x) in &a {
        if let Some(y) = b.get(id)
            && x != y
        {
            let mut list = Vec::new();
            changes("", x.as_ref(), y.as_ref(), &mut list);
            changed.insert(id.clone(), json!(list));
        }
    }
    if added.is_empty() && removed.is_empty() && changed.is_empty() {
        return None;
    }
    Some(json!({"added": added, "removed": removed, "changed": changed}))
}

/// What changed in `ryucad.lock`, by label: `{added, removed, changed}`,
/// each `{project, dependency?, part, label, commit}` (a change `from` and
/// `to` commits), named as either side's manifest names the project;
/// `None` when no label changed. A lock that doesn't read is no labels.
fn lock_changes(from: &Files, to: &Files) -> Option<Value> {
    use crate::doc::lock::{LOCK, Lock};
    let labels = |files: &Files| -> BTreeMap<(String, String), String> {
        json_at(files, LOCK)
            .and_then(|v| Lock::from_json(&v).ok())
            .map(|l| {
                l.projects
                    .iter()
                    .flat_map(|(p, locked)| {
                        locked.labels.iter().map(move |((part, label), x)| {
                            ((p.to_string(), format!("{part}/{label}")), x.commit.clone())
                        })
                    })
                    .collect()
            })
            .unwrap_or_default()
    };
    let (a, b) = (labels(from), labels(to));
    let mut names = BTreeMap::new();
    for files in [from, to] {
        if let Some(m) = json_at(files, crate::doc::project::MANIFEST) {
            names.extend(dependency_names(&m));
        }
    }
    let row = |(project, key): &(String, String)| {
        let (part, label) = key.split_once('/').unwrap_or_default();
        let mut v = json!({"project": project, "part": part, "label": label});
        if let Some(n) = names.get(project) {
            v["dependency"] = json!(n);
        }
        v
    };
    let (mut added, mut removed, mut changed) = (Vec::new(), Vec::new(), Vec::new());
    let keys: BTreeSet<&(String, String)> = a.keys().chain(b.keys()).collect();
    for k in keys {
        let mut v = row(k);
        match (a.get(k), b.get(k)) {
            (None, Some(c)) => {
                v["commit"] = json!(c);
                added.push(v);
            }
            (Some(c), None) => {
                v["commit"] = json!(c);
                removed.push(v);
            }
            (Some(f), Some(t)) if f != t => {
                v["commit"] = json!({"from": f, "to": t});
                changed.push(v);
            }
            _ => {}
        }
    }
    (!added.is_empty() || !removed.is_empty() || !changed.is_empty())
        .then(|| json!({"added": added, "removed": removed, "changed": changed}))
}

/// The files under `imports/` that differ, each with how and its BLAKE3
/// on each side it is on: what import features name them by.
fn imports(from: &Files, to: &Files) -> Vec<Value> {
    let prefix = format!("{}/", crate::doc::project::IMPORTS);
    let under = |files: &Files| -> BTreeMap<String, String> {
        files
            .iter()
            .filter(|(p, _)| p.starts_with(&prefix))
            .map(|(p, b)| (p.clone(), blake3::hash(b).to_hex().to_string()))
            .collect()
    };
    let (a, b) = (under(from), under(to));
    let paths: BTreeSet<&String> = a.keys().chain(b.keys()).collect();
    paths
        .into_iter()
        .filter_map(|p| {
            let (f, t) = (a.get(p), b.get(p));
            if f == t {
                return None;
            }
            let mut v = json!({"path": p, "status": status(f.is_some(), t.is_some(), true)});
            if let Some(f) = f {
                v["from"] = json!(f);
            }
            if let Some(t) = t {
                v["to"] = json!(t);
            }
            Some(v)
        })
        .collect()
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
    /// An assembly's solve, in brief: its state, freedom and whether it
    /// holds.
    pub solve: Option<Value>,
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
            solve: r.solve.as_ref().map(|s| {
                json!({"state": s.state_str(), "dof": s.report.dof,
                       "converged": s.report.converged})
            }),
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
                // A datum's geometry on each side, its bits as rebuild
                // reports them.
                if let (Some(p), Some(q)) = (x.datum(), y.datum()) {
                    v["datum"] = json!({"from": p.report(), "to": q.report()});
                }
                // A path's or helix's: how many segments, how long,
                // whether it closes.
                if let (Some(p), Some(q)) = (x.path(), y.path()) {
                    let sum = |m: &crate::path::Made| {
                        json!({"segments": m.pieces.len(), "length_m": m.length.value(),
                               "closed": m.closed})
                    };
                    v[p.kind()] = json!({"from": sum(p), "to": sum(q)});
                }
            }
        },
        _ => {}
    }
    // An unfold's pattern on each side: how many bend lines, and its box
    // in the pattern's frame, in meters.
    if let (Some(crate::rebuild::Output::Solids(x)), Some(crate::rebuild::Output::Solids(y))) =
        (ok(a), ok(b))
        && let (Some(p), Some(q)) = (&x.flat, &y.flat)
        && x.hash != y.hash
    {
        let sum = |f: &crate::rebuild::Flat| {
            let (lo, hi) = (f.pattern.bounds.min(), f.pattern.bounds.max());
            json!({"bends": f.pattern.bend_lines.len(),
                   "box_m": [[lo.x, lo.y], [hi.x, hi.y]]})
        };
        v["flat"] = json!({"from": sum(p), "to": sum(q)});
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

/// Each drawing built at a revision: by part and drawing, each view's
/// fingerprint by key, or the code of why it didn't build.
pub type Drawn =
    BTreeMap<(String, String), std::result::Result<BTreeMap<String, String>, crate::error::Code>>;

/// Adds to a structural diff `v` each drawing whose built views differ
/// (design 417): under its part's `drawings`, `model` by drawing, each
/// `{views: {added, removed, changed}}`, or `{from, to}` with what didn't
/// build.
pub fn drawings_model(v: &mut Value, from: &Drawn, to: &Drawn, opts: &Options) {
    let keys: BTreeSet<&(String, String)> = from.keys().chain(to.keys()).collect();
    for key in keys {
        let (part, drawing) = key;
        if opts.part.as_ref().is_some_and(|p| p != part) {
            continue;
        }
        let (a, b) = (from.get(key), to.get(key));
        if a == b {
            continue;
        }
        let m = match (a, b) {
            (Some(Ok(a)), Some(Ok(b))) => {
                let added: Vec<&String> = b.keys().filter(|k| !a.contains_key(*k)).collect();
                let removed: Vec<&String> = a.keys().filter(|k| !b.contains_key(*k)).collect();
                let changed: Vec<&String> = a
                    .iter()
                    .filter(|(k, x)| b.get(*k).is_some_and(|y| y != *x))
                    .map(|(k, _)| k)
                    .collect();
                json!({"views": {"added": added, "removed": removed, "changed": changed}})
            }
            _ => {
                let side = |x: Option<
                    &std::result::Result<BTreeMap<String, String>, crate::error::Code>,
                >| match x {
                    None => Value::Null,
                    Some(Ok(_)) => json!({"ok": true}),
                    Some(Err(c)) => json!({"ok": false, "error": c}),
                };
                json!({"from": side(a), "to": side(b)})
            }
        };
        let p = &mut v["parts"][part.as_str()];
        if p.is_null() {
            *p = json!({
                "status": "same",
                "order": {"added": [], "removed": [], "moved": []},
                "parameters": {"added": [], "removed": [], "changed": []},
                "features": {},
            });
        }
        let d = &mut p["drawings"];
        if d.is_null() {
            *d = json!({"added": [], "removed": [], "changed": []});
        }
        d["model"][drawing.as_str()] = m;
    }
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
                Some(Some(b)) => {
                    json!({"ok": true, "hash": b.hash, "kind": crate::kind::name(&b.body)})
                }
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
        if a.solve != b.solve || (opts.all && a.solve.is_some()) {
            let p = &mut v["parts"][part.as_str()];
            if p.is_null() {
                *p = json!({
                    "status": "same",
                    "order": {"added": [], "removed": [], "moved": []},
                    "parameters": {"added": [], "removed": [], "changed": []},
                    "features": {},
                });
            }
            p["solve"] = json!({"from": a.solve, "to": b.solve});
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
/// each of those with both its `sides` as [`crate::assembly::Occurrence`]
/// reports them (`null` where it is absent), and, for a solid on both
/// sides whose body differs, what differs in it (`bodies`, by
/// `<path>/<solid>`, as `body` is for a part's solid): what a compare
/// paints and ghosts (issue #109). `None` when nothing differs, unless
/// `all`.
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
    if same && !all {
        return None;
    }
    let mut sides = serde_json::Map::new();
    let mut differ = serde_json::Map::new();
    let listed: BTreeSet<&String> = added
        .iter()
        .chain(&removed)
        .chain(&moved)
        .chain(&changed)
        .copied()
        .collect();
    for path in listed {
        let (p, q) = (x.get(path), y.get(path));
        let side = |o: Option<&Occurrence>| o.map_or(Value::Null, Occurrence::to_json);
        sides.insert(path.clone(), json!({"from": side(p), "to": side(q)}));
        let (Some(p), Some(q)) = (p, q) else {
            continue;
        };
        for s in &q.solids {
            if let Some(was) = p.solids.iter().find(|w| w.founder == s.founder)
                && was.built.hash != s.built.hash
            {
                differ.insert(
                    format!("{path}/{}", s.founder),
                    bodies(&was.built.body, &s.built.body),
                );
            }
        }
    }
    Some(
        json!({"added": added, "removed": removed, "moved": moved, "changed": changed,
                "sides": sides, "bodies": differ}),
    )
}

/// A revision's files, and how the answer names it: the working files
/// (`working`), another worktree's (`worktree:<path>`, with its branch and
/// head) or a commit.
pub(crate) fn files_at(session: &mut Session, rev: &str) -> Result<(Value, activity::Files)> {
    if rev == "working" {
        return Ok((json!({"rev": rev}), session.snapshot()));
    }
    if let Some((named, files, stale)) = session.annotation_files(rev)? {
        if !stale.is_empty() {
            session
                .rev_warnings
                .push(crate::session::stale_warning(rev, &stale));
        }
        return Ok((named, files));
    }
    if let Some((named, files)) = session.draft_files(rev)? {
        return Ok((named, files));
    }
    if let Some((w, files, _)) = session.worktree_files(rev)? {
        let named = json!({"rev": rev, "worktree": w.root, "branch": w.branch, "head": w.head});
        return Ok((named, files));
    }
    let r = crate::git::revision(session.root(), rev)?;
    Ok((json!({"rev": rev, "commit": r.commit}), r.files))
}

/// Every part of a revision, rebuilt.
fn rebuilt_at(session: &mut Session, rev: &str) -> Result<BTreeMap<String, crate::diff::Built>> {
    let (_, s) = session.at(Some(rev))?;
    let ids: Vec<Id> = s.project().parts.keys().cloned().collect();
    let mut out = BTreeMap::new();
    for id in ids {
        let part = s.project().part(&id)?.clone();
        let r = s.rebuilt(&id)?;
        out.insert(id.to_string(), crate::diff::Built::of(&part, r));
    }
    Ok(out)
}

/// Each drawing of every part at `rev`, built: by part and drawing, each
/// view's fingerprint by key, or why it didn't build.
fn drawn_at(session: &mut Session, rev: &str) -> Result<crate::diff::Drawn> {
    let (_, s) = session.at(Some(rev))?;
    let mut out = crate::diff::Drawn::new();
    let ids: Vec<(Id, Vec<Id>)> = s
        .project()
        .parts
        .iter()
        .map(|(id, p)| (id.clone(), p.drawings.keys().cloned().collect()))
        .collect();
    for (pid, drawings) in ids {
        for d in drawings {
            let views = match crate::drawing::build(s, &pid, &d) {
                Ok(b) => Ok(b
                    .sheets
                    .iter()
                    .flat_map(|sh| {
                        sh.views
                            .iter()
                            .map(move |v| (v.key.to_string(), v.fingerprint(sh)))
                    })
                    .collect()),
                Err(e) => Err(e.code),
            };
            out.insert((pid.to_string(), d.to_string()), views);
        }
    }
    Ok(out)
}

/// `diff`: what changed from one revision to another.
pub(crate) fn diff(
    session: &mut Session,
    from: &str,
    to: &str,
    model: bool,
    opts: &crate::diff::Options,
) -> Result<Value> {
    if let Some(p) = &opts.part {
        part_id(p)?;
    }
    let (a, fa) = files_at(session, from)?;
    let (b, fb) = files_at(session, to)?;
    let (a_rev, b_rev) = (from.to_owned(), to.to_owned());
    let releases = crate::release::history(session)
        .map(|(_, r)| r)
        .unwrap_or_default();
    let mut v = crate::diff::structural(&fa, &fb, opts, &releases);
    v["from"] = a;
    v["to"] = b;
    if model {
        let job = session.job.clone();
        let stage = job.begin(Phase::Rebuild, "revisions", 2);
        stage.step(|| from.to_owned())?;
        let from = rebuilt_at(session, from)?;
        stage.step(|| to.to_owned())?;
        let to = rebuilt_at(session, to)?;
        drop(stage);
        crate::diff::model(&mut v, &from, &to, opts);
        // Each drawing whose built views differ (design 417).
        let (dfrom, dto) = (drawn_at(session, &a_rev)?, drawn_at(session, &b_rev)?);
        crate::diff::drawings_model(&mut v, &dfrom, &dto, opts);
        // Each imported file that differs, by product.
        if let Some(Value::Array(files)) = v.get_mut("imports") {
            for f in files {
                let path = f["path"].as_str().unwrap_or_default().to_owned();
                let (a, b) = (fa.get(&path), fb.get(&path));
                f["products"] =
                    crate::import::products_diff(&path, a.map(|x| &x[..]), b.map(|x| &x[..]));
            }
        }
    }
    Ok(v)
}

#[cfg(test)]
mod tests {
    #[test]
    fn an_annotations_marks_read_a_line_each() {
        let v = json!({"kind": "ryucad.annotation", "format": 2, "id": "aaaaaaaaaa",
                       "by": {"participant": "dave", "kind": "human"},
                       "at": "2026-11-05T14:02:11Z", "status": "open", "claimed_by": null,
                       "pins": [], "comment": "move here",
                       "view": {"eye_m": [0.3, -0.3, 0.3], "target_m": [0, 0, 0], "up": [0, 0, 1],
                                "projection": "perspective", "fov_deg": 40},
                       "change": null, "replies": {}, "resolution": null,
                       "marks": {"part": "rig", "list": [
                           {"kind": "arrow", "in": "view", "xy_m": [[-0.02, 0.011], [0.009, -0.004]],
                            "touches": [{"at": "tail", "ref": "rig/boss/3c"},
                                        {"at": "tip", "ref": "rig/plate/77"}]},
                           {"kind": "ellipse", "in": "view", "xy_m": [[0.004, 0.002], [0.016, 0.009]],
                            "touches": []},
                           {"kind": "stroke", "in": "face", "on": "rig/plate",
                            "points_m": [[0, 0, 0.005], [0.01, 0.002, 0.005], [0.02, 0, 0.005]],
                            "touches": [{"at": "under", "ref": "rig/plate/5f"}]},
                           {"kind": "text", "in": "view", "xy_m": [[0.02, 0.015]], "text": "2 mm",
                            "touches": []}]}});
        let text =
            String::from_utf8(textconv(crate::doc::canonical::to_string(&v).as_bytes())).unwrap();
        let lines: Vec<&str> = text.lines().collect();
        for want in [
            "marks.part = rig",
            "marks[0] = arrow in view (-20, 11) mm -> (9, -4) mm; tail rig/boss/3c, tip rig/plate/77",
            "marks[1] = ellipse in view (4, 2) mm to (16, 9) mm",
            "marks[2] = stroke on rig/plate, 3 points, (0, 0, 5) mm to (20, 2, 5) mm; under rig/plate/5f",
            "marks[3] = text in view (20, 15) mm: 2 mm",
        ] {
            assert!(lines.contains(&want), "{want:?} in\n{text}");
        }
        assert!(!text.contains("xy_m"), "no raw points:\n{text}");
    }

    #[test]
    fn diff_lists_an_annotation_whose_marks_changed() {
        let a = |x: f64| {
            json!({"kind": "ryucad.annotation", "id": "aaaaaaaaaa", "status": "open",
                                "marks": {"part": "p", "list": [{"kind": "line", "in": "view",
                                    "xy_m": [[0, 0], [x, 0]], "touches": []}]}})
        };
        let (mut from, mut to) = (Files::new(), Files::new());
        let put = |f: &mut Files, p: &str, v: Value| {
            f.insert(
                p.to_owned(),
                crate::doc::canonical::to_string(&v).into_bytes().into(),
            );
        };
        put(&mut from, "annotations/aaaaaaaaaa.json", a(0.01));
        put(&mut to, "annotations/aaaaaaaaaa.json", a(0.02));
        let v = structural(&from, &to, &Options::default(), &Default::default());
        let at = v["annotations"]["changed"]["aaaaaaaaaa"][0]["at"]
            .as_str()
            .unwrap()
            .to_owned();
        assert!(at.starts_with("/marks"), "{at}");
    }

    use super::*;

    #[test]
    fn an_annotation_reads_a_line_per_field_pin_and_reply() {
        let v = json!({"kind": "ryucad.annotation", "format": 1, "id": "aaaaaaaaaa",
                       "by": {"participant": "dave (window 1)", "kind": "human"},
                       "at": "2026-11-05T14:02:11Z", "status": "open", "claimed_by": null,
                       "pins": [{"ref": "plate/slab/9f", "point_m": [0.01, 0, 0]},
                                {"part": "plate", "point_m": [0, 0, 0.005]}],
                       "comment": "Move this hole", "view": null,
                       "change": {"request": {"method": "feature.edit", "params": {}},
                                  "rough": true,
                                  "base": {"parts/plate/features/a.json": null},
                                  "writes": {"parts/plate/features/a.json": {}},
                                  "after": ["bbbbbbbbbb"]},
                       "replies": {"r1": {"by": {"participant": "agent", "kind": "agent"},
                                          "at": "2026-11-05T14:05:00Z", "text": "On it."}},
                       "resolution": null});
        let text =
            String::from_utf8(textconv(crate::doc::canonical::to_string(&v).as_bytes())).unwrap();
        let lines: Vec<&str> = text.lines().collect();
        assert_eq!(lines[0], "ryucad.annotation (format 1)");
        for want in [
            "by = dave (window 1) (human)",
            "comment = Move this hole",
            "pin 1 = plate/slab/9f at 0.01, 0, 0",
            "pin 2 = plate at 0, 0, 0.005",
            "change = feature.edit (rough), after bbbbbbbbbb",
            "change.writes = parts/plate/features/a.json",
            "reply r1 = agent (agent) 2026-11-05T14:05:00Z: On it.",
            "status = open",
        ] {
            assert!(lines.contains(&want), "{want:?} in\n{text}");
        }
    }

    #[test]
    fn diff_lists_annotations_at_the_top() {
        let a = |status: &str| json!({"kind": "ryucad.annotation", "id": "aaaaaaaaaa", "status": status});
        let mut from = Files::new();
        let mut to = Files::new();
        let put = |f: &mut Files, p: &str, v: Value| {
            f.insert(
                p.to_owned(),
                crate::doc::canonical::to_string(&v).into_bytes().into(),
            );
        };
        put(&mut from, "annotations/aaaaaaaaaa.json", a("open"));
        put(&mut to, "annotations/aaaaaaaaaa.json", a("done"));
        put(&mut to, "annotations/bbbbbbbbbb.json", a("open"));
        put(&mut from, "annotations/cccccccccc.json", a("open"));
        let v = structural(&from, &to, &Options::default(), &Default::default());
        assert_eq!(v["annotations"]["added"], json!(["bbbbbbbbbb"]));
        assert_eq!(v["annotations"]["removed"], json!(["cccccccccc"]));
        assert_eq!(
            v["annotations"]["changed"]["aaaaaaaaaa"][0]["at"],
            "/status"
        );
        assert!(
            structural(&from, &from, &Options::default(), &Default::default())
                .get("annotations")
                .is_none()
        );
    }

    #[test]
    fn mirrors_and_splits_read_a_line_per_field() {
        let v = json!({"format": 10, "kind": "ryucad.feature", "type": "mirror", "solid": "half",
                       "plane": {"feature": "mid"}, "operation": "join", "targets": ["half"]});
        assert_eq!(
            text(v),
            "ryucad.feature mirror (format 10)\n\
             operation = join\n\
             plane = mid\n\
             solid = half\n\
             targets = [half]\n"
        );
        let n = "0123456789abcdef0123456789abcdef";
        let v = json!({"format": 10, "kind": "ryucad.feature", "type": "split", "target": "h",
                       "plane": {"feature": "h", "name": n}, "flip": true, "keep": "behind"});
        assert_eq!(
            text(v),
            format!(
                "ryucad.feature split (format 10)\n\
                 flip = true\n\
                 keep = behind\n\
                 plane = h/{n}\n\
                 target = h\n"
            )
        );
    }

    #[test]
    fn a_splits_tool_is_one_line() {
        let n = "0123456789abcdef0123456789abcdef";
        let m = "fedcba9876543210fedcba9876543210";
        let by_body = json!({"format": 19, "kind": "ryucad.feature", "type": "split",
                             "target": "h", "tool": {"body": "parting"}, "keep": "behind"});
        let t = text(by_body);
        assert!(t.contains("tool = body parting\n"), "{t}");
        assert!(t.contains("keep = behind\n"), "{t}");
        let by_faces = json!({"format": 19, "kind": "ryucad.feature", "type": "split",
                              "target": "h", "tool": {"faces": [
                                  {"feature": "k", "name": n}, {"feature": "k", "name": m}]}});
        let t = text(by_faces);
        assert!(t.contains(&format!("tool = faces k/{n}, k/{m}\n")), "{t}");
    }

    #[test]
    fn sheet_metal_features_textconv_one_line_a_field() {
        let n = |k: u8| format!("{k:032x}");
        let v = json!({"format": 11, "kind": "ryucad.feature", "type": "flange", "target": "plate",
                       "edges": [{"feature": "plate", "name": n(1)},
                                 {"edge": {"feature": "plate", "name": n(2)}, "side": {"feature": "plate", "name": n(3)}}],
                       "angle": "90 deg", "length": "20 mm",
                       "ends": [{"vertex": {"feature": "plate", "name": n(4)}, "setback": "5 mm",
                                 "relief": {"kind": "obround", "width": "1 mm", "depth": "1.5 mm"}}]});
        let t = text(v.clone());
        assert!(
            t.contains(&format!(
                "edges = plate/{}, plate/{} (side plate/{})",
                n(1),
                n(2),
                n(3)
            )),
            "{t}"
        );
        assert!(
            t.contains(&format!(
                "ends[plate/{}] = setback 5 mm, relief obround 1 mm × 1.5 mm",
                n(4)
            )),
            "{t}"
        );
        let v = json!({"format": 11, "kind": "ryucad.feature", "type": "base_flange", "sketch": "s",
                       "thickness": "1.5 mm", "radius": "1 mm", "allowance": {"deduction": "2.4 mm"},
                       "relief": {"kind": "tear", "width": "0.5 mm"}});
        let t = text(v.clone());
        assert!(t.contains("allowance = deduction 2.4 mm"), "{t}");
        assert!(t.contains("relief = tear 0.5 mm"), "{t}");
    }

    #[test]
    fn a_tweaks_changes_are_by_field() {
        let n = |k: u8| json!({"feature": "b", "name": format!("{k:032x}")});
        let before = json!({"format": 16, "kind": "ryucad.feature", "type": "move_face",
                            "target": "b", "by": "offset", "distance": "1 mm", "faces": [n(1)]});
        let mut after = before.clone();
        after["distance"] = json!("2 mm");
        after["groups"] = json!([{"distance": "3 mm", "faces": [n(2)]}]);
        let mut out = Vec::new();
        changes("", Some(&before), Some(&after), &mut out);
        let pointers: Vec<&str> = out.iter().map(|c| c["at"].as_str().unwrap()).collect();
        assert_eq!(pointers, ["/distance", "/groups"]);
    }

    #[test]
    fn a_flanges_changes_are_by_field() {
        let n = |k: u8| format!("{k:032x}");
        let before = json!({"format": 11, "kind": "ryucad.feature", "type": "flange", "target": "plate",
                            "edges": [{"feature": "plate", "name": n(1)}], "angle": "90 deg", "length": "20 mm"});
        let mut after = before.clone();
        after["angle"] = json!("80 deg");
        after["ends"] = json!([{"vertex": {"feature": "plate", "name": n(2)}, "setback": "5 mm"}]);
        let mut out = Vec::new();
        changes("", Some(&before), Some(&after), &mut out);
        let pointers: Vec<&str> = out.iter().map(|c| c["at"].as_str().unwrap()).collect();
        assert_eq!(pointers, ["/angle", "/ends"]);
    }

    #[test]
    fn unfolds_and_refolds_textconv_one_line_a_field() {
        let n = |k: u8| format!("{k:032x}");
        let r = |f: &str, k: u8| json!({"feature": f, "name": n(k)});
        let v = json!({"format": 14, "kind": "ryucad.feature", "type": "unfold", "target": "tray",
                       "fixed": r("plate", 1), "only": [r("f1", 2), r("f1", 3)],
                       "allowance": {"k_factor": "0.44"},
                       "bends": [{"bend": r("f1", 2), "deduction": "2.2 mm"},
                                 {"bend": r("f1", 3), "allowance": "3 mm"}]});
        assert_eq!(
            text(v),
            format!(
                "ryucad.feature unfold (format 14)\n\
                 allowance = k_factor 0.44\n\
                 bends[f1/{b}] = deduction 2.2 mm\n\
                 bends[f1/{c}] = allowance 3 mm\n\
                 fixed = plate/{a}\n\
                 only = f1/{b}, f1/{c}\n\
                 target = tray\n",
                a = n(1),
                b = n(2),
                c = n(3)
            )
        );
        let v = json!({"format": 14, "kind": "ryucad.feature", "type": "refold", "target": "tray",
                       "unfold": "u1", "only": [r("f1", 2)]});
        assert_eq!(
            text(v),
            format!(
                "ryucad.feature refold (format 14)\n\
                 only = f1/{b}\n\
                 target = tray\n\
                 unfold = u1\n",
                b = n(2)
            )
        );
    }

    #[test]
    fn surface_features_textconv_a_line_a_side_and_a_group() {
        const A: &str = "0123456789abcdef0123456789abcdef";
        let n = crate::doc::surface::FORMAT;
        let fill = text(
            json!({"format": n, "kind": "ryucad.feature", "type": "fill",
            "sides": [{"edge": {"feature": "wall", "name": A}, "tangent": true},
                      {"curve": {"feature": "s2", "entity": "a1"}}, {"curve": {"feature": "p1"}}],
            "tolerance": "0.001 mm", "angle": "1 deg"}),
        );
        for line in [
            format!("side 1 = wall/{A} (tangent)"),
            "side 2 = s2/a1".into(),
            "side 3 = p1".into(),
            "tolerance = 0.001 mm".into(),
            "angle = 1 deg".into(),
        ] {
            assert!(fill.lines().any(|l| l == line), "{line:?} in\n{fill}");
        }
        let sew = text(json!({"format": n, "kind": "ryucad.feature", "type": "sew",
            "sheets": ["top", "side1"], "tolerance": "0.01 mm", "solid": true}));
        assert!(sew.lines().any(|l| l == "sheets = [top, side1]"), "{sew}");
        let th = text(
            json!({"format": n, "kind": "ryucad.feature", "type": "thicken",
            "from": {"sheet": "skin"}, "front": "2 mm", "back": "0 mm", "operation": "new",
            "faces": [{"front": "3 mm", "back": "0 mm", "faces": [{"feature": "skin", "name": A}]}]}),
        );
        for line in [
            "from = sheet skin".to_owned(),
            format!("faces[0] = 3 mm, 0 mm: skin/{A}"),
        ] {
            assert!(th.lines().any(|l| l == line), "{line:?} in\n{th}");
        }
        let th = text(
            json!({"format": n, "kind": "ryucad.feature", "type": "thicken",
            "from": {"faces": [{"feature": "base", "name": A}]}, "front": "2 mm", "back": "0 mm",
            "operation": "join", "targets": ["base"]}),
        );
        assert!(
            th.lines().any(|l| l == format!("from = faces base/{A}")),
            "{th}"
        );
        let x = text(
            json!({"format": n, "kind": "ryucad.feature", "type": "extract",
            "faces": [{"feature": "base", "name": A}]}),
        );
        assert!(x.lines().any(|l| l == format!("faces = [base/{A}]")), "{x}");
    }

    #[test]
    fn a_shells_groups_read_on_a_line_each() {
        let (a, b) = (
            "0123456789abcdef0123456789abcdef",
            "fedcba9876543210fedcba9876543210",
        );
        let v = json!({"format": 9, "kind": "ryucad.feature", "type": "shell", "target": "base",
            "thickness": "2 mm", "open": [{"feature": "base", "name": a}],
            "faces": [{"thickness": "4 mm",
                       "faces": [{"feature": "base", "name": b}, {"feature": "base", "name": a}]}]});
        assert_eq!(
            text(v),
            format!(
                "ryucad.feature shell (format 9)\n\
                 faces[0] = 4 mm: base/{b}, base/{a}\n\
                 open = [base/{a}]\n\
                 target = base\n\
                 thickness = 2 mm\n"
            )
        );
        let v = json!({"format": 9, "kind": "ryucad.feature", "type": "offset", "target": "base",
            "distance": "0 mm",
            "faces": [{"distance": "-3 mm", "faces": [{"feature": "base", "name": a}]}]});
        assert_eq!(
            text(v),
            format!(
                "ryucad.feature offset (format 9)\n\
                 distance = 0 mm\n\
                 faces[0] = -3 mm: base/{a}\n\
                 target = base\n"
            )
        );
    }
    #[test]
    fn a_drafts_groups_read_on_a_line_each() {
        let (a, b) = (
            "0123456789abcdef0123456789abcdef",
            "fedcba9876543210fedcba9876543210",
        );
        let v = json!({"format": 18, "kind": "ryucad.feature", "type": "draft",
            "target": "base", "angle": "2 deg", "neutral": "xy", "reverse": true,
            "faces": [{"feature": "base", "name": a}],
            "groups": [{"angle": "5 deg", "faces": [{"feature": "base", "name": b}]}],
            "flip": [{"feature": "base", "name": b}], "tangent": false});
        assert_eq!(
            text(v),
            format!(
                "ryucad.feature draft (format 18)\n\
                 angle = 2 deg\n\
                 faces = [base/{a}]\n\
                 flip = [base/{b}]\n\
                 groups[0] = 5 deg: base/{b}\n\
                 neutral = xy\n\
                 reverse = true\n\
                 tangent = false\n\
                 target = base\n"
            )
        );
        let v = json!({"format": 18, "kind": "ryucad.feature", "type": "draft",
            "target": "base", "angle": "3 deg",
            "parting": [{"feature": "base", "name": b}], "pull": "z",
            "faces": [{"feature": "base", "name": a}]});
        assert_eq!(
            text(v),
            format!(
                "ryucad.feature draft (format 18)\n\
                 angle = 3 deg\n\
                 faces = [base/{a}]\n\
                 parting = [base/{b}]\n\
                 pull = z\n\
                 target = base\n"
            )
        );
    }

    #[test]
    fn a_drafts_changes_are_by_field() {
        let n = |k: u8| json!({"feature": "b", "name": format!("{k:032x}")});
        let before = json!({"format": 18, "kind": "ryucad.feature", "type": "draft",
                            "target": "b", "angle": "2 deg", "neutral": "xy", "faces": [n(1)]});
        let mut after = before.clone();
        after["angle"] = json!("3 deg");
        after["flip"] = json!([n(1)]);
        let mut out = Vec::new();
        changes("", Some(&before), Some(&after), &mut out);
        let pointers: Vec<&str> = out.iter().map(|c| c["at"].as_str().unwrap()).collect();
        assert_eq!(pointers, ["/angle", "/flip"]);
    }

    #[test]
    fn a_delete_and_a_tweaks_new_fields_read_a_line_each() {
        let r = |n: u128| json!({"feature": "b", "name": format!("{n:032x}")});
        let v = json!({"format": 20, "kind": "ryucad.feature", "type": "delete_face",
            "target": "b", "faces": [r(1), r(2)], "seeds": [r(3)], "heal": "grow"});
        let t = text(v.clone());
        assert!(
            t.starts_with("ryucad.feature delete_face (format 20)\n"),
            "{t}"
        );
        for line in ["heal = grow", "target = b"] {
            assert!(t.lines().any(|l| l == line), "{line}: {t}");
        }
        assert_eq!(
            t.lines().filter(|l| l.starts_with("faces")).count(),
            1,
            "{t}"
        );
        assert_eq!(
            t.lines().filter(|l| l.starts_with("seeds")).count(),
            1,
            "{t}"
        );
        let m = json!({"format": 20, "kind": "ryucad.feature", "type": "move_face", "target": "b",
            "by": "translate", "direction": "z", "distance": "1 mm", "faces": [r(1)],
            "topology": "change", "blends": "rebuild"});
        let t = text(m.clone());
        for line in ["topology = change", "blends = rebuild"] {
            assert!(t.lines().any(|l| l == line), "{line}: {t}");
        }
    }

    #[test]
    fn a_tweaks_groups_read_on_a_line_each() {
        let (a, b) = (
            "0123456789abcdef0123456789abcdef",
            "fedcba9876543210fedcba9876543210",
        );
        let v = json!({"format": 16, "kind": "ryucad.feature", "type": "move_face",
            "target": "base", "by": "offset", "distance": "0.5 mm",
            "faces": [{"feature": "base", "name": a}],
            "groups": [{"distance": "1 mm",
                        "faces": [{"feature": "base", "name": b}, {"feature": "base", "name": a}]}],
            "tangent": false});
        assert_eq!(
            text(v),
            format!(
                "ryucad.feature move_face (format 16)\n\
                 by = offset\n\
                 distance = 0.5 mm\n\
                 faces = [base/{a}]\n\
                 groups[0] = 1 mm: base/{b}, base/{a}\n\
                 tangent = false\n\
                 target = base\n"
            )
        );
        let v = json!({"format": 16, "kind": "ryucad.feature", "type": "move_face",
            "target": "base", "by": "rotate", "axis": {"feature": "base", "name": b},
            "angle": "3 deg", "faces": [{"feature": "base", "name": a}]});
        assert_eq!(
            text(v),
            format!(
                "ryucad.feature move_face (format 16)\n\
                 angle = 3 deg\n\
                 axis = base/{b}\n\
                 by = rotate\n\
                 faces = [base/{a}]\n\
                 target = base\n"
            )
        );
        let v = json!({"format": 16, "kind": "ryucad.feature", "type": "replace_face",
            "target": "base",
            "groups": [{"with": {"feature": "lid", "name": b}, "faces": [{"feature": "base", "name": a}]},
                       {"with": "xy", "faces": [{"feature": "base", "name": b}]}]});
        assert_eq!(
            text(v),
            format!(
                "ryucad.feature replace_face (format 16)\n\
                 groups[0] = lid/{b}: base/{a}\n\
                 groups[1] = xy: base/{b}\n\
                 target = base\n"
            )
        );
    }

    use crate::session::Session;
    use serde_json::json;

    fn text(v: Value) -> String {
        String::from_utf8(textconv(crate::doc::canonical::to_string(&v).as_bytes())).unwrap()
    }

    #[test]
    fn a_datum_reads_one_line_per_field() {
        let n = "0123456789abcdef0123456789abcdef";
        let v = json!({"format": 7, "kind": "ryucad.feature", "type": "plane", "by": "offset",
            "from": {"feature": "base", "name": n}, "distance": "30 mm", "x": {"feature": "a1"}});
        assert_eq!(
            text(v),
            format!(
                "ryucad.feature plane (format 7)\n\
                 by = offset\n\
                 distance = 30 mm\n\
                 from = base/{n}\n\
                 x = a1\n"
            )
        );
        let v = json!({"format": 7, "kind": "ryucad.feature", "type": "plane",
            "by": "three_points", "points": ["origin", {"entity": "p3", "feature": "s"}, {"feature": "q1"}]});
        assert!(
            text(v).contains("points = [origin, s/p3, q1]\n"),
            "a datum's points are one line"
        );
    }

    #[test]
    fn a_loft_reads_a_line_per_section_in_order() {
        let n = "0123456789abcdef0123456789abcdef";
        let v = json!({"format": 8, "kind": "ryucad.feature", "type": "loft",
            "sections": [{"sketch": "s3", "loops": ["p7", "l3"]}, {"sketch": "s1"},
                         {"face": {"feature": "base", "name": n}}],
            "start_tangent": {"direction": "normal", "weight": "1.5"},
            "end_tangent": {"direction": {"feature": "s1", "entity": "l2"}},
            "operation": "join", "targets": ["base"]});
        assert_eq!(
            text(v),
            format!(
                "ryucad.feature loft (format 8)\n\
                 section 1 = sketch s3 (loops p7, l3)\n\
                 section 2 = sketch s1\n\
                 section 3 = face base/{n}\n\
                 end_tangent = s1/l2\n\
                 operation = join\n\
                 start_tangent = normal (weight 1.5)\n\
                 targets = [base]\n"
            )
        );
        let v = json!({"format": 8, "kind": "ryucad.feature", "type": "loft", "closed": true,
            "sections": [{"sketch": "a"}, {"sketch": "b"}, {"sketch": "c"}]});
        assert_eq!(
            text(v),
            "ryucad.feature loft (format 8)\n\
             section 1 = sketch a\n\
             section 2 = sketch b\n\
             section 3 = sketch c\n\
             closed = true\n"
        );
    }

    #[test]
    fn a_planar_reads_its_profile_on_a_line_and_a_surface_its_flag() {
        let p = json!({"format": 12, "kind": "ryucad.feature", "type": "planar",
                       "profile": {"sketch": "s6", "loops": ["l2"]}});
        assert_eq!(
            text(p),
            "ryucad.feature planar (format 12)\n\
             profile = sketch s6 (loops l2)\n"
        );
        let e = json!({"format": 12, "kind": "ryucad.feature", "type": "extrude", "sketch": "s",
                       "surface": true, "extent": {"type": "blind", "length": "20 mm"}});
        let t = text(e);
        assert!(t.contains("surface = true\n"), "{t}");
    }

    #[test]
    fn a_sweep_reads_its_profile_and_law_on_a_line_each() {
        let n = "0123456789abcdef0123456789abcdef";
        let v = json!({"format": 8, "kind": "ryucad.feature", "type": "sweep",
            "profile": {"sketch": "s3", "loops": ["p7", "l3"]}, "path": {"feature": "rim"},
            "law": "binormal", "direction": "z", "twist": "90 deg", "scale": "0.5",
            "operation": "cut", "targets": ["base"]});
        assert_eq!(
            text(v),
            "ryucad.feature sweep (format 8)\n\
             law = binormal z\n\
             operation = cut\n\
             path = rim\n\
             profile = sketch s3 (loops p7, l3)\n\
             scale = 0.5\n\
             targets = [base]\n\
             twist = 90 deg\n"
        );
        let v = json!({"format": 8, "kind": "ryucad.feature", "type": "sweep",
            "profile": {"face": {"feature": "base", "name": n}},
            "path": {"feature": "s", "entity": "l1"}, "law": "minimal"});
        assert_eq!(
            text(v),
            format!(
                "ryucad.feature sweep (format 8)\n\
                 law = minimal\n\
                 path = s/l1\n\
                 profile = face base/{n}\n"
            )
        );
    }

    #[test]
    fn a_path_reads_one_line_per_segment_and_a_helix_per_field() {
        let n = "0123456789abcdef0123456789abcdef";
        let v = json!({"format": 7, "kind": "ryucad.feature", "type": "path",
            "segments": [{"feature": "base", "name": n, "propagate": true},
                         {"feature": "s", "entity": "l2"}, {"feature": "h1"}],
            "reverse": true});
        assert_eq!(
            text(v),
            format!(
                "ryucad.feature path (format 7)\n\
                 segment 1 = base/{n} (propagate)\n\
                 segment 2 = s/l2\n\
                 segment 3 = h1\n\
                 reverse = true\n"
            )
        );
        let v = json!({"format": 7, "kind": "ryucad.feature", "type": "helix", "axis": "z",
            "radius": "8 mm", "pitch": "2 mm", "turns": "10", "hand": "left"});
        assert_eq!(
            text(v),
            "ryucad.feature helix (format 7)\n\
             axis = z\n\
             hand = left\n\
             pitch = 2 mm\n\
             radius = 8 mm\n\
             turns = 10\n"
        );
    }

    #[test]
    fn a_model_diff_says_how_a_path_moved() {
        use crate::git::tests::git;
        use crate::server::tests::{call, fresh};
        let (dir, mut s) = fresh();
        let root = dir.path();
        git(root, &["init", "-q", "-b", "main"]);
        call(&mut s, "part.create", json!({"part": "p"})).unwrap();
        for f in [
            json!({"type": "helix", "id": "h", "axis": "z", "radius": 8, "pitch": 2, "turns": 3}),
            json!({"type": "path", "id": "w", "segments": ["h"]}),
        ] {
            let mut f = f;
            f["part"] = json!("p");
            call(&mut s, "feature.add", f).unwrap();
        }
        git(root, &["add", "."]);
        git(root, &["commit", "-q", "-m", "start"]);
        call(
            &mut s,
            "feature.edit",
            json!({"part": "p", "id": "h", "turns": 4}),
        )
        .unwrap();
        let v = call(&mut s, "diff", json!({"model": true})).unwrap();
        let f = &v["parts"]["p"]["features"];
        let h = &f["h"]["model"]["helix"];
        assert_eq!(h["from"]["segments"], 1, "{v}");
        let (a, b) = (
            h["from"]["length_m"].as_f64().unwrap(),
            h["to"]["length_m"].as_f64().unwrap(),
        );
        assert!((b / a - 4.0 / 3.0).abs() < 1e-6, "{h}");
        // The path along it changed only as its result did.
        assert_eq!(f["w"]["status"], "same");
        assert!(f["w"]["model"]["path"]["to"]["length_m"].as_f64().unwrap() > a);
    }

    #[test]
    fn a_model_diff_says_how_an_unfolds_pattern_moved() {
        use crate::git::tests::git;
        use crate::server::tests::{call, fresh};
        let (dir, mut s) = fresh();
        let root = dir.path();
        git(root, &["init", "-q", "-b", "main"]);
        crate::sheet_metal::tests::tray(&mut s, "p");
        call(
            &mut s,
            "feature.add",
            json!({"part": "p", "type": "unfold", "id": "u", "target": "plate"}),
        )
        .unwrap();
        git(root, &["add", "."]);
        git(root, &["commit", "-q", "-m", "start"]);
        call(
            &mut s,
            "feature.edit",
            json!({"part": "p", "id": "u", "allowance": {"k_factor": "0.3"}}),
        )
        .unwrap();
        let v = call(&mut s, "diff", json!({"model": true})).unwrap();
        let flat = &v["parts"]["p"]["features"]["u"]["model"]["flat"];
        assert_eq!(flat["from"]["bends"], flat["to"]["bends"], "{v}");
        assert!(flat["from"]["bends"].as_u64().unwrap() > 0, "{v}");
        // A smaller K-factor makes every strip shorter: the box narrower.
        let width =
            |s: &Value| s["box_m"][1][0].as_f64().unwrap() - s["box_m"][0][0].as_f64().unwrap();
        assert!(width(&flat["to"]) < width(&flat["from"]), "{flat}");
    }

    #[test]
    fn dependencies_the_lock_and_foreign_instances_read_as_text() {
        let lib = "7c1e9a0f3b2d4e5f8a9b0c1d2e3f4a5b";
        let sub = "9d0b0000000000000000000000000000";
        let me = "3f2a0000000000000000000000000000";
        let commit = "c".repeat(40);
        let m = json!({"default_unit": "mm", "format": 3, "kind": "ryucad.project", "project": me,
            "dependencies": {lib: {"name": "fasteners", "path": "", "url": "https://github.com/acme/fasteners.git"}},
            "forked_from": {"project": sub, "commit": commit}});
        assert_eq!(
            text(m),
            format!(
                "ryucad.project (format 3)\n\
                 default_unit = mm\n\
                 dependency fasteners = {lib} https://github.com/acme/fasteners.git (path \"\")\n\
                 forked_from = {sub} at {commit}\n\
                 project = {me}\n"
            )
        );
        let content = "d".repeat(64);
        let lock = json!({"format": 1, "kind": "ryucad.lock", "projects": {
            lib: {"labels": {"m6-bolt/B": {"commit": commit, "content": content}},
                  "path": "", "release_branch": "main", "url": "https://x/f.git", "via": []},
            sub: {"labels": {}, "path": "lib", "release_branch": "main", "url": "https://x/s.git",
                  "via": [lib]}}});
        let names = BTreeMap::from([(lib.to_owned(), "fasteners".to_owned())]);
        let bytes = crate::doc::canonical::to_string(&lock);
        assert_eq!(
            String::from_utf8(textconv_named(bytes.as_bytes(), &names)).unwrap(),
            format!(
                "ryucad.lock (format 1)\n\
                 fasteners m6-bolt/B = {commit} content {content}\n\
                 project 9d0b0000000000000000000000000000 = {sub} https://x/s.git (path \"lib\") on main via fasteners\n\
                 project fasteners = {lib} https://x/f.git (path \"\") on main\n"
            )
        );
        let i = json!({"format": 6, "kind": "ryucad.feature", "type": "instance", "project": lib,
            "part": "m6-bolt", "rev": {"label": "B"},
            "pose": {"origin_m": [0, 0, 0], "x": [1, 0, 0], "z": [0, 0, 1]}});
        let bytes = crate::doc::canonical::to_string(&i);
        let t = String::from_utf8(textconv_named(bytes.as_bytes(), &names)).unwrap();
        assert!(t.contains("\nrev = fasteners:m6-bolt @ label B\n"), "{t}");
        assert!(!t.contains("project ="), "{t}");
        // Without names, by ID.
        assert!(text(i).contains(&format!("rev = {lib}:m6-bolt @ label B")));
        // A manifest names what it lists, however its project is found.
        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(dir.path().join("parts/a")).unwrap();
        crate::doc::project::write_json(
            &dir.path().join(crate::doc::project::MANIFEST),
            &json!({"default_unit": "mm", "format": 3, "kind": "ryucad.project",
                    "dependencies": {lib: {"name": "fasteners", "path": "", "url": "file:///f"}}}),
        )
        .unwrap();
        assert_eq!(names_near(&dir.path().join("parts/a")), names);
        assert!(names_near(std::path::Path::new("/")).is_empty());
    }

    #[test]
    fn a_mate_reads_as_text_with_a_connector_a_line() {
        let n = "0".repeat(32);
        let v = json!({"format": 4, "kind": "ryucad.feature", "type": "mate", "form": "revolute",
                       "a": {"instance": ["base"], "at": {"feature": "slab", "name": n},
                             "offset": {"origin": ["0 mm", "0 mm", "2 mm"], "rotate": "90 deg",
                                        "flip": true}},
                       "b": {"instance": ["ring:2", "pin"], "at": "origin"},
                       "angle": "30 deg"});
        assert_eq!(
            text(v),
            format!(
                "ryucad.feature mate (format 4)\n\
                 a = base : slab/{n} offset origin 0 mm, 0 mm, 2 mm rotate 90 deg flip\n\
                 angle = 30 deg\n\
                 b = ring:2/pin : origin\n\
                 form = revolute\n"
            )
        );
    }

    #[test]
    fn an_imports_mode_is_one_line() {
        let h = "ab".repeat(32);
        let v = json!({"body": 0, "file": {"blake3": h, "path": "imports/b.step"}, "format": 13,
                       "kind": "ryucad.feature", "type": "import", "mode": "reference"});
        assert!(text(v).contains("\nmode = reference\n"));
    }

    #[test]
    fn an_imports_place_is_one_line() {
        let h = "ab".repeat(32);
        let v = json!({"body": 0, "file": {"blake3": h, "path": "imports/b.step"}, "format": 17,
                       "kind": "ryucad.feature", "type": "import", "mode": "reference",
                       "place": {"pose": {"origin_m": [0.01, 0, 0], "x": [1, 0, 0], "z": [0, 0, 1]},
                                 "via": [{"product": {"id": "S-1", "name": "Sub"}, "usage": "1"},
                                         {"ordinal": 1, "product": {"id": "P-1", "name": "Pin"},
                                          "usage": "1"}]}});
        let t = text(v);
        assert!(
            t.contains(
                "\nplace = 1 of S-1 > 1#1 of P-1 at (0.01, 0, 0) m, z [0, 0, 1], x [1, 0, 0]\n"
            ),
            "{t}"
        );
        assert!(!t.contains("place."), "{t}");
    }

    #[test]
    fn a_converted_import_diffs_by_its_mode() {
        let h = "ab".repeat(32);
        let solid = json!({"body": 0, "file": {"blake3": h, "path": "imports/b.step"},
                           "format": 5, "kind": "ryucad.feature", "type": "import"});
        let mut reference = solid.clone();
        reference["mode"] = json!("reference");
        reference["format"] = json!(13);
        let d = feature(Some(&solid), Some(&reference), &Options::default());
        assert_eq!(d["status"], "changed", "{d}");
        let pointers: Vec<&Value> = d["changes"]
            .as_array()
            .unwrap()
            .iter()
            .map(|c| &c["at"])
            .collect();
        assert!(pointers.contains(&&json!("/mode")), "{d}");
        let mode = d["changes"]
            .as_array()
            .unwrap()
            .iter()
            .find(|c| c["at"] == "/mode")
            .unwrap();
        assert_eq!(mode["to"], "reference", "{d}");
    }

    #[test]
    fn an_import_reads_as_its_file_product_and_body() {
        let h = "ab".repeat(32);
        let v = json!({"body": 1, "file": {"blake3": h, "path": "imports/bracket.step"},
                       "format": 5, "kind": "ryucad.feature", "type": "import",
                       "product": {"id": "BRK-100", "name": "Bracket"}});
        assert_eq!(
            text(v),
            format!(
                "ryucad.feature import (format 5)\n\
                 body = 1\n\
                 file.blake3 = {h}\n\
                 file.path = imports/bracket.step\n\
                 product.id = BRK-100\n\
                 product.name = Bracket\n"
            )
        );
        // A STEP file itself is never JSON: it comes back as it is.
        let step = b"ISO-10303-21;\nHEADER;\n";
        assert_eq!(textconv(step), step);
    }

    #[test]
    fn imported_files_that_differ_are_listed_with_their_hashes() {
        let files = |pairs: &[(&str, &str)]| -> Files {
            pairs
                .iter()
                .map(|(p, b)| ((*p).to_owned(), b.as_bytes().into()))
                .collect()
        };
        let base = [("ryucad.json", "{}")];
        let from = files(&[
            base[0],
            ("imports/a.step", "one"),
            ("imports/b.step", "same"),
        ]);
        let to = files(&[
            base[0],
            ("imports/a.step", "two"),
            ("imports/b.step", "same"),
            ("imports/c.step", "new"),
        ]);
        let hash = |s: &str| blake3::hash(s.as_bytes()).to_hex().to_string();
        let v = structural(&from, &to, &Options::default(), &Releases::default());
        assert_eq!(
            v["imports"],
            json!([
                {"path": "imports/a.step", "status": "changed", "from": hash("one"), "to": hash("two")},
                {"path": "imports/c.step", "status": "added", "to": hash("new")},
            ])
        );
        let v = structural(&to, &from, &Options::default(), &Releases::default());
        assert_eq!(
            v["imports"][1],
            json!({"path": "imports/c.step", "status": "removed",
                                            "from": hash("new")})
        );
        // Nothing differs: no key at all, as before imports.
        let v = structural(&from, &from, &Options::default(), &Releases::default());
        assert!(v.get("imports").is_none());
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
        let a = json!({"features": ["i1", "i2"], "format": 3, "kind": "ryucad.part",
                       "part_kind": "assembly", "items": {"plate": 10, "bolt": 2, "nut": 2}});
        assert_eq!(
            text(a),
            "ryucad.part (format 3)\nfeature i1\nfeature i2\nitem 2 = bolt\nitem 2 = nut\n\
             item 10 = plate\npart_kind = assembly\n"
        );
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
        let ruled = json!({
            "approvals": {
                "keys": {"3f9a0c1d2e4b5a69": "00".repeat(32)},
                "rules": [
                    {"count": 1, "from": ["alice", "bob", "carol"], "parts": ["*"]},
                    {"count": 2, "from": ["dave", "erin", "frank"], "parts": ["motor-*", "gearbox"]}
                ]
            },
            "format": 2, "kind": "ryupdm.config", "release_branch": "main", "revisions": "letters",
        });
        assert_eq!(
            text(ruled),
            format!(
                "ryupdm.config (format 2)\n\
                 approvals.keys.3f9a0c1d2e4b5a69 = {}\n\
                 approvals.rules.0 = 1 of alice, bob, carol for *\n\
                 approvals.rules.1 = 2 of dave, erin, frank for motor-*, gearbox\n\
                 release_branch = main\n\
                 revisions = letters\n",
                "00".repeat(32)
            )
        );
        let teams = json!({
            "approvals": {
                "keys": {"3f9a0c1d2e4b5a69": {"key": "00".repeat(32), "revoked": "2026-11-10"}},
                "rules": [{"count": 2, "from": ["@acme/quality", "dave"], "parts": ["*"]}]
            },
            "format": 3, "kind": "ryupdm.config", "release_branch": "main", "revisions": "letters",
        });
        assert_eq!(
            text(teams),
            format!(
                "ryupdm.config (format 3)\n\
                 approvals.keys.3f9a0c1d2e4b5a69 = {} (revoked 2026-11-10)\n\
                 approvals.rules.0 = 2 of @acme/quality, dave for *\n\
                 release_branch = main\n\
                 revisions = letters\n",
                "00".repeat(32)
            )
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

    #[test]
    fn the_material_library_textconvs_and_diffs_by_pointer() {
        let library = |steel: &str| {
            json!({"default": "steel", "format": 1, "kind": "ryucad.materials",
                   "materials": {"aluminium": {"colour": "#b8bcc2", "density": "2.7 g/cm^3"},
                                 "steel": {"density": steel}}})
        };
        assert_eq!(
            text(library("7850 kg/m^3")),
            "ryucad.materials (format 1)\n\
             default = steel\n\
             materials.aluminium = density 2.7 g/cm^3, colour #b8bcc2\n\
             materials.steel = density 7850 kg/m^3\n"
        );
        let manifest = (
            "ryucad.json",
            json!({"default_unit": "mm", "format": 1, "kind": "ryucad.project"}),
        );
        let from = files(&[manifest.clone(), ("materials.json", library("7850 kg/m^3"))]);
        let to = files(&[manifest, ("materials.json", library("7.9 g/cm^3"))]);
        let v = structural(&from, &to, &Options::default(), &Releases::default());
        assert_eq!(
            v["materials"]["changes"],
            json!([{"at": "/materials/steel/density", "from": "7850 kg/m^3", "to": "7.9 g/cm^3"}])
        );
        let same = structural(&from, &from, &Options::default(), &Releases::default());
        assert!(same.get("materials").is_none());
    }

    #[test]
    fn the_locks_labels_diff_at_the_top() {
        let lib = "7c1e9a0f3b2d4e5f8a9b0c1d2e3f4a5b";
        let (a, b) = ("a".repeat(40), "b".repeat(40));
        let content = "d".repeat(64);
        let manifest = (
            "ryucad.json",
            json!({"default_unit": "mm", "format": 3, "kind": "ryucad.project",
                   "dependencies": {lib: {"name": "fasteners", "path": "", "url": "file:///f"}}}),
        );
        let lock = |labels: Value| {
            (
                "ryucad.lock",
                json!({"format": 1, "kind": "ryucad.lock", "projects": {lib: {
                    "labels": labels, "path": "", "release_branch": "main", "url": "file:///f",
                    "via": []}}}),
            )
        };
        let from = files(&[
            manifest.clone(),
            lock(json!({"bolt/A": {"commit": a, "content": content},
                        "nut/A": {"commit": a, "content": content}})),
        ]);
        let to = files(&[
            manifest,
            lock(json!({"bolt/A": {"commit": b, "content": content},
                        "washer/A": {"commit": b, "content": content}})),
        ]);
        let v = structural(&from, &to, &Options::default(), &Releases::default());
        let row = |part: &str, commit: Value| {
            json!({"project": lib, "dependency": "fasteners", "part": part, "label": "A",
                   "commit": commit})
        };
        assert_eq!(
            v["deps"],
            json!({"added": [row("washer", json!(b))], "removed": [row("nut", json!(a))],
                   "changed": [row("bolt", json!({"from": a, "to": b}))]})
        );
        let same = structural(&from, &from, &Options::default(), &Releases::default());
        assert!(same.get("deps").is_none());
    }

    #[test]
    fn appearance_textconvs_one_line_an_entry_and_diffs_per_part() {
        let name = "0123456789abcdef0123456789abcdef";
        let file = |colour: &str| {
            json!({"faces": {format!("box/{name}"): {"colour": colour, "opacity": 0.5}},
                   "format": 1, "kind": "ryucad.appearance",
                   "part": {"material": true},
                   "solids": {"lid": {"opacity": 0.25}}})
        };
        assert_eq!(
            text(file("#d03030")),
            format!(
                "ryucad.appearance (format 1)\n\
                 faces.box/{name} = colour #d03030, opacity 0.5\n\
                 part = material colour\n\
                 solids.lid = opacity 0.25\n"
            )
        );
        assert_eq!(
            text(json!({"format": 1, "kind": "ryucad.appearance",
                        "occurrences": {"p1/left": {"colour": "#000000"}}})),
            "ryucad.appearance (format 1)\noccurrences.p1/left = colour #000000\n"
        );
        let manifest = (
            "ryucad.json",
            json!({"default_unit": "mm", "format": 1, "kind": "ryucad.project"}),
        );
        let part = (
            "parts/p/part.json",
            json!({"features": [], "format": 1, "kind": "ryucad.part"}),
        );
        let from = files(&[
            manifest.clone(),
            part.clone(),
            ("parts/p/appearance.json", file("#d03030")),
        ]);
        let to = files(&[
            manifest.clone(),
            part.clone(),
            ("parts/p/appearance.json", file("#2050c0")),
        ]);
        let v = structural(&from, &to, &Options::default(), &Releases::default());
        assert_eq!(
            v["parts"]["p"]["appearance"],
            json!({"changes": [{"at": format!("/faces/box~1{name}/colour"),
                                "from": "#d03030", "to": "#2050c0"}]})
        );
        assert_eq!(v["parts"]["p"]["status"], "changed");
        let none = files(&[manifest, part]);
        let v = structural(&none, &to, &Options::default(), &Releases::default());
        assert_eq!(v["parts"]["p"]["appearance"]["changes"][0]["at"], json!(""));
        let same = structural(&from, &from, &Options::default(), &Releases::default());
        assert!(same["parts"].get("p").is_none());
    }

    #[test]
    fn named_views_textconv_and_diff_per_part() {
        let view = |x: f64| {
            json!({"eye_m": [x, -0.1, 0.1], "format": 1, "fov_deg": 30, "kind": "ryucad.view",
                   "projection": "perspective", "target_m": [0, 0, 0], "up": [0, 0, 1]})
        };
        assert_eq!(
            text(view(0.1)),
            "ryucad.view (format 1)\n\
             eye_m = [0.1, -0.1, 0.1]\n\
             fov_deg = 30\n\
             projection = perspective\n\
             target_m = [0, 0, 0]\n\
             up = [0, 0, 1]\n"
        );
        let manifest = (
            "ryucad.json",
            json!({"default_unit": "mm", "format": 1, "kind": "ryucad.project"}),
        );
        let part = (
            "parts/p/part.json",
            json!({"features": [], "format": 1, "kind": "ryucad.part"}),
        );
        let from = files(&[
            manifest.clone(),
            part.clone(),
            ("parts/p/views/detail.json", view(0.1)),
            ("parts/p/views/old.json", view(0.1)),
        ]);
        let to = files(&[
            manifest,
            part,
            ("parts/p/views/detail.json", view(0.2)),
            ("parts/p/views/new.json", view(0.1)),
        ]);
        let v = structural(&from, &to, &Options::default(), &Releases::default());
        assert_eq!(
            v["parts"]["p"]["views"],
            json!({"added": ["new"], "removed": ["old"],
                   "changed": [{"view": "detail",
                                "changes": [{"at": "/eye_m", "from": [0.1, -0.1, 0.1],
                                             "to": [0.2, -0.1, 0.1]}]}]})
        );
        assert_eq!(v["parts"]["p"]["status"], "changed");
        assert!(v["parts"]["p"]["features"].as_object().unwrap().is_empty());
        let same = structural(&from, &from, &Options::default(), &Releases::default());
        assert!(same["parts"].get("p").is_none());
    }

    #[test]
    fn explodes_textconv_a_step_a_line_and_diff_per_assembly() {
        let file = |z: &str| {
            json!({"format": 1, "kind": "ryucad.explode", "steps": [
                {"instances": ["lid", "p1/left"], "translate": ["0 mm", "0 mm", z]},
                {"instances": ["base"], "rotate": {"angle": "90 deg", "axis": [0, 0, 1]}}
            ]})
        };
        assert_eq!(
            text(file("40 mm")),
            "ryucad.explode (format 1)\n\
             step 1 = lid, p1/left: translate 0 mm, 0 mm, 40 mm\n\
             step 2 = base: rotate 90 deg about [0,0,1]\n"
        );
        let manifest = (
            "ryucad.json",
            json!({"default_unit": "mm", "format": 1, "kind": "ryucad.project"}),
        );
        let asm = (
            "parts/a/part.json",
            json!({"features": [], "format": 2, "kind": "ryucad.part", "part_kind": "assembly"}),
        );
        let from = files(&[
            manifest.clone(),
            asm.clone(),
            ("parts/a/explodes/apart.json", file("40 mm")),
        ]);
        let to = files(&[
            manifest,
            asm,
            ("parts/a/explodes/apart.json", file("50 mm")),
            ("parts/a/explodes/more.json", file("40 mm")),
        ]);
        let v = structural(&from, &to, &Options::default(), &Releases::default());
        let d = &v["parts"]["a"]["explodes"];
        assert_eq!(d["added"], json!(["more"]), "{v}");
        assert_eq!(d["removed"], json!([]));
        assert_eq!(d["changed"][0]["explode"], "apart");
        assert_eq!(d["changed"][0]["changes"][0]["at"], "/steps");
        assert_eq!(v["parts"]["a"]["status"], "changed");
        assert!(v["parts"]["a"].get("views").is_none());
    }

    #[test]
    fn sections_details_flats_and_explodes_textconv_a_line_each() {
        let v = json!({"format": 1, "kind": "ryucad.drawing", "name": "P",
        "sheets": {"s1": {"name": "Sheet 1", "size": "ansi_b"}},
        "views": {
            "v1": {"type": "base", "sheet": "s1", "orient": "top"},
            "v2": {"type": "section", "sheet": "s1", "from": "v1", "label": "A",
                   "cut": {"plane": {"feature": "b", "name": "0123456789abcdef0123456789abcdef"}, "offset": "5 mm"}},
            "v3": {"type": "section", "sheet": "s1", "from": "v1", "label": "B", "flip": true,
                   "cut": {"through": [["0 mm", "1 mm", "0 mm"], "origin"]}},
            "v4": {"type": "detail", "sheet": "s1", "from": "v2", "label": "C",
                   "centre": ["1 mm", "2 mm", "3 mm"], "radius": "4 mm"},
            "v5": {"type": "flat", "sheet": "s1", "solid": "plate"},
            "v6": {"type": "exploded", "sheet": "s1", "of": "rig", "explode": "e1", "orient": "iso"}
        }});
        let t = text(v);
        for line in [
            "views.v2 = section A from v1, on b/0123456789abcdef0123456789abcdef offset 5 mm, sheet s1",
            "views.v3 = section B from v1, through (0 mm, 1 mm, 0 mm) (origin), flip true, sheet s1",
            "views.v4 = detail C from v2, about 1 mm, 2 mm, 3 mm radius 4 mm, sheet s1",
            "views.v5 = flat solid plate, sheet s1",
            "views.v6 = exploded e1 of rig, iso, sheet s1",
        ] {
            assert!(t.contains(line), "{line} in\n{t}");
        }
    }

    #[test]
    fn annotations_textconv_one_line_each() {
        let n = "0123456789abcdef0123456789abcdef";
        let v = json!({"format": 1, "kind": "ryucad.drawing", "name": "P",
        "sheets": {"s1": {"name": "Sheet 1", "size": "ansi_b"}},
        "views": {"v1": {"type": "base", "sheet": "s1", "orient": "top"}},
        "annotations": {
            "d3": {"type": "linear", "view": "v1", "a": format!("b/{n}"), "b": format!("b/{n}"),
                   "along": "horizontal", "offset": "12 mm", "tolerance": {"symmetric": "0.1 mm"}},
            "g1": {"type": "datum_feature", "view": "v1", "letter": "A", "on": format!("b/{n}")}
        }});
        let t = text(v);
        for line in [
            format!(
                "annotations.d3 = linear v1, a b/{n}, b b/{n}, tolerance symmetric 0.1 mm, along horizontal, offset 12 mm"
            ),
            format!("annotations.g1 = datum_feature v1, on b/{n}, letter A"),
        ] {
            assert!(t.contains(&line), "{line} in\n{t}");
        }
    }

    #[test]
    fn drawings_textconv_a_member_a_line_and_diff_per_part() {
        let file = |offset: &str| {
            json!({"format": 1, "kind": "ryucad.drawing", "name": "P",
                "sheets": {"s1": {"name": "Sheet 1", "size": "ansi_b", "scale": "1:2"}},
                "views": {
                    "v1": {"type": "base", "sheet": "s1", "at": ["120 mm", "160 mm"], "orient": "front"},
                    "v10": {"type": "projected", "sheet": "s1", "from": "v1", "side": "iso", "at": ["300 mm", "200 mm"]},
                    "v2": {"type": "projected", "sheet": "s1", "from": "v1", "side": "top", "offset": offset},
                    "v3": {"type": "auxiliary", "sheet": "s1", "from": "v1",
                           "normal_to": {"feature": "b", "name": "0123456789abcdef0123456789abcdef"}}
                },
                "fields": {"drawn_by": "agent"}})
        };
        assert_eq!(
            text(file("70 mm")),
            "ryucad.drawing (format 1)\n\
             sheets.s1 = Sheet 1, ansi_b, scale 1:2\n\
             views.v1 = base front, at 120 mm, 160 mm, sheet s1\n\
             views.v2 = projected from v1, top, offset 70 mm, sheet s1\n\
             views.v3 = auxiliary from v1, normal to b/0123456789abcdef0123456789abcdef, sheet s1\n\
             views.v10 = projected from v1, iso, at 300 mm, 200 mm, sheet s1\n\
             fields.drawn_by = agent\n\
             name = P\n"
        );
        let manifest = (
            "ryucad.json",
            json!({"default_unit": "mm", "format": 1, "kind": "ryucad.project"}),
        );
        let part = (
            "parts/p/part.json",
            json!({"features": [], "format": 1, "kind": "ryucad.part"}),
        );
        let template = json!({"format": 1, "kind": "ryucad.drawing_template",
            "size": "ansi_b", "area": [[10, 10], [421.8, 269.4]]});
        let from = files(&[
            manifest.clone(),
            part.clone(),
            ("parts/p/drawings/d1.json", file("70 mm")),
        ]);
        let to = files(&[
            manifest,
            part,
            ("parts/p/drawings/d1.json", file("80 mm")),
            ("parts/p/drawings/d2.json", file("70 mm")),
            ("templates/ours.json", template),
        ]);
        let v = structural(&from, &to, &Options::default(), &Releases::default());
        let d = &v["parts"]["p"]["drawings"];
        assert_eq!(d["added"], json!(["d2"]), "{v}");
        assert_eq!(d["changed"][0]["drawing"], "d1");
        assert_eq!(d["changed"][0]["changes"][0]["at"], "/views/v2/offset");
        assert_eq!(v["parts"]["p"]["status"], "changed");
        assert_eq!(v["templates"]["added"], json!(["ours"]));
    }

    fn files(list: &[(&str, Value)]) -> Files {
        list.iter()
            .map(|(p, v)| {
                let text = crate::doc::canonical::to_string(v);
                ((*p).to_owned(), text.into_bytes().into())
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
    fn an_assemblys_items_diff_by_part() {
        let asm = |items: Value| {
            json!({"features": [], "format": 3, "items": items, "kind": "ryucad.part",
                   "part_kind": "assembly"})
        };
        let manifest = json!({"default_unit": "mm", "format": 1, "kind": "ryucad.project"});
        let from = files(&[
            ("ryucad.json", manifest.clone()),
            ("parts/a/part.json", asm(json!({"plate": 1, "bolt": 2}))),
            (
                "parts/p/part.json",
                json!({"features": [], "format": 1, "kind": "ryucad.part"}),
            ),
        ]);
        let to = files(&[
            ("ryucad.json", manifest),
            (
                "parts/a/part.json",
                asm(json!({"plate": 1, "bolt": 3, "nut": 4})),
            ),
            (
                "parts/p/part.json",
                json!({"features": ["x"], "format": 1, "kind": "ryucad.part"}),
            ),
        ]);
        let v = structural(&from, &to, &Options::default(), &Releases::default());
        assert_eq!(
            v["parts"]["a"]["items"],
            json!({"added": [{"part": "nut", "to": 4}], "removed": [],
                   "changed": [{"part": "bolt", "from": 2, "to": 3}]})
        );
        // A part with no items has no such block.
        assert!(v["parts"]["p"].get("items").is_none());
    }

    #[test]
    fn project_parameters_diff_with_the_parts_that_read_them() {
        let manifest = json!({"default_unit": "mm", "format": 1, "kind": "ryucad.project"});
        let part = json!({"features": ["a"], "format": 1, "kind": "ryucad.part"});
        let params = |p: Value| json!({"format": 1, "kind": "ryucad.parameters", "parameters": p});
        let side = |wall: &str| {
            files(&[
                ("ryucad.json", manifest.clone()),
                (
                    "parameters.json",
                    params(json!({"wall": wall, "gap": "1 mm"})),
                ),
                ("parts/p/part.json", part.clone()),
                ("parts/p/features/a.json", box_x("wall")),
                ("parts/q/part.json", part.clone()),
                ("parts/q/features/a.json", box_x("gap")),
            ])
        };
        let (from, to) = (side("3 mm"), side("4 mm"));
        let v = structural(&from, &to, &Options::default(), &Releases::default());
        assert_eq!(
            v["parameters"],
            json!({"added": [], "removed": [], "parts": ["p"],
                   "changed": [{"name": "wall", "from": "3 mm", "to": "4 mm",
                                "read_by": {"from": ["p"], "to": ["p"]}}]})
        );
        // No part's own files changed.
        assert_eq!(v["parts"], json!({}));
        let same = structural(&from, &from, &Options::default(), &Releases::default());
        assert!(same.get("parameters").is_none());
        // The root file reads as a part's does.
        let text = String::from_utf8(textconv(&from["parameters.json"])).unwrap();
        assert_eq!(
            text,
            "ryucad.parameters (format 1)\nparameters.gap = 1 mm\nparameters.wall = 3 mm\n"
        );
    }

    #[test]
    fn a_changed_parameter_says_what_reads_it_on_each_side() {
        let manifest = json!({"default_unit": "mm", "format": 1, "kind": "ryucad.project"});
        let part = json!({"features": ["fin"], "format": 1, "kind": "ryucad.part"});
        let params = |p: Value| json!({"format": 1, "kind": "ryucad.parameters", "parameters": p});
        // #672: `fin_t` is read by nothing on either side; `fin_k` reads
        // it, and nothing reads `fin_k`. `span` stops being read.
        let side = |t: &str, x: &str| {
            files(&[
                ("ryucad.json", manifest.clone()),
                ("parameters.json", params(json!({"gap": "1 mm"}))),
                ("parts/heatsink/part.json", part.clone()),
                (
                    "parts/heatsink/parameters.json",
                    params(json!({"fin_t": t, "fin_k": "fin_t * 2", "span": "72 mm",
                                  "fin_h": "30 mm"})),
                ),
                ("parts/heatsink/features/fin.json", box_x(x)),
            ])
        };
        let (from, to) = (side("1.6 mm", "span"), side("1.2 mm", "fin_h"));
        let v = structural(&from, &to, &Options::default(), &Releases::default());
        assert_eq!(
            v["parts"]["heatsink"]["parameters"]["changed"],
            json!([{"name": "fin_t", "from": "1.6 mm", "to": "1.2 mm",
                    "read_by": {"from": [], "to": []}, "unused": true}])
        );
        // The project's `gap`, changed and read by no part, is unused too.
        let mut to = to;
        to.insert(
            "parameters.json".to_owned(),
            crate::doc::canonical::to_string(&params(json!({"gap": "2 mm"})))
                .into_bytes()
                .into(),
        );
        let v = structural(&from, &to, &Options::default(), &Releases::default());
        assert_eq!(
            v["parameters"]["changed"],
            json!([{"name": "gap", "from": "1 mm", "to": "2 mm",
                    "read_by": {"from": [], "to": []}, "unused": true}])
        );
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
        // Who reads each, on each side: `w` the sketch both times; `k`
        // (new) and `h` (gone) nothing, so unused.
        assert_eq!(
            p["parameters"],
            json!({"added": [{"name": "k", "to": "w * 2",
                              "read_by": {"from": [], "to": []}, "unused": true}],
                   "removed": [{"name": "h", "from": "1 mm",
                                "read_by": {"from": [], "to": []}, "unused": true}],
                   "changed": [{"name": "w", "from": "3 mm", "to": "4 mm",
                                "read_by": {"from": ["s"], "to": ["s"]}}]})
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
        broken.insert(
            "parts/p/features/a.json".into(),
            b"{ nope".as_slice().into(),
        );
        let v = structural(&from, &broken, &Options::default(), &Releases::default());
        assert_eq!(
            v["parts"]["p"]["features"]["a"],
            json!({"status": "changed", "invalid": ["to"]})
        );
    }

    #[test]
    fn a_models_change_shows_in_the_drawings_views_it_moves() {
        use crate::git::tests::git;
        use crate::server::tests::{call, fresh};
        let (dir, mut s) = fresh();
        let root = dir.path();
        git(root, &["init", "-q", "-b", "main"]);
        call(&mut s, "part.create", json!({"part": "p"})).unwrap();
        call(
            &mut s,
            "feature.add",
            json!({"part": "p", "type": "box", "id": "b", "params": {"x": 40, "y": 20, "z": 10}}),
        )
        .unwrap();
        call(
            &mut s,
            "drawing.create",
            json!({"part": "p", "drawing": "main", "views": "standard"}),
        )
        .unwrap();
        git(root, &["add", "."]);
        git(root, &["commit", "-q", "-m", "a block drawn"]);
        let v = call(&mut s, "diff", json!({"model": true})).unwrap();
        assert!(v["parts"]["p"].get("drawings").is_none(), "{v}");
        // Taller: the drawing's file doesn't change, every view it shows
        // the box's height in does; the top view doesn't.
        call(
            &mut s,
            "feature.edit",
            json!({"part": "p", "id": "b", "params": {"z": 15}}),
        )
        .unwrap();
        let v = call(&mut s, "diff", json!({"model": true})).unwrap();
        let d = &v["parts"]["p"]["drawings"];
        assert_eq!(d["changed"], json!([]), "{v}");
        assert_eq!(
            d["model"]["main"]["views"]["changed"],
            json!(["v1", "v3", "v4"]),
            "{v}"
        );
        // Without --model, no built views.
        let v = call(&mut s, "diff", json!({})).unwrap();
        assert!(v["parts"]["p"]["drawings"].get("model").is_none(), "{v}");
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
    fn a_model_diff_says_what_kind_each_body_is() {
        use crate::git::tests::git;
        use crate::server::tests::{call, fresh};
        let (dir, mut s) = fresh();
        let root = dir.path();
        git(root, &["init", "-q", "-b", "main"]);
        call(&mut s, "part.create", json!({"part": "p"})).unwrap();
        call(
            &mut s,
            "feature.add",
            json!({"part": "p", "type": "sketch", "id": "s", "plane": "xy"}),
        )
        .unwrap();
        call(
            &mut s,
            "sketch.edit",
            json!({"part": "p", "sketch": "s", "ops": crate::sketch::tests::rectangle_ops()}),
        )
        .unwrap();
        call(
            &mut s,
            "feature.add",
            json!({"part": "p", "type": "extrude", "id": "w", "sketch": "s",
                   "extent": {"type": "blind", "length": "5 mm"}}),
        )
        .unwrap();
        git(root, &["add", "."]);
        git(root, &["commit", "-q", "-m", "start"]);
        // The extrude becomes a surface: a sheet of its sides.
        call(
            &mut s,
            "feature.edit",
            json!({"part": "p", "id": "w", "surface": true}),
        )
        .unwrap();
        let v = call(&mut s, "diff", json!({"model": true})).unwrap();
        let w = &v["parts"]["p"]["solids"]["w"];
        assert_eq!(
            (&w["from"]["kind"], &w["to"]["kind"]),
            (&json!("solid"), &json!("sheet")),
            "{v}"
        );
    }

    #[test]
    fn a_split_whose_plane_moved_changes_both_its_sides() {
        use crate::git::tests::git;
        use crate::server::tests::{call, fresh};
        let (dir, mut s) = fresh();
        let root = dir.path();
        git(root, &["init", "-q", "-b", "main"]);
        call(&mut s, "part.create", json!({"part": "p"})).unwrap();
        for f in [
            json!({"type": "box", "id": "h", "params": {"x": 40, "y": 30, "z": 20}}),
            json!({"type": "plane", "id": "parting", "by": "offset", "from": "xy",
                   "distance": "8 mm"}),
            json!({"type": "split", "id": "s", "target": "h", "plane": "p/parting"}),
        ] {
            let mut f = f;
            f["part"] = json!("p");
            call(&mut s, "feature.add", f).unwrap();
        }
        git(root, &["add", "."]);
        git(root, &["commit", "-q", "-m", "start"]);
        call(
            &mut s,
            "feature.edit",
            json!({"part": "p", "id": "parting", "distance": "10 mm"}),
        )
        .unwrap();
        let v = call(&mut s, "diff", json!({"model": true})).unwrap();
        let solids = &v["parts"]["p"]["solids"];
        for side in ["h", "s"] {
            let x = &solids[side];
            assert_eq!(
                (&x["from"]["ok"], &x["to"]["ok"]),
                (&json!(true), &json!(true)),
                "{v}"
            );
            assert_ne!(x["from"]["hash"], x["to"]["hash"], "{side}: {v}");
        }
        assert_eq!(v["parts"]["p"]["features"]["s"]["status"], "same", "{v}");
    }

    #[test]
    fn a_split_whose_tool_sheet_moved_changes_both_its_sides() {
        use crate::git::tests::git;
        use crate::server::tests::{call, fresh};
        let (dir, mut s) = fresh();
        let root = dir.path();
        git(root, &["init", "-q", "-b", "main"]);
        call(&mut s, "part.create", json!({"part": "p"})).unwrap();
        for f in [
            json!({"type": "box", "id": "h", "params": {"x": 40, "y": 30, "z": 20}}),
            json!({"type": "sketch", "id": "a", "plane": "xy"}),
        ] {
            let mut f = f;
            f["part"] = json!("p");
            call(&mut s, "feature.add", f).unwrap();
        }
        call(
            &mut s,
            "sketch.edit",
            json!({"part": "p", "sketch": "a", "ops": [
                {"op": "add", "id": "p1", "type": "point", "at": [10, -5]},
                {"op": "add", "id": "p2", "type": "point", "at": [10, 35]},
                {"op": "add", "id": "l1", "type": "line", "start": "p1", "end": "p2"}]}),
        )
        .unwrap();
        for f in [
            json!({"type": "extrude", "id": "parting", "sketch": "a", "surface": true,
                   "extent": {"type": "symmetric", "length": "50 mm"}}),
            json!({"type": "split", "id": "s", "target": "h", "tool": "parting"}),
        ] {
            let mut f = f;
            f["part"] = json!("p");
            call(&mut s, "feature.add", f).unwrap();
        }
        git(root, &["add", "."]);
        git(root, &["commit", "-q", "-m", "start"]);
        call(
            &mut s,
            "sketch.edit",
            json!({"part": "p", "sketch": "a", "ops": [
                {"op": "move", "id": "p1", "at": [12, -5]},
                {"op": "move", "id": "p2", "at": [12, 35]}]}),
        )
        .unwrap();
        let v = call(&mut s, "diff", json!({"model": true})).unwrap();
        let solids = &v["parts"]["p"]["solids"];
        for side in ["h", "s"] {
            let x = &solids[side];
            assert_eq!(
                (&x["from"]["ok"], &x["to"]["ok"]),
                (&json!(true), &json!(true)),
                "{v}"
            );
            assert_eq!(
                (&x["from"]["kind"], &x["to"]["kind"]),
                (&json!("solid"), &json!("solid")),
                "{v}"
            );
            assert_ne!(x["from"]["hash"], x["to"]["hash"], "{side}: {v}");
        }
        assert_eq!(v["parts"]["p"]["features"]["s"]["status"], "same", "{v}");
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

    #[test]
    fn blend_edits_read_a_line_a_field_and_a_group() {
        let r = |n: u128| json!({"feature": "b", "name": format!("{n:032x}")});
        let d = json!({"format": 20, "kind": "ryucad.feature", "type": "delete_blend",
            "target": "b", "below": "2 mm", "faces": [r(1), r(2)], "tolerance": "0.001 mm"});
        let t = text(d);
        assert!(
            t.starts_with("ryucad.feature delete_blend (format 20)\n"),
            "{t}"
        );
        for line in ["below = 2 mm", "target = b", "tolerance = 0.001 mm"] {
            assert!(t.lines().any(|l| l == line), "{line}: {t}");
        }
        assert_eq!(
            t.lines().filter(|l| l.starts_with("faces")).count(),
            1,
            "{t}"
        );
        let z = json!({"format": 20, "kind": "ryucad.feature", "type": "resize_blend",
            "target": "b", "groups": [
                {"radius": "3 mm", "faces": [r(1)]},
                {"form": "distances", "d1": "1 mm", "d2": "2 mm", "face": r(9), "faces": [r(2), r(3)]},
                {"form": "distance_angle", "d": "1 mm", "angle": "30 deg", "face": r(9), "faces": [r(4)]},
                {"form": "equal", "d": "1 mm", "faces": [r(5)]}]});
        let t = text(z);
        let b = |n: u128| format!("b/{n:032x}");
        for line in [
            format!("groups[0] = 3 mm: {}", b(1)),
            format!(
                "groups[1] = distances 1 mm, 2 mm from {}: {}, {}",
                b(9),
                b(2),
                b(3)
            ),
            format!(
                "groups[2] = distance_angle 1 mm, 30 deg from {}: {}",
                b(9),
                b(4)
            ),
            format!("groups[3] = equal 1 mm: {}", b(5)),
        ] {
            assert!(t.lines().any(|l| l == line), "{line}: {t}");
        }
    }
}
