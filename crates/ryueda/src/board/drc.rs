//! DRC (design 732): a board's findings, ERC's shape, from its shapes,
//! its copper's connectivity against the design's netlist, and its
//! preset's rules. Every topology decision is `ryu-copper`'s exact
//! predicate; a rule a bound can't decide is reported, never dropped.

use super::connect::{self, Connectivity};
use super::layout::Key;
use super::load::BoardFiles;
use super::occ::{self, Occ};
use super::preset::{self, HoleKind, Preset};
use super::rules::Rules;
use super::shapes::BoardShapes;
use crate::error::{Code, Result};
use crate::fault::Severity;
use crate::project::Project;
use rayon::prelude::*;
use ryu_board::geom::{Edge, Nm, Pen, Stroke};
use ryu_board::model::Polarity;
use ryu_board::shape::{Part, Piece, Shape};
use ryu_copper::connect::{IslandId, Node, Placement};
use ryu_copper::layout::HoleShape;
use ryu_copper::measure::{self, Body, BodyOptions, Verdict};
use ryu_copper::poly::{self, FlattenOptions};
use serde::Serialize;
use serde_json::{Value, json};
use std::collections::{BTreeMap, BTreeSet};
use std::sync::Arc;

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
    /// The waiver that waives it (`rules.json`'s), when it holds.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub waived: Option<Value>,
}

impl Finding {
    /// An error not waived.
    pub fn is_error(&self) -> bool {
        self.severity == Severity::Error && self.waived.is_none()
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
        waived: None,
    }
}

/// One finding per ID, the IDs' order kept: a later finding with an ID
/// already seen is folded into the first as `details.also` (its message
/// and details), so an answer's IDs are unique and nothing is dropped.
pub fn unique(f: Vec<Finding>) -> Vec<Finding> {
    let mut out: Vec<Finding> = Vec::with_capacity(f.len());
    let mut at: BTreeMap<String, usize> = BTreeMap::new();
    for x in f {
        match at.get(&x.id) {
            Some(&i) => {
                let first = &mut out[i];
                if !first.details.is_object() {
                    first.details = json!({});
                }
                let also = first.details["also"].take();
                let mut list = match also {
                    Value::Array(l) => l,
                    _ => Vec::new(),
                };
                list.push(json!({"message": x.message, "details": x.details}));
                first.details["also"] = json!(list);
            }
            None => {
                at.insert(x.id.clone(), out.len());
                out.push(x);
            }
        }
    }
    out
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
    // Uncertain contacts, once per pair of references (a pad or via is
    // one reference on both layers), with the layers it is on.
    let mut uncertain: BTreeMap<(String, String), (BTreeSet<usize>, i64)> = BTreeMap::new();
    for u in c.copper.uncertain() {
        let (a, b) = (node_ref(c, &u.a), node_ref(c, &u.b));
        let key = if a <= b { (a, b) } else { (b, a) };
        let e = uncertain.entry(key).or_insert((BTreeSet::new(), u.bound));
        e.0.insert(u.layer);
        e.1 = e.1.max(u.bound);
    }
    for ((a, b), (layers, bound)) in uncertain {
        let names: Vec<&str> = layers.iter().map(|l| c.built.layer_name(*l)).collect();
        out.push(finding(
            Code::CopperContactUncertain,
            Severity::Warning,
            board,
            vec![a.clone(), b.clone()],
            format!("{a} and {b} lie within {bound} nm of touching: kept apart"),
            json!({"layer": names[0], "layers": names, "bound_nm": bound}),
        ));
    }
    out.extend(netless(c, incomplete));
    out.extend(pad_free_shorts(c, incomplete));
    out
}

/// The islands the netlist's points landed on.
fn pointed(c: &Connectivity) -> BTreeSet<IslandId<Key>> {
    c.placed
        .iter()
        .filter_map(|p| match p {
            Placement::On(id) => Some(id.clone()),
            _ => None,
        })
        .collect()
}

/// The references of the points on an island, and of the items on it
/// that claim a net.
fn carriers(c: &Connectivity, id: &IslandId<Key>) -> Vec<String> {
    let mut out: BTreeSet<String> = c
        .built
        .layout
        .points
        .iter()
        .zip(&c.placed)
        .filter(|(_, at)| matches!(at, Placement::On(i) if i == id))
        .map(|(p, _)| c.built.reference(&p.key))
        .collect();
    if let Some(island) = c.copper.island(&Node::Piece(id.0.clone(), id.1)) {
        for k in &island.items {
            if c.copper.item(k).is_some_and(|it| it.net.is_some()) {
                out.insert(c.built.reference(k));
            }
        }
    }
    out.into_iter().collect()
}

/// Copper of a pad with no net joined to a net. Such a pad has no netlist
/// point, so the compare never sees it: every island holding one and
/// carrying a net (from points or claims) is a short, once per island.
fn netless(c: &Connectivity, incomplete: bool) -> Vec<Finding> {
    let board = c.built.board.as_str();
    let nets = island_nets(c);
    let mut found: BTreeMap<IslandId<Key>, BTreeSet<String>> = BTreeMap::new();
    for (k, it) in &c.built.layout.items {
        if !matches!(k, Key::Pad { .. }) || it.net.is_some() {
            continue;
        }
        for i in 0..c.copper.pieces(k) {
            if let Some(id) = c.copper.island_of(&Node::Piece(k.clone(), i))
                && nets.get(&id).is_some_and(|n| !n.is_empty())
            {
                found.entry(id).or_default().insert(c.built.reference(k));
            }
        }
    }
    found
        .into_iter()
        .map(|(id, pads)| {
            let on: Vec<&String> = nets[&id].iter().collect();
            let mut refs: Vec<String> = pads.iter().cloned().collect();
            refs.extend(carriers(c, &id));
            let pads: Vec<String> = pads.into_iter().collect();
            finding(
                Code::NetShort,
                Severity::Error,
                board,
                refs,
                format!(
                    "{} {} no net, but copper joins {} to net {}",
                    pads.join(", "),
                    if pads.len() == 1 { "has" } else { "have" },
                    if pads.len() == 1 { "it" } else { "them" },
                    on.iter().map(|n| n.as_str()).collect::<Vec<_>>().join(", ")
                ),
                json!({"nets": on, "netless": pads, "incomplete": incomplete}),
            )
        })
        .collect()
}

/// Islands no netlist point landed on whose items claim two nets or more:
/// the compare has no pad to judge them by, and clearance sees one
/// island, so they are shorts here, with the touching pairs of items.
fn pad_free_shorts(c: &Connectivity, incomplete: bool) -> Vec<Finding> {
    let board = c.built.board.as_str();
    let pointed = pointed(c);
    let net_of = |k: &Key| c.copper.item(k).and_then(|it| it.net.clone());
    let mut out = Vec::new();
    for island in c.copper.islands() {
        let claims: BTreeSet<&String> = island.claims.iter().collect();
        if pointed.contains(&island.id) || claims.len() < 2 {
            continue;
        }
        let mut contacts: BTreeSet<[String; 2]> = BTreeSet::new();
        for (k, i) in &island.pieces {
            let Some(net) = net_of(k) else { continue };
            for n in c.copper.contacts(&Node::Piece(k.clone(), *i)) {
                if let Node::Piece(other, _) = &n
                    && net_of(other).is_some_and(|m| m != net)
                {
                    let mut pair = [c.built.reference(k), c.built.reference(other)];
                    pair.sort();
                    contacts.insert(pair);
                }
            }
        }
        let refs = carriers(c, &island.id);
        let nets: Vec<&String> = claims.into_iter().collect();
        out.push(finding(
            Code::NetShort,
            Severity::Error,
            board,
            refs,
            format!(
                "nets {} are joined by copper on no pad",
                nets.iter()
                    .map(|n| n.as_str())
                    .collect::<Vec<_>>()
                    .join(", ")
            ),
            json!({
                "nets": nets,
                "contacts": contacts,
                "holes": island.holes.iter().map(|k| c.built.reference(k)).collect::<Vec<_>>(),
                "incomplete": incomplete,
            }),
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
type Closest = (
    ryu_copper::connect::Gap<Key>,
    String,
    String,
    super::rules::Resolved,
    Verdict,
);

/// Copper of two islands closer than the rule on a layer, once per pair
/// of items (their closest pieces); not two islands of one net alone.
pub fn clearance(c: &Connectivity, rules: &Rules) -> Vec<Finding> {
    let board = c.built.board.as_str();
    let nets = island_nets(c);
    let one_net = |a: &IslandId<Key>, b: &IslandId<Key>| match (nets.get(a), nets.get(b)) {
        (Some(x), Some(y)) => x.len() == 1 && x == y,
        _ => false,
    };
    // One finding per pair of references (a pad or via is one reference
    // on both layers): the closest gap, with every layer it is under on.
    let mut best: BTreeMap<(String, String), Closest> = BTreeMap::new();
    let mut layers: BTreeMap<(String, String), BTreeSet<usize>> = BTreeMap::new();
    let widest = rules.widest_clearance();
    let nets_of = |id: &IslandId<Key>| -> Vec<&str> {
        nets.get(id)
            .map(|s| s.iter().map(String::as_str).collect())
            .unwrap_or_default()
    };
    for layer in 0..c.built.layout.layers.len() {
        for g in c.copper.clearance(layer, widest) {
            if one_net(&g.islands.0, &g.islands.1) {
                continue;
            }
            // The pair's own clearance: the engine was asked the widest.
            let need = rules.pair_clearance(&nets_of(&g.islands.0), &nets_of(&g.islands.1));
            let verdict = if need.value == widest {
                g.verdict
            } else {
                under(&g.distance, need.value)
            };
            if verdict == Verdict::No {
                continue;
            }
            let (ra, rb) = (c.built.reference(&g.a.0), c.built.reference(&g.b.0));
            let (ra, rb) = if ra <= rb { (ra, rb) } else { (rb, ra) };
            let key = (ra.clone(), rb.clone());
            layers.entry(key.clone()).or_default().insert(layer);
            if best
                .get(&key)
                .is_none_or(|(x, ..)| g.distance.nm < x.distance.nm)
            {
                best.insert(key, (g, ra, rb, need, verdict));
            }
        }
    }
    best.into_iter()
        .map(|(key, (g, ra, rb, need, verdict))| {
            let on: Vec<&str> = layers[&key]
                .iter()
                .map(|l| c.built.layer_name(*l))
                .collect();
            let mut details = json!({
                "layer": c.built.layer_name(g.layer),
                "layers": on,
                "distance_nm": g.distance.nm,
                "exact": g.distance.exact,
                "bound_nm": g.distance.bound,
                "uncertain": verdict == Verdict::Uncertain,
                "a_at": [g.distance.a.x, g.distance.a.y],
                "b_at": [g.distance.b.x, g.distance.b.y],
                "rule_nm": need.value,
            });
            if rules.has_clearance_rules() {
                details["rule"] = json!(need.source);
            }
            finding(
                Code::DrcClearance,
                Severity::Error,
                board,
                vec![ra.clone(), rb.clone()],
                format!(
                    "{ra} and {rb} are {} nm apart on {}, under the {} nm clearance",
                    g.distance.nm,
                    c.built.layer_name(g.layer),
                    need.value
                ),
                details,
            )
        })
        .collect()
}

/// Whether a measured distance is under `rule`: exact distances compare
/// directly, bounded ones by their bound either way, else uncertain.
fn under(d: &ryu_copper::measure::Distance, rule: Nm) -> Verdict {
    if d.exact {
        if d.nm < rule {
            Verdict::Yes
        } else {
            Verdict::No
        }
    } else if d.nm + d.bound < rule {
        Verdict::Yes
    } else if d.nm - d.bound >= rule {
        Verdict::No
    } else {
        Verdict::Uncertain
    }
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
    let n = s.n();
    // The mask's two sides over the outer copper layers.
    for (side, layer) in [(0usize, 0usize), (1, n - 1)] {
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
            .filter(|t| t.layer.index(n) == Some(layer))
            .flat_map_iter(|t| {
                let body = Body::of_shape(&t.shape, &opts);
                let tk = Key::Track(t.id.to_string());
                let mut f = Vec::new();
                for n in index.near_body(0, &body, rule) {
                    let pd = &s.pads[n.key];
                    let pk = Key::Pad {
                        occ: pd.occurrence.clone(),
                        pad: pd.pad.clone(),
                        layer: layer as u8,
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
                            "layer": c.built.layer_name(layer),
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

/// The bound a pad's outline is flattened at when it isn't all regions.
const FLAT: Nm = 10;

/// Tracks narrower than the rule.
pub fn widths(s: &BoardShapes, board: &str, rules: &Rules) -> Vec<Finding> {
    s.tracks
        .iter()
        .filter_map(|t| {
            let rule = rules.track_width_min(Some(&t.net));
            if t.width_nm >= rule.value {
                return None;
            }
            let mut details = json!({"width_nm": t.width_nm, "rule_nm": rule.value});
            if rules.has_width_rules() {
                details["rule"] = json!(rule.source);
            }
            Some(finding(
                Code::DrcTrackWidth,
                Severity::Error,
                board,
                vec![format!("{board}:track/{}", t.id)],
                format!(
                    "track {} is {} nm wide, under the {} nm minimum",
                    t.id, t.width_nm, rule.value
                ),
                details,
            ))
        })
        .collect()
}

fn drill_of(h: &HoleShape) -> Nm {
    match *h {
        HoleShape::Round { d, .. } | HoleShape::Slot { d, .. } => d,
    }
}

/// Drills outside the range, and vias under the least diameter.
pub fn drills(s: &BoardShapes, board: &str, preset: &Preset) -> Vec<Finding> {
    let mut out = Vec::new();
    for h in &s.holes {
        if matches!(h.of, super::shapes::HoleOf::Via(_)) {
            continue;
        }
        let d = drill_of(&h.shape);
        // Plated holes (vias' and through pads') and unplated ones have
        // their own least drills.
        let min = if h.plated {
            preset.drill_min_nm
        } else {
            preset.npth_drill_min_nm
        };
        if d < min || d > preset.drill_max_nm {
            out.push(finding(
                Code::DrcDrill,
                Severity::Error,
                board,
                vec![h.owner.clone()],
                format!("{}'s drill is {d} nm, outside {min}-{} nm", h.owner, preset.drill_max_nm),
                json!({"drill_nm": d, "plated": h.plated, "min_nm": min, "max_nm": preset.drill_max_nm}),
            ));
        }
    }
    // A via's drill and diameter: one finding saying what is wrong.
    for v in &s.vias {
        let (min, max) = (preset.drill_min_nm, preset.drill_max_nm);
        let mut wrong = Vec::new();
        if v.drill_nm < min || v.drill_nm > max {
            wrong.push(format!(
                "its drill is {} nm, outside {min}-{max} nm",
                v.drill_nm
            ));
        }
        if v.diameter_nm < preset.via_diameter_min_nm {
            wrong.push(format!(
                "its diameter is {} nm, under the {} nm minimum",
                v.diameter_nm, preset.via_diameter_min_nm
            ));
        }
        if wrong.is_empty() {
            continue;
        }
        out.push(finding(
            Code::DrcDrill,
            Severity::Error,
            board,
            vec![format!("{board}:via/{}", v.id)],
            format!("via {}: {}", v.id, wrong.join("; ")),
            json!({
                "drill_nm": v.drill_nm,
                "plated": true,
                "min_nm": min,
                "max_nm": max,
                "diameter_nm": v.diameter_nm,
                "diameter_min_nm": preset.via_diameter_min_nm,
            }),
        ));
    }
    out
}

/// A shape's outline as zero-width strokes, and the bound it carries:
/// exact when every part is a positive region, else its copper merged and
/// flattened at [`FLAT`].
fn outline_of(s: &Shape) -> (Shape, Nm) {
    let zero = |path: Vec<Edge>| Part {
        polarity: Polarity::Positive,
        piece: Piece::Stroke(Stroke {
            path,
            pen: Pen::Round(0),
        }),
    };
    let mut out = Shape::default();
    if s.parts
        .iter()
        .all(|p| p.polarity == Polarity::Positive && matches!(p.piece, Piece::Region(_)))
    {
        for p in &s.parts {
            if let Piece::Region(r) = &p.piece {
                for c in std::iter::once(&r.outer).chain(&r.holes) {
                    out.parts.push(zero(c.edges().to_vec()));
                }
            }
        }
        return (out, 0);
    }
    let merged = poly::merge_layer(&poly::flatten_shape(s, &FlattenOptions::new(FLAT)), &[]);
    for ring in &merged.rings().rings {
        let n = ring.len();
        let edges: Vec<Edge> = (0..n)
            .map(|i| Edge::line(ring[i], ring[(i + 1) % n]))
            .filter(|e| e.start() != e.end())
            .collect();
        if !edges.is_empty() {
            out.parts.push(zero(edges));
        }
    }
    (out, FLAT)
}

fn ring_finding(
    board: &str,
    owner: String,
    ring: Option<Nm>,
    inside: bool,
    uncertain: bool,
    min: Nm,
) -> Finding {
    finding(
        Code::DrcAnnularRing,
        Severity::Error,
        board,
        vec![owner.clone()],
        if inside {
            format!("{owner}'s ring is under the {min} nm minimum")
        } else {
            format!("{owner}'s hole isn't inside its copper")
        },
        json!({"ring_nm": ring, "inside": inside, "uncertain": uncertain, "rule_nm": min}),
    )
}

/// Vias' rings, exactly from their numbers, against the via rule;
/// through pads' from the hole to the pad's outline, against the PTH rule.
pub fn rings(c: &Connectivity, s: &BoardShapes, preset: &Preset) -> Vec<Finding> {
    let board = c.built.board.as_str();
    let min = preset.annular_ring_min_nm;
    let via_min = preset.via_annular_ring_min_nm;
    let opts = BodyOptions::default();
    let mut out = Vec::new();
    for v in &s.vias {
        // (diameter - drill) / 2 < min, without halving.
        if v.diameter_nm - v.drill_nm < 2 * via_min {
            out.push(ring_finding(
                board,
                format!("{board}:via/{}", v.id),
                Some((v.diameter_nm - v.drill_nm).div_euclid(2)),
                v.diameter_nm > v.drill_nm,
                false,
                via_min,
            ));
        }
    }
    let pads: BTreeMap<(&str, &str), &super::shapes::PadInfo> = s
        .pads
        .iter()
        .map(|p| ((p.occurrence.as_str(), p.pad.as_str()), p))
        .collect();
    let found: Vec<Finding> = s
        .holes
        .par_iter()
        .filter(|h| h.plated)
        .filter_map(|h| {
            let super::shapes::HoleOf::Pad { occ, pad } = &h.of else {
                return None;
            };
            let pd = pads.get(&(occ.as_str(), pad.as_str()))?;
            let hole = h.shape.body();
            let copper = Body::of_shape(&pd.copper, &opts);
            let (outline, flat) = outline_of(&pd.copper);
            let edge = Body::of_shape(&outline, &opts);
            let probe = match h.shape {
                HoleShape::Round { at, .. } => at,
                HoleShape::Slot { a, .. } => a,
            };
            let inside = measure::contains(&copper, probe).is_yes()
                && measure::closer(&hole, &edge, 1) == Verdict::No;
            let v = measure::closer(&hole, &edge, min + flat);
            if inside && v == Verdict::No {
                return None;
            }
            let ring = inside
                .then(|| measure::distance(&hole, &edge).map(|d| d.nm))
                .flatten();
            Some(ring_finding(
                board,
                pd.reference.clone(),
                ring,
                inside,
                v == Verdict::Uncertain || flat > 0,
                min,
            ))
        })
        .collect();
    out.extend(found);
    out
}

/// Two holes' walls closer than the rule: the via rule when both are
/// vias', the general one otherwise.
pub fn hole_spacing(s: &BoardShapes, board: &str, preset: &Preset) -> Vec<Finding> {
    let wide = preset.hole_spacing_nm.max(preset.via_hole_spacing_nm);
    let is_via = |h: &super::shapes::HoleInfo| matches!(h.of, super::shapes::HoleOf::Via(_));
    let opts = BodyOptions::default();
    let index = ryu_copper::index::Index::from_bodies(
        1,
        s.holes
            .iter()
            .enumerate()
            .map(|(i, h)| (i, 0, h.shape.body()))
            .collect(),
        opts,
    );
    index
        .pairs(0, wide)
        .into_par_iter()
        .filter_map(|p| {
            let (a, b) = (&s.holes[p.a], &s.holes[p.b]);
            let rule = if is_via(a) && is_via(b) {
                preset.via_hole_spacing_nm
            } else {
                preset.hole_spacing_nm
            };
            let (ba, bb) = (a.shape.body(), b.shape.body());
            let v = measure::closer(&ba, &bb, rule);
            if v == Verdict::No {
                return None;
            }
            let d = measure::distance(&ba, &bb).map(|d| d.nm);
            Some(finding(
                Code::DrcHoleSpacing,
                Severity::Error,
                board,
                vec![a.owner.clone(), b.owner.clone()],
                format!(
                    "{} and {}'s holes are under {rule} nm apart",
                    a.owner, b.owner
                ),
                json!({"distance_nm": d, "uncertain": v == Verdict::Uncertain, "rule_nm": rule}),
            ))
        })
        .collect()
}

/// Copper and holes closer to the outline or a cutout than the rule, or
/// outside the board: once per item (pads and vias on both layers once).
pub fn edges(c: &Connectivity, s: &BoardShapes, rules: &Rules) -> Vec<Finding> {
    let board = c.built.board.as_str();
    let opts = BodyOptions::default();
    let edge = Body::of_shape(&s.edges, &opts);
    let inside = Body::of_shape(&s.outline, &opts);
    if edge.is_empty() {
        return Vec::new();
    }
    // Each thing with its rule: its net's edge clearance (a hole's is the
    // board's).
    let mut things: BTreeMap<String, (Body, Nm)> = BTreeMap::new();
    for (k, it) in c.copper.items() {
        if let Some(b) = c.copper.bodies().body(k) {
            let rule = rules.edge_clearance(it.net.as_deref()).value;
            things
                .entry(c.built.reference(k))
                .or_insert_with(|| (b.clone(), rule));
        }
    }
    let hole_rule = rules.edge_clearance(None).value;
    for h in &s.holes {
        // A through pad's drill shares its pad's reference: `#hole` keeps
        // the two findings' IDs apart.
        things
            .entry(format!("{}#hole", h.owner))
            .or_insert_with(|| (h.shape.body(), hole_rule));
    }
    things
        .par_iter()
        .filter_map(|(r, (b, rule))| {
            let rule = *rule;
            let v = measure::closer(b, &edge, rule);
            let outside = v == Verdict::No
                && !b.solids.first().is_some_and(|x| measure::contains(&inside, x.probe()).is_yes());
            if v == Verdict::No && !outside {
                return None;
            }
            let reference = r.clone();
            let d = measure::distance(b, &edge).map(|d| d.nm);
            Some(finding(
                Code::DrcEdgeClearance,
                Severity::Error,
                board,
                vec![reference.clone()],
                if outside {
                    format!("{reference} is outside the board")
                } else {
                    format!("{reference} is under {rule} nm from the board's edge")
                },
                json!({"distance_nm": d, "outside": outside, "hole": r.ends_with("#hole"), "uncertain": v == Verdict::Uncertain, "rule_nm": rule}),
            ))
        })
        .collect()
}

/// Courtyards touching, and courtyards judged by their box.
pub fn courtyards(s: &BoardShapes, board: &str, rules: &Rules) -> Vec<Finding> {
    let gap = rules.courtyard_clearance().value;
    let opts = BodyOptions::default();
    let mut out: Vec<Finding> = s
        .courtyards
        .iter()
        .filter(|c| c.open)
        .map(|c| {
            finding(
                Code::DrcCourtyardOpen,
                Severity::Warning,
                board,
                vec![c.owner.clone()],
                format!("{}'s courtyard doesn't close; judged by its box", c.owner),
                Value::Null,
            )
        })
        .collect();
    let bodies: Vec<Body> = s
        .courtyards
        .iter()
        .map(|c| Body::of_shape(&c.shape, &opts))
        .collect();
    let index = ryu_copper::index::Index::from_bodies(
        1,
        bodies
            .iter()
            .enumerate()
            .map(|(i, b)| (i, 0, b.clone()))
            .collect(),
        opts,
    );
    let found: Vec<Finding> = index
        .pairs(0, gap)
        .into_par_iter()
        .filter_map(|p| {
            // Courtyards only meet on one side.
            if s.courtyards[p.a].bottom != s.courtyards[p.b].bottom {
                return None;
            }
            let v = if gap == 0 {
                measure::touches(&bodies[p.a], &bodies[p.b])
            } else {
                measure::closer(&bodies[p.a], &bodies[p.b], gap)
            };
            if v == Verdict::No {
                return None;
            }
            let (a, b) = (&s.courtyards[p.a], &s.courtyards[p.b]);
            Some(finding(
                Code::DrcCourtyardOverlap,
                Severity::Error,
                board,
                vec![a.owner.clone(), b.owner.clone()],
                if gap == 0 {
                    format!("{} and {}'s courtyards touch", a.owner, b.owner)
                } else {
                    format!(
                        "{} and {}'s courtyards are closer than {gap} nm",
                        a.owner, b.owner
                    )
                },
                if gap == 0 {
                    json!({"uncertain": v == Verdict::Uncertain})
                } else {
                    json!({"uncertain": v == Verdict::Uncertain, "rule_nm": gap})
                },
            ))
        })
        .collect();
    out.extend(found);
    out
}

/// A board checked: its shapes, its copper connected and compared, its
/// findings sorted, and whether a placement's pads were left out.
pub struct BoardCheck {
    pub shapes: Arc<BoardShapes>,
    pub conn: Connectivity,
    pub preset: Arc<Preset>,
    /// The board's rules, resolved (design 924).
    pub rules: Rules,
    pub findings: Vec<Finding>,
    pub incomplete: bool,
}

impl BoardCheck {
    pub fn errors(&self) -> usize {
        self.findings.iter().filter(|f| f.is_error()).count()
    }

    pub fn warnings(&self) -> usize {
        self.findings
            .iter()
            .filter(|f| f.severity == Severity::Warning)
            .count()
    }

    /// Errors a waiver holds.
    pub fn waived(&self) -> usize {
        self.findings.iter().filter(|f| f.waived.is_some()).count()
    }
}

/// Copper closer to a hole's wall than the preset's rule for that kind of
/// hole (a via's, a plated pad's, an unplated hole's), on a layer where
/// the hole doesn't join it: once per hole and item, the closest layer
/// named and every layer listed. A hole's own pad or via is never a
/// finding; nor, for a plated hole, is copper of the hole's own single
/// net (two islands of one net: its `net_open`).
pub fn hole_clearance(c: &Connectivity, s: &BoardShapes, rules: &Rules) -> Vec<Finding> {
    let board = c.built.board.as_str();
    let preset: &Preset = &rules.preset;
    let plated_pad = |occ: &str, pad: &str| {
        s.holes.iter().any(|h| {
            h.plated
                && matches!(&h.of, super::shapes::HoleOf::Pad { occ: o, pad: p } if o == occ && p == pad)
        })
    };
    let n = c.built.layout.layers.len();
    // The rule of a hole on a layer: its kind's, inner or outer.
    let rule_of = |k: &Key, layer: usize| {
        let inner = layer != 0 && layer + 1 != n;
        let kind = match k {
            Key::ViaDrill(_) => HoleKind::Via,
            Key::PadDrill { occ, pad } if plated_pad(occ, pad) => HoleKind::Pth,
            _ => HoleKind::Npth,
        };
        preset.hole_clearance(kind, inner)
    };
    let own = |hole: &Key, item: &Key| match (hole, item) {
        (Key::ViaDrill(a), Key::Via(b, _)) => a == b,
        (Key::PadDrill { occ: a, pad: b }, Key::Pad { occ: x, pad: y, .. }) => a == x && b == y,
        _ => false,
    };
    let nets = island_nets(c);
    type Best = (
        ryu_copper::connect::HoleGap<Key>,
        super::rules::Resolved,
        Verdict,
    );
    let mut best: BTreeMap<(String, String), Best> = BTreeMap::new();
    let mut layers: BTreeMap<(String, String), BTreeSet<usize>> = BTreeMap::new();
    for layer in 0..n {
        let inner = layer != 0 && layer + 1 != n;
        let mut floors: Vec<Nm> = [HoleKind::Via, HoleKind::Pth, HoleKind::Npth]
            .into_iter()
            .map(|k| preset.hole_clearance(k, inner))
            .collect();
        floors.sort_unstable();
        floors.dedup();
        // With hole rules of the board's, the engine is asked the widest
        // and each gap judged at its own need; without, each kind's rule at
        // its own distance, as 7.A.
        let ruled = rules.has_hole_rules();
        let widest = if ruled {
            let board_max = std::iter::once(&rules.file.defaults)
                .chain(rules.file.classes.values())
                .filter_map(|x| x.hole_clearance_nm)
                .max()
                .unwrap_or(0);
            vec![floors.iter().copied().max().unwrap_or(0).max(board_max)]
        } else {
            floors.clone()
        };
        let net_rule = |id: &IslandId<Key>| -> Option<super::rules::Resolved> {
            nets.get(id)?
                .iter()
                .filter_map(|n| rules.hole_clearance(Some(n)))
                .max_by_key(|r| r.value)
        };
        for &r in &widest {
            for g in c.copper.hole_clearance(layer, r) {
                if own(&g.hole, &g.piece.0) {
                    continue;
                }
                let floor = rule_of(&g.hole, layer);
                if !ruled && floor != r {
                    continue;
                }
                let mut need = super::rules::Resolved {
                    value: floor,
                    source: format!("preset:{}", preset.name),
                };
                if ruled {
                    for x in [Some(&g.island), g.joined.as_ref()].into_iter().flatten() {
                        if let Some(n) = net_rule(x)
                            && n.value > need.value
                        {
                            need = n;
                        }
                    }
                }
                let verdict = if need.value == r {
                    g.verdict
                } else {
                    under(&g.distance, need.value)
                };
                if verdict == Verdict::No {
                    continue;
                }
                if let Some(j) = &g.joined
                    && let (Some(x), Some(y)) = (nets.get(j), nets.get(&g.island))
                    && x.len() == 1
                    && x == y
                {
                    continue;
                }
                let key = (
                    format!("{}#hole", c.built.reference(&g.hole)),
                    c.built.reference(&g.piece.0),
                );
                layers.entry(key.clone()).or_default().insert(layer);
                if best
                    .get(&key)
                    .is_none_or(|(x, _, _)| g.distance.nm < x.distance.nm)
                {
                    best.insert(key, (g, need, verdict));
                }
            }
        }
    }
    best.into_iter()
        .map(|((hole, item), (g, need, verdict))| {
            let rule = need.value;
            let on: Vec<&str> = layers[&(hole.clone(), item.clone())]
                .iter()
                .map(|l| c.built.layer_name(*l))
                .collect();
            let owner = hole.trim_end_matches("#hole");
            let plated = s
                .holes
                .iter()
                .find(|h| h.owner == owner)
                .is_some_and(|h| h.plated);
            finding(
                Code::DrcHoleClearance,
                Severity::Error,
                board,
                vec![hole.clone(), item.clone()],
                format!(
                    "{item} is {} nm from {owner}'s hole wall on {}, under the {rule} nm hole clearance",
                    g.distance.nm,
                    c.built.layer_name(g.layer)
                ),
                {
                    let mut d = json!({
                        "layer": c.built.layer_name(g.layer),
                        "layers": on,
                        "distance_nm": g.distance.nm,
                        "exact": g.distance.exact,
                        "bound_nm": g.distance.bound,
                        "uncertain": verdict == Verdict::Uncertain,
                        "plated": plated,
                        "rule_nm": rule,
                    });
                    if rules.has_hole_rules() {
                        d["rule"] = json!(need.source);
                    }
                    d
                },
            )
        })
        .collect()
}

/// Blind and buried vias where the preset makes only through vias.
pub fn via_spans(s: &BoardShapes, board: &str, preset: &Preset) -> Vec<Finding> {
    if preset.blind_vias {
        return Vec::new();
    }
    let n = s.n();
    let name = |l: usize| super::file::CopperLayer::at(l, n).short();
    s.vias
        .iter()
        .filter(|v| (v.from, v.to) != (0, n - 1))
        .map(|v| {
            finding(
                Code::DrcViaSpan,
                Severity::Error,
                board,
                vec![format!("{board}:via/{}", v.id)],
                format!(
                    "via {} runs from {} to {}; preset {} makes only through vias",
                    v.id,
                    name(v.from),
                    name(v.to),
                    preset.name
                ),
                json!({"from": name(v.from), "to": name(v.to)}),
            )
        })
        .collect()
}

/// What every rule reads.
struct Ctx<'a> {
    board: &'a str,
    files: &'a BoardFiles,
    occs: &'a [Occ<'a>],
    shapes: &'a BoardShapes,
    conn: &'a Connectivity,
    preset: &'a Preset,
    rules: &'a Rules,
    incomplete: bool,
}

type Rule = fn(&Ctx) -> Vec<Finding>;

const RULES: [Rule; 12] = [
    |c| placements(c.board, c.files, c.occs, c.shapes),
    |c| copper(c.conn, c.incomplete),
    |c| clearance(c.conn, c.rules),
    |c| mask_clearance(c.conn, c.shapes, c.preset),
    |c| widths(c.shapes, c.board, c.rules),
    |c| drills(c.shapes, c.board, c.preset),
    |c| via_spans(c.shapes, c.board, c.preset),
    |c| rings(c.conn, c.shapes, c.preset),
    |c| hole_spacing(c.shapes, c.board, c.preset),
    |c| hole_clearance(c.conn, c.shapes, c.rules),
    |c| edges(c.conn, c.shapes, c.rules),
    |c| courtyards(c.shapes, c.board, c.rules),
];

/// A board checked from shapes already built. A design that doesn't
/// flatten refuses with its faults.
pub fn check_shapes(p: &Project, b: &BoardFiles, shapes: Arc<BoardShapes>) -> Result<BoardCheck> {
    let file = b.board.item.as_ref().map_err(Clone::clone)?;
    let preset =
        preset::resolve(p, &file.preset).unwrap_or_else(|| Arc::new(preset::DEFAULT.clone()));
    let (netlist, occs) = occ::occurrences(p, &file.design)?;
    let rules = Rules::new(
        preset.clone(),
        b.rules.as_ref().and_then(|l| l.item.as_ref().ok()),
        &netlist,
    );
    let board = b.id.to_string();
    let conn = connect::connect(&board, &shapes);
    let incomplete = shapes
        .notices
        .iter()
        .any(|n| n.error.code == Code::BoardFootprintUnsupported);
    let ctx = Ctx {
        board: &board,
        files: b,
        occs: &occs,
        shapes: &shapes,
        conn: &conn,
        preset: &preset,
        rules: &rules,
        incomplete,
    };
    let mut findings: Vec<Finding> = RULES.par_iter().flat_map_iter(|r| r(&ctx)).collect();
    sort(&mut findings);
    let mut findings = unique(findings);
    let unused = super::rules::apply_waivers(&mut findings, &rules.file.waivers, &board);
    findings.extend(unused);
    sort(&mut findings);
    Ok(BoardCheck {
        shapes: shapes.clone(),
        conn,
        preset,
        rules,
        findings,
        incomplete,
    })
}

/// A board checked from its files.
pub fn check(p: &Project, b: &BoardFiles) -> Result<BoardCheck> {
    check_shapes(p, b, Arc::new(super::shapes::build(p, b)?))
}

/// The board's findings now, if it checks (its design flattens).
pub fn now(session: &mut crate::session::Session, id: &crate::id::Id) -> Option<Vec<Finding>> {
    session.board_check(id).ok().map(|c| c.findings.clone())
}

/// What a write did to the board's findings: those it added, the IDs it
/// removed, and the counts left; `{error}` when the board doesn't check.
pub fn delta(
    session: &mut crate::session::Session,
    id: &crate::id::Id,
    before: Option<Vec<Finding>>,
) -> Value {
    let after = match session.board_check(id) {
        Ok(c) => c,
        Err(e) => return json!({"error": e}),
    };
    let was: BTreeSet<String> = before.iter().flatten().map(|f| f.id.clone()).collect();
    let is: BTreeSet<&str> = after.findings.iter().map(|f| f.id.as_str()).collect();
    let added: Vec<&Finding> = after
        .findings
        .iter()
        .filter(|f| !was.contains(&f.id))
        .collect();
    let removed: Vec<&String> = was.iter().filter(|i| !is.contains(i.as_str())).collect();
    json!({
        "added": added,
        "removed": removed,
        "errors": after.errors(),
        "warnings": after.warnings(),
    })
}

/// `check`'s `boards` section: each board's findings and counts, or why
/// it doesn't check (its design's faults are counted where they are).
pub fn section(p: &Project) -> Vec<Value> {
    p.boards
        .iter()
        .map(|(id, b)| match check(p, b) {
            Ok(c) => {
                let mut v = json!({
                    "board": format!("board/{id}"),
                    "revision": b.revision,
                    "findings": c.findings,
                    "errors": c.errors(),
                    "warnings": c.warnings(),
                });
                if c.waived() > 0 {
                    v["waived"] = json!(c.waived());
                }
                v
            }
            Err(e) => json!({"board": format!("board/{id}"), "error": e}),
        })
        .collect()
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
    fn a_blind_via_is_a_span_finding_where_the_preset_makes_none() {
        let mut s = board(20 * MM, 10 * MM);
        s.layers(4)
            .via("v0", "a", (5 * MM, 5 * MM), 300 * UM, 600 * UM)
            .blind("v1", "a", (10 * MM, 5 * MM), 300 * UM, 600 * UM, (0, 1))
            .blind("v2", "a", (15 * MM, 5 * MM), 300 * UM, 600 * UM, (1, 2));
        s.layers(4); // v0 through all four; the blind ones set their spans after
        s.vias[1].from = 0;
        s.vias[1].to = 1;
        s.holes[1].span = (0, 1);
        s.vias[2].from = 1;
        s.vias[2].to = 2;
        s.holes[2].span = (1, 2);
        let f = via_spans(&s, "b", &preset::JLC_2L);
        let refs: Vec<&str> = f.iter().map(|x| x.refs[0].as_str()).collect();
        assert_eq!(refs, ["b:via/v1", "b:via/v2"]);
        assert!(f.iter().all(|x| x.code == Code::DrcViaSpan && x.is_error()));
        assert_eq!(f[0].details["from"], "top");
        assert_eq!(f[0].details["to"], "in1");
        let open = Preset {
            blind_vias: true,
            ..preset::JLC_2L
        };
        assert!(via_spans(&s, "b", &open).is_empty());
    }

    #[test]
    fn mask_clearance_is_judged_on_the_outer_layers_of_four() {
        // A bottom track beside a through pad's bottom mask opening, on four
        // layers: the bottom is layer 3, not 1.
        let mut s = board(20 * MM, 10 * MM);
        s.through(
            "j1",
            "1",
            (10 * MM, 5 * MM),
            1700 * UM,
            1000 * UM,
            Some("a"),
        )
        .layers(4)
        .track(
            "t1",
            "b",
            CopperLayer::Bottom,
            250 * UM,
            &[
                (2 * MM, 5 * MM + 850 * UM + 125 * UM + 50 * UM),
                (18 * MM, 5 * MM + 850 * UM + 125 * UM + 50 * UM),
            ],
        )
        .track(
            "t2",
            "b",
            CopperLayer::Inner(1),
            250 * UM,
            &[(2 * MM, MM), (18 * MM, MM)],
        );
        let conn = connect::connect("b", &s);
        let f = mask_clearance(&conn, &s, &preset::JLC_2L);
        assert_eq!(f.len(), 1, "{f:?}");
        assert_eq!(f[0].details["layer"], "copper.bottom");
    }

    #[test]
    fn courtyards_on_opposite_sides_dont_overlap() {
        let mut s = board(20 * MM, 10 * MM);
        s.courtyard("u1", (5 * MM, 5 * MM), (4 * MM, 4 * MM), false)
            .courtyard("u2", (6 * MM, 5 * MM), (4 * MM, 4 * MM), false);
        assert_eq!(courtyards(&s, "b", &Rules::plain(P)).len(), 1);
        s.courtyards[1].bottom = true;
        assert!(courtyards(&s, "b", &Rules::plain(P)).is_empty());
        s.courtyards[0].bottom = true;
        assert_eq!(courtyards(&s, "b", &Rules::plain(P)).len(), 1);
    }

    #[test]
    fn copper_near_a_hole_it_isnt_joined_to() {
        let p = preset::JLC_2L;
        let run = |s: &BoardShapes| hole_clearance(&connect::connect("b", s), s, &Rules::plain(&p));
        // A track of another net passing a via's barrel on an inner layer,
        // a nanometre inside the rule and exactly at it.
        for (gap, found) in [
            (p.via_hole_clearance_nm - 1, 1),
            (p.via_hole_clearance_nm, 0),
        ] {
            let mut s = board(20 * MM, 10 * MM);
            s.layers(4);
            s.blind("v1", "a", (10 * MM, 5 * MM), 300 * UM, 600 * UM, (0, 1));
            // Layer in2 is outside the via's span: nothing there. Layer in1 is inside
            // it, but the via has its own disc there: keep the track clear of the disc
            // by using a via whose ring is narrower than the rule.
            let y = 5 * MM + 150 * UM + gap + 125 * UM;
            s.track(
                "t1",
                "b",
                CopperLayer::Inner(1),
                250 * UM,
                &[(2 * MM, y), (18 * MM, y)],
            );
            s.vias[0].diameter_nm = 320 * UM;
            s.vias[0].shape =
                crate::board::shapes::disc(ryu_board::geom::Point::new(10 * MM, 5 * MM), 320 * UM);
            let f = run(&s);
            assert_eq!(f.len(), found, "gap {gap}: {f:?}");
            if found == 1 {
                assert_eq!(f[0].code, Code::DrcHoleClearance);
                assert_eq!(f[0].refs, ["b:track/t1", "b:via/v1#hole"]);
                assert_eq!(f[0].details["layer"], "copper.in1");
                assert_eq!(f[0].details["distance_nm"], gap);
                assert_eq!(f[0].details["rule_nm"], p.via_hole_clearance_nm);
            }
        }
        // An unplated hole beside a top track: the NPTH rule.
        let mut s = board(20 * MM, 10 * MM);
        let y = 5 * MM + 500 * UM + p.npth_hole_clearance_nm - 1 + 125 * UM;
        s.hole("h1", (10 * MM, 5 * MM), 1000 * UM).track(
            "t1",
            "b",
            CopperLayer::Top,
            250 * UM,
            &[(2 * MM, y), (18 * MM, y)],
        );
        let f = run(&s);
        assert_eq!(f.len(), 1, "{f:?}");
        assert_eq!(f[0].details["rule_nm"], p.npth_hole_clearance_nm);
        // A through pad's own copper, and a track of its own net touching it, are not findings.
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
        );
        assert!(run(&s).is_empty(), "{:?}", run(&s));
    }

    #[test]
    fn inner_layers_take_the_inner_pth_hole_clearance() {
        // A through pad (1.1 mm, 1.0 mm drill) on four layers, and tracks of
        // another net 290 µm from its wall on in1 and on top.
        let at = (10 * MM, 5 * MM);
        let y = at.1 + 500 * UM + 290 * UM + 125 * UM;
        let mut s = board(20 * MM, 10 * MM);
        s.through("j1", "1", at, 1100 * UM, 1000 * UM, Some("a"))
            .layers(4);
        s.track(
            "ti",
            "b",
            CopperLayer::Inner(1),
            250 * UM,
            &[(2 * MM, y), (18 * MM, y)],
        )
        .track(
            "tt",
            "c",
            CopperLayer::Top,
            250 * UM,
            &[(2 * MM, y), (18 * MM, y)],
        );
        let conn = connect::connect("b", &s);
        let f = hole_clearance(&conn, &s, &Rules::plain(&preset::JLC_4L));
        assert_eq!(f.len(), 1, "{f:?}");
        assert_eq!(f[0].refs, ["b:j1/1#hole", "b:track/ti"]);
        assert_eq!(f[0].details["rule_nm"], 300 * UM);
        assert!(hole_clearance(&conn, &s, &Rules::plain(&preset::JLC_2L)).is_empty());
    }

    fn rules_with(f: super::super::rules::RulesFile, class_of: &[(&str, &str)]) -> Rules {
        Rules::from_parts(
            Arc::new(preset::JLC_2L),
            Some(f),
            class_of
                .iter()
                .map(|(n, c)| ((*n).to_owned(), crate::id::Id::parse(c, "class").unwrap()))
                .collect(),
        )
    }

    #[test]
    fn a_class_clearance_judges_its_pairs_and_no_others() {
        // Two pairs of tracks, each 150 µm apart edge to edge: a-b (b of
        // class power, 200 µm) and c-d (no class: the preset's 100 µm).
        let mut s = board(20 * MM, 10 * MM);
        let y = |k: i64| 2 * MM + k * (250 * UM + 150 * UM);
        s.track(
            "ta",
            "a",
            CopperLayer::Top,
            250 * UM,
            &[(2 * MM, y(0)), (18 * MM, y(0))],
        )
        .track(
            "tb",
            "b",
            CopperLayer::Top,
            250 * UM,
            &[(2 * MM, y(1)), (18 * MM, y(1))],
        )
        .track(
            "tc",
            "c",
            CopperLayer::Top,
            250 * UM,
            &[(2 * MM, y(4)), (18 * MM, y(4))],
        )
        .track(
            "td",
            "d",
            CopperLayer::Top,
            250 * UM,
            &[(2 * MM, y(5)), (18 * MM, y(5))],
        );
        let conn = connect::connect("b", &s);
        let mut f = super::super::rules::RulesFile::default();
        f.classes.insert(
            crate::id::Id::parse("power", "class").unwrap(),
            super::super::rules::RuleSet {
                clearance_nm: Some(200 * UM),
                ..Default::default()
            },
        );
        let r = rules_with(f, &[("b", "power")]);
        let got = clearance(&conn, &r);
        assert_eq!(got.len(), 1, "{got:?}");
        assert_eq!(got[0].refs, ["b:track/ta", "b:track/tb"]);
        assert_eq!(got[0].details["rule_nm"], 200 * UM);
        assert_eq!(got[0].details["rule"], "class:power");
        // Without rules: nothing (150 µm clears 100 µm), as stage 5.
        let plain = rules_with(super::super::rules::RulesFile::default(), &[]);
        assert!(clearance(&conn, &plain).is_empty());
    }

    #[test]
    fn a_board_width_rule_and_a_courtyard_clearance() {
        let mut s = board(20 * MM, 10 * MM);
        s.track(
            "t1",
            "a",
            CopperLayer::Top,
            150 * UM,
            &[(2 * MM, 2 * MM), (18 * MM, 2 * MM)],
        )
        .courtyard("u1", (5 * MM, 7 * MM), (2 * MM, 2 * MM), false)
        .courtyard("u2", (7 * MM + 100 * UM, 7 * MM), (2 * MM, 2 * MM), false);
        let mut f = super::super::rules::RulesFile::default();
        f.defaults.track_width_nm = Some(200 * UM);
        f.defaults.courtyard_clearance_nm = Some(150 * UM);
        let r = rules_with(f, &[]);
        let w = widths(&s, "b", &r);
        assert_eq!(w.len(), 1);
        assert_eq!(w[0].details["rule_nm"], 200 * UM);
        let c = courtyards(&s, "b", &r);
        assert_eq!(
            c.iter()
                .filter(|x| x.code == Code::DrcCourtyardOverlap)
                .count(),
            1,
            "{c:?}"
        );
        // 100 µm apart and no rule: no overlap, as today.
        assert!(
            courtyards(
                &s,
                "b",
                &rules_with(super::super::rules::RulesFile::default(), &[])
            )
            .is_empty()
        );
    }

    #[test]
    fn a_board_hole_clearance_raises_the_floor() {
        // A via of a and a track of b 250 µm from its wall on in1.
        let mut s = board(20 * MM, 10 * MM);
        s.layers(4);
        s.via("v1", "a", (10 * MM, 5 * MM), 300 * UM, 320 * UM);
        let y = 5 * MM + 150 * UM + 250 * UM + 125 * UM;
        s.track(
            "t1",
            "b",
            CopperLayer::Inner(1),
            250 * UM,
            &[(2 * MM, y), (18 * MM, y)],
        );
        let conn = connect::connect("b", &s);
        let mut f = super::super::rules::RulesFile::default();
        f.defaults.hole_clearance_nm = Some(300 * UM);
        let got = hole_clearance(&conn, &s, &rules_with(f, &[]));
        assert_eq!(got.len(), 1, "{got:?}");
        assert_eq!(got[0].details["rule_nm"], 300 * UM);
        assert_eq!(got[0].details["rule"], "board");
        assert!(hole_clearance(&conn, &s, &Rules::plain(&preset::JLC_2L)).is_empty());
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
        let at =
            |gap, net: &str| clearance(&connect::connect("b", &apart(gap, net)), &Rules::plain(P));
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

    #[test]
    fn widths_drills_rings_and_hole_spacing_fire_at_their_edges() {
        let w = |width| {
            let mut s = board(20 * MM, 10 * MM);
            s.track(
                "t1",
                "a",
                CopperLayer::Top,
                width,
                &[(2 * MM, 5 * MM), (8 * MM, 5 * MM)],
            );
            widths(&s, "b", &Rules::plain(P))
        };
        assert!(w(P.track_width_min_nm).is_empty());
        assert_eq!(codes(&w(P.track_width_min_nm - 1)), [Code::DrcTrackWidth]);

        let via = |drill, dia| {
            let mut s = board(20 * MM, 10 * MM);
            s.via("v1", "a", (10 * MM, 5 * MM), drill, dia);
            let c = connect::connect("b", &s);
            let mut f = drills(&s, "b", P);
            f.extend(rings(&c, &s, P));
            f
        };
        // A via's ring is judged by the via rule; a drill one over keeps
        // the diameter a nanometre short of the ring at or above its own
        // minimum.
        let ring = P.via_annular_ring_min_nm;
        let d = P.drill_min_nm.max(P.via_diameter_min_nm - 2 * ring + 1);
        assert!(
            via(d, d + 2 * ring).is_empty(),
            "{:?}",
            via(d, d + 2 * ring)
        );
        assert_eq!(codes(&via(d, d + 2 * ring - 1)), [Code::DrcAnnularRing]);
        // ryueda's default via and the page's smallest one pass.
        assert!(via(P.default_via_drill_nm, P.default_via_diameter_nm).is_empty());
        assert!(via(P.drill_min_nm, P.via_diameter_min_nm).is_empty());
        assert_eq!(
            codes(&via(
                P.drill_min_nm - 1,
                P.drill_min_nm - 1 + 4 * ring + P.via_diameter_min_nm
            )),
            [Code::DrcDrill]
        );
        let small = via(d, P.via_diameter_min_nm - 1);
        assert!(
            small.iter().any(|f| f.code == Code::DrcDrill
                && f.details["diameter_nm"] == json!(P.via_diameter_min_nm - 1)),
            "{small:?}"
        );

        // A square through pad: the ring is half the side less half the drill, exactly.
        let pad = |side| {
            let mut s = board(20 * MM, 10 * MM);
            s.through("j1", "1", (10 * MM, 5 * MM), side, 1000 * UM, None);
            rings(&connect::connect("b", &s), &s, P)
        };
        assert!(pad(1000 * UM + 2 * P.annular_ring_min_nm).is_empty());
        let f = pad(1000 * UM + 2 * P.annular_ring_min_nm - 2);
        assert_eq!(codes(&f), [Code::DrcAnnularRing]);
        assert_eq!(f[0].refs, ["b:j1/1"]);

        // An unplated hole's least drill is its own.
        let npth = |drill| {
            let mut s = board(20 * MM, 10 * MM);
            s.hole("h1", (10 * MM, 5 * MM), drill);
            drills(&s, "b", P)
        };
        assert!(npth(P.npth_drill_min_nm).is_empty());
        assert_eq!(codes(&npth(P.npth_drill_min_nm - 1)), [Code::DrcDrill]);

        let holes = |sep| {
            let mut s = board(20 * MM, 10 * MM);
            s.hole("h1", (5 * MM, 5 * MM), 1000 * UM)
                .hole("h2", (5 * MM + sep, 5 * MM), 1000 * UM);
            hole_spacing(&s, "b", P)
        };
        assert!(holes(1000 * UM + P.hole_spacing_nm).is_empty());
        let f = holes(1000 * UM + P.hole_spacing_nm - 1);
        assert_eq!(codes(&f), [Code::DrcHoleSpacing]);
        assert_eq!(f[0].details["distance_nm"], json!(P.hole_spacing_nm - 1));
        // Two vias' holes take the via rule.
        let vias = |sep| {
            let mut s = board(20 * MM, 10 * MM);
            s.via("v1", "a", (5 * MM, 5 * MM), 300 * UM, 600 * UM).via(
                "v2",
                "a",
                (5 * MM + sep, 5 * MM),
                300 * UM,
                600 * UM,
            );
            hole_spacing(&s, "b", P)
        };
        assert!(vias(300 * UM + P.via_hole_spacing_nm).is_empty());
        let f = vias(300 * UM + P.via_hole_spacing_nm - 1);
        assert_eq!(codes(&f), [Code::DrcHoleSpacing]);
        assert_eq!(f[0].details["rule_nm"], json!(P.via_hole_spacing_nm));
    }

    #[test]
    fn edge_clearance_and_copper_outside_the_board_fire() {
        let at = |x| {
            let mut s = board(20 * MM, 10 * MM);
            s.smd("r1", "1", (x, 5 * MM), (600 * UM, 600 * UM), None);
            edges(&connect::connect("b", &s), &s, &Rules::plain(P))
        };
        assert!(at(300 * UM + P.edge_clearance_nm).is_empty());
        let f = at(300 * UM + P.edge_clearance_nm - 1);
        assert_eq!(codes(&f), [Code::DrcEdgeClearance]);
        assert_eq!(f[0].details["distance_nm"], json!(P.edge_clearance_nm - 1));
        let out = at(-5 * MM);
        assert_eq!(codes(&out), [Code::DrcEdgeClearance]);
        assert_eq!(out[0].details["outside"], true);
        let mut s = board(20 * MM, 10 * MM);
        s.hole(
            "h1",
            (500 * UM + P.edge_clearance_nm - 1, 5 * MM),
            1000 * UM,
        );
        let f = edges(&connect::connect("b", &s), &s, &Rules::plain(P));
        assert_eq!(codes(&f), [Code::DrcEdgeClearance]);
        assert_eq!(f[0].refs, ["b:hole/h1#hole"]);
    }

    #[test]
    fn courtyards_touching_overlap_and_open_ones_warn() {
        let two = |gap| {
            let mut s = board(20 * MM, 10 * MM);
            s.courtyard("r1", (5 * MM, 5 * MM), (2 * MM, MM), false)
                .courtyard("r2", (7 * MM + gap, 5 * MM), (2 * MM, MM), false);
            courtyards(&s, "b", &Rules::plain(P))
        };
        assert!(two(1).is_empty());
        let f = two(0);
        assert_eq!(codes(&f), [Code::DrcCourtyardOverlap]);
        assert_eq!(f[0].refs, ["b:r1", "b:r2"]);
        let mut s = board(20 * MM, 10 * MM);
        s.courtyard("r1", (5 * MM, 5 * MM), (2 * MM, MM), true);
        let f = courtyards(&s, "b", &Rules::plain(P));
        assert_eq!(codes(&f), [Code::DrcCourtyardOpen]);
        assert_eq!(f[0].severity, Severity::Warning);
    }

    #[test]
    fn a_placed_board_checks_the_same_on_one_thread_and_eight() {
        let (_d, p, id) = crate::board::shapes::tests::placed();
        let run = |n| {
            rayon::ThreadPoolBuilder::new()
                .num_threads(n)
                .build()
                .unwrap()
                .install(|| {
                    let c = check(&p, &p.boards[&id]).unwrap();
                    serde_json::to_string(&c.findings).unwrap()
                })
        };
        let one = run(1);
        assert_eq!(one, run(8));
        let c = check(&p, &p.boards[&id]).unwrap();
        // Nothing is routed: every net with two pads or more is open.
        assert!(c.findings.iter().any(|f| f.code == Code::NetOpen), "{one}");
        assert!(c.findings.iter().all(|f| f.code != Code::BoardUnplaced));
        let mut sorted = c.findings.clone();
        sort(&mut sorted);
        assert_eq!(sorted, c.findings);
    }

    #[test]
    fn copper_joining_a_net_to_a_pad_with_no_net_is_reported() {
        let mut s = board(20 * MM, 10 * MM);
        s.smd("r1", "1", (2 * MM, 5 * MM), (600 * UM, 600 * UM), Some("a"))
            .smd("u1", "5", (10 * MM, 5 * MM), (600 * UM, 600 * UM), None)
            .track(
                "t1",
                "a",
                CopperLayer::Top,
                250 * UM,
                &[(2 * MM, 5 * MM), (10 * MM, 5 * MM)],
            );
        let f = copper(&connect::connect("b", &s), false);
        assert_eq!(codes(&f), [Code::NetShort], "{f:?}");
        assert!(f[0].refs.contains(&"b:u1/5".to_owned()), "{f:?}");
        assert_eq!(f[0].details["netless"], json!(["b:u1/5"]));
        assert_eq!(f[0].details["nets"], json!(["a"]));
        // A netless pad alone, or touching only netless copper, is no finding.
        let mut s = board(20 * MM, 10 * MM);
        s.smd("u1", "5", (10 * MM, 5 * MM), (600 * UM, 600 * UM), None)
            .smd(
                "u1",
                "6",
                (10 * MM + 500 * UM, 5 * MM),
                (600 * UM, 600 * UM),
                None,
            );
        assert!(copper(&connect::connect("b", &s), false).is_empty());
    }

    #[test]
    fn tracks_of_two_nets_touching_with_no_pad_are_a_short() {
        let mut s = board(20 * MM, 10 * MM);
        s.track(
            "t1",
            "a",
            CopperLayer::Top,
            250 * UM,
            &[(2 * MM, 5 * MM), (8 * MM, 5 * MM)],
        )
        .track(
            "t2",
            "b",
            CopperLayer::Top,
            250 * UM,
            &[(5 * MM, 2 * MM), (5 * MM, 8 * MM)],
        );
        let f = copper(&connect::connect("b", &s), false);
        assert_eq!(codes(&f), [Code::NetShort], "{f:?}");
        assert_eq!(f[0].refs, ["b:track/t1", "b:track/t2"]);
        assert_eq!(f[0].details["nets"], json!(["a", "b"]));
        assert_eq!(
            f[0].details["contacts"],
            json!([["b:track/t1", "b:track/t2"]])
        );
        // Apart on two layers, they are no finding here.
        let mut s = board(20 * MM, 10 * MM);
        s.track(
            "t1",
            "a",
            CopperLayer::Top,
            250 * UM,
            &[(2 * MM, 5 * MM), (8 * MM, 5 * MM)],
        )
        .track(
            "t2",
            "b",
            CopperLayer::Bottom,
            250 * UM,
            &[(5 * MM, 2 * MM), (5 * MM, 8 * MM)],
        );
        assert!(copper(&connect::connect("b", &s), false).is_empty());
    }

    #[test]
    fn a_gap_on_both_layers_is_one_finding_with_its_layers() {
        // A via a micron under the clearance from a through pad: the same
        // gap on the top and the bottom.
        let mut s = board(20 * MM, 10 * MM);
        let x = 5 * MM + 850 * UM + 300 * UM + P.clearance_nm - UM;
        s.through("j1", "1", (5 * MM, 5 * MM), 1700 * UM, 1000 * UM, Some("a"))
            .via("v1", "b", (x, 5 * MM), 300 * UM, 600 * UM);
        let f = clearance(&connect::connect("b", &s), &Rules::plain(P));
        assert_eq!(codes(&f), [Code::DrcClearance], "{f:?}");
        assert_eq!(
            f[0].details["layers"],
            json!(["copper.top", "copper.bottom"])
        );
        assert_eq!(f[0].details["distance_nm"], json!(P.clearance_nm - UM));
    }

    #[test]
    fn a_via_with_a_bad_drill_and_diameter_is_one_finding() {
        let mut s = board(20 * MM, 10 * MM);
        s.via(
            "v1",
            "a",
            (10 * MM, 5 * MM),
            P.drill_min_nm - 1,
            P.via_diameter_min_nm - 1,
        );
        let f = drills(&s, "b", P);
        assert_eq!(codes(&f), [Code::DrcDrill], "{f:?}");
        assert_eq!(f[0].details["drill_nm"], json!(P.drill_min_nm - 1));
        assert_eq!(
            f[0].details["diameter_nm"],
            json!(P.via_diameter_min_nm - 1)
        );
    }

    #[test]
    fn findings_sharing_an_id_are_merged() {
        let a = finding(
            Code::DrcDrill,
            Severity::Error,
            "b",
            vec!["x".into()],
            "one".into(),
            json!({"n": 1}),
        );
        let b = finding(
            Code::DrcDrill,
            Severity::Error,
            "b",
            vec!["x".into()],
            "two".into(),
            json!({"n": 2}),
        );
        let c = finding(
            Code::DrcDrill,
            Severity::Error,
            "b",
            vec!["y".into()],
            "three".into(),
            Value::Null,
        );
        let f = unique(vec![a, b, c]);
        assert_eq!(f.len(), 2);
        assert_eq!(
            f[0].details["also"],
            json!([{"message": "two", "details": {"n": 2}}])
        );
        let ids: BTreeSet<&str> = f.iter().map(|x| x.id.as_str()).collect();
        assert_eq!(ids.len(), f.len());
    }
}
