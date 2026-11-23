//! `board measure` (design 924): the exact distance between two items of a
//! board (pads, tracks, vias, holes, parts by their courtyards, nets) on
//! each copper layer they share, with the clearance the rules ask of them.

use super::connect::Connectivity;
use super::layout::Key;
use super::rules::Rules;
use super::shapes::BoardShapes;
use crate::error::{Code, Error, Result};
use ryu_copper::connect::Node;
use ryu_copper::measure::{self, Body, BodyOptions, Verdict};
use serde_json::{Value, json};
use std::collections::BTreeMap;

/// What a reference stands for: copper pieces by layer (with each piece's
/// island and net), or a shape on no layer (a hole, a courtyard).
enum Target {
    Copper(
        BTreeMap<usize, Vec<(Body, Option<(Key, u32)>)>>,
        Vec<String>,
    ),
    Plane(Body),
}

fn full(board: &str, r: &str) -> String {
    if r.contains(':') {
        r.to_owned()
    } else {
        format!("{board}:{r}")
    }
}

fn target(c: &Connectivity, s: &BoardShapes, r: &str) -> Result<Target> {
    let board = c.built.board.as_str();
    let not_found = || {
        Error::new(
            Code::NotFound,
            format!("board {board} has no pad, track, via, hole, part or net {r}"),
        )
        .with("ref", r)
    };
    let opts = BodyOptions::default();
    if let Some(net) = r.strip_prefix("net:") {
        let keys: Vec<Key> = c
            .built
            .layout
            .items
            .iter()
            .filter(|(_, it)| it.net.as_deref() == Some(net))
            .map(|(k, _)| k.clone())
            .collect();
        if keys.is_empty() {
            return Err(not_found());
        }
        return Ok(copper(c, &keys, vec![net.to_owned()]));
    }
    let f = full(board, r);
    if let Some(owner) = f.strip_suffix("#hole") {
        let h = s
            .holes
            .iter()
            .find(|h| h.owner == owner)
            .ok_or_else(not_found)?;
        return Ok(Target::Plane(h.shape.body()));
    }
    if let Some(h) = s
        .holes
        .iter()
        .find(|h| h.owner == f && matches!(h.of, super::shapes::HoleOf::Hole(_)))
    {
        return Ok(Target::Plane(h.shape.body()));
    }
    let keys: Vec<Key> = c
        .built
        .layout
        .items
        .keys()
        .filter(|k| c.built.reference(k) == f)
        .cloned()
        .collect();
    if !keys.is_empty() {
        let nets = keys
            .iter()
            .filter_map(|k| c.built.layout.items[k].net.clone())
            .collect();
        return Ok(copper(c, &keys, nets));
    }
    if let Some(ct) = s.courtyards.iter().find(|ct| ct.owner == f) {
        return Ok(Target::Plane(Body::of_shape(&ct.shape, &opts)));
    }
    Err(not_found())
}

fn copper(c: &Connectivity, keys: &[Key], mut nets: Vec<String>) -> Target {
    let mut by: BTreeMap<usize, Vec<(Body, Option<(Key, u32)>)>> = BTreeMap::new();
    for k in keys {
        let layer = c.built.layout.items[k].layer;
        for (i, b) in c.copper.piece_bodies(k) {
            by.entry(layer).or_default().push((b, Some((k.clone(), i))));
        }
    }
    nets.sort();
    nets.dedup();
    Target::Copper(by, nets)
}

/// The closest pair of bodies of two lists: its distance and whether the
/// two touch, and whether they lie on one island.
fn closest(
    c: &Connectivity,
    a: &[(Body, Option<(Key, u32)>)],
    b: &[(Body, Option<(Key, u32)>)],
) -> Option<Value> {
    let mut best: Option<(ryu_copper::measure::Distance, Verdict, bool)> = None;
    for (x, xk) in a {
        for (y, yk) in b {
            let Some(d) = measure::distance(x, y) else {
                continue;
            };
            if best.as_ref().is_some_and(|(e, ..)| e.nm <= d.nm) {
                continue;
            }
            let island = |k: &Option<(Key, u32)>| {
                k.as_ref()
                    .and_then(|(k, i)| c.copper.island_of(&Node::Piece(k.clone(), *i)))
            };
            let same = matches!((island(xk), island(yk)), (Some(p), Some(q)) if p == q);
            best = Some((d, measure::touches(x, y), same));
        }
    }
    let (d, t, same) = best?;
    Some(json!({
        "distance_nm": d.nm,
        "exact": d.exact,
        "bound_nm": d.bound,
        "a_at": [d.a.x, d.a.y],
        "b_at": [d.b.x, d.b.y],
        "touching": t == Verdict::Yes || same,
        "touch_uncertain": t == Verdict::Uncertain,
        "same_island": same,
    }))
}

/// The nets a target's copper claims.
fn nets(t: &Target) -> Vec<&str> {
    match t {
        Target::Copper(_, n) => n.iter().map(String::as_str).collect(),
        Target::Plane(_) => Vec::new(),
    }
}

/// `board.measure`'s core: per copper layer both items are on (or the
/// one asked), the closest distance, and the clearance the pair needs.
pub fn measure(
    c: &Connectivity,
    s: &BoardShapes,
    rules: &Rules,
    a: &str,
    b: &str,
    layer: Option<&str>,
) -> Result<Value> {
    let only = match layer {
        None => None,
        Some(l) => Some(
            c.built
                .layout
                .layers
                .iter()
                .position(|x| x.name == l || x.name == format!("copper.{l}"))
                .ok_or_else(|| {
                    Error::new(
                        Code::BoardLayerNotFound,
                        format!("the board has no copper layer {l}"),
                    )
                    .with("layer", l)
                })?,
        ),
    };
    let (ta, tb) = (target(c, s, a)?, target(c, s, b)?);
    let need = rules.pair_clearance(&nets(&ta), &nets(&tb));
    let mut layers = Vec::new();
    let with = |v: Option<Value>, l: Option<usize>| -> Option<Value> {
        let mut v = v?;
        v["layer"] = json!(l.map(|l| c.built.layer_name(l)));
        v["clearance"] = json!(need);
        let d = v["distance_nm"].as_i64().unwrap_or(0);
        v["under"] = json!(d < need.value && v["same_island"] != true);
        Some(v)
    };
    match (&ta, &tb) {
        (Target::Copper(x, _), Target::Copper(y, _)) => {
            for (l, xs) in x {
                if only.is_some_and(|o| o != *l) {
                    continue;
                }
                if let Some(ys) = y.get(l)
                    && let Some(v) = with(closest(c, xs, ys), Some(*l))
                {
                    layers.push(v);
                }
            }
        }
        (Target::Copper(x, _), Target::Plane(p)) | (Target::Plane(p), Target::Copper(x, _)) => {
            let plane = vec![(p.clone(), None)];
            for (l, xs) in x {
                if only.is_some_and(|o| o != *l) {
                    continue;
                }
                if let Some(v) = with(closest(c, xs, &plane), Some(*l)) {
                    layers.push(v);
                }
            }
        }
        (Target::Plane(p), Target::Plane(q)) => {
            if let Some(v) = with(closest(c, &[(p.clone(), None)], &[(q.clone(), None)]), None) {
                layers.push(v);
            }
        }
    }
    if layers.is_empty() {
        return Err(Error::new(
            Code::InvalidParam,
            format!(
                "{a} and {b} share no copper layer{}",
                layer.map(|l| format!(" ({l})")).unwrap_or_default()
            ),
        ));
    }
    let closest = layers
        .iter()
        .min_by_key(|v| v["distance_nm"].as_i64().unwrap_or(i64::MAX))
        .cloned();
    Ok(json!({
        "board": format!("board/{}", c.built.board),
        "a": full(&c.built.board, a),
        "b": full(&c.built.board, b),
        "layers": layers,
        "closest": closest,
    }))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::board::file::CopperLayer;
    use crate::board::testing::{MM, UM, board};

    fn rules() -> super::super::rules::Rules {
        super::super::rules::Rules::from_parts(
            std::sync::Arc::new(super::super::preset::JLC_2L),
            None,
            Default::default(),
        )
    }

    #[test]
    fn two_tracks_are_measured_exactly_on_their_layer() {
        let mut s = board(20 * MM, 10 * MM);
        s.track(
            "t1",
            "a",
            CopperLayer::Top,
            250 * UM,
            &[(2 * MM, 2 * MM), (18 * MM, 2 * MM)],
        )
        .track(
            "t2",
            "b",
            CopperLayer::Top,
            250 * UM,
            &[(2 * MM, 3 * MM), (18 * MM, 3 * MM)],
        );
        let c = crate::board::connect::connect("b", &s);
        let v = measure(&c, &s, &rules(), "track/t1", "track/t2", None).unwrap();
        let on = &v["layers"][0];
        assert_eq!(on["layer"], "copper.top");
        assert_eq!(on["distance_nm"], 1_000_000 - 250_000);
        assert_eq!(on["exact"], true);
        assert_eq!(on["touching"], false);
        assert_eq!(on["clearance"]["value"], 100_000);
        assert_eq!(on["clearance"]["source"], "preset:jlc-2l");
        assert_eq!(on["under"], false);
        assert_eq!(v["closest"]["distance_nm"], 750_000);
    }

    #[test]
    fn a_pad_and_its_own_track_touch_and_a_hole_measures_in_the_plane() {
        let mut s = board(20 * MM, 10 * MM);
        s.through(
            "j1",
            "1",
            (10 * MM, 5 * MM),
            1700 * UM,
            1000 * UM,
            Some("a"),
        )
        .track(
            "t1",
            "a",
            CopperLayer::Top,
            250 * UM,
            &[(2 * MM, 5 * MM), (10 * MM, 5 * MM)],
        )
        .hole("h1", (15 * MM, 5 * MM), 1000 * UM);
        let c = crate::board::connect::connect("b", &s);
        let v = measure(&c, &s, &rules(), "j1/1", "track/t1", Some("copper.top")).unwrap();
        assert_eq!(v["layers"][0]["touching"], true);
        assert_eq!(v["layers"][0]["same_island"], true);
        let h = measure(&c, &s, &rules(), "hole/h1", "j1/1#hole", None).unwrap();
        assert_eq!(h["layers"][0]["layer"], serde_json::Value::Null);
        assert_eq!(h["closest"]["distance_nm"], 5 * MM - 1000 * UM);
        let e = measure(&c, &s, &rules(), "track/zz", "j1/1", None).unwrap_err();
        assert_eq!(e.code, crate::error::Code::NotFound);
        let e = measure(&c, &s, &rules(), "track/t1", "hole/h1", Some("copper.in7")).unwrap_err();
        assert_eq!(e.code, crate::error::Code::BoardLayerNotFound);
    }

    #[test]
    fn a_net_is_every_piece_claiming_it() {
        let mut s = board(20 * MM, 10 * MM);
        s.track(
            "t1",
            "a",
            CopperLayer::Top,
            250 * UM,
            &[(2 * MM, 2 * MM), (4 * MM, 2 * MM)],
        )
        .track(
            "t2",
            "a",
            CopperLayer::Top,
            250 * UM,
            &[(14 * MM, 2 * MM), (16 * MM, 2 * MM)],
        )
        .track(
            "t3",
            "b",
            CopperLayer::Top,
            250 * UM,
            &[(15 * MM, 4 * MM), (16 * MM, 4 * MM)],
        );
        let c = crate::board::connect::connect("b", &s);
        let v = measure(&c, &s, &rules(), "net:a", "track/t3", None).unwrap();
        assert_eq!(v["closest"]["distance_nm"], 2 * MM - 250 * UM);
    }
}
