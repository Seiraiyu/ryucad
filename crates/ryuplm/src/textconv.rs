//! `ryuplm git textconv`: a ryuplm file as one sorted line per value, so
//! `git diff` shows what changed as values. A header names the kind and
//! format; an item's lists are keyed by entry (`aml` by maker and part
//! number, `links` by product, `bom` by line ID), so a line names its
//! entry rather than its place. Anything that isn't a ryuplm JSON file
//! comes back as it is.

use serde_json::{Map, Value};

/// The file as text for `git diff`.
pub fn textconv(bytes: &[u8]) -> Vec<u8> {
    let Some(mut v) = std::str::from_utf8(bytes)
        .ok()
        .and_then(|t| crate::canonical::parse(t).ok())
    else {
        return bytes.to_vec();
    };
    let Some(kind) = v.get("kind").and_then(Value::as_str).map(str::to_owned) else {
        return bytes.to_vec();
    };
    if kind == crate::item::KIND {
        keyed(&mut v);
    }
    let Some(o) = v.as_object() else {
        return bytes.to_vec();
    };
    let format = v.get("format").map(one).unwrap_or_default();
    let mut lines = Vec::new();
    let mut last = Vec::new();
    let mut body: Map<String, Value> = o
        .iter()
        .filter(|(k, _)| *k != "kind" && *k != "format")
        .map(|(k, x)| (k.clone(), x.clone()))
        .collect();
    // ryupdm's settings and records read as ryupdm writes them.
    let flat = |at: &str, v: &Value, out: &mut Vec<String>| flatten(at, v, out);
    ryupdm::git::textconv(&kind, &mut body, &mut lines, &mut last, &flat);
    flatten("", &Value::Object(body), &mut lines);
    lines.sort();
    let mut out = format!("{kind} format {format}\n");
    for l in lines.into_iter().chain(last) {
        out.push_str(&l);
        out.push('\n');
    }
    out.into_bytes()
}

/// The item's lists as objects keyed as the merge keys them: `aml` by
/// `<manufacturer>/<mpn>`, `links` by product, `bom` by line ID (the key
/// removed from each entry), so a diff line names its entry.
pub fn keyed(v: &mut serde_json::Value) {
    let Some(o) = v.as_object_mut() else { return };
    for (list, key) in [
        ("aml", None),
        ("links", Some("product")),
        ("bom", Some("line")),
    ] {
        let Some(serde_json::Value::Array(items)) = o.remove(list) else {
            continue;
        };
        let mut m = serde_json::Map::new();
        for mut e in items {
            let k = match key {
                Some(k) => e
                    .as_object_mut()
                    .and_then(|x| x.remove(k))
                    .and_then(|x| x.as_str().map(str::to_owned)),
                None => {
                    let x = e.as_object_mut();
                    x.and_then(|x| {
                        let m = x.remove("manufacturer")?.as_str()?.to_owned();
                        let p = x.remove("mpn")?.as_str()?.to_owned();
                        // Each part escaped as a JSON pointer token (`/` is `~1`),
                        // so the one `/` between them splits back exactly.
                        Some(format!(
                            "{}/{}",
                            ryu_git::value::escape(&m),
                            ryu_git::value::escape(&p)
                        ))
                    })
                }
            };
            m.insert(k.unwrap_or_else(|| format!("#{}", m.len())), e);
        }
        o.insert(list.to_owned(), serde_json::Value::Object(m));
    }
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

#[cfg(test)]
mod tests {
    use serde_json::json;

    #[test]
    fn an_item_has_a_line_per_field() {
        let v = json!({"format": 1, "kind": "ryuplm.item", "number": "A-1",
            "aml": [{"manufacturer": "Acme", "mpn": "S-1", "status": "active"}],
            "bom": [{"line": "0000000000001", "item": "k3v9q2m8x4c1p", "quantity": "4"}]});
        let out =
            String::from_utf8(super::textconv(crate::canonical::to_string(&v).as_bytes())).unwrap();
        assert_eq!(
            out,
            "ryuplm.item format 1\n\
             aml.Acme/S-1.status = active\n\
             bom.0000000000001.item = k3v9q2m8x4c1p\n\
             bom.0000000000001.quantity = 4\n\
             number = A-1\n"
        );
        assert_eq!(super::textconv(b"not json"), b"not json");
    }
}
