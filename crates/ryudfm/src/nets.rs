//! `nets`: the copper's islands as the copper connects them, a page at a
//! time: each island's name (its least piece), layers, item and hole
//! counts, the netlist's nets on it, the nets its items claim, and its
//! point contacts. With `net`, the islands holding that net's points;
//! with `layer`, those with copper on it; with `island`, one island with
//! its items, pieces and holes. The board step only.

use crate::copper::{CopperBoard, Key, Source};
use crate::error::{Code, Error, Result};
use ryu_copper::connect::Island;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

/// What to list.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Args {
    /// The islands holding this net's points.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub net: Option<String>,
    /// One island, by name.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub island: Option<String>,
    /// The islands with copper on this layer.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub layer: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub limit: Option<usize>,
    /// The page after this island.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub after: Option<String>,
}

fn invalid(msg: &str, param: &str) -> Error {
    Error::new(Code::InvalidParam, msg).with("param", param)
}

/// Where the netlist came from, as `nets` and `check` name it.
pub fn source_name(s: Source) -> &'static str {
    match s {
        Source::Cad => "cad",
        Source::Pins => "pins",
        Source::None => "none",
    }
}

/// An island as `nets` lists it.
pub fn island_json(cb: &CopperBoard, prefix: Option<&str>, i: &Island<Key>) -> Value {
    let contacts = cb.point_contacts(i);
    json!({
        "island": cb.island_name(prefix, &i.id),
        "layers": i.layers.iter().map(|&l| cb.layer_name(l)).collect::<Vec<_>>(),
        "item_count": i.items.len(),
        "hole_count": i.holes.len(),
        "nets": cb.nets.get(&i.id).map(|s| s.iter().cloned().collect::<Vec<_>>()).unwrap_or_default(),
        "claims": i.claims,
        "point_contacts": contacts.iter().map(|pc| json!({
            "layer": cb.layer_name(pc.layer),
            "a": cb.reference(prefix, pc.a),
            "b": cb.reference(prefix, pc.b),
            "near_nm": crate::inspect::rect(pc.near),
        })).collect::<Vec<_>>(),
    })
}

/// Board `cb`'s islands; `prefix` is its ID in a project.
pub fn nets(cb: &CopperBoard, prefix: Option<&str>, args: &Args) -> Result<Value> {
    if args.limit == Some(0) {
        return Err(invalid("a page holds at least one island", "limit"));
    }
    let islands = cb.copper.islands();
    if let Some(text) = &args.island {
        if args.net.is_some() || args.layer.is_some() || args.after.is_some() {
            return Err(invalid(
                "--island shows one island: it takes no --net, --layer or --after",
                "island",
            ));
        }
        let id = cb.island_of_name(prefix, text)?;
        let i = islands
            .iter()
            .find(|i| i.id == id)
            .expect("a named island is one");
        let mut v = island_json(cb, prefix, i);
        v["items"] = json!(
            i.items
                .iter()
                .map(|k| cb.reference(prefix, *k))
                .collect::<Vec<_>>()
        );
        v["pieces"] = json!(
            i.pieces
                .iter()
                .map(|p| cb.island_name(prefix, p))
                .collect::<Vec<_>>()
        );
        v["holes"] = json!(
            i.holes
                .iter()
                .map(|h| cb.reference(prefix, *h))
                .collect::<Vec<_>>()
        );
        return Ok(json!({"board": prefix, "step": cb.step_name(), "island": v}));
    }
    let layer = args.layer.as_deref().map(|l| cb.layer_of(l)).transpose()?;
    if let Some(n) = &args.net
        && !cb.built.layout.points.iter().any(|p| &p.net == n)
    {
        return Err(
            Error::new(Code::NotFound, format!("no net {n} in the board's netlist")).with("net", n),
        );
    }
    let chosen: Vec<&Island<Key>> = islands
        .iter()
        .filter(|i| layer.is_none_or(|l| i.layers.contains(&l)))
        .filter(|i| {
            args.net
                .as_ref()
                .is_none_or(|n| cb.nets.get(&i.id).is_some_and(|s| s.contains(n)))
        })
        .collect();
    let total = chosen.len();
    let start = match &args.after {
        None => 0,
        Some(a) => {
            let id = cb.island_of_name(prefix, a)?;
            chosen
                .iter()
                .position(|i| i.id == id)
                .map(|p| p + 1)
                .ok_or_else(|| {
                    Error::new(Code::NotFound, format!("no island {a} to start after"))
                        .with("after", a)
                })?
        }
    };
    let limit = args.limit.unwrap_or(crate::inspect::PAGE);
    let page: Vec<&Island<Key>> = chosen.into_iter().skip(start).take(limit).collect();
    let next = (start + page.len() < total)
        .then(|| page.last().map(|i| cb.island_name(prefix, &i.id)))
        .flatten();
    Ok(json!({
        "board": prefix,
        "step": cb.step_name(),
        "islands": page.iter().map(|i| island_json(cb, prefix, i)).collect::<Vec<_>>(),
        "total": total,
        "next": next,
        "islands_total": islands.len(),
        "source": source_name(cb.built.source),
        "judged": cb.report.judged,
        "points": cb.built.layout.points.len(),
        "left_out": cb.built.left_out.len(),
        "plating_from_tools": cb.built.plating_from_tools,
        "uncertain_contacts": cb.copper.uncertain().len(),
    }))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::copper::CopperBoard;
    use std::sync::Arc;

    fn mc() -> CopperBoard {
        let path =
            std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/corpus/minimal-clean.tgz");
        CopperBoard::new(Arc::new(
            crate::odb::read_path(&path, &Default::default()).unwrap(),
        ))
    }

    #[test]
    fn islands_a_page_at_a_time() {
        let cb = mc();
        let all = nets(&cb, None, &Args::default()).unwrap();
        let total = all["total"].as_u64().unwrap() as usize;
        assert_eq!(total, cb.copper.islands().len());
        assert_eq!(all["source"], "cad");
        assert!(all["judged"].as_bool().unwrap());
        let one = Args {
            limit: Some(1),
            ..Args::default()
        };
        let p1 = nets(&cb, None, &one).unwrap();
        assert_eq!(p1["islands"].as_array().unwrap().len(), 1);
        let after = p1["next"].as_str().unwrap().to_string();
        let p2 = nets(
            &cb,
            None,
            &Args {
                after: Some(after.clone()),
                ..one.clone()
            },
        )
        .unwrap();
        assert_eq!(p2["islands"][0], all["islands"][1]);
        // One island, by its name.
        let shown = nets(
            &cb,
            None,
            &Args {
                island: Some(after.clone()),
                ..Args::default()
            },
        )
        .unwrap();
        assert_eq!(shown["island"]["island"], serde_json::json!(after));
        assert!(!shown["island"]["items"].as_array().unwrap().is_empty());
        // A net's islands carry it.
        let gnd = nets(
            &cb,
            None,
            &Args {
                net: Some("GND".into()),
                ..Args::default()
            },
        )
        .unwrap();
        assert!(gnd["total"].as_u64().unwrap() >= 1);
        for i in gnd["islands"].as_array().unwrap() {
            assert!(
                i["nets"]
                    .as_array()
                    .unwrap()
                    .contains(&serde_json::json!("GND"))
            );
        }
        let bottom = nets(
            &cb,
            None,
            &Args {
                layer: Some("bottom".into()),
                ..Args::default()
            },
        )
        .unwrap();
        for i in bottom["islands"].as_array().unwrap() {
            assert!(
                i["layers"]
                    .as_array()
                    .unwrap()
                    .contains(&serde_json::json!("bottom"))
            );
        }
    }

    #[test]
    fn refusals() {
        let cb = mc();
        let code = |a: Args| nets(&cb, None, &a).unwrap_err().code;
        assert_eq!(
            code(Args {
                limit: Some(0),
                ..Args::default()
            }),
            Code::InvalidParam
        );
        assert_eq!(
            code(Args {
                net: Some("NOPE".into()),
                ..Args::default()
            }),
            Code::NotFound
        );
        assert_eq!(
            code(Args {
                layer: Some("top_overlay".into()),
                ..Args::default()
            }),
            Code::InvalidParam
        );
        assert_eq!(
            code(Args {
                layer: Some("nope".into()),
                ..Args::default()
            }),
            Code::NotFound
        );
        assert_eq!(
            code(Args {
                island: Some("pcb/top/999999".into()),
                ..Args::default()
            }),
            Code::NotFound
        );
        assert_eq!(
            code(Args {
                island: Some("pcb/top/0".into()),
                net: Some("GND".into()),
                ..Args::default()
            }),
            Code::InvalidParam
        );
    }
}
