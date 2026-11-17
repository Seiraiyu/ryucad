//! `clearance`: every pair of different islands closer than a length on
//! each copper layer asked (all by default), closest first, then by
//! layer and pieces: the least distance between them, with the pieces it
//! lies between and their nearest points. Once per pair of islands per
//! layer unless `all`, which lists every pair of pieces. Gaps a resolved
//! piece's bound leaves open are listed with `uncertain`.

use std::collections::BTreeSet;
use std::sync::Arc;

use crate::copper::{CopperBoard, Key};
use crate::error::{Code, Error, Result};
use crate::units::Unit;
use rayon::prelude::*;
use ryu_copper::connect::{Gap, IslandId};
use ryu_copper::measure::Verdict;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

/// What to list.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Args {
    /// The length gaps are under: a quantity, bare numbers in the default
    /// unit.
    pub below: String,
    /// Copper layers; every one by default.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub layers: Vec<String>,
    /// Pairs with this net (the netlist's, or claimed) on either side.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub net: Option<String>,
    /// Every pair of pieces, not each pair of islands once.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub all: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub limit: Option<usize>,
    /// The page after this pair's `id`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub after: Option<String>,
}

fn invalid(msg: &str, param: &str) -> Error {
    Error::new(Code::InvalidParam, msg).with("param", param)
}

/// Board `cb`'s gaps; bare lengths are in `unit`.
pub fn clearance(cb: &CopperBoard, prefix: Option<&str>, args: &Args, unit: Unit) -> Result<Value> {
    let below = crate::quantity::length(&args.below, unit)?;
    if below <= 0 {
        return Err(invalid("--below takes a positive length", "below"));
    }
    if args.limit == Some(0) {
        return Err(invalid("a page holds at least one pair", "limit"));
    }
    let layers: Vec<usize> = if args.layers.is_empty() {
        (0..cb.built.copper.len()).collect()
    } else {
        args.layers
            .iter()
            .map(|l| cb.layer_of(l))
            .collect::<Result<_>>()?
    };
    let nets = |id: &IslandId<Key>| -> Vec<String> {
        let mut v: BTreeSet<String> = cb.nets.get(id).cloned().unwrap_or_default();
        v.extend(cb.claims().get(id).cloned().unwrap_or_default());
        v.into_iter().collect()
    };
    if let Some(n) = &args.net
        && !cb.built.layout.points.iter().any(|p| &p.net == n)
        && !cb.claims().values().any(|c| c.contains(n))
    {
        return Err(Error::new(Code::NotFound, format!("no net {n} on the board")).with("net", n));
    }
    let found: Vec<Arc<Vec<Gap<Key>>>> = layers.par_iter().map(|&l| cb.gaps(l, below)).collect();
    let mut gaps: Vec<&Gap<Key>> = found.iter().flat_map(|g| g.iter()).collect();
    gaps.sort_by(|x, y| {
        (x.distance.nm, x.layer, &x.a, &x.b).cmp(&(y.distance.nm, y.layer, &y.a, &y.b))
    });
    if let Some(n) = &args.net {
        gaps.retain(|g| nets(&g.islands.0).contains(n) || nets(&g.islands.1).contains(n));
    }
    if !args.all {
        let mut seen = BTreeSet::new();
        gaps.retain(|g| {
            let (x, y) = (&g.islands.0, &g.islands.1);
            seen.insert((g.layer, *x.min(y), *x.max(y)))
        });
    }
    let id = |g: &Gap<Key>| {
        format!(
            "{}|{}|{}",
            cb.layer_name(g.layer),
            cb.island_name(prefix, &g.a),
            cb.island_name(prefix, &g.b)
        )
    };
    let total = gaps.len();
    let start = match &args.after {
        None => 0,
        Some(a) => gaps
            .iter()
            .position(|g| &id(g) == a)
            .map(|p| p + 1)
            .ok_or_else(|| {
                Error::new(Code::NotFound, format!("no pair {a} to start after")).with("after", a)
            })?,
    };
    let limit = args.limit.unwrap_or(crate::inspect::PAGE);
    let page: Vec<&Gap<Key>> = gaps.into_iter().skip(start).take(limit).collect();
    let next = (start + page.len() < total)
        .then(|| page.last().map(|g| id(g)))
        .flatten();
    let side = |island: &IslandId<Key>, piece: &(Key, u32), at| {
        json!({
            "island": cb.island_name(prefix, island),
            "piece": cb.island_name(prefix, piece),
            "nets": nets(island),
            "at": crate::inspect::point(at),
        })
    };
    Ok(json!({
        "board": prefix,
        "step": cb.step_name(),
        "below_nm": below,
        "layers": layers.iter().map(|&l| cb.layer_name(l)).collect::<Vec<_>>(),
        "all": args.all,
        "pairs": page.iter().map(|g| json!({
            "id": id(g),
            "layer": cb.layer_name(g.layer),
            "distance_nm": g.distance.nm,
            "exact": g.distance.exact,
            "bound_nm": g.distance.bound,
            "uncertain": g.verdict == Verdict::Uncertain,
            "a": side(&g.islands.0, &g.a, g.distance.a),
            "b": side(&g.islands.1, &g.b, g.distance.b),
        })).collect::<Vec<_>>(),
        "total": total,
        "next": next,
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

    fn below(b: &str) -> Args {
        Args {
            below: b.into(),
            ..Args::default()
        }
    }

    #[test]
    fn pairs_of_islands_closest_first() {
        let cb = mc();
        // 50 mm: every pair of islands on a layer.
        let v = clearance(&cb, None, &below("50"), Unit::Mm).unwrap();
        let pairs = v["pairs"].as_array().unwrap();
        assert!(!pairs.is_empty());
        let d = |p: &Value| p["distance_nm"].as_i64().unwrap();
        assert!(pairs.windows(2).all(|w| d(&w[0]) <= d(&w[1])));
        let mut seen = std::collections::BTreeSet::new();
        for p in pairs {
            assert_ne!(p["a"]["island"], p["b"]["island"]);
            let key = (
                p["layer"].to_string(),
                p["a"]["island"].to_string(),
                p["b"]["island"].to_string(),
            );
            assert!(seen.insert(key), "a pair of islands listed twice: {p}");
        }
        let all = clearance(
            &cb,
            None,
            &Args {
                all: true,
                ..below("50")
            },
            Unit::Mm,
        )
        .unwrap();
        assert!(all["total"].as_u64() >= v["total"].as_u64());
        // A page, and the page after it.
        let one = Args {
            limit: Some(1),
            ..below("50")
        };
        let p1 = clearance(&cb, None, &one, Unit::Mm).unwrap();
        let next = p1["next"].as_str().unwrap().to_string();
        let p2 = clearance(
            &cb,
            None,
            &Args {
                after: Some(next),
                ..one
            },
            Unit::Mm,
        )
        .unwrap();
        assert_eq!(p2["pairs"][0], pairs[1]);
        // Nothing is closer than a nanometre on a board that checks clean.
        let tiny = clearance(&cb, None, &below("1 nm"), Unit::Mm).unwrap();
        assert_eq!(tiny["total"], 0);
        // A net's pairs carry it on one side.
        let gnd = clearance(
            &cb,
            None,
            &Args {
                net: Some("GND".into()),
                ..below("50")
            },
            Unit::Mm,
        )
        .unwrap();
        for p in gnd["pairs"].as_array().unwrap() {
            let has = |s: &Value| {
                s["nets"]
                    .as_array()
                    .unwrap()
                    .contains(&serde_json::json!("GND"))
            };
            assert!(has(&p["a"]) || has(&p["b"]), "{p}");
        }
    }

    #[test]
    fn refusals() {
        let cb = mc();
        let code = |a: Args| clearance(&cb, None, &a, Unit::Mm).unwrap_err().code;
        assert_eq!(code(below("-1")), Code::InvalidParam);
        assert_eq!(code(below("0")), Code::InvalidParam);
        assert_eq!(code(below("x")), Code::InvalidQuantity);
        assert_eq!(
            code(Args {
                layers: vec!["top_overlay".into()],
                ..below("1")
            }),
            Code::InvalidParam
        );
        assert_eq!(
            code(Args {
                net: Some("NOPE".into()),
                ..below("1")
            }),
            Code::NotFound
        );
        assert_eq!(
            code(Args {
                limit: Some(0),
                ..below("1")
            }),
            Code::InvalidParam
        );
    }
}
