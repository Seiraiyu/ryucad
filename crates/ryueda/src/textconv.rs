//! `ryueda git textconv`: a ryueda file as one sorted line per value, so
//! `git diff` shows what changed as values. A header names the kind and
//! format. Textconv sees one file, so it names other objects by ID;
//! `ryueda diff` (plan 4.B) resolves names. Anything that isn't a ryueda
//! JSON file comes back as it is.

use serde_json::{Map, Value};

/// The file as text for `git diff`.
pub fn textconv(bytes: &[u8]) -> Vec<u8> {
    let Some(v) = std::str::from_utf8(bytes)
        .ok()
        .and_then(|t| crate::canonical::parse(t).ok())
    else {
        return bytes.to_vec();
    };
    let (Some(kind), Some(o)) = (v.get("kind").and_then(Value::as_str), v.as_object()) else {
        return bytes.to_vec();
    };
    let format = v.get("format").map(one).unwrap_or_default();
    let mut lines = Vec::new();
    let body: Map<String, Value> = o
        .iter()
        .filter(|(k, _)| *k != "kind" && *k != "format")
        .map(|(k, x)| (k.clone(), x.clone()))
        .collect();
    match kind {
        "ryueda.component" => component(&body, &mut lines),
        "ryueda.block" => block(&body, &mut lines),
        "ryueda.layout" => layout(&body, &mut lines),
        "ryueda.symbol" => members(
            &body,
            &[("pins", "pin"), ("graphics", "graphic")],
            &mut lines,
        ),
        "ryueda.footprint" => members(
            &body,
            &[
                ("pads", "pad"),
                ("graphics", "graphic"),
                ("keepouts", "keepout"),
            ],
            &mut lines,
        ),
        "ryueda.style" => style(&body, &mut lines),
        "ryueda.net" | "ryueda.rail" => net(&body, &mut lines),
        _ => flatten("", &Value::Object(body), &mut lines),
    }
    lines.sort();
    let mut out = format!("{kind} format {format}\n");
    for l in lines {
        out.push_str(&l);
        out.push('\n');
    }
    out.into_bytes()
}

/// A value on one line: strings bare, arrays of scalars joined by `, `,
/// anything else as compact JSON.
fn one(v: &Value) -> String {
    match v {
        Value::String(s) => s.clone(),
        Value::Array(a) if a.iter().all(|x| !x.is_object() && !x.is_array()) => {
            a.iter().map(one).collect::<Vec<_>>().join(", ")
        }
        other => other.to_string(),
    }
}

/// Every scalar under `v` as `path = value`, objects by key (`a.b`);
/// arrays are one value.
fn flatten(at: &str, v: &Value, out: &mut Vec<String>) {
    match v {
        Value::Object(o) if !o.is_empty() => {
            for (k, x) in o {
                let path = if at.is_empty() {
                    k.clone()
                } else {
                    format!("{at}.{k}")
                };
                flatten(&path, x, out);
            }
        }
        _ if at.is_empty() => {}
        other => out.push(format!("{at} = {}", one(other))),
    }
}

/// A library reference `{library?, id}`: `<library>/<id>`, `self` for the
/// file's own library.
fn libref(v: &Value) -> String {
    let lib = v.get("library").and_then(Value::as_str).unwrap_or("self");
    format!("{lib}/{}", v.get("id").map(one).unwrap_or_default())
}

/// What a pin or a port connects to.
fn node(v: &Value) -> String {
    match v {
        Value::String(s) if s == "nc" => "nc".to_owned(),
        Value::String(id) => format!("net {id}"),
        Value::Object(o) if o.contains_key("rail") => format!("rail {}", one(&o["rail"])),
        Value::Object(o) if o.contains_key("port") => format!("port {}", one(&o["port"])),
        other => other.to_string(),
    }
}

fn component(o: &Map<String, Value>, out: &mut Vec<String>) {
    for (k, v) in o {
        match k.as_str() {
            "of" | "footprint" => out.push(format!("{k} = {}", libref(v))),
            "pins" => {
                for (pin, n) in v.as_object().into_iter().flatten() {
                    out.push(format!("pin {pin} = {}", node(n)));
                }
            }
            _ => flatten(k, v, out),
        }
    }
}

fn net(o: &Map<String, Value>, out: &mut Vec<String>) {
    for (k, v) in o {
        match (k.as_str(), v.get("with"), v.get("polarity")) {
            ("pair", Some(w), Some(p)) => out.push(format!("pair = {} ({})", one(w), one(p))),
            _ => flatten(k, v, out),
        }
    }
}

fn block(o: &Map<String, Value>, out: &mut Vec<String>) {
    for (k, v) in o {
        match k.as_str() {
            "ports" => {
                for (id, p) in v.as_object().into_iter().flatten() {
                    let mut what = vec![p.get("direction").map(one).unwrap_or_default()];
                    what.extend(p.get("voltage").map(one));
                    out.push(format!(
                        "port {id} = {} ({})",
                        p.get("name").map(one).unwrap_or_default(),
                        what.join(", ")
                    ));
                }
            }
            "buses" => {
                for (id, b) in v.as_object().into_iter().flatten() {
                    let nets = b.get("nets").map(one).unwrap_or_default();
                    out.push(format!(
                        "bus {id} = {}: {nets}",
                        b.get("name").map(one).unwrap_or_default()
                    ));
                }
            }
            _ => flatten(k, v, out),
        }
    }
}

/// Members of a map one line each (`pin 1 = {…}`), the rest flattened.
fn members(o: &Map<String, Value>, maps: &[(&str, &str)], out: &mut Vec<String>) {
    for (k, v) in o {
        match maps.iter().find(|(m, _)| m == k) {
            Some((_, word)) => {
                for (id, x) in v.as_object().into_iter().flatten() {
                    out.push(format!("{word} {id} = {x}"));
                }
            }
            None => flatten(k, v, out),
        }
    }
}

fn style(o: &Map<String, Value>, out: &mut Vec<String>) {
    for (k, v) in o {
        match k.as_str() {
            // Order sets specificity: one numbered line per rule.
            "rules" => {
                for (i, r) in v.as_array().into_iter().flatten().enumerate() {
                    out.push(format!("rule {:03} = {r}", i + 1));
                }
            }
            "overrides" => {
                for (key, x) in v.as_object().into_iter().flatten() {
                    out.push(format!("override {key} = {x}"));
                }
            }
            "locks" => out.push(format!("locks = {}", one(v))),
            _ => flatten(k, v, out),
        }
    }
}

/// Nanometres as exact millimetres: `50800000` is `50.8 mm`.
fn mm(nm: i64) -> String {
    let sign = if nm < 0 { "-" } else { "" };
    let n = nm.unsigned_abs();
    let (whole, frac) = (n / 1_000_000, n % 1_000_000);
    let frac = format!("{frac:06}");
    let frac = frac.trim_end_matches('0');
    if frac.is_empty() {
        format!("{sign}{whole} mm")
    } else {
        format!("{sign}{whole}.{frac} mm")
    }
}

/// A layout in ryusch's key space (`ryusch::merge::Key`): a part's place
/// as words, every other key as JSON on one line.
fn layout(o: &Map<String, Value>, out: &mut Vec<String>) {
    for (section, v) in o {
        let entries = v.as_object().cloned().unwrap_or_default();
        match section.as_str() {
            "placed" => {
                for (part, p) in &entries {
                    let at = p.get("at").and_then(Value::as_array);
                    let xy: Vec<String> = at
                        .into_iter()
                        .flatten()
                        .filter_map(Value::as_i64)
                        .map(mm)
                        .collect();
                    let mut s = format!(
                        "placed/{part} = sheet {} at {}",
                        p.get("sheet").map(one).unwrap_or_default(),
                        xy.join(", ")
                    );
                    s.push_str(&format!(
                        ", {} deg",
                        p.get("rotation").and_then(Value::as_u64).unwrap_or(0)
                    ));
                    for flag in ["mirror", "held"] {
                        if p.get(flag) == Some(&Value::Bool(true)) {
                            s.push_str(&format!(", {flag}"));
                        }
                    }
                    out.push(s);
                }
            }
            "labels" => {
                for (net, list) in &entries {
                    for l in list.as_array().into_iter().flatten() {
                        out.push(format!(
                            "labels/{net}|{} = {l}",
                            l.get("on").map(one).unwrap_or_default()
                        ));
                    }
                }
            }
            "routes" | "buses" => {
                for (key, by_sheet) in &entries {
                    for (sheet, x) in by_sheet.as_object().into_iter().flatten() {
                        out.push(format!("{section}/{key}|{sheet} = {x}"));
                    }
                }
            }
            "instructions" => {
                for (id, i) in &entries {
                    out.push(format!("instruction {id} = {i}"));
                }
            }
            _ => {
                for (key, x) in &entries {
                    out.push(format!("{section}/{key} = {x}"));
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn conv(json: &str) -> String {
        String::from_utf8(textconv(json.as_bytes())).unwrap()
    }

    #[test]
    fn a_component_reads_by_pin() {
        let t = conv(
            r#"{"format": 2, "kind": "ryueda.component", "of": {"id": "r_0402"},
            "refdes": "R5", "value": "10k", "footprint": {"library": "passives", "id": "r0402"},
            "parameters": {"tol": "1%"}, "pins": {"1": "4k2m9x0q1a", "2": {"rail": "gnd"}, "3": "nc", "4": {"port": "p1"}},
            "tags": ["dnp", "hv"], "refdes_locked": true}"#,
        );
        assert_eq!(
            t,
            "ryueda.component format 2\n\
footprint = passives/r0402\n\
of = self/r_0402\n\
parameters.tol = 1%\n\
pin 1 = net 4k2m9x0q1a\n\
pin 2 = rail gnd\n\
pin 3 = nc\n\
pin 4 = port p1\n\
refdes = R5\n\
refdes_locked = true\n\
tags = dnp, hv\n\
value = 10k\n"
        );
    }

    #[test]
    fn a_layout_reads_by_key() {
        let t = conv(
            r#"{"format": 4, "kind": "ryueda.layout",
            "placed": {"R5": {"sheet": "s1", "at": [50800000, 25400000], "rotation": 90}},
            "labels": {"n1": [{"sheet": "s1", "at": [1, 2], "side": "left", "kind": "local", "on": "R5/1", "text": "SDA"}]},
            "instructions": {"i3": {"beside": {"parts": ["U1"], "of": "U2", "side": "left"}}}}"#,
        );
        assert!(t.starts_with("ryueda.layout format 4\n"), "{t}");
        assert!(
            t.contains("placed/R5 = sheet s1 at 50.8 mm, 25.4 mm, 90 deg\n"),
            "{t}"
        );
        assert!(t.contains("labels/n1|R5/1 = "), "{t}");
        assert!(t.contains("instruction i3 = "), "{t}");
    }

    #[test]
    fn a_block_reads_ports_and_buses() {
        let t = conv(
            r#"{"format": 1, "kind": "ryueda.block", "name": "power",
            "ports": {"p1": {"name": "VIN", "direction": "power", "voltage": "5 V"}},
            "buses": {"b1": {"name": "I2C", "nets": ["n1", "n2"]}}}"#,
        );
        assert!(t.contains("port p1 = VIN (power, 5 V)\n"), "{t}");
        assert!(t.contains("bus b1 = I2C: n1, n2\n"), "{t}");
        assert!(t.contains("name = power\n"), "{t}");
    }

    #[test]
    fn symbols_footprints_and_styles_read_by_member() {
        let s = conv(
            r#"{"format": 1, "kind": "ryueda.symbol", "name": "R", "units": 1,
            "pins": {"1": {"name": "", "type": "passive", "at": [0, 0], "side": 0, "length_nm": 2540000}},
            "graphics": {"g1": {"rect": {"from": [0, 0], "to": [1, 1]}}}}"#,
        );
        assert!(s.contains("pin 1 = {"), "{s}");
        assert!(s.contains("graphic g1 = {"), "{s}");
        let f = conv(
            r#"{"format": 1, "kind": "ryueda.footprint", "pads": {"p1": {"number": "1"}}, "graphics": {}}"#,
        );
        assert!(f.contains("pad p1 = {\"number\":\"1\"}\n"), "{f}");
        let st = conv(
            r##"{"format": 1, "kind": "ryueda.style", "palette": {"ink": "#000000"},
            "rules": [{"select": "net", "set": {"width": 1}}, {"select": "part", "set": {}}], "locks": ["ink"]}"##,
        );
        assert!(st.contains("palette.ink = #000000\n"), "{st}");
        assert!(st.contains("rule 001 = {"), "{st}");
        assert!(st.contains("rule 002 = {"), "{st}");
        assert!(st.contains("locks = ink\n"), "{st}");
    }

    #[test]
    fn anything_else_flattens_and_non_json_comes_back() {
        let t = conv(
            r#"{"format": 1, "kind": "ryueda.net", "name": "SDA", "pair": {"with": "x", "polarity": "p"}}"#,
        );
        assert_eq!(t, "ryueda.net format 1\nname = SDA\npair = x (p)\n");
        let m = conv(
            r#"{"format": 1, "kind": "ryueda.project", "project": "ab", "libraries": {"passives": {"path": "../lib"}}}"#,
        );
        assert!(m.contains("libraries.passives.path = ../lib\n"), "{m}");
        assert_eq!(
            textconv(b"library/models/** binary\n"),
            b"library/models/** binary\n"
        );
        assert_eq!(textconv(b"{not json"), b"{not json");
    }
}
