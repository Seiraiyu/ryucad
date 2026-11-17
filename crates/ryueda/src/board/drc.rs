//! DRC (design 732): a board's findings, ERC's shape, from its shapes,
//! its copper's connectivity against the design's netlist, and its
//! preset's rules. Every topology decision is `ryu-copper`'s exact
//! predicate; a rule a bound can't decide is reported, never dropped.

use super::connect::{self, Connectivity};
use super::layout::Key;
use super::load::BoardFiles;
use super::occ::Occ;
use super::preset::Preset;
use super::shapes::BoardShapes;
use crate::error::Code;
use crate::fault::Severity;
use rayon::prelude::*;
use ryu_copper::connect::{IslandId, Node, Placement};
use ryu_copper::measure::{self, Body, BodyOptions, Verdict};
use serde::Serialize;
use serde_json::{Value, json};
use std::collections::{BTreeMap, BTreeSet};

/// One finding on a board.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct Finding {
    pub id: String,
    pub code: Code,
    pub severity: Severity,
    pub board: String,
    pub refs: Vec<String>,
    pub message: String,
    #[serde(skip_serializing_if = "Value::is_null")]
    pub details: Value,
}

impl Finding {
    pub fn is_error(&self) -> bool {
        self.severity == Severity::Error
    }
}

/// A finding's stable ID: 16 hex digits of BLAKE3 over its code, board
/// and sorted references.
pub fn finding_id(code: Code, board: &str, refs: &[String]) -> String {
    let mut h = blake3::Hasher::new();
    h.update(b"ryueda.board-finding.1\0");
    h.update(json!(code).as_str().unwrap_or_default().as_bytes());
    h.update(b"\0");
    h.update(board.as_bytes());
    let mut sorted: Vec<&String> = refs.iter().collect();
    sorted.sort();
    for r in sorted {
        h.update(b"\0");
        h.update(r.as_bytes());
    }
    h.finalize().to_hex()[..16].to_owned()
}

/// A finding; its references sorted.
pub fn finding(
    code: Code,
    severity: Severity,
    board: &str,
    mut refs: Vec<String>,
    message: String,
    details: Value,
) -> Finding {
    refs.sort();
    refs.dedup();
    Finding {
        id: finding_id(code, board, &refs),
        code,
        severity,
        board: board.to_owned(),
        refs,
        message,
        details,
    }
}

/// By code, then references.
pub fn sort(f: &mut [Finding]) {
    f.sort_by(|a, b| {
        (json!(a.code).as_str().unwrap_or_default(), &a.refs)
            .cmp(&(json!(b.code).as_str().unwrap_or_default(), &b.refs))
    });
}

/// The board's notices (5.A) as findings, and the placements: every
/// occurrence with a footprint and no placement, every placement whose
/// occurrence is gone.
pub fn placements(board: &str, b: &BoardFiles, occs: &[Occ], s: &BoardShapes) -> Vec<Finding> {
    let mut out = Vec::new();
    for n in &s.notices {
        let occurrence = n.error.details["occurrence"].as_str().unwrap_or_default();
        out.push(finding(
            n.error.code,
            n.severity,
            board,
            vec![format!("{board}:{occurrence}")],
            n.error.message.clone(),
            n.error.details.clone(),
        ));
    }
    for o in occs {
        if o.footprint_ref.is_some() && !b.placements.contains_key(&o.reference) {
            out.push(finding(
                Code::BoardUnplaced,
                Severity::Error,
                board,
                vec![format!("{board}:{}", o.label())],
                format!(
                    "{} isn't placed; `ryueda board place {board} {} --at X,Y`",
                    o.label(),
                    o.reference
                ),
                json!({"occurrence": o.reference, "footprint": o.footprint_ref}),
            ));
        }
    }
    let known: BTreeSet<&str> = occs.iter().map(|o| o.reference.as_str()).collect();
    for occurrence in b.placements.keys() {
        if !known.contains(occurrence.as_str()) {
            out.push(finding(
                Code::BoardPlacementOrphan,
                Severity::Error,
                board,
                vec![format!("{board}:{occurrence}")],
                format!("the design has no occurrence {occurrence} any more; `ryueda board unplace {board} {occurrence}`"),
                json!({"occurrence": occurrence}),
            ));
        }
    }
    out
}

/// A point's reference.
fn point_refs(c: &Connectivity, keys: &[Key]) -> Vec<String> {
    keys.iter().map(|k| c.built.reference(k)).collect()
}

fn node_ref(c: &Connectivity, n: &ryu_copper::connect::Node<Key>) -> String {
    match n {
        ryu_copper::connect::Node::Piece(k, _) | ryu_copper::connect::Node::Hole(k) => {
            c.built.reference(k)
        }
    }
}

/// The compare's findings: opens (with the connections that would close
/// them), shorts, items claiming another net than their island's pads,
/// pads on no copper, and contacts the bounds leave open. `incomplete`
/// when a placement's pads were left out.
pub fn copper(c: &Connectivity, incomplete: bool) -> Vec<Finding> {
    let board = c.built.board.as_str();
    let mut out = Vec::new();
    for o in &c.report.opens {
        let refs: Vec<String> = o
            .islands
            .iter()
            .flat_map(|(_, ks)| point_refs(c, ks))
            .collect();
        let next: Vec<Value> = connect::connections(c, o)
            .iter()
            .map(|x| {
                json!({
                    "a": c.built.reference(&x.a.0),
                    "a_layer": c.built.layer_name(c.layer_of(&x.a.0)),
                    "a_at": [x.distance.a.x, x.distance.a.y],
                    "b": c.built.reference(&x.b.0),
                    "b_layer": c.built.layer_name(c.layer_of(&x.b.0)),
                    "b_at": [x.distance.b.x, x.distance.b.y],
                    "distance_nm": x.distance.nm,
                })
            })
            .collect();
        out.push(finding(
            Code::NetOpen,
            Severity::Error,
            board,
            refs,
            format!("net {} is open: its pads lie on {} islands", o.net, o.islands.len()),
            json!({
                "net": o.net,
                "islands": o.islands.iter().map(|(_, ks)| json!({"pads": point_refs(c, ks)})).collect::<Vec<_>>(),
                "connections": next,
                "incomplete": incomplete,
            }),
        ));
    }
    for s in &c.report.shorts {
        let refs: Vec<String> = s
            .nets
            .iter()
            .flat_map(|(_, ks)| point_refs(c, ks))
            .collect();
        let contacts: Vec<[String; 2]> = s
            .contacts
            .iter()
            .map(|(a, b)| {
                let mut p = [c.built.reference(a), c.built.reference(b)];
                p.sort();
                p
            })
            .collect();
        out.push(finding(
            Code::NetShort,
            Severity::Error,
            board,
            refs,
            format!(
                "nets {} are joined",
                s.nets.iter().map(|(n, _)| n.as_str()).collect::<Vec<_>>().join(", ")
            ),
            json!({
                "nets": s.nets.iter().map(|(n, ks)| json!({"net": n, "pads": point_refs(c, ks)})).collect::<Vec<_>>(),
                "contacts": contacts,
                "holes": point_refs(c, &s.holes),
                "incomplete": incomplete,
            }),
        ));
    }
    for m in &c.report.mismatches {
        let refs: Vec<String> = m.items.iter().map(|(k, _)| c.built.reference(k)).collect();
        out.push(finding(
            Code::NetClaimMismatch,
            Severity::Error,
            board,
            refs,
            format!("copper on net {}'s pads claims another net; change its --net", m.net),
            json!({
                "net": m.net,
                "items": m.items.iter().map(|(k, n)| json!({"item": c.built.reference(k), "claims": n})).collect::<Vec<_>>(),
            }),
        ));
    }
    for o in &c.report.off_copper {
        out.push(finding(
            Code::NetlistPointOffCopper,
            Severity::Warning,
            board,
            vec![c.built.reference(&o.key)],
            format!(
                "a pad of net {} lies on {}",
                o.net,
                if o.uncertain {
                    "copper the bounds can't place"
                } else {
                    "no copper"
                }
            ),
            json!({"net": o.net, "at": [o.at.x, o.at.y], "uncertain": o.uncertain}),
        ));
    }
    for u in c.copper.uncertain() {
        out.push(finding(
            Code::CopperContactUncertain,
            Severity::Warning,
            board,
            vec![node_ref(c, &u.a), node_ref(c, &u.b)],
            format!(
                "{} and {} lie within {} nm of touching: kept apart",
                node_ref(c, &u.a),
                node_ref(c, &u.b),
                u.bound
            ),
            json!({"layer": c.built.layer_name(u.layer), "bound_nm": u.bound}),
        ));
    }
    out
}

/// Each island's nets: its pads' and what its items claim.
fn island_nets(c: &Connectivity) -> BTreeMap<IslandId<Key>, BTreeSet<String>> {
    let mut out: BTreeMap<IslandId<Key>, BTreeSet<String>> = BTreeMap::new();
    for (p, at) in c.built.layout.points.iter().zip(&c.placed) {
        if let Placement::On(id) = at {
            out.entry(id.clone()).or_default().insert(p.net.clone());
        }
    }
    for i in c.copper.islands() {
        out.entry(i.id.clone())
            .or_default()
            .extend(i.claims.iter().cloned());
    }
    out
}

/// The closest gap between two items on a layer, and their references.
type Closest = (ryu_copper::connect::Gap<Key>, String, String);

/// Copper of two islands closer than the rule on a layer, once per pair
/// of items (their closest pieces); not two islands of one net alone.
pub fn clearance(c: &Connectivity, preset: &Preset) -> Vec<Finding> {
    let board = c.built.board.as_str();
    let nets = island_nets(c);
    let one_net = |a: &IslandId<Key>, b: &IslandId<Key>| match (nets.get(a), nets.get(b)) {
        (Some(x), Some(y)) => x.len() == 1 && x == y,
        _ => false,
    };
    let mut best: BTreeMap<(usize, String, String), Closest> = BTreeMap::new();
    for layer in 0..c.built.layout.layers.len() {
        for g in c.copper.clearance(layer, preset.clearance_nm) {
            if one_net(&g.islands.0, &g.islands.1) {
                continue;
            }
            let (ra, rb) = (c.built.reference(&g.a.0), c.built.reference(&g.b.0));
            let (ra, rb) = if ra <= rb { (ra, rb) } else { (rb, ra) };
            let key = (layer, ra.clone(), rb.clone());
            if best
                .get(&key)
                .is_none_or(|(x, ..)| g.distance.nm < x.distance.nm)
            {
                best.insert(key, (g, ra, rb));
            }
        }
    }
    best.into_values()
        .map(|(g, ra, rb)| {
            finding(
                Code::DrcClearance,
                Severity::Error,
                board,
                vec![ra.clone(), rb.clone()],
                format!(
                    "{ra} and {rb} are {} nm apart on {}, under the {} nm clearance",
                    g.distance.nm,
                    c.built.layer_name(g.layer),
                    preset.clearance_nm
                ),
                json!({
                    "layer": c.built.layer_name(g.layer),
                    "distance_nm": g.distance.nm,
                    "exact": g.distance.exact,
                    "bound_nm": g.distance.bound,
                    "uncertain": g.verdict == Verdict::Uncertain,
                    "a_at": [g.distance.a.x, g.distance.a.y],
                    "b_at": [g.distance.b.x, g.distance.b.y],
                    "rule_nm": preset.clearance_nm,
                }),
            )
        })
        .collect()
}

/// Whether a track's copper and a pad's copper share an island.
fn same_island(c: &Connectivity, a: &Key, b: &Key) -> bool {
    let ids = |k: &Key| -> BTreeSet<IslandId<Key>> {
        (0..c.copper.pieces(k).max(1))
            .filter_map(|i| c.copper.island_of(&Node::Piece(k.clone(), i)))
            .collect()
    };
    !ids(a).is_disjoint(&ids(b))
}

/// Tracks closer than the preset's mask rule to a mask opening on their
/// side whose pad is on another island.
pub fn mask_clearance(c: &Connectivity, s: &BoardShapes, preset: &Preset) -> Vec<Finding> {
    let board = c.built.board.as_str();
    let rule = preset.mask_trace_clearance_nm;
    let opts = BodyOptions::default();
    let mut out = Vec::new();
    for side in 0..2usize {
        let masks: Vec<(usize, Body)> = s
            .pads
            .iter()
            .enumerate()
            .filter_map(|(i, p)| Some((i, Body::of_shape(p.mask[side].as_ref()?, &opts))))
            .collect();
        if masks.is_empty() {
            continue;
        }
        let index = ryu_copper::index::Index::from_bodies(
            1,
            masks.iter().map(|(i, b)| (*i, 0, b.clone())).collect(),
            opts,
        );
        let found: Vec<Finding> = s
            .tracks
            .par_iter()
            .filter(|t| t.layer.index() == side)
            .flat_map_iter(|t| {
                let body = Body::of_shape(&t.shape, &opts);
                let tk = Key::Track(t.id.to_string());
                let mut f = Vec::new();
                for n in index.near_body(0, &body, rule) {
                    let pd = &s.pads[n.key];
                    let pk = Key::Pad {
                        occ: pd.occurrence.clone(),
                        pad: pd.pad.clone(),
                        layer: side as u8,
                    };
                    if same_island(c, &tk, &pk) {
                        continue;
                    }
                    let mbody = &masks.iter().find(|(i, _)| *i == n.key).unwrap().1;
                    let v = measure::closer(&body, mbody, rule);
                    if v == Verdict::No {
                        continue;
                    }
                    let d = measure::distance(&body, mbody);
                    f.push(finding(
                        Code::DrcMaskClearance,
                        Severity::Error,
                        board,
                        vec![c.built.reference(&tk), pd.reference.clone()],
                        format!(
                            "track {} is under {rule} nm from {}'s mask opening",
                            t.id, pd.reference
                        ),
                        json!({
                            "layer": c.built.layer_name(side),
                            "distance_nm": d.map(|d| d.nm),
                            "uncertain": v == Verdict::Uncertain,
                            "rule_nm": rule,
                        }),
                    ));
                }
                f
            })
            .collect();
        out.extend(found);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::board::file::CopperLayer;
    use crate::board::preset;
    use crate::board::testing::{MM, UM, board};

    fn two_and_one() -> BoardShapes {
        let mut s = board(20 * MM, 10 * MM);
        s.smd("r1", "1", (2 * MM, 5 * MM), (600 * UM, 600 * UM), Some("a"))
            .smd(
                "r2",
                "1",
                (10 * MM, 5 * MM),
                (600 * UM, 600 * UM),
                Some("a"),
            )
            .smd("r3", "1", (6 * MM, 8 * MM), (600 * UM, 600 * UM), Some("b"));
        s
    }

    fn codes(f: &[Finding]) -> Vec<Code> {
        f.iter().map(|x| x.code).collect()
    }

    #[test]
    fn ids_ignore_reference_order_but_not_code_or_board() {
        let a = finding_id(Code::DrcClearance, "b", &["x".into(), "y".into()]);
        assert_eq!(
            a,
            finding_id(Code::DrcClearance, "b", &["y".into(), "x".into()])
        );
        assert_ne!(
            a,
            finding_id(Code::DrcHoleSpacing, "b", &["x".into(), "y".into()])
        );
        assert_ne!(
            a,
            finding_id(Code::DrcClearance, "c", &["x".into(), "y".into()])
        );
        assert_eq!(a.len(), 16);
    }

    #[test]
    fn unplaced_orphaned_and_unsupported_placements_are_found() {
        let orphan =
            r#"{"at":[1000000,1000000],"format":1,"kind":"ryueda.placement","rotation_mdeg":0}"#;
        let (_d, mut p, id) = crate::board::shapes::tests::placed_with(&[(
            "boards/b/placements/zz9.json".into(),
            orphan.into(),
        )]);
        let first = p.boards[&id]
            .placements
            .keys()
            .find(|k| *k != "zz9")
            .cloned()
            .unwrap();
        p.boards.get_mut(&id).unwrap().placements.remove(&first);
        let design = p.boards[&id].board.item.as_ref().unwrap().design.clone();
        let (_, occs) = crate::board::occ::occurrences(&p, &design).unwrap();
        let s = crate::board::shapes::build(&p, &p.boards[&id]).unwrap();
        let f = placements("b", &p.boards[&id], &occs, &s);
        let codes: Vec<Code> = f.iter().map(|x| x.code).collect();
        assert_eq!(
            codes,
            [Code::BoardUnplaced, Code::BoardPlacementOrphan],
            "{f:?}"
        );
        assert_eq!(f[1].refs, ["b:zz9"]);
        assert!(f.iter().all(|x| x.severity == Severity::Error));
    }

    #[test]
    fn opens_shorts_and_mismatches_are_findings() {
        let s = two_and_one();
        let f = copper(&connect::connect("b", &s), false);
        assert_eq!(codes(&f), [Code::NetOpen]);
        assert_eq!(f[0].refs, ["b:r1/1", "b:r2/1"]);
        let next = &f[0].details["connections"][0];
        assert_eq!(next["distance_nm"], json!(8 * MM - 600 * UM));
        assert_eq!(next["a"], "b:r1/1");
        assert_eq!(next["a_layer"], "copper.top");

        let mut s = two_and_one();
        s.track(
            "t1",
            "a",
            CopperLayer::Top,
            250 * UM,
            &[(2 * MM, 5 * MM), (10 * MM, 5 * MM)],
        )
        .track(
            "t2",
            "a",
            CopperLayer::Top,
            250 * UM,
            &[(6 * MM, 5 * MM), (6 * MM, 8 * MM)],
        );
        let f = copper(&connect::connect("b", &s), true);
        assert_eq!(codes(&f), [Code::NetShort]);
        assert_eq!(f[0].details["incomplete"], true);
        assert!(
            f[0].details["contacts"]
                .as_array()
                .unwrap()
                .iter()
                .any(|c| c == &json!(["b:r3/1", "b:track/t2"])),
            "{f:?}"
        );

        let mut s = two_and_one();
        s.track(
            "t1",
            "b",
            CopperLayer::Top,
            250 * UM,
            &[(2 * MM, 5 * MM), (10 * MM, 5 * MM)],
        );
        let f = copper(&connect::connect("b", &s), false);
        assert_eq!(codes(&f), [Code::NetClaimMismatch]);
        assert_eq!(f[0].severity, Severity::Error);
        assert_eq!(f[0].refs, ["b:track/t1"]);
    }

    const P: &Preset = &preset::JLC_2L;

    /// Pads of nets a and b `gap` apart, edge to edge.
    fn apart(gap: i64, b_net: &str) -> BoardShapes {
        let mut s = board(20 * MM, 10 * MM);
        s.smd("r1", "1", (5 * MM, 5 * MM), (600 * UM, 600 * UM), Some("a"))
            .smd(
                "r2",
                "1",
                (5 * MM + 600 * UM + gap, 5 * MM),
                (600 * UM, 600 * UM),
                Some(b_net),
            );
        s
    }

    #[test]
    fn clearance_fires_a_nanometre_under_the_rule() {
        let at = |gap, net: &str| clearance(&connect::connect("b", &apart(gap, net)), P);
        assert!(at(P.clearance_nm, "b").is_empty());
        let f = at(P.clearance_nm - 1, "b");
        assert_eq!(codes(&f), [Code::DrcClearance]);
        assert_eq!(f[0].refs, ["b:r1/1", "b:r2/1"]);
        assert_eq!(f[0].details["distance_nm"], json!(P.clearance_nm - 1));
        assert_eq!(f[0].details["layer"], "copper.top");
        // Two islands of one net are its open, not a clearance finding.
        assert!(at(P.clearance_nm - 1, "a").is_empty());
    }

    #[test]
    fn mask_clearance_fires_a_nanometre_under_its_rule() {
        let g = P.clearance_nm + MM;
        let y = 5 * MM + 300 * UM + g + 125 * UM;
        let with_mask = |margin: i64| {
            let mut s = board(20 * MM, 20 * MM);
            s.smd("r1", "1", (5 * MM, 5 * MM), (600 * UM, 600 * UM), Some("a"))
                .track(
                    "t1",
                    "b",
                    CopperLayer::Top,
                    250 * UM,
                    &[(2 * MM, y), (8 * MM, y)],
                );
            s.pads[0].mask[0] = Some(crate::board::testing::rect(
                ryu_board::geom::Point::new(5 * MM, 5 * MM),
                600 * UM + 2 * margin,
                600 * UM + 2 * margin,
            ));
            mask_clearance(&connect::connect("b", &s), &s, P)
        };
        assert!(with_mask(g - P.mask_trace_clearance_nm).is_empty());
        let f = with_mask(g - P.mask_trace_clearance_nm + 1);
        assert_eq!(codes(&f), [Code::DrcMaskClearance]);
        assert_eq!(f[0].refs, ["b:r1/1", "b:track/t1"]);
        // A track on the pad's own island crosses its opening freely.
        let mut s = board(20 * MM, 20 * MM);
        s.smd("r1", "1", (5 * MM, 5 * MM), (600 * UM, 600 * UM), Some("a"))
            .track(
                "t1",
                "a",
                CopperLayer::Top,
                250 * UM,
                &[(5 * MM, 5 * MM), (8 * MM, 5 * MM)],
            );
        assert!(mask_clearance(&connect::connect("b", &s), &s, P).is_empty());
    }
}
