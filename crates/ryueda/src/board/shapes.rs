//! Every shape of a board, built once per revision in parallel (one task
//! per placement), the same bits on any thread count: each placement's
//! pads (copper per side, mask, paste, drills), graphics and text on
//! their layers, and the outline as a region and its boundary.

use super::file::Board;
use super::load::BoardFiles;
use super::occ::{self, Occ};
use super::{graphic, opening, pad, preset, text};
use crate::error::{Code, Error, Result};
use crate::fault::Fault;
use crate::footprint::{Footprint, Shape as Drawn};
use crate::project::Project;
use rayon::prelude::*;
use ryu_board::geom::{Mdeg, Place, Point, Region};
use ryu_board::model::Polarity;
use ryu_board::shape::{Part, Piece, Shape};
use ryu_copper::layout::HoleShape;
use serde_json::{Value, json};
use std::collections::BTreeMap;

/// One pad as an agent routes by it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PadInfo {
    /// `<board>:<refdes>/<number>` (see the plan's decisions).
    pub reference: String,
    pub occurrence: String,
    pub number: Option<String>,
    pub at: Point,
    pub rotation_mdeg: i64,
    pub size: [i64; 2],
    pub top: bool,
    pub bottom: bool,
    pub net: Option<String>,
    pub copper: Shape,
}

/// A drilled hole and what owns it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct HoleInfo {
    pub owner: String,
    pub shape: HoleShape,
    pub plated: bool,
}

/// Everything a board draws.
#[derive(Clone, Debug, Default)]
pub struct BoardShapes {
    /// By layer (`copper.top`, `mask.top`, `paste.top`, `silk.top`,
    /// `fab.top`, `courtyard.top`, the same `.bottom`, and the
    /// footprints' other layers by their names), each with its owner.
    pub layers: BTreeMap<String, Vec<(String, Shape)>>,
    pub holes: Vec<HoleInfo>,
    pub pads: Vec<PadInfo>,
    /// The outline less its cutouts.
    pub outline: Shape,
    /// The outline's and cutouts' boundaries, zero width.
    pub edges: Shape,
    /// What couldn't be built, and characters the font lacks.
    pub notices: Vec<Fault>,
}

/// The outline region (cutouts as holes) and its boundary.
fn outline(b: &Board) -> (Shape, Shape) {
    let Ok(outer) = super::outline::contour(&b.outline) else {
        return (Shape::default(), Shape::default());
    };
    let outer = super::outline::ccw(outer);
    let cuts: Vec<_> = b
        .cutouts
        .iter()
        .filter_map(|c| super::outline::contour(c).ok())
        .map(super::outline::ccw)
        .collect();
    let mut edges = Shape::default();
    for c in std::iter::once(&outer).chain(&cuts) {
        edges.parts.push(Part {
            polarity: Polarity::Positive,
            piece: Piece::Stroke(ryu_board::geom::Stroke {
                path: c.edges().to_vec(),
                pen: ryu_board::geom::Pen::Round(0),
            }),
        });
    }
    let region = Region {
        outer,
        holes: cuts.into_iter().map(|c| c.reversed()).collect(),
    };
    (Shape::regions(Polarity::Positive, [region]), edges)
}

/// A sided layer name for a footprint layer on a top-side placement.
fn layer_of(l: &str) -> String {
    l.to_owned()
}

/// Pad references within one footprint: the number when it is unique,
/// `<number>@<id>` when it repeats, `@<id>` with none.
fn pad_names(f: &Footprint) -> BTreeMap<String, String> {
    let mut count: BTreeMap<&str, usize> = BTreeMap::new();
    for p in f.pads.values() {
        if let Some(n) = &p.number {
            *count.entry(n).or_default() += 1;
        }
    }
    f.pads
        .iter()
        .map(|(id, p)| {
            let name = match &p.number {
                Some(n) if count[n.as_str()] == 1 => n.clone(),
                Some(n) => format!("{n}@{id}"),
                None => format!("@{id}"),
            };
            (id.to_string(), name)
        })
        .collect()
}

/// What one placement adds.
#[derive(Default)]
struct One {
    layers: Vec<(String, String, Shape)>,
    holes: Vec<HoleInfo>,
    pads: Vec<PadInfo>,
    notices: Vec<Fault>,
}

fn unsupported(o: &Occ, why: &str) -> Fault {
    Fault::error(
        Code::BoardFootprintUnsupported,
        format!("{}'s footprint {why}; stage 5 can't build it", o.label()),
    )
    .with("occurrence", &o.reference)
}

fn one(board: &str, o: &Occ, f: &Footprint, place: Place, preset: &preset::Preset) -> One {
    let mut out = One::default();
    if f.graphics.values().any(|g| g.layer == "edge") {
        out.notices.push(unsupported(o, "draws on the edge layer"));
        return out;
    }
    if f.pads.values().any(|p| pad::copper_sides(p).2) {
        out.notices
            .push(unsupported(o, "has a pad on copper.inner"));
        return out;
    }
    let owner = format!("{board}:{}", o.label());
    let names = pad_names(f);
    let put = |out: &mut One, layer: &str, s: Shape| {
        if !s.parts.is_empty() {
            out.layers.push((
                layer.to_owned(),
                owner.clone(),
                s.placed(&place, Polarity::Positive),
            ));
        }
    };
    for (id, p) in &f.pads {
        let Ok(copper) = pad::copper(p) else {
            out.notices.push(unsupported(
                o,
                &format!("has a pad ({id}) whose shape can't be built"),
            ));
            continue;
        };
        let (top, bottom, _) = pad::copper_sides(p);
        if top {
            put(&mut out, "copper.top", copper.clone());
        }
        if bottom {
            put(&mut out, "copper.bottom", copper.clone());
        }
        for side in ["top", "bottom"] {
            if p.layers.iter().any(|l| l == &format!("mask.{side}"))
                && let Ok(m) = opening::mask(f, p, preset)
            {
                put(&mut out, &format!("mask.{side}"), m);
            }
            if p.layers.iter().any(|l| l == &format!("paste.{side}"))
                && let Ok(m) = opening::paste(f, p, preset)
            {
                put(&mut out, &format!("paste.{side}"), m);
            }
        }
        if let Some((h, plated)) = pad::hole(p) {
            let shape = match h {
                HoleShape::Round { at, d } => HoleShape::Round {
                    at: place.point(at),
                    d,
                },
                HoleShape::Slot { a, b, d } => HoleShape::Slot {
                    a: place.point(a),
                    b: place.point(b),
                    d,
                },
            };
            out.holes.push(HoleInfo {
                owner: format!("{owner}/{}", names[&id.to_string()]),
                shape,
                plated,
            });
        }
        let at = place.point(Point::new(p.at[0], p.at[1]));
        out.pads.push(PadInfo {
            reference: format!("{owner}/{}", names[&id.to_string()]),
            occurrence: o.reference.clone(),
            number: p.number.clone(),
            at,
            rotation_mdeg: i64::from(Mdeg::new(p.rotation_mdeg + i64::from(place.rot.get())).get()),
            size: p.size,
            top,
            bottom,
            net: p.number.as_ref().and_then(|n| o.nets.get(n)).cloned(),
            copper: copper.placed(&place, Polarity::Positive),
        });
    }
    let refdes = o.label().to_owned();
    let value = o.value.clone().unwrap_or_default();
    for g in f.graphics.values() {
        let s = match &g.shape {
            Drawn::Text {
                text: t,
                at,
                rotation_mdeg,
                size_nm,
                thickness_nm,
                hidden,
            } => {
                if *hidden {
                    continue;
                }
                match text::centred(
                    &text::substitute(t, &refdes, &value),
                    *at,
                    *rotation_mdeg,
                    *size_nm,
                    *thickness_nm,
                ) {
                    Ok(s) => s,
                    Err(_) => continue,
                }
            }
            other => graphic::drawn(other),
        };
        note_missing(&mut out.notices, o, &s);
        put(&mut out, &layer_of(&g.layer), s);
    }
    for (field, string) in [
        (&f.fields.reference, refdes.as_str()),
        (&f.fields.value, value.as_str()),
    ] {
        let Some(fl) = field else { continue };
        if fl.hidden || string.is_empty() {
            continue;
        }
        if let Ok(s) = text::centred(string, fl.at, fl.rotation_mdeg, fl.size_nm, fl.thickness_nm) {
            note_missing(&mut out.notices, o, &s);
            put(&mut out, &layer_of(&fl.layer), s);
        }
    }
    out
}

fn note_missing(notices: &mut Vec<Fault>, o: &Occ, s: &Shape) {
    if !s.missing.is_empty() {
        notices.push(
            Fault::warning(
                Code::BoardTextMissingGlyph,
                format!(
                    "{}'s text has characters the stroke font lacks: {}",
                    o.label(),
                    s.missing.iter().collect::<String>()
                ),
            )
            .with("occurrence", &o.reference),
        );
    }
}

/// The board's shapes. A design that doesn't flatten refuses with its
/// faults; a board file that doesn't read refuses with its error.
pub fn build(p: &Project, b: &BoardFiles) -> Result<BoardShapes> {
    let file = b.board.item.as_ref().map_err(Clone::clone)?;
    let preset = preset::by_name(&file.preset).ok_or_else(|| {
        Error::new(
            Code::InvalidDocument,
            format!("preset {} isn't known", file.preset),
        )
    })?;
    let (_, occs) = occ::occurrences(p, &file.design)?;
    let board = b.id.to_string();
    let placed: Vec<(&Occ, Place)> = occs
        .iter()
        .filter_map(|o| {
            let pl = b.placements.get(&o.reference)?.item.as_ref().ok()?;
            Some((
                o,
                Place {
                    at: Point::new(pl.at[0], pl.at[1]),
                    rot: Mdeg::new(pl.rotation_mdeg),
                    mirror: false,
                    scale: None,
                },
            ))
        })
        .collect();
    let mut notices = Vec::new();
    let parts: Vec<One> = placed
        .par_iter()
        .map(|(o, place)| match (&o.footprint, &o.footprint_error) {
            (Some(f), _) => one(&board, o, f, *place, preset),
            (None, e) => One {
                notices: vec![unsupported(
                    o,
                    &format!(
                        "can't be read ({})",
                        e.as_ref()
                            .map(|e| e.message.as_str())
                            .unwrap_or("none chosen")
                    ),
                )],
                ..One::default()
            },
        })
        .collect();
    let mut out = BoardShapes::default();
    for one in parts {
        for (layer, owner, s) in one.layers {
            out.layers.entry(layer).or_default().push((owner, s));
        }
        out.holes.extend(one.holes);
        out.pads.extend(one.pads);
        notices.extend(one.notices);
    }
    let (region, edges) = outline(file);
    out.outline = region;
    out.edges = edges;
    out.notices = notices;
    Ok(out)
}

/// `board show --pads`' list: each pad's reference, place, size, copper
/// layers and net.
pub fn pads_json(s: &BoardShapes) -> Value {
    let pads: Vec<Value> = s
        .pads
        .iter()
        .map(|pd| {
            let layers: Vec<&str> = [
                pd.top.then_some("copper.top"),
                pd.bottom.then_some("copper.bottom"),
            ]
            .into_iter()
            .flatten()
            .collect();
            json!({
                "reference": pd.reference,
                "occurrence": pd.occurrence,
                "number": pd.number,
                "at": [pd.at.x, pd.at.y],
                "rotation_mdeg": pd.rotation_mdeg,
                "size": pd.size,
                "layers": layers,
                "net": pd.net,
            })
        })
        .collect();
    json!(pads)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The netlist fixture with a board placing every occurrence on a
    /// 5 mm grid.
    fn placed() -> (tempfile::TempDir, Project, crate::id::Id) {
        let (dir, p) = crate::netlist::tests::project(&[]);
        let design = crate::netlist::one_design(&p, None).unwrap();
        let (_, occs) = occ::occurrences(&p, &design).unwrap();
        let mut files: BTreeMap<String, Vec<u8>> = BTreeMap::new();
        let board = super::super::file::Board {
            name: "B".into(),
            design: design.clone(),
            preset: "jlc-2l".into(),
            outline: super::super::file::Contour::rect(50_000_000, 50_000_000),
            cutouts: vec![],
        };
        files.insert(
            "boards/b/board.json".into(),
            crate::doc::to_string(&board, super::super::BOARD_KIND, 1).into_bytes(),
        );
        for (i, o) in occs.iter().filter(|o| o.footprint.is_some()).enumerate() {
            let pl = super::super::file::Placement {
                at: [5_000_000 * (i as i64 + 1), 10_000_000],
                rotation_mdeg: 90_000 * i as i64,
            };
            files.insert(
                format!(
                    "boards/b/placements/{}.json",
                    super::super::file::placement_name(&o.reference)
                ),
                crate::doc::to_string(&pl, super::super::PLACEMENT_KIND, 1).into_bytes(),
            );
        }
        let (boards, _) = super::super::load::load(&files).unwrap();
        let mut p = p;
        p.boards = boards;
        (dir, p, crate::id::Id::parse("b", "board").unwrap())
    }

    #[test]
    fn a_placed_board_builds_pads_text_and_its_outline() {
        let (_dir, p, id) = placed();
        let s = build(&p, &p.boards[&id]).unwrap();
        assert!(!s.pads.is_empty());
        assert!(s.layers["copper.top"].len() >= s.pads.len());
        assert!(s.layers.contains_key("mask.top"));
        assert!(
            s.layers.get("silk.top").is_some_and(|l| !l.is_empty()),
            "designators on silk"
        );
        assert!(
            s.notices
                .iter()
                .all(|n| n.error.code != Code::BoardFootprintUnsupported),
            "{:?}",
            s.notices
        );
        let b = s.outline.bounds().unwrap();
        assert_eq!((b.max.x, b.max.y), (50_000_000, 50_000_000));
        assert!(s.pads.iter().any(|p| p.net.is_some()));
    }

    #[test]
    fn shapes_are_the_same_on_one_thread_and_eight() {
        let (_dir, p, id) = placed();
        let run = |n| {
            rayon::ThreadPoolBuilder::new()
                .num_threads(n)
                .build()
                .unwrap()
                .install(|| {
                    let s = build(&p, &p.boards[&id]).unwrap();
                    format!("{:?}{:?}{:?}", s.layers, s.holes, s.pads)
                })
        };
        assert_eq!(run(1), run(8));
    }

    #[test]
    fn edge_graphics_are_unsupported_not_dropped() {
        let (_dir, mut p, id) = placed();
        // Give every footprint an edge graphic.
        for lib in p.libraries.values_mut().flatten() {
            for f in lib.footprints.values_mut() {
                if let Ok(f) = f.item.as_mut()
                    && let Some(g) = f.graphics.values().next().cloned()
                {
                    f.graphics.insert(
                        crate::id::Id::parse("edge1", "g").unwrap(),
                        crate::footprint::Graphic {
                            layer: "edge".into(),
                            ..g
                        },
                    );
                }
            }
        }
        let s = build(&p, &p.boards[&id]).unwrap();
        assert!(
            s.notices
                .iter()
                .any(|n| n.error.code == Code::BoardFootprintUnsupported)
        );
    }
}
