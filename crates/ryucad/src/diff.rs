//! What changed between two versions of a project: [`textconv`] renders
//! one file as lines for Git's own diff to compare.

use serde_json::{Map, Value};

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

    #[test]
    fn what_isnt_json_comes_back_as_it_is() {
        for b in [&b"{ nope"[..], b"<<<<<<< ours\n", b"\xff\xfe", b"[1, 2]"] {
            assert_eq!(textconv(b), b);
        }
        assert_eq!(textconv(b"{\"a\": [{\"b\": 1}]}"), b"json\na.0.b = 1\n");
    }
}
