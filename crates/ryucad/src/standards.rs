//! `standards.list`: what the hole standards hold, built in and the
//! project's, each value with its source, so an agent can look a size up
//! before writing it. No fastener or pipe tap drill is built in: those
//! come from the project's `hole_standards.json`.

use crate::doc::hole_standards::{HOLE_STANDARDS, HoleStandards, Kind};
use crate::error::{Code, Error, Result};
use ryu_standards::thread::{Pitch, Standard, lookup, sizes};
use ryu_standards::{Source, drill};
use serde_json::{Value, json};

fn valued(text: String, source: Source) -> Value {
    json!({"value": text, "source": source.text()})
}

/// A length as an exact decimal: inches where it is a whole number of
/// hundred-thousandths of an inch and `inch`, else millimetres.
fn length_text(nm: i64, inch: bool) -> String {
    match inch.then(|| ryu_standards::in_decimal(nm)).flatten() {
        Some(t) => format!("{t} in"),
        None => format!("{} mm", ryu_standards::mm_decimal(nm)),
    }
}

fn length(nm: i64, inch: bool, source: Source) -> Value {
    valued(length_text(nm, inch), source)
}

fn not_found(msg: String, standard: &str) -> Error {
    Error::new(Code::NotFound, msg).with("standard", standard)
}

/// The tables `standards.list` reads: `table` is `threads`, `fasteners`
/// or `drills`; `standard` a standard's key (or a drill series), and
/// `size` one of its sizes. `project` is the project's
/// `hole_standards.json`, or why it can't be read.
pub fn list(
    project: Option<&std::result::Result<HoleStandards, Error>>,
    table: &str,
    standard: Option<&str>,
    size: Option<&str>,
) -> Result<Value> {
    let ok = project.and_then(|r| r.as_ref().ok());
    // A table that can't be read is the answer wherever it would be read.
    let needs_table = match (table, standard) {
        ("fasteners", _) => true,
        ("threads", Some(s)) => Standard::parse(s).is_none(),
        _ => false,
    };
    if needs_table && let Some(Err(e)) = project {
        return Err(e.clone());
    }
    match (table, standard, size) {
        ("threads", None, _) => {
            let mut out: Vec<Value> = Standard::ALL
                .iter()
                .map(|s| json!({"standard": s.key(), "built_in": true, "sizes": sizes(*s).len()}))
                .collect();
            for (k, m) in ok.map(|t| &t.threads).into_iter().flatten() {
                if Standard::parse(k).is_none() {
                    out.push(json!({"standard": k, "built_in": false, "sizes": m.len(), "source": "project"}));
                }
            }
            Ok(json!({"standards": out}))
        }
        ("threads", Some(s), None) => {
            let mut rows = Vec::new();
            if let Some(b) = Standard::parse(s) {
                for name in sizes(b) {
                    let Ok(t) = lookup(b, name, None, None) else {
                        continue;
                    };
                    let pitches: Vec<String> = match (b, ryu_standards::iso_metric::size(name)) {
                        (Standard::IsoMetric, Some(z)) => {
                            z.pitches.iter().map(|&p| Pitch::Mm(p).text()).collect()
                        }
                        _ => vec![t.pitch.value.text()],
                    };
                    let mut row = json!({"size": name, "pitches": pitches});
                    if let Some(c) = &t.class {
                        row["class"] = json!(c);
                    }
                    rows.push(row);
                }
            }
            for k in ok
                .and_then(|t| t.threads.get(s))
                .into_iter()
                .flat_map(|m| m.keys())
            {
                rows.push(json!({"size": k, "source": "project",
                                 "entry": ok.and_then(|t| t.entry_json(Kind::Thread, s, k))}));
            }
            if rows.is_empty() {
                return Err(not_found(
                    format!(
                        "no thread standard {s}: built in are {}, and a project's come from {HOLE_STANDARDS}",
                        Standard::ALL.map(Standard::key).join(", ")
                    ),
                    s,
                ));
            }
            Ok(json!({"standard": s, "sizes": rows}))
        }
        ("threads", Some(s), Some(z)) => {
            let entry = ok.and_then(|t| t.entry_json(Kind::Thread, s, z));
            let missing = || not_found(format!("{s} has no size {z}"), s).with("size", z);
            let Some(b) = Standard::parse(s) else {
                return entry
                    .map(|e| json!({"standard": s, "size": z, "project": e}))
                    .ok_or_else(missing);
            };
            let t = lookup(b, z, None, None).map_err(|_| missing())?;
            let inch = b.inch();
            let mut v = json!({"standard": s, "size": t.size});
            if let Some(m) = t.major {
                v["major"] = length(m.value, inch, m.source);
            }
            match t.pitch.value {
                Pitch::Mm(p) => v["pitch"] = length(p, false, t.pitch.source),
                p => v["tpi"] = valued(p.text(), t.pitch.source),
            }
            if let Some((a, c)) = t.taper {
                v["taper"] = valued(format!("{a}:{c}"), Source::Fact);
            }
            let classes: Vec<Value> = class_names(b)
                .into_iter()
                .filter_map(|c| lookup(b, z, None, Some(&c)).ok().map(|x| (c, x)))
                .filter_map(|(c, x)| {
                    let (lo, hi) = x.minor?;
                    let mut row = json!({"class": c, "side": "internal", "minor": [
                        length(lo.value, inch, lo.source), length(hi.value, inch, hi.source)]});
                    if let Some(m) = x.major_min {
                        row["major_min"] = length(m.value, inch, m.source);
                    }
                    Some(row)
                })
                .collect();
            // Then the external classes built in (ISO metric g and h), with
            // their major diameter's limits.
            let external = external_names(b)
                .into_iter()
                .filter_map(|c| lookup(b, z, None, Some(&c)).ok().map(|x| (c, x)))
                .filter_map(|(c, x)| {
                    let (lo, hi) = (x.major_min?, x.major_max?);
                    Some(json!({"class": c, "side": "external", "major": [
                        length(lo.value, inch, lo.source), length(hi.value, inch, hi.source)]}))
                });
            let classes: Vec<Value> = classes.into_iter().chain(external).collect();
            v["classes"] = json!(classes);
            v["tap_drill"] = match ryu_standards::tap::rule(&t) {
                Some(d) => {
                    json!({"value": length_text(d.nm, inch), "source": "rule", "drill": d.name})
                }
                None => Value::Null,
            };
            if let Some(e) = entry {
                v["project"] = e;
            }
            Ok(v)
        }
        ("fasteners", None, _) => {
            let out: Vec<Value> = ok
                .map(|t| {
                    t.fasteners
                        .iter()
                        .map(|(k, m)| json!({"standard": k, "sizes": m.len(), "source": "project"}))
                        .collect()
                })
                .unwrap_or_default();
            Ok(json!({"standards": out}))
        }
        ("fasteners", Some(s), size) => {
            let m = ok.and_then(|t| t.fasteners.get(s)).ok_or_else(|| {
                not_found(
                    format!("no fastener standard {s}: fasteners come from {HOLE_STANDARDS}"),
                    s,
                )
                .with("file", HOLE_STANDARDS)
            })?;
            if let Some(z) = size
                && !m.contains_key(z)
            {
                return Err(not_found(format!("{s} has no size {z}"), s).with("size", z));
            }
            let rows: Vec<Value> = m
                .keys()
                .filter(|k| size.is_none_or(|z| z == k.as_str()))
                .map(|k| {
                    json!({"size": k, "source": "project",
                                "entry": ok.and_then(|t| t.entry_json(Kind::Fastener, s, k))})
                })
                .collect();
            Ok(json!({"standard": s, "sizes": rows}))
        }
        ("drills", series, _) => {
            let want = match series {
                None => None,
                Some("metric") => Some(drill::Series::Metric),
                Some("fractional") => Some(drill::Series::Fractional),
                Some("number") => Some(drill::Series::Number),
                Some("letter") => Some(drill::Series::Letter),
                Some(x) => {
                    return Err(Error::new(
                        Code::Usage,
                        format!("{x:?}: drill series are metric, fractional, number or letter"),
                    )
                    .with("param", "standard"));
                }
            };
            let mut list: Vec<&drill::Drill> = drill::all()
                .iter()
                .filter(|d| want.is_none_or(|w| d.series == w))
                .collect();
            if want == Some(drill::Series::Number) {
                // #1 first, as the gauge counts.
                list.sort_by_key(|d| d.name[1..].parse::<u32>().unwrap_or(0));
            }
            let out: Vec<Value> = list
                .iter()
                .map(|d| {
                    let inch = d.series != drill::Series::Metric;
                    json!({"name": d.name, "value": length_text(d.nm, inch), "source": Source::Fact.text()})
                })
                .collect();
            Ok(json!({"drills": out}))
        }
        _ => Err(
            Error::new(Code::Usage, "standards lists threads, fasteners or drills")
                .with("param", "table"),
        ),
    }
}

/// A standard's built-in external classes, by name: ISO metric's g and
/// h. Unified external classes come from a project's table (H28/2B
/// prints no external limits).
fn external_names(s: Standard) -> Vec<String> {
    match s {
        Standard::IsoMetric => ryu_standards::iso_metric_limits::ExtClass::all()
            .iter()
            .map(|c| c.text())
            .collect(),
        _ => Vec::new(),
    }
}

/// A standard's internal classes, by name.
fn class_names(s: Standard) -> Vec<String> {
    match s {
        Standard::IsoMetric => ryu_standards::iso_metric_limits::Class::all()
            .iter()
            .map(|c| c.text())
            .collect(),
        Standard::Unc | Standard::Unf | Standard::Unef | Standard::Un => {
            ryu_standards::unified::Class::ALL
                .iter()
                .map(|c| c.text().to_owned())
                .collect()
        }
        _ => Vec::new(),
    }
}
