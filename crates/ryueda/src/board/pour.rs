//! A board's pours (design 924): `pours/<id>.json`, copper of one net on
//! one layer over a boundary (a contour, or the whole outline), filled by
//! `ryu_copper::fill` on every build. The fill is derived, never stored.

use super::file::{Contour, CopperLayer};
use super::preset::{HoleKind, Preset};
use super::rules::{PourRules, Rules};
use super::shapes::{BoardShapes, HoleOf};
use crate::error::Code;
use crate::fault::Fault;
use crate::footprint::ZoneConnect;
use crate::id::Id;
use rayon::prelude::*;
use ryu_board::geom::Region;
use ryu_board::model::Polarity;
use ryu_board::shape::Shape;
use ryu_copper::fill::{self, Ask, BOUND, ThermalPad};
use ryu_copper::measure::{Body, BodyOptions};
use ryu_copper::poly::{self, Polygons};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

pub use super::file::{POUR_KIND, POURS};

/// How a pour meets its net's pads.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Connect {
    #[default]
    Thermal,
    Solid,
}

impl Connect {
    fn is_thermal(&self) -> bool {
        *self == Connect::Thermal
    }
}

/// What a pour does with copper its net doesn't reach.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Islands {
    #[default]
    Remove,
    Keep,
}

impl Islands {
    fn is_remove(&self) -> bool {
        *self == Islands::Remove
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ThermalRule {
    pub gap_nm: i64,
    pub spoke_nm: i64,
}

fn is_zero(v: &i64) -> bool {
    *v == 0
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Pour {
    pub net: String,
    pub layer: CopperLayer,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub contour: Option<Contour>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub cutouts: Vec<Contour>,
    /// The board's outline, following its edits (a plane).
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub whole: bool,
    /// Higher fills first; a lower pour clears it.
    #[serde(default, skip_serializing_if = "is_zero")]
    pub priority: i64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub clearance_nm: Option<i64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub min_width_nm: Option<i64>,
    #[serde(default, skip_serializing_if = "Connect::is_thermal")]
    pub connect: Connect,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub thermal: Option<ThermalRule>,
    #[serde(default, skip_serializing_if = "Islands::is_remove")]
    pub islands: Islands,
}

impl Pour {
    /// A plane: `net` over the whole outline on `layer`, defaults else.
    pub fn whole(net: &str, layer: CopperLayer) -> Pour {
        Pour {
            net: net.to_owned(),
            layer,
            contour: None,
            cutouts: Vec::new(),
            whole: true,
            priority: 0,
            clearance_nm: None,
            min_width_nm: None,
            connect: Connect::Thermal,
            thermal: None,
            islands: Islands::Remove,
        }
    }
}

fn contour_fault(c: &Contour, pointer: &str) -> Option<Fault> {
    let bad = |why: &str| {
        Some(
            Fault::error(Code::BoardContourInvalid, format!("a pour's contour {why}"))
                .with("pointer", pointer),
        )
    };
    match super::outline::contour(c) {
        Err(super::outline::Bad::OutOfRange) => bad("reaches past ±2^40 nm"),
        Err(super::outline::Bad::Invalid(why)) => bad(why),
        Ok(k) => {
            let r = super::outline::ring(&k);
            if r.len() < 3 || !super::outline::simple(&r) {
                bad("crosses or touches itself")
            } else {
                None
            }
        }
    }
}

/// A pour's faults on a board of `n` copper layers.
pub fn judge(p: &Pour, n: usize) -> Vec<Fault> {
    let mut out = Vec::new();
    let invalid = |msg: &str, ptr: &str| {
        Fault::error(Code::InvalidDocument, msg.to_owned()).with("pointer", ptr)
    };
    match (&p.contour, p.whole) {
        (Some(_), true) => out.push(invalid(
            "a pour is a contour or the whole board, not both",
            "/whole",
        )),
        (None, false) => out.push(invalid(
            "a pour needs a contour, or whole: true",
            "/contour",
        )),
        (Some(c), false) => out.extend(contour_fault(c, "/contour")),
        (None, true) => {}
    }
    for (i, c) in p.cutouts.iter().enumerate() {
        out.extend(contour_fault(c, &format!("/cutouts/{i}")));
    }
    if p.layer.index(n).is_none() {
        out.push(
            Fault::error(
                Code::BoardLayerNotFound,
                format!(
                    "the board has {n} copper layers and no {}; they are {}",
                    p.layer.short(),
                    super::load::layer_list(n)
                ),
            )
            .with("pointer", "/layer")
            .with("layer", p.layer.short()),
        );
    }
    for (v, ptr) in [
        (p.clearance_nm, "/clearance_nm"),
        (p.min_width_nm, "/min_width_nm"),
        (p.thermal.map(|t| t.gap_nm), "/thermal/gap_nm"),
        (p.thermal.map(|t| t.spoke_nm), "/thermal/spoke_nm"),
    ] {
        if v.is_some_and(|v| v <= 0) {
            out.push(invalid("a pour's lengths are positive", ptr));
        }
    }
    out
}

/// A pour's values under its preset's floor.
pub fn floors(p: &Pour, preset: &super::preset::Preset) -> Vec<Fault> {
    let mut out = Vec::new();
    let below = |what: &str, v: i64, floor: i64, ptr: &str| {
        Fault::error(
            Code::RuleBelowPreset,
            format!(
                "a pour's {what} of {v} nm is under {}'s {floor} nm",
                preset.name
            ),
        )
        .with("pointer", ptr)
    };
    if let Some(c) = p.clearance_nm.filter(|c| *c < preset.clearance_nm) {
        out.push(below("clearance", c, preset.clearance_nm, "/clearance_nm"));
    }
    if let Some(w) = p.min_width_nm.filter(|w| *w < preset.track_width_min_nm) {
        out.push(below(
            "least width",
            w,
            preset.track_width_min_nm,
            "/min_width_nm",
        ));
    }
    out
}

/// A pour as filled.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PourInfo {
    pub id: Id,
    pub net: String,
    /// Its copper layer, by index.
    pub layer: usize,
    pub rules: PourRules,
    pub polygons: Polygons,
    /// Each kept polygon as a shape, in the polygons' order.
    pub regions: Vec<Shape>,
    /// Each region's tile corner and index in its tile (ryueda 7.E).
    pub places: Vec<(i64, i64, u32)>,
    pub islands_removed: usize,
    /// Regions (by index) whose contact with the net's copper was uncertain.
    pub uncertain: Vec<usize>,
    /// Each thermal pad's reference and its kept spokes.
    pub thermals: Vec<(String, u8)>,
    /// The fill by tile (ryueda 7.E): the next fill reuses the tiles
    /// whose inputs held.
    pub tiled: fill::Tiled,
    /// What the fill grew: the next fill reuses each obstacle whose shape
    /// and growth held. Derived; never compared.
    pub grown: std::sync::Arc<Grown>,
}

/// What a fill grows into an obstacle: a shape, or a hole's wall, by how
/// far.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
enum Grow {
    Shape(Shape, i64),
    Hole(ryu_copper::layout::HoleShape, i64),
}

/// Obstacles grown, by what and how far (ryueda 7.E): a refill grows
/// only the copper that changed.
#[derive(Clone, Debug, Default)]
pub struct Grown(std::collections::HashMap<Grow, std::sync::Arc<Polygons>>);

impl PartialEq for Grown {
    fn eq(&self, _: &Grown) -> bool {
        true
    }
}

impl Eq for Grown {}

/// A shape flattened at the fill's bound.
fn flat(shape: &Shape) -> Polygons {
    poly::merge_layer(&poly::flatten_shape(shape, &fill::opts()), &[])
}

/// A polygon as a region shape: its outer ring and holes as contours.
fn region(p: &poly::Polygon) -> Option<Shape> {
    let outer = ryu_board::shape::polygon(&p.outer).ok()?;
    let holes = p
        .holes
        .iter()
        .filter_map(|h| ryu_board::shape::polygon(h).ok())
        .collect();
    Some(Shape::regions(
        Polarity::Positive,
        [Region { outer, holes }],
    ))
}

fn contour_polys(c: &super::file::Contour) -> Option<Polygons> {
    let k = super::outline::ccw(super::outline::contour(c).ok()?);
    Some(flat(&Shape::regions(
        Polarity::Positive,
        [Region::solid(k)],
    )))
}

/// How far an obstacle grows: its rule, the bound and a nanometre.
fn reach(rule: i64) -> i64 {
    // The tiles' seams snap the fill's outline by a nanometre more.
    rule + BOUND + 1 + fill::TILE_SNAP
}

/// One pour filled on its layer, after `done` (the layer's higher pours).
#[allow(clippy::too_many_arguments)]
fn fill_one(
    id: &Id,
    p: &Pour,
    l: usize,
    s: &BoardShapes,
    rules: &Rules,
    preset: &Preset,
    outline: &Polygons,
    done: &[PourInfo],
    prev: Option<&PourInfo>,
) -> PourInfo {
    let made: std::sync::Mutex<std::collections::HashMap<Grow, std::sync::Arc<Polygons>>> =
        Default::default();
    let grow = |k: Grow| -> Polygons {
        let kept = prev.and_then(|x| x.grown.0.get(&k).cloned());
        let g = kept.unwrap_or_else(|| {
            std::sync::Arc::new(match &k {
                Grow::Shape(shape, d) => poly::grow(&flat(shape), *d, &fill::opts()),
                Grow::Hole(h, d) => poly::grow(
                    &poly::simplify(
                        &poly::flatten_hole(h, &fill::opts()),
                        poly::FillRule::NonZero,
                    ),
                    *d,
                    &fill::opts(),
                ),
            })
        });
        let out = (*g).clone();
        made.lock().unwrap().insert(k, g);
        out
    };
    let n = s.n();
    let opts = fill::opts();
    let net = p.net.as_str();
    let prep = crate::profile::stage("pour.prep");
    let r = rules.pour(p);
    let inner = l != 0 && l + 1 != n;
    // The area.
    let boundary = if p.whole {
        outline.clone()
    } else {
        let c = p
            .contour
            .as_ref()
            .and_then(contour_polys)
            .unwrap_or_default();
        let cuts: Vec<Polygons> = p.cutouts.iter().filter_map(contour_polys).collect();
        poly::difference(&c, &fill::union_all(&cuts))
    };
    let limit = poly::shrink(outline, reach(rules.edge_clearance(Some(net)).value), &opts);
    let mut area = poly::intersection(&boundary, &limit);
    let mut cut: Vec<Polygons> = Vec::new();
    for k in &s.keepouts {
        let keeps = k.keeps.iter().any(|x| {
            matches!(
                x,
                crate::footprint::Keep::Pours | crate::footprint::Keep::Copper
            )
        });
        if keeps && k.layers.contains(&l) {
            cut.push(poly::grow(&flat(&k.shape), BOUND + 1, &opts));
        }
    }
    for d in done {
        if d.net == net {
            cut.push(d.polygons.clone());
        } else {
            let need = rules
                .pair_clearance(&[net], &[d.net.as_str()])
                .value
                .max(r.clearance.value);
            cut.push(poly::grow(&d.polygons, reach(need), &opts));
        }
    }
    if !cut.is_empty() {
        area = poly::difference(&area, &fill::union_all(&cut));
    }
    // The obstacles, the spokes' obstacles, the thermals and the net's own copper.
    let mut obstacles = Vec::new();
    let mut blocked = Vec::new();
    let mut thermals = Vec::new();
    let mut names = Vec::new();
    let mut own = Vec::new();
    let body_opts = BodyOptions::default();
    // Another net's copper grown by the pair's clearance: an obstacle,
    // and in the spokes' way.
    let other = |shape: &Shape, their: Option<&str>| -> Polygons {
        let need = rules
            .pair_clearance(&[net], &their.into_iter().collect::<Vec<_>>())
            .value
            .max(r.clearance.value);
        grow(Grow::Shape(shape.clone(), reach(need)))
    };
    // Other nets' copper, grown in parallel below, in this order.
    let mut others: Vec<(&Shape, Option<&str>)> = Vec::new();
    for pd in s.pads.iter().filter(|pd| pd.on(l, n)) {
        if pd.net.as_deref() != Some(net) {
            others.push((&pd.copper, pd.net.as_deref()));
            continue;
        }
        own.push(Body::of_shape(&pd.copper, &body_opts));
        let through = pd.top && pd.bottom;
        let connect = match pd.connect {
            Some(ZoneConnect::Solid) => Connect::Solid,
            Some(ZoneConnect::ThermalThrough) if !through => Connect::Solid,
            Some(ZoneConnect::None) => {
                let gap = pd.thermal.map_or(r.gap.value, |t| t.0);
                obstacles.push(grow(Grow::Shape(pd.copper.clone(), reach(gap))));
                continue;
            }
            Some(_) => Connect::Thermal,
            None => p.connect,
        };
        if connect == Connect::Thermal {
            let (gap, spoke) = pd.thermal.unwrap_or((r.gap.value, r.spoke.value));
            obstacles.push(grow(Grow::Shape(pd.copper.clone(), reach(gap))));
            // Only a pad the area reaches within its spokes' length has
            // thermals: another's are no starved connection.
            let spokes_reach = grow(Grow::Shape(
                pd.copper.clone(),
                reach(gap) + spoke.max(r.min_width.value),
            ));
            if poly::intersection(&spokes_reach, &area).is_empty() {
                continue;
            }
            thermals.push(ThermalPad {
                at: pd.at,
                rotation_mdeg: pd.rotation_mdeg,
                half: [pd.size[0] / 2, pd.size[1] / 2],
                gap,
                spoke,
            });
            names.push(pd.reference.clone());
        }
    }
    for t in s.tracks.iter().filter(|t| t.layer.index(n) == Some(l)) {
        if t.net == net {
            own.push(Body::of_shape(&t.shape, &body_opts));
        } else {
            others.push((&t.shape, Some(&t.net)));
        }
    }
    for v in s.vias.iter().filter(|v| v.from <= l && l <= v.to) {
        if v.net == net {
            own.push(Body::of_shape(&v.shape, &body_opts));
        } else {
            others.push((&v.shape, Some(&v.net)));
        }
    }
    let grown: Vec<Polygons> = others
        .par_iter()
        .map(|(shape, their)| other(shape, *their))
        .collect();
    obstacles.extend(grown.iter().cloned());
    blocked.extend(grown);
    for h in s.holes.iter().filter(|h| h.span.0 <= l && l <= h.span.1) {
        let owned = h.plated
            && match &h.of {
                HoleOf::Pad { occ, pad } => s.pads.iter().any(|x| {
                    &x.occurrence == occ && &x.pad == pad && x.net.as_deref() == Some(net)
                }),
                HoleOf::Via(vid) => s.vias.iter().any(|x| &x.id == vid && x.net == net),
                HoleOf::Hole(_) => false,
            };
        if owned {
            continue;
        }
        let kind = match (&h.of, h.plated) {
            (HoleOf::Via(_), _) => HoleKind::Via,
            (HoleOf::Pad { .. }, true) => HoleKind::Pth,
            _ => HoleKind::Npth,
        };
        let rule = rules
            .hole_clearance(Some(net))
            .map_or(0, |x| x.value)
            .max(preset.hole_clearance(kind, inner));
        obstacles.push(grow(Grow::Hole(h.shape, reach(rule))));
    }
    let ask = Ask {
        area,
        obstacles,
        spoke_obstacles: blocked,
        thermals,
        min_width: r.min_width.value,
        own,
        keep_islands: p.islands == Islands::Keep,
    };
    drop(prep);
    let stage = crate::profile::stage("pour.tiled");
    let t = fill::fill_tiled(&ask, fill::TILE, prev.map(|x| &x.tiled));
    drop(stage);
    // Each kept piece is a region, tile by tile; an uncertain piece's
    // region index is its place among them.
    let pieces: Vec<(usize, usize)> = t.pieces().map(|(ti, i, _)| (ti, i)).collect();
    let (regions, places): (Vec<Shape>, Vec<(i64, i64, u32)>) = t
        .pieces()
        .filter_map(|(ti, i, p)| {
            let at = t.tiles[ti].at;
            region(p).map(|r| (r, (at.x, at.y, i as u32)))
        })
        .unzip();
    let uncertain = t
        .uncertain
        .iter()
        .filter_map(|u| pieces.iter().position(|x| x == u))
        .collect();
    PourInfo {
        id: id.clone(),
        net: p.net.clone(),
        layer: l,
        rules: r,
        regions,
        places,
        islands_removed: t.islands_removed,
        uncertain,
        thermals: names.into_iter().zip(t.spokes.iter().copied()).collect(),
        polygons: t.polygons(),
        tiled: t,
        grown: std::sync::Arc::new(Grown(made.into_inner().unwrap())),
    }
}

/// Every pour filled: on each layer by priority (higher first) then ID,
/// layers in parallel. `pours` are the board's readable, fault-free ones.
/// `prev` are the last fills: each pour reuses its own's tiles whose
/// inputs held (ryueda 7.E).
pub fn fill_all(
    pours: &[(Id, Pour)],
    s: &BoardShapes,
    rules: &Rules,
    preset: &Preset,
    prev: &[PourInfo],
) -> Vec<PourInfo> {
    let n = s.n();
    let outline = flat(&s.outline);
    let mut by_layer: BTreeMap<usize, Vec<&(Id, Pour)>> = BTreeMap::new();
    for x in pours {
        if let Some(l) = x.1.layer.index(n) {
            by_layer.entry(l).or_default().push(x);
        }
    }
    let layers: Vec<(usize, Vec<&(Id, Pour)>)> = by_layer.into_iter().collect();
    layers
        .par_iter()
        .flat_map_iter(|(l, list)| {
            let mut list = list.clone();
            list.sort_by(|a, b| b.1.priority.cmp(&a.1.priority).then_with(|| a.0.cmp(&b.0)));
            let mut done: Vec<PourInfo> = Vec::new();
            for (id, p) in list {
                let old = prev.iter().find(|x| &x.id == id && x.layer == *l);
                let f = fill_one(id, p, *l, s, rules, preset, &outline, &done, old);
                done.push(f);
            }
            done
        })
        .collect()
}

/// `pour_empty`, `pour_thermal_starved` and `copper_contact_uncertain` of
/// each fill (a DRC rule).
pub fn findings(s: &BoardShapes, board: &str) -> Vec<super::drc::Finding> {
    use crate::fault::Severity;
    use serde_json::json;
    let mut out = Vec::new();
    for f in &s.pours {
        let me = format!("{board}:pour/{}", f.id);
        if f.polygons.is_empty() {
            out.push(super::drc::finding(
                Code::PourEmpty,
                Severity::Warning,
                board,
                vec![me.clone()],
                format!("{me} fills nothing"),
                json!({"net": f.net, "islands_removed": f.islands_removed}),
            ));
        }
        for (pad, spokes) in &f.thermals {
            if *spokes < 2 {
                out.push(super::drc::finding(
                    Code::PourThermalStarved,
                    Severity::Warning,
                    board,
                    vec![me.clone(), pad.clone()],
                    format!("{pad} meets {me} by {spokes} thermal spoke(s)"),
                    json!({"spokes": spokes}),
                ));
            }
        }
        for k in &f.uncertain {
            out.push(super::drc::finding(
                Code::CopperContactUncertain,
                Severity::Warning,
                board,
                vec![format!("{me}#{k}")],
                format!("whether {me}'s region {k} touches its net's copper is within the bound"),
                json!({"region": k}),
            ));
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::board::preset::JLC_2L;
    use crate::board::rules::Rules;
    use crate::board::testing::{MM, UM, board};
    use crate::doc;
    use ryu_copper::measure::{self, Body, BodyOptions, Verdict};
    use serde_json::json;

    fn rules() -> Rules {
        Rules::from_parts(std::sync::Arc::new(JLC_2L), None, Default::default())
    }

    fn plane(net: &str) -> Pour {
        Pour::whole(net, CopperLayer::Top)
    }

    #[test]
    fn a_pour_reads_writes_and_is_judged() {
        let v = json!({"kind": POUR_KIND, "format": 1, "net": "rail:gnd", "layer": "in1", "whole": true,
                       "priority": 2, "connect": "solid", "islands": "keep",
                       "thermal": {"gap_nm": 300_000, "spoke_nm": 400_000}});
        let p: Pour = doc::from_value(v.clone(), "p.json", POUR_KIND, 1).unwrap();
        assert_eq!(doc::to_value(&p, POUR_KIND, 1), v);
        assert!(judge(&p, 4).is_empty());
        assert_eq!(judge(&p, 2)[0].error.code, Code::BoardLayerNotFound);
        let both = Pour {
            contour: Some(crate::board::file::Contour::rect(10, 10)),
            ..p.clone()
        };
        assert_eq!(judge(&both, 4)[0].error.code, Code::InvalidDocument);
        let neither = Pour {
            whole: false,
            ..p.clone()
        };
        assert_eq!(judge(&neither, 4)[0].error.code, Code::InvalidDocument);
        let open = Pour {
            whole: false,
            contour: Some(crate::board::file::Contour {
                start: [0, 0],
                edges: vec![crate::board::file::Op::Line { to: [5, 0] }],
            }),
            ..p.clone()
        };
        assert_eq!(judge(&open, 4)[0].error.code, Code::BoardContourInvalid);
        // Defaults are left out of the file.
        let bare: Pour = doc::from_value(
            json!({"kind": POUR_KIND, "format": 1, "net": "a", "layer": "top", "whole": true}),
            "p.json",
            POUR_KIND,
            1,
        )
        .unwrap();
        assert_eq!(bare.connect, Connect::Thermal);
        assert_eq!(bare.islands, Islands::Remove);
        assert_eq!(
            doc::to_value(&bare, POUR_KIND, 1)
                .as_object()
                .unwrap()
                .len(),
            5
        );
    }

    #[test]
    fn floors_are_the_presets() {
        let p = Pour {
            clearance_nm: Some(10),
            min_width_nm: Some(10),
            ..Pour::whole("a", crate::board::file::CopperLayer::Top)
        };
        let f = floors(&p, &crate::board::preset::JLC_2L);
        assert_eq!(
            f.iter()
                .filter(|x| x.error.code == Code::RuleBelowPreset)
                .count(),
            2
        );
    }

    #[test]
    fn a_plane_avoids_other_nets_and_reaches_its_own() {
        let mut s = board(20 * MM, 10 * MM);
        s.smd("r1", "1", (5 * MM, 5 * MM), (MM, MM), Some("gnd"))
            .smd("r1", "2", (15 * MM, 5 * MM), (MM, MM), Some("sig"));
        let pours = vec![(Id::parse("p1", "pour").unwrap(), plane("gnd"))];
        let f = fill_all(&pours, &s, &rules(), &JLC_2L, &[]);
        assert_eq!(f.len(), 1);
        let p = &f[0];
        assert!(!p.polygons.is_empty());
        let opts = BodyOptions::default();
        let fill = p.polygons.body();
        let sig = Body::of_shape(&s.pads[1].copper, &opts);
        // Exactly the clearance away from the other net's pad, at least.
        assert_eq!(measure::closer(&fill, &sig, 100 * UM), Verdict::No);
        // Its own pad by four spokes, and touching it.
        assert_eq!(p.thermals, vec![("b:r1/1".to_owned(), 4)]);
        let gnd = Body::of_shape(&s.pads[0].copper, &opts);
        assert_ne!(measure::touches(&fill, &gnd), Verdict::No);
        // Inside the edge clearance.
        let edges = Body::of_shape(&s.edges, &opts);
        assert_eq!(
            measure::closer(&fill, &edges, JLC_2L.edge_clearance_nm),
            Verdict::No
        );
    }

    #[test]
    fn higher_priority_fills_first_and_a_lower_clears_it() {
        let mut s = board(20 * MM, 10 * MM);
        s.smd("r1", "1", (5 * MM, 5 * MM), (MM, MM), Some("a")).smd(
            "r1",
            "2",
            (15 * MM, 5 * MM),
            (MM, MM),
            Some("b"),
        );
        let hi = Pour {
            priority: 1,
            ..plane("a")
        };
        let lo = plane("b");
        let pours = vec![
            (Id::parse("lo", "pour").unwrap(), lo),
            (Id::parse("hi", "pour").unwrap(), hi),
        ];
        let f = fill_all(&pours, &s, &rules(), &JLC_2L, &[]);
        let a = f.iter().find(|x| x.net == "a").unwrap();
        let b = f.iter().find(|x| x.net == "b").unwrap();
        assert!(a.polygons.area2() > 0);
        // The lower pour is what is left: cleared from the higher's fill.
        assert_eq!(
            measure::closer(&a.polygons.body(), &b.polygons.body(), 100 * UM),
            Verdict::No
        );
    }

    #[test]
    fn fills_are_the_same_on_one_thread_and_eight() {
        let mut s = board(20 * MM, 10 * MM);
        s.smd("r1", "1", (5 * MM, 5 * MM), (MM, MM), Some("gnd"))
            .via("v1", "sig", (12 * MM, 5 * MM), 300 * UM, 600 * UM);
        let pours = vec![(Id::parse("p1", "pour").unwrap(), plane("gnd"))];
        let run = |n| {
            rayon::ThreadPoolBuilder::new()
                .num_threads(n)
                .build()
                .unwrap()
                .install(|| fill_all(&pours, &s, &rules(), &JLC_2L, &[]))
        };
        assert_eq!(run(1), run(8));
    }
    #[test]
    fn an_empty_fill_and_a_starved_pad_are_warnings() {
        let mut s = board(20 * MM, 10 * MM);
        s.smd("r1", "1", (5 * MM, 5 * MM), (MM, MM), Some("gnd"));
        // A contour too small to hold anything past its clearances.
        let tiny = Pour {
            whole: false,
            contour: Some(crate::board::file::Contour {
                start: [15 * MM, 5 * MM],
                edges: vec![
                    crate::board::file::Op::Line {
                        to: [15 * MM + 100, 5 * MM],
                    },
                    crate::board::file::Op::Line {
                        to: [15 * MM + 100, 5 * MM + 100],
                    },
                    crate::board::file::Op::Line {
                        to: [15 * MM, 5 * MM],
                    },
                ],
            }),
            ..plane("gnd")
        };
        let pours = vec![(Id::parse("p1", "pour").unwrap(), tiny)];
        s.pours = fill_all(&pours, &s, &rules(), &JLC_2L, &[]);
        let f = findings(&s, "b");
        assert_eq!(
            f.iter().map(|x| x.code).collect::<Vec<_>>(),
            [Code::PourEmpty]
        );
        assert_eq!(f[0].severity, crate::fault::Severity::Warning);
    }

    #[test]
    fn a_pad_boxed_in_on_three_sides_is_starved() {
        let mut s = board(20 * MM, 10 * MM);
        s.smd("r1", "1", (5 * MM, 5 * MM), (MM, MM), Some("gnd"))
            .smd("r2", "1", (34 * MM / 10, 5 * MM), (MM, MM), Some("sig"))
            .smd("r3", "1", (5 * MM, 66 * MM / 10), (MM, MM), Some("sig"))
            .smd("r4", "1", (5 * MM, 34 * MM / 10), (MM, MM), Some("sig"));
        let pours = vec![(Id::parse("p1", "pour").unwrap(), plane("gnd"))];
        s.pours = fill_all(&pours, &s, &rules(), &JLC_2L, &[]);
        let f = findings(&s, "b");
        let starved: Vec<_> = f
            .iter()
            .filter(|x| x.code == Code::PourThermalStarved)
            .collect();
        assert_eq!(starved.len(), 1, "{f:?}");
        assert_eq!(starved[0].details["spokes"], 1);
        assert!(starved[0].refs.contains(&"b:r1/1".to_owned()));
    }

    #[test]
    fn a_refill_reuses_the_tiles_an_edit_doesnt_reach() {
        let mut s = board(40 * MM, 20 * MM);
        s.smd("r1", "1", (5 * MM, 5 * MM), (MM, MM), Some("gnd"))
            .smd("r1", "2", (35 * MM, 15 * MM), (MM, MM), Some("sig"));
        let pours = vec![(Id::parse("p1", "pour").unwrap(), plane("gnd"))];
        let first = fill_all(&pours, &s, &rules(), &JLC_2L, &[]);
        assert!(
            first[0].tiled.tiles.len() > 8,
            "a 40 by 20 mm board in 8 mm tiles"
        );
        s.smd("r2", "1", (35 * MM, 5 * MM), (MM, MM), Some("sig"));
        let again = fill_all(&pours, &s, &rules(), &JLC_2L, &first);
        assert_eq!(again, fill_all(&pours, &s, &rules(), &JLC_2L, &[]));
        let changed = again[0]
            .tiled
            .tiles
            .iter()
            .filter(|t| {
                first[0]
                    .tiled
                    .tiles
                    .iter()
                    .find(|u| u.at == t.at)
                    .is_none_or(|u| u.key != t.key)
            })
            .count();
        assert!(
            (1..=4).contains(&changed),
            "{changed} tiles changed for one pad"
        );
    }
}
