//! The transfer report (design 448, #593): what an import interpreted
//! from a STEP file, kept in its committed bytes, found unreferenced and
//! failed; what an export wrote and left out. ryustep's census counts;
//! this gives it the protocol's shape. Category and bucket names are
//! stable, as error codes are; `summary` strings are for people.

use ryustep::census::{Bucket, Counted, Failed, Line, Transfer};
use serde_json::{Value, json};

/// How many names a bucket lists without `detail`.
pub const SUMMARY_NAMES: usize = 8;

/// `t`, the census of the file at `file`, as answers give it.
pub fn json(t: &Transfer, file: &str, detail: bool) -> Value {
    let categories: Vec<Value> = t
        .lines
        .iter()
        .map(|l| {
            let mut v = json!({"category": l.category.as_str()});
            for b in Bucket::ALL {
                v[b.as_str()] = counted(&l.buckets[b as usize], detail);
            }
            if !l.codes.is_empty() {
                v["codes"] = l
                    .codes
                    .iter()
                    .map(|(code, count)| json!({"code": code, "count": count}))
                    .collect();
            }
            v["summary"] = json!(summary(l));
            v
        })
        .collect();
    let summary: Vec<String> = t.lines.iter().map(summary).collect();
    json!({"file": file, "instances": t.instances, "detail": detail,
           "categories": categories, "summary": summary})
}

fn counted(c: &Counted, detail: bool) -> Value {
    let take = if detail { usize::MAX } else { SUMMARY_NAMES };
    let names: Vec<Value> = c
        .names
        .iter()
        .take(take)
        .map(|n| {
            let mut v = json!({"name": n.name, "count": n.count});
            if detail {
                v["instances"] = n.first.iter().map(|i| json!(format!("#{i}"))).collect();
            }
            v
        })
        .collect();
    let mut v = json!({"count": c.count, "names": names});
    if c.names.len() > take {
        v["more"] = json!(c.names.len() - take);
    }
    v
}

/// One category as a line for people: each non-empty bucket's count and
/// its three largest names.
pub fn summary(l: &Line) -> String {
    let parts: Vec<String> = Bucket::ALL
        .iter()
        .map(|&b| (b, &l.buckets[b as usize]))
        .filter(|(_, c)| c.count > 0)
        .map(|(b, c)| {
            let top: Vec<String> = c
                .names
                .iter()
                .take(3)
                .map(|n| format!("{} {}", n.name, n.count))
                .collect();
            format!("{} {} ({})", c.count, b.as_str(), top.join(", "))
        })
        .collect();
    format!("{}: {}", l.category.as_str(), parts.join("; "))
}

/// The instances an error (as answers give it: `{code, details}`) names
/// in `details.step` (`#n`), with its code.
pub fn failed_instances(error: &Value) -> Vec<Failed> {
    let code = error["code"].as_str().unwrap_or("import_failed");
    error["details"]["step"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|s| s.as_str()?.strip_prefix('#')?.parse().ok())
        .map(|instance| Failed {
            instance,
            code: code.to_owned(),
        })
        .collect()
}

/// How many unplaced occurrence styles a report lists.
pub const UNPLACED: usize = 64;

/// Adds the occurrence styles no instance stands for (design 448) to
/// `report`'s appearance category ([`json`]'s shape): they stay in the
/// committed bytes, counted `retained`, each with its styled item and the
/// usages its path names; at most [`UNPLACED`], with `unplaced_more`.
pub fn with_unplaced(report: &mut Value, unplaced: Vec<Value>) {
    if unplaced.is_empty() {
        return;
    }
    let Some(c) = report["categories"]
        .as_array_mut()
        .and_then(|cs| cs.iter_mut().find(|c| c["category"] == "appearance"))
    else {
        return;
    };
    let more = unplaced.len().saturating_sub(UNPLACED);
    c["unplaced"] = Value::Array(unplaced.into_iter().take(UNPLACED).collect());
    if more > 0 {
        c["unplaced_more"] = json!(more);
    }
}
