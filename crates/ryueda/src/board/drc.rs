//! DRC (design 732): a board's findings, ERC's shape, from its shapes,
//! its copper's connectivity against the design's netlist, and its
//! preset's rules. Every topology decision is `ryu-copper`'s exact
//! predicate; a rule a bound can't decide is reported, never dropped.

use super::connect::{self, Connectivity};
use super::layout::Key;
use super::load::BoardFiles;
use super::occ::{self, Occ};
use super::preset::{self, Preset};
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

/// The bound a pad's outline is flattened at when it isn't all regions.
const FLAT: Nm = 10;

/// Tracks narrower than the rule.
pub fn widths(s: &BoardShapes, board: &str, preset: &Preset) -> Vec<Finding> {
    s.tracks
        .iter()
        .filter(|t| t.width_nm < preset.track_width_min_nm)
        .map(|t| {
            finding(
                Code::DrcTrackWidth,
                Severity::Error,
                board,
                vec![format!("{board}:track/{}", t.id)],
                format!(
                    "track {} is {} nm wide, under the {} nm minimum",
                    t.id, t.width_nm, preset.track_width_min_nm
                ),
                json!({"width_nm": t.width_nm, "rule_nm": preset.track_width_min_nm}),
            )
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
    for v in &s.vias {
        if v.diameter_nm < preset.via_diameter_min_nm {
            out.push(finding(
                Code::DrcDrill,
                Severity::Error,
                board,
                vec![format!("{board}:via/{}", v.id)],
                format!(
                    "via {}'s diameter is {} nm, under the {} nm minimum",
                    v.id, v.diameter_nm, preset.via_diameter_min_nm
                ),
                json!({"diameter_nm": v.diameter_nm, "min_nm": preset.via_diameter_min_nm}),
            ));
        }
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
pub fn edges(c: &Connectivity, s: &BoardShapes, preset: &Preset) -> Vec<Finding> {
    let board = c.built.board.as_str();
    let rule = preset.edge_clearance_nm;
    let opts = BodyOptions::default();
    let edge = Body::of_shape(&s.edges, &opts);
    let inside = Body::of_shape(&s.outline, &opts);
    if edge.is_empty() {
        return Vec::new();
    }
    let mut things: BTreeMap<String, Body> = BTreeMap::new();
    for (k, _) in c.copper.items() {
        if let Some(b) = c.copper.bodies().body(k) {
            things
                .entry(c.built.reference(k))
                .or_insert_with(|| b.clone());
        }
    }
    for h in &s.holes {
        // A through pad's drill shares its pad's reference: `#hole` keeps
        // the two findings' IDs apart.
        things
            .entry(format!("{}#hole", h.owner))
            .or_insert_with(|| h.shape.body());
    }
    things
        .par_iter()
        .filter_map(|(r, b)| {
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
pub fn courtyards(s: &BoardShapes, board: &str) -> Vec<Finding> {
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
        .pairs(0, 0)
        .into_par_iter()
        .filter_map(|p| {
            let v = measure::touches(&bodies[p.a], &bodies[p.b]);
            if v == Verdict::No {
                return None;
            }
            let (a, b) = (&s.courtyards[p.a], &s.courtyards[p.b]);
            Some(finding(
                Code::DrcCourtyardOverlap,
                Severity::Error,
                board,
                vec![a.owner.clone(), b.owner.clone()],
                format!("{} and {}'s courtyards touch", a.owner, b.owner),
                json!({"uncertain": v == Verdict::Uncertain}),
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
    pub preset: &'static Preset,
    pub findings: Vec<Finding>,
    pub incomplete: bool,
}

impl BoardCheck {
    pub fn errors(&self) -> usize {
        self.findings.iter().filter(|f| f.is_error()).count()
    }

    pub fn warnings(&self) -> usize {
        self.findings.len() - self.errors()
    }
}

/// What every rule reads.
struct Ctx<'a> {
    board: &'a str,
    files: &'a BoardFiles,
    occs: &'a [Occ<'a>],
    shapes: &'a BoardShapes,
    conn: &'a Connectivity,
    preset: &'a Preset,
    incomplete: bool,
}

type Rule = fn(&Ctx) -> Vec<Finding>;

const RULES: [Rule; 10] = [
    |c| placements(c.board, c.files, c.occs, c.shapes),
    |c| copper(c.conn, c.incomplete),
    |c| clearance(c.conn, c.preset),
    |c| mask_clearance(c.conn, c.shapes, c.preset),
    |c| widths(c.shapes, c.board, c.preset),
    |c| drills(c.shapes, c.board, c.preset),
    |c| rings(c.conn, c.shapes, c.preset),
    |c| hole_spacing(c.shapes, c.board, c.preset),
    |c| edges(c.conn, c.shapes, c.preset),
    |c| courtyards(c.shapes, c.board),
];

/// A board checked from shapes already built. A design that doesn't
/// flatten refuses with its faults.
pub fn check_shapes(p: &Project, b: &BoardFiles, shapes: Arc<BoardShapes>) -> Result<BoardCheck> {
    let file = b.board.item.as_ref().map_err(Clone::clone)?;
    let preset = preset::by_name(&file.preset).unwrap_or(preset::DEFAULT);
    let (_, occs) = occ::occurrences(p, &file.design)?;
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
        preset,
        incomplete,
    };
    let mut findings: Vec<Finding> = RULES.par_iter().flat_map_iter(|r| r(&ctx)).collect();
    sort(&mut findings);
    Ok(BoardCheck {
        shapes: shapes.clone(),
        conn,
        preset,
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
            widths(&s, "b", P)
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
            edges(&connect::connect("b", &s), &s, P)
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
        let f = edges(&connect::connect("b", &s), &s, P);
        assert_eq!(codes(&f), [Code::DrcEdgeClearance]);
        assert_eq!(f[0].refs, ["b:hole/h1#hole"]);
    }

    #[test]
    fn courtyards_touching_overlap_and_open_ones_warn() {
        let two = |gap| {
            let mut s = board(20 * MM, 10 * MM);
            s.courtyard("r1", (5 * MM, 5 * MM), (2 * MM, MM), false)
                .courtyard("r2", (7 * MM + gap, 5 * MM), (2 * MM, MM), false);
            courtyards(&s, "b")
        };
        assert!(two(1).is_empty());
        let f = two(0);
        assert_eq!(codes(&f), [Code::DrcCourtyardOverlap]);
        assert_eq!(f[0].refs, ["b:r1", "b:r2"]);
        let mut s = board(20 * MM, 10 * MM);
        s.courtyard("r1", (5 * MM, 5 * MM), (2 * MM, MM), true);
        let f = courtyards(&s, "b");
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
}
