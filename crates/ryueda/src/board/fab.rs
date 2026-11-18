//! A board's fabrication layers and drills as `ryu-gerber`'s input
//! (design 732): copper per side with each pad's, track's and via's X2
//! attributes, mask, paste and silk per side, the profile, and plated and
//! unplated drills. Gerber writes no region with a hole, so one (a custom
//! pad's opening) is cut into pieces without one first.

use super::file::CopperLayer;
use super::shapes::{BoardShapes, HoleOf};
use crate::error::{Code, Error, Result};
use ryu_board::geom::{Contour, Edge, Point, Rect, Region};
use ryu_board::shape::{Part, Piece, polygon};
use ryu_copper::layout::HoleShape;
use ryu_copper::poly::{self, FillRule, Rings};
use ryu_gerber::excellon::{Drill, Drills, Hole};
use ryu_gerber::gerber::{Item, Layer};

/// One Gerber file of the package.
pub struct LayerFile {
    /// Its name's function part: `copper-top`.
    pub function: &'static str,
    /// Its Protel extension: `gtl`.
    pub ext: &'static str,
    pub layer: Layer,
}

/// The package's Gerber layers in order: (function, extension, the
/// board's layer, X2's `.FileFunction`, negative).
pub const LAYERS: [(&str, &str, &str, &str, bool); 9] = [
    ("copper-top", "gtl", "copper.top", "Copper,L1,Top", false),
    (
        "copper-bottom",
        "gbl",
        "copper.bottom",
        "Copper,L2,Bot",
        false,
    ),
    ("mask-top", "gts", "mask.top", "Soldermask,Top", true),
    ("mask-bottom", "gbs", "mask.bottom", "Soldermask,Bot", true),
    ("paste-top", "gtp", "paste.top", "Paste,Top", false),
    ("paste-bottom", "gbp", "paste.bottom", "Paste,Bot", false),
    ("silk-top", "gto", "silk.top", "Legend,Top", false),
    ("silk-bottom", "gbo", "silk.bottom", "Legend,Bot", false),
    ("profile", "gm1", "outline", "Profile,NP", false),
];

/// A side's copper: each pad on it (`.P`, `.N`, `.C`), then its tracks
/// and every via (`.N`).
fn copper(board: &str, s: &BoardShapes, top: bool) -> Vec<Item> {
    let prefix = format!("{board}:");
    let mut out = Vec::new();
    for pd in s.pads.iter().filter(|p| if top { p.top } else { p.bottom }) {
        let local = pd.reference.strip_prefix(&prefix).unwrap_or(&pd.reference);
        let refdes = local
            .rsplit_once('/')
            .map(|(r, _)| r.to_owned())
            .unwrap_or_else(|| local.to_owned());
        let through = pd.top && pd.bottom;
        out.push(Item {
            shape: pd.copper.clone(),
            function: Some(
                if through {
                    "ComponentPad"
                } else {
                    "SMDPad,CuDef"
                }
                .to_owned(),
            ),
            net: pd.net.clone(),
            pad: Some((
                refdes.clone(),
                pd.number.clone().unwrap_or_else(|| pd.pad.clone()),
            )),
            component: Some(refdes),
        });
    }
    let side = if top {
        CopperLayer::Top
    } else {
        CopperLayer::Bottom
    };
    for t in s.tracks.iter().filter(|t| t.layer == side) {
        out.push(Item {
            shape: t.shape.clone(),
            function: Some("Conductor".into()),
            net: Some(t.net.clone()),
            ..Item::default()
        });
    }
    for v in &s.vias {
        out.push(Item {
            shape: v.shape.clone(),
            function: Some("ViaPad".into()),
            net: Some(v.net.clone()),
            ..Item::default()
        });
    }
    out
}

/// A layer drawn as it is, with no attributes.
fn plain(s: &BoardShapes, name: &str) -> Vec<Item> {
    s.layers
        .get(name)
        .map(|v| {
            v.iter()
                .map(|(_, shape)| Item {
                    shape: shape.clone(),
                    ..Item::default()
                })
                .collect()
        })
        .unwrap_or_default()
}

/// A region with holes as pieces without one: the region cut along a
/// vertical line through each hole (a hole meets every vertical line
/// strictly between its least and greatest x, so each of its pieces
/// opens onto a cut) into strips (`ryu_copper::poly::clip`, crossings
/// rounded to the nanometre). Straight edges only: ryueda's holed
/// regions are custom pads' openings, which `ryu_copper::poly` makes as
/// polygons. `None` for arcs, a hole under 2 nm wide, or a strip that
/// still holds a hole.
pub fn fracture(r: &Region) -> Option<Vec<Region>> {
    let ring = |c: &Contour| -> Option<Vec<Point>> {
        c.edges()
            .iter()
            .map(|e| match e {
                Edge::Segment(s) => Some(s.a),
                Edge::Arc(_) => None,
            })
            .collect()
    };
    let mut rings = vec![ring(&r.outer)?];
    let mut cuts = Vec::new();
    for h in &r.holes {
        let pts = ring(h)?;
        let lo = pts.iter().map(|p| p.x).min()?;
        let hi = pts.iter().map(|p| p.x).max()?;
        if hi - lo < 2 {
            return None;
        }
        cuts.push(lo + (hi - lo) / 2);
        rings.push(pts);
    }
    cuts.sort_unstable();
    cuts.dedup();
    let all = Rings::new(rings, 0);
    let b = r.bounds();
    let mut xs = vec![b.min.x - 1];
    xs.extend(cuts);
    xs.push(b.max.x + 1);
    let mut out = Vec::new();
    for w in xs.windows(2) {
        let strip = Rect::new(Point::new(w[0], b.min.y - 1), Point::new(w[1], b.max.y + 1));
        let cut = poly::simplify(&poly::clip(&all, strip), FillRule::NonZero);
        for pg in cut.polygons {
            if !pg.holes.is_empty() {
                return None;
            }
            out.push(Region::solid(polygon(&pg.outer).ok()?));
        }
    }
    Some(out)
}

/// The item with every holed region replaced by its pieces.
fn hole_free(mut item: Item, layer: &str) -> Result<Item> {
    let holed = |p: &Part| matches!(&p.piece, Piece::Region(r) if !r.holes.is_empty());
    if !item.shape.parts.iter().any(holed) {
        return Ok(item);
    }
    let mut parts = Vec::with_capacity(item.shape.parts.len());
    for p in std::mem::take(&mut item.shape.parts) {
        match &p.piece {
            Piece::Region(r) if !r.holes.is_empty() => {
                let pieces = fracture(r).ok_or_else(|| {
                    Error::new(
                        Code::Internal,
                        format!("a region with curved holes on {layer} can't be written to Gerber"),
                    )
                    .with("layer", layer)
                })?;
                parts.extend(pieces.into_iter().map(|r| Part {
                    polarity: p.polarity,
                    piece: Piece::Region(r),
                }));
            }
            _ => parts.push(p),
        }
    }
    item.shape.parts = parts;
    Ok(item)
}

/// The package's Gerber layers, in [`LAYERS`]' order; every one, even
/// when empty, so a package always has the full set.
pub fn layers(board: &str, s: &BoardShapes) -> Result<Vec<LayerFile>> {
    LAYERS
        .iter()
        .map(|&(function, ext, name, x2, negative)| {
            let items = match name {
                "copper.top" => copper(board, s, true),
                "copper.bottom" => copper(board, s, false),
                "outline" => vec![Item {
                    shape: super::render::outline_drawn(s),
                    function: Some("Profile".into()),
                    ..Item::default()
                }],
                other => plain(s, other),
            };
            let items = items
                .into_iter()
                .map(|i| hole_free(i, function))
                .collect::<Result<Vec<_>>>()?;
            Ok(LayerFile {
                function,
                ext,
                layer: Layer {
                    function: x2.to_owned(),
                    negative,
                    items,
                },
            })
        })
        .collect()
}

/// The plated and the unplated drills, each with its holes' functions.
pub fn drills(s: &BoardShapes) -> [Drills; 2] {
    let mut plated = Drills {
        function: "Plated,1,2,PTH".into(),
        holes: Vec::new(),
    };
    let mut unplated = Drills {
        function: "NonPlated,1,2,NPTH".into(),
        holes: Vec::new(),
    };
    for h in &s.holes {
        let drill = match &h.shape {
            HoleShape::Round { at, d } => Drill::Round { at: *at, d: *d },
            HoleShape::Slot { a, b, d } => Drill::Slot {
                a: *a,
                b: *b,
                d: *d,
            },
        };
        let function = match (&h.of, h.plated) {
            (HoleOf::Via(_), _) => "Plated,PTH,ViaDrill",
            (HoleOf::Pad { .. }, true) => "Plated,PTH,ComponentDrill",
            (HoleOf::Pad { .. }, false) => "NonPlated,NPTH,ComponentDrill",
            (HoleOf::Hole(_), _) => "NonPlated,NPTH,MechanicalDrill",
        };
        let hole = Hole {
            drill,
            function: Some(function.to_owned()),
        };
        if h.plated {
            plated.holes.push(hole);
        } else {
            unplated.holes.push(hole);
        }
    }
    [plated, unplated]
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::board::shapes::{build, tests::a_net, tests::placed_with};

    fn routed() -> BoardShapes {
        let (_d, p0, _) = crate::board::shapes::tests::placed();
        let net = a_net(&p0);
        let extra = vec![
            (
                "boards/b/tracks/t1.json".to_owned(),
                format!(
                    r#"{{"format":1,"kind":"ryueda.track","layer":"bottom","net":"{net}","points":[[1000000,1000000],[9000000,1000000]],"width_nm":250000}}"#
                ),
            ),
            (
                "boards/b/vias/v1.json".to_owned(),
                format!(
                    r#"{{"at":[9000000,1000000],"diameter_nm":600000,"drill_nm":300000,"format":1,"kind":"ryueda.via","net":"{net}"}}"#
                ),
            ),
            (
                "boards/b/holes/h1.json".to_owned(),
                r#"{"at":[40000000,40000000],"drill_nm":3200000,"format":1,"kind":"ryueda.hole"}"#
                    .to_owned(),
            ),
        ];
        let (_dir, p, id) = placed_with(&extra);
        build(&p, &p.boards[&id]).unwrap()
    }

    #[test]
    fn copper_carries_pads_tracks_and_vias_with_their_attributes() {
        let s = routed();
        let files = layers("b", &s).unwrap();
        let names: Vec<&str> = files.iter().map(|f| f.function).collect();
        assert_eq!(
            names,
            [
                "copper-top",
                "copper-bottom",
                "mask-top",
                "mask-bottom",
                "paste-top",
                "paste-bottom",
                "silk-top",
                "silk-bottom",
                "profile"
            ]
        );
        let top = &files[0].layer;
        assert_eq!(top.function, "Copper,L1,Top");
        let pads_on_top = s.pads.iter().filter(|p| p.top).count();
        assert_eq!(top.items.len(), pads_on_top + s.vias.len());
        assert!(
            top.items
                .iter()
                .take(pads_on_top)
                .all(|i| i.pad.is_some() && i.component.is_some())
        );
        let bottom = &files[1].layer;
        assert!(
            bottom
                .items
                .iter()
                .any(|i| i.function.as_deref() == Some("Conductor"))
        );
        assert!(
            bottom
                .items
                .iter()
                .any(|i| i.function.as_deref() == Some("ViaPad"))
        );
        assert!(files[2].layer.negative && !files[0].layer.negative);
        assert_eq!(files[8].layer.items[0].function.as_deref(), Some("Profile"));
        // Every layer writes.
        let sw = ryu_gerber::Software {
            vendor: "v".into(),
            application: "a".into(),
            version: "0".into(),
        };
        for f in &files {
            ryu_gerber::gerber::write(&f.layer, &sw)
                .unwrap_or_else(|e| panic!("{}: {e}", f.function));
        }
    }

    #[test]
    fn drills_split_by_plating_with_their_functions() {
        let s = routed();
        let [plated, unplated] = drills(&s);
        assert_eq!(plated.function, "Plated,1,2,PTH");
        assert_eq!(unplated.function, "NonPlated,1,2,NPTH");
        assert!(
            plated
                .holes
                .iter()
                .any(|h| h.function.as_deref() == Some("Plated,PTH,ViaDrill"))
        );
        assert_eq!(
            unplated
                .holes
                .iter()
                .filter(|h| h.function.as_deref() == Some("NonPlated,NPTH,MechanicalDrill"))
                .count(),
            1
        );
        assert_eq!(plated.holes.len() + unplated.holes.len(), s.holes.len());
    }

    #[test]
    fn a_region_with_a_hole_is_cut_into_pieces_without_one() {
        let sq = |x0: i64, y0: i64, x1: i64, y1: i64| {
            ryu_board::shape::polygon(&[
                Point::new(x0, y0),
                Point::new(x1, y0),
                Point::new(x1, y1),
                Point::new(x0, y1),
            ])
            .unwrap()
        };
        let r = Region {
            outer: sq(0, 0, 1000, 1000),
            holes: vec![sq(300, 300, 700, 700).reversed()],
        };
        let pieces = fracture(&r).unwrap();
        assert!(pieces.len() >= 2);
        assert!(pieces.iter().all(|p| p.holes.is_empty()));
        let area: i128 = pieces
            .iter()
            .map(|p| p.outer.polygon_area2().unwrap().abs())
            .sum();
        assert_eq!(area, 2 * (1000 * 1000 - 400 * 400));
        // Arcs aren't cut.
        let ring = Region::ring(
            ryu_board::shape::circle(Point::new(0, 0), 1000).unwrap(),
            ryu_board::shape::circle(Point::new(0, 0), 500).unwrap(),
        );
        assert!(fracture(&ring).is_none());
    }
}
