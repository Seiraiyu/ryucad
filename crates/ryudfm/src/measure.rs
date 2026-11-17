//! `measure`: the exact distance between two features, component pins,
//! holes or nets on a copper layer they share: `--layer`, else the least
//! over every one. A feature is its pieces, a pin the pieces holding it
//! on its side's outer layer (or the hole holding it), a hole its wall on
//! every layer it spans, a net the pieces of every island holding its
//! points. Plain pieces are exact; a resolved piece's distance carries
//! its bound (`bound_nm`), and `touching` is `null` where the bounds
//! can't say.

use std::collections::BTreeMap;

use crate::copper::{CopperBoard, Key};
use crate::error::{Code, Error, Result};
use ryu_board::model::{Ref, Side};
use ryu_copper::connect::Node;
use ryu_copper::index::nearest_pair;
use ryu_copper::layout::PointSide;
use ryu_copper::measure::{self as m, Body, Distance, Verdict};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

/// What to measure.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Args {
    pub a: String,
    pub b: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub layer: Option<String>,
}

/// Labelled bodies on each copper layer.
type Sets = BTreeMap<usize, Vec<(String, Body)>>;

fn invalid(msg: String, param: &str) -> Error {
    Error::new(Code::InvalidParam, msg).with("param", param)
}

fn add_item(cb: &CopperBoard, prefix: Option<&str>, key: Key, out: &mut Sets) {
    let Some(layer) = cb.copper.item(&key).map(|it| it.layer) else {
        return;
    };
    for (i, b) in cb.copper.piece_bodies(&key) {
        out.entry(layer)
            .or_default()
            .push((cb.island_name(prefix, &(key, i)), b));
    }
}

/// What a reference stands for, per copper layer.
fn sets(cb: &CopperBoard, prefix: Option<&str>, text: &str, what: &str) -> Result<Sets> {
    let s = &cb.board.steps[cb.built.step];
    let mut out = Sets::new();
    if let Ref::Net { step, name } = crate::inspect::parse_reference(prefix, text, what)?
        && step == s.name
    {
        let ids: Vec<_> = cb
            .nets
            .iter()
            .filter(|(_, n)| n.contains(&name))
            .map(|(id, _)| id)
            .collect();
        if ids.is_empty() {
            return Err(Error::new(
                Code::NotFound,
                format!("no point of net {name} lies on copper"),
            )
            .with(what, text));
        }
        for id in ids {
            let island = cb
                .copper
                .island(&Node::Piece(id.0, id.1))
                .expect("an island");
            let mut items = island.items.clone();
            items.dedup();
            for k in items {
                add_item(cb, prefix, k, &mut out);
            }
        }
        return Ok(out);
    }
    let key = cb.key_of(prefix, text, what)?;
    match key {
        Key::Feature { .. } => {
            if cb.copper.item(&key).is_some() {
                add_item(cb, prefix, key, &mut out);
            } else if let Some(h) = cb.built.layout.holes.get(&key) {
                let body = h.shape.body();
                for l in h.span.0..=h.span.1 {
                    out.entry(l)
                        .or_default()
                        .push((cb.reference(prefix, key), body.clone()));
                }
            } else {
                return Err(invalid(
                    format!("{text} is neither copper nor a hole"),
                    what,
                ));
            }
        }
        Key::Pin { component, pin } => {
            let c = &s.components[component as usize];
            let at = c.pins[pin as usize].at;
            let side = match c.side {
                Side::Top => PointSide::Top,
                Side::Bottom => PointSide::Bottom,
                Side::Inner => {
                    return Err(invalid(format!("{text}'s component is on no side"), what));
                }
            };
            let layer = cb
                .copper
                .outer(side)
                .ok_or_else(|| invalid("the board has no copper".into(), what))?;
            let pt = Body::point(at);
            for (hk, h) in &cb.built.layout.holes {
                if h.span.0 <= layer
                    && layer <= h.span.1
                    && m::touches(&h.shape.body(), &pt) == Verdict::Yes
                {
                    out.entry(layer)
                        .or_default()
                        .push((cb.reference(prefix, *hk), h.shape.body()));
                }
            }
            if out.is_empty() {
                for n in cb.copper.bodies().near_body(layer, &pt, 0) {
                    for (i, b) in cb.copper.piece_bodies(&n.key) {
                        if m::contains(&b, at) == Verdict::Yes {
                            out.entry(layer)
                                .or_default()
                                .push((cb.island_name(prefix, &(n.key, i)), b));
                        }
                    }
                }
            }
            if out.is_empty() {
                return Err(invalid(format!("{text} lies on no copper"), what));
            }
        }
        Key::Cad { .. } => unreachable!("key_of names features and pins"),
    }
    Ok(out)
}

/// The body labelled `t` on layer `l`.
fn find<'a>(s: &'a Sets, l: usize, t: &str) -> &'a Body {
    &s[&l].iter().find(|(x, _)| x == t).expect("measured").1
}

/// Measures between two references on board `cb`; `prefix` is its ID in a
/// project.
pub fn measure(cb: &CopperBoard, prefix: Option<&str>, args: &Args) -> Result<Value> {
    let a = sets(cb, prefix, &args.a, "a")?;
    let b = sets(cb, prefix, &args.b, "b")?;
    let layers: Vec<usize> = match &args.layer {
        Some(name) => {
            let l = cb.layer_of(name)?;
            if !a.contains_key(&l) || !b.contains_key(&l) {
                return Err(invalid(
                    format!("{} and {} aren't both on {name}", args.a, args.b),
                    "layer",
                ));
            }
            vec![l]
        }
        None => a.keys().filter(|l| b.contains_key(l)).copied().collect(),
    };
    if layers.is_empty() {
        return Err(invalid(
            format!("{} and {} share no copper layer", args.a, args.b),
            "b",
        ));
    }
    let mut best: Option<(usize, Distance, String, String)> = None;
    for l in layers {
        if let Some((d, ta, tb)) = nearest_pair(&a[&l], &b[&l])
            && best
                .as_ref()
                .is_none_or(|(bl, bd, ..)| (d.nm, l) < (bd.nm, *bl))
        {
            best = Some((l, d, ta, tb));
        }
    }
    let (l, d, ta, tb) =
        best.ok_or_else(|| invalid("nothing to measure: no copper".into(), "a"))?;
    let touching = match m::touches(find(&a, l, &ta), find(&b, l, &tb)) {
        Verdict::Yes => json!(true),
        Verdict::No => json!(false),
        Verdict::Uncertain => Value::Null,
    };
    Ok(json!({
        "board": prefix,
        "a": args.a,
        "b": args.b,
        "layer": cb.layer_name(l),
        "distance_nm": d.nm,
        "exact": d.exact,
        "bound_nm": d.bound,
        "touching": touching,
        "a_item": ta,
        "b_item": tb,
        "a_at": crate::inspect::point(d.a),
        "b_at": crate::inspect::point(d.b),
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

    fn args(a: &str, b: &str) -> Args {
        Args {
            a: a.into(),
            b: b.into(),
            layer: None,
        }
    }

    #[test]
    fn two_features_as_ryu_copper_measures_them() {
        let cb = mc();
        // The first two items on the top layer on different islands.
        let top: Vec<_> = cb
            .copper
            .items()
            .filter(|(_, it)| it.layer == 0)
            .map(|(k, _)| *k)
            .collect();
        let island = |k| cb.copper.island_of(&ryu_copper::connect::Node::Piece(k, 0));
        let a = top[0];
        let b = *top.iter().find(|k| island(**k) != island(a)).unwrap();
        let v = measure(
            &cb,
            None,
            &args(&cb.reference(None, a), &cb.reference(None, b)),
        )
        .unwrap();
        let want = ryu_copper::index::nearest_pair(
            &cb.copper.piece_bodies(&a),
            &cb.copper.piece_bodies(&b),
        )
        .unwrap()
        .0;
        assert_eq!(v["distance_nm"], serde_json::json!(want.nm));
        assert_eq!(v["layer"], "top");
        assert_eq!(v["touching"], serde_json::json!(false));
        // A feature against itself touches.
        let same = measure(
            &cb,
            None,
            &args(&cb.reference(None, a), &cb.reference(None, a)),
        )
        .unwrap();
        assert_eq!(
            (same["distance_nm"].clone(), same["touching"].clone()),
            (serde_json::json!(0), serde_json::json!(true))
        );
        // Two nets, on every layer they share.
        let nets = measure(&cb, None, &args("pcb/net/GND", "pcb/net/+5")).unwrap();
        assert!(nets["distance_nm"].as_i64().unwrap() > 0);
    }

    #[test]
    fn refusals() {
        let cb = mc();
        let code = |a: &str, b: &str| measure(&cb, None, &args(a, b)).unwrap_err().code;
        assert_eq!(code("pcb/top/999999", "pcb/net/GND"), Code::NotFound);
        assert_eq!(code("pcb/net/NOPE", "pcb/net/GND"), Code::NotFound);
        assert_eq!(code("pcb/top_overlay/0", "pcb/net/GND"), Code::InvalidParam);
        assert_eq!(code("nonsense", "pcb/net/GND"), Code::InvalidParam);
        let on = |l: &str| {
            measure(
                &cb,
                None,
                &Args {
                    layer: Some(l.into()),
                    ..args("pcb/net/GND", "pcb/net/+5")
                },
            )
        };
        assert_eq!(on("top_overlay").unwrap_err().code, Code::InvalidParam);
    }
}
