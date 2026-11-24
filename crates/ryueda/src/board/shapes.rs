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
use crate::footprint::{Field, Footprint, Shape as Drawn};
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
    /// Its name in the footprint, as its reference ends (`1`, `1@p3`,
    /// `@p3`).
    pub pad: String,
    pub at: Point,
    pub rotation_mdeg: i64,
    pub size: [i64; 2],
    pub top: bool,
    pub bottom: bool,
    /// On every inner layer.
    pub inner: bool,
    pub net: Option<String>,
    pub copper: Shape,
    /// Its solder mask openings on the top and bottom, placed.
    pub mask: [Option<Shape>; 2],
    /// The footprint pad's own pour connection, when it sets one.
    pub connect: Option<crate::footprint::ZoneConnect>,
    /// The footprint pad's own thermal: (gap, spoke).
    pub thermal: Option<(i64, i64)>,
}

/// A drilled hole and what owns it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct HoleInfo {
    pub owner: String,
    pub shape: HoleShape,
    pub plated: bool,
    pub of: HoleOf,
    /// The copper layers it passes through, by index: the whole board
    /// but for a blind or buried via's.
    pub span: (usize, usize),
}

/// What a drill belongs to.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum HoleOf {
    Pad { occ: String, pad: String },
    Via(crate::id::Id),
    Hole(crate::id::Id),
}

/// A placement's courtyard, placed.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CourtyardInfo {
    /// `<board>:<refdes>`.
    pub owner: String,
    pub occurrence: String,
    pub shape: Shape,
    /// Its graphics didn't close: this is their box.
    pub open: bool,
    /// The part is on the bottom.
    pub bottom: bool,
}

/// A keepout as placed: the board's own, or a part's.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct KeepoutInfo {
    /// `<board>:keepout/<id>`, or `<board>:<label>#keepout/<id>`.
    pub reference: String,
    /// The part whose footprint holds it (its occurrence): its own pads
    /// and drills are exempt.
    pub part: Option<String>,
    /// Copper layers it covers, by index, sorted.
    pub layers: Vec<usize>,
    pub keeps: Vec<crate::footprint::Keep>,
    pub shape: Shape,
}

/// A track as drawn.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TrackInfo {
    pub id: crate::id::Id,
    pub net: String,
    pub layer: super::file::CopperLayer,
    pub width_nm: i64,
    pub shape: Shape,
}

/// A via as drawn: a disc on every copper layer of its span and a plated
/// drill.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ViaInfo {
    pub id: crate::id::Id,
    /// Its span's first and last copper layers, by index.
    pub from: usize,
    pub to: usize,
    pub net: String,
    pub at: Point,
    pub drill_nm: i64,
    pub diameter_nm: i64,
    pub shape: Shape,
}

/// A track's copper: its polyline, repeated points skipped, drawn with a
/// round pen of its width.
pub fn track_shape(t: &super::file::Track) -> Shape {
    let pts: Vec<Point> = t.points.iter().map(|p| Point::new(p[0], p[1])).collect();
    let path: Vec<ryu_board::geom::Edge> = pts
        .windows(2)
        .filter(|w| w[0] != w[1])
        .map(|w| ryu_board::geom::Edge::line(w[0], w[1]))
        .collect();
    Shape {
        parts: vec![Part {
            polarity: Polarity::Positive,
            piece: Piece::Stroke(ryu_board::geom::Stroke {
                path,
                pen: ryu_board::geom::Pen::Round(t.width_nm),
            }),
        }],
        ..Shape::default()
    }
}

/// A disc of diameter `d` at `at`; empty when `d` isn't positive.
pub fn disc(at: Point, d: i64) -> Shape {
    ryu_board::shape::circle(at, ryu_board::geom::half(d))
        .map(|c| Shape::regions(Polarity::Positive, [Region::solid(c)]))
        .unwrap_or_default()
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
    pub tracks: Vec<TrackInfo>,
    pub vias: Vec<ViaInfo>,
    pub courtyards: Vec<CourtyardInfo>,
    /// The board's keepouts and its parts' (ryueda 7.C).
    pub keepouts: Vec<KeepoutInfo>,
    /// The outline less its cutouts.
    pub outline: Shape,
    /// The outline's and cutouts' boundaries, zero width.
    pub edges: Shape,
    /// What couldn't be built, and characters the font lacks.
    pub notices: Vec<Fault>,
    /// How many copper layers the board has; 0 in shapes made by hand,
    /// read through [`BoardShapes::n`].
    pub copper_layers: usize,
}

impl BoardShapes {
    /// The board's copper layer count (two when unset).
    pub fn n(&self) -> usize {
        self.copper_layers.max(2)
    }
}

impl PadInfo {
    /// Whether the pad has copper on layer `l` of `n`.
    pub fn on(&self, l: usize, n: usize) -> bool {
        if l == 0 {
            self.top
        } else if l + 1 >= n {
            self.bottom
        } else {
            self.inner
        }
    }
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

/// A footprint's sided layer on the board: as it is for a top part,
/// swapped (`silk.top` to `silk.bottom`, and back) for a bottom one.
fn sided(l: &str, flipped: bool) -> String {
    if !flipped {
        return l.to_owned();
    }
    match l.rsplit_once('.') {
        Some((kind, "top")) => format!("{kind}.bottom"),
        Some((kind, "bottom")) => format!("{kind}.top"),
        _ => l.to_owned(),
    }
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

/// A footprint's field with a placement's overrides, key by key. A field
/// the footprint lacks comes from the override alone (silk.top, 1 mm,
/// 0.15 mm thick) when it says where.
fn field_with(f: Option<&Field>, over: Option<&super::file::FieldPlace>) -> Option<Field> {
    let mut out = match (f, over) {
        (Some(f), _) => f.clone(),
        (None, Some(o)) if o.at.is_some() => Field {
            layer: "silk.top".into(),
            at: [0, 0],
            rotation_mdeg: 0,
            size_nm: [1_000_000, 1_000_000],
            thickness_nm: 150_000,
            hidden: false,
        },
        _ => return None,
    };
    if let Some(o) = over {
        if let Some(a) = o.at {
            out.at = a;
        }
        if let Some(r) = o.rotation_mdeg {
            out.rotation_mdeg = r;
        }
        if let Some(h) = o.hidden {
            out.hidden = h;
        }
        if let Some(s) = o.size_nm {
            out.size_nm = s;
        }
    }
    Some(out)
}

/// What one placement adds.
#[derive(Default)]
struct One {
    layers: Vec<(String, String, Shape)>,
    holes: Vec<HoleInfo>,
    pads: Vec<PadInfo>,
    courtyards: Vec<CourtyardInfo>,
    keepouts: Vec<KeepoutInfo>,
    notices: Vec<Fault>,
}

fn unsupported(o: &Occ, why: &str) -> Fault {
    Fault::error(
        Code::BoardFootprintUnsupported,
        format!("{}'s footprint {why}; ryueda can't build it yet", o.label()),
    )
    .with("occurrence", &o.reference)
}

fn one(
    board: &str,
    o: &Occ,
    f: &Footprint,
    place: Place,
    preset: &preset::Preset,
    n: usize,
    fields: &super::file::PartFields,
) -> One {
    let mut out = One::default();
    if f.graphics.values().any(|g| g.layer == "edge") {
        out.notices.push(unsupported(o, "draws on the edge layer"));
        return out;
    }
    // A part is mirrored exactly when it is on the bottom.
    let flipped = place.mirror;
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
        let (t, b, inner) = pad::copper_sides(p);
        let (top, bottom) = if flipped { (b, t) } else { (t, b) };
        for (l, name) in super::stackup::names(n).iter().enumerate() {
            let on = if l == 0 {
                top
            } else if l + 1 == n {
                bottom
            } else {
                inner
            };
            if on {
                put(&mut out, name, copper.clone());
            }
        }
        let mut mask: [Option<Shape>; 2] = [None, None];
        for (i, side) in ["top", "bottom"].into_iter().enumerate() {
            // `i` is the footprint's side; the mask's index the board's.
            let on_board = if flipped { 1 - i } else { i };
            if p.layers.iter().any(|l| l == &format!("mask.{side}"))
                && let Ok(m) = opening::mask(f, p, preset)
            {
                mask[on_board] = Some(m.placed(&place, Polarity::Positive));
                put(&mut out, &sided(&format!("mask.{side}"), flipped), m);
            }
            if p.layers.iter().any(|l| l == &format!("paste.{side}"))
                && let Ok(m) = opening::paste(f, p, preset)
            {
                put(&mut out, &sided(&format!("paste.{side}"), flipped), m);
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
                of: HoleOf::Pad {
                    occ: o.reference.clone(),
                    pad: names[&id.to_string()].clone(),
                },
                span: (0, n - 1),
            });
        }
        let at = place.point(Point::new(p.at[0], p.at[1]));
        out.pads.push(PadInfo {
            reference: format!("{owner}/{}", names[&id.to_string()]),
            occurrence: o.reference.clone(),
            number: p.number.clone(),
            pad: names[&id.to_string()].clone(),
            at,
            rotation_mdeg: i64::from(
                Mdeg::new(if flipped {
                    i64::from(place.rot.get()) - p.rotation_mdeg
                } else {
                    p.rotation_mdeg + i64::from(place.rot.get())
                })
                .get(),
            ),
            size: p.size,
            top,
            bottom,
            inner,
            net: p.number.as_ref().and_then(|n| o.nets.get(n)).cloned(),
            copper: copper.placed(&place, Polarity::Positive),
            mask,
            connect: p.margins.zone_connect,
            thermal: p.thermal.as_ref().map(|t| (t.gap_nm, t.spoke_nm)),
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
        put(&mut out, &sided(&g.layer, flipped), s);
    }
    for (field, over, string) in [
        (
            f.fields.reference.as_ref(),
            fields.reference.as_ref(),
            refdes.as_str(),
        ),
        (
            f.fields.value.as_ref(),
            fields.value.as_ref(),
            value.as_str(),
        ),
    ] {
        let Some(fl) = field_with(field, over) else {
            continue;
        };
        if fl.hidden || string.is_empty() {
            continue;
        }
        if let Ok(s) = text::centred(string, fl.at, fl.rotation_mdeg, fl.size_nm, fl.thickness_nm) {
            note_missing(&mut out.notices, o, &s);
            put(&mut out, &sided(&fl.layer, flipped), s);
        }
    }
    let court: Vec<&crate::footprint::Graphic> = f
        .graphics
        .values()
        .filter(|g| g.layer == "courtyard.top")
        .collect();
    if let Some((shape, open)) = super::courtyard::of_graphics(&court) {
        out.courtyards.push(CourtyardInfo {
            owner: owner.clone(),
            occurrence: o.reference.clone(),
            shape: shape.placed(&place, Polarity::Positive),
            open,
            bottom: flipped,
        });
    }
    for (kid, k) in &f.keepouts {
        let (layers, shape) = super::keepout::placed_footprint_keepout(k, place, n);
        if !shape.parts.is_empty() {
            out.keepouts.push(KeepoutInfo {
                reference: format!("{owner}#keepout/{kid}"),
                part: Some(o.reference.clone()),
                layers,
                keeps: k.keeps.clone(),
                shape,
            });
        }
    }
    out
}

/// What one part would draw at `place`: its pads and its courtyard (the
/// legaliser's candidates).
pub(crate) fn part(
    board: &str,
    o: &Occ,
    f: &Footprint,
    place: Place,
    preset: &preset::Preset,
    n: usize,
    fields: &super::file::PartFields,
) -> (Vec<PadInfo>, Option<CourtyardInfo>) {
    let one = one(board, o, f, place, preset, n, fields);
    (one.pads, one.courtyards.into_iter().next())
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
    let preset = preset::resolve(p, &file.preset).ok_or_else(|| {
        Error::new(
            Code::InvalidDocument,
            format!("preset {} isn't known", file.preset),
        )
    })?;
    let (_, occs) = occ::occurrences(p, &file.design)?;
    let board = b.id.to_string();
    let n = super::stackup::copper_count(file);
    let placed: Vec<(&Occ, Place, &super::file::PartFields)> = occs
        .iter()
        .filter_map(|o| {
            let pl = b.placements.get(&o.reference)?.item.as_ref().ok()?;
            Some((
                o,
                Place {
                    at: Point::new(pl.at[0], pl.at[1]),
                    rot: Mdeg::new(pl.rotation_mdeg),
                    mirror: pl.side == super::file::PlacementSide::Bottom,
                    scale: None,
                },
                &pl.fields,
            ))
        })
        .collect();
    let mut notices = Vec::new();
    let parts: Vec<One> = placed
        .par_iter()
        .map(
            |(o, place, fields)| match (&o.footprint, &o.footprint_error) {
                (Some(f), _) => one(&board, o, f, *place, &preset, n, fields),
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
            },
        )
        .collect();
    let mut out = BoardShapes {
        copper_layers: n,
        ..BoardShapes::default()
    };
    for one in parts {
        for (layer, owner, s) in one.layers {
            out.layers.entry(layer).or_default().push((owner, s));
        }
        out.holes.extend(one.holes);
        out.pads.extend(one.pads);
        out.courtyards.extend(one.courtyards);
        out.keepouts.extend(one.keepouts);
        notices.extend(one.notices);
    }
    // The routes; files with an error fault are left out (`check`
    // reports them).
    let fine = |faults: &[Fault]| faults.iter().all(|f| !f.is_error());
    for (tid, t) in &b.tracks {
        let (Ok(track), true) = (&t.item, fine(&t.faults)) else {
            continue;
        };
        if track.layer.index(n).is_none() {
            continue;
        }
        let shape = track_shape(track);
        out.layers
            .entry(track.layer.name())
            .or_default()
            .push((format!("{board}:track/{tid}"), shape.clone()));
        out.tracks.push(TrackInfo {
            id: tid.clone(),
            net: track.net.clone(),
            layer: track.layer,
            width_nm: track.width_nm,
            shape,
        });
    }
    for (vid, v) in &b.vias {
        let (Ok(via), true) = (&v.item, fine(&v.faults)) else {
            continue;
        };
        let Some((from, to)) = via.span(n) else {
            continue;
        };
        let at = Point::new(via.at[0], via.at[1]);
        let shape = disc(at, via.diameter_nm);
        for layer in &super::stackup::names(n)[from..=to] {
            out.layers
                .entry(layer.clone())
                .or_default()
                .push((format!("{board}:via/{vid}"), shape.clone()));
        }
        out.holes.push(HoleInfo {
            owner: format!("{board}:via/{vid}"),
            shape: HoleShape::Round {
                at,
                d: via.drill_nm,
            },
            plated: true,
            of: HoleOf::Via(vid.clone()),
            span: (from, to),
        });
        out.vias.push(ViaInfo {
            id: vid.clone(),
            from,
            to,
            net: via.net.clone(),
            at,
            drill_nm: via.drill_nm,
            diameter_nm: via.diameter_nm,
            shape,
        });
    }
    for (hid, h) in &b.holes {
        let (Ok(hole), true) = (&h.item, fine(&h.faults)) else {
            continue;
        };
        out.holes.push(HoleInfo {
            owner: format!("{board}:hole/{hid}"),
            shape: HoleShape::Round {
                at: Point::new(hole.at[0], hole.at[1]),
                d: hole.drill_nm,
            },
            plated: false,
            of: HoleOf::Hole(hid.clone()),
            span: (0, n - 1),
        });
    }
    for (kid, k) in &b.keepouts {
        let (Ok(ko), true) = (&k.item, fine(&k.faults)) else {
            continue;
        };
        let Ok(c) = super::outline::contour(&ko.contour) else {
            continue;
        };
        out.keepouts.push(KeepoutInfo {
            reference: format!("{board}:keepout/{kid}"),
            part: None,
            layers: ko.indices(n),
            keeps: ko.keeps.clone(),
            shape: Shape::regions(Polarity::Positive, [Region::solid(super::outline::ccw(c))]),
        });
    }
    for (did, d) in &b.drawings {
        let (Ok(g), true) = (&d.item, fine(&d.faults)) else {
            continue;
        };
        let Some((shape, missing)) = super::drawing::shape(g) else {
            continue;
        };
        if !missing.is_empty() {
            notices.push(
                Fault::warning(
                    Code::BoardTextMissingGlyph,
                    format!(
                        "drawing {did}'s text has characters the stroke font lacks: {}",
                        missing.iter().collect::<String>()
                    ),
                )
                .with("drawing", did.as_str()),
            );
        }
        out.layers
            .entry(g.layer.clone())
            .or_default()
            .push((format!("{board}:drawing/{did}"), shape));
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
            let n = s.n();
            let layers: Vec<String> = super::stackup::names(n)
                .into_iter()
                .enumerate()
                .filter(|(l, _)| pd.on(*l, n))
                .map(|(_, name)| name)
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
pub(crate) mod tests {
    use super::*;

    /// The netlist fixture with a board placing every occurrence on a
    /// 5 mm grid.
    pub(crate) fn placed() -> (tempfile::TempDir, Project, crate::id::Id) {
        placed_with(&[])
    }

    /// [`placed`] with `extra` files (path, text) beside it.
    pub(crate) fn placed_with(
        extra: &[(String, String)],
    ) -> (tempfile::TempDir, Project, crate::id::Id) {
        placed_on(2, extra)
    }

    /// [`placed_with`] on a board of `layers` copper layers (an even
    /// stackup when more than two).
    pub(crate) fn placed_on(
        layers: usize,
        extra: &[(String, String)],
    ) -> (tempfile::TempDir, Project, crate::id::Id) {
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
            stackup: (layers > 2).then(|| {
                super::super::stackup::even(&super::super::preset::JLC_2L, layers).unwrap()
            }),
        };
        let format = if layers > 2 { 2 } else { 1 };
        files.insert(
            "boards/b/board.json".into(),
            crate::doc::to_string(&board, super::super::BOARD_KIND, format).into_bytes(),
        );
        for (i, o) in occs.iter().filter(|o| o.footprint.is_some()).enumerate() {
            let pl = super::super::file::Placement {
                at: [5_000_000 * (i as i64 + 1), 10_000_000],
                rotation_mdeg: 90_000 * i as i64,
                side: super::super::file::PlacementSide::Top,
                ..super::super::file::Placement::default()
            };
            files.insert(
                format!(
                    "boards/b/placements/{}.json",
                    super::super::file::placement_name(&o.reference)
                ),
                crate::doc::to_string(&pl, super::super::PLACEMENT_KIND, 1).into_bytes(),
            );
        }
        for (path, text) in extra {
            files.insert(path.clone(), text.clone().into_bytes());
        }
        let (boards, _) = super::super::load::load(&files).unwrap();
        let mut p = p;
        p.boards = boards;
        (dir, p, crate::id::Id::parse("b", "board").unwrap())
    }

    /// The first net any pad of the fixture's occurrences carries.
    pub(crate) fn a_net(p: &Project) -> String {
        let design = crate::netlist::one_design(p, None).unwrap();
        occ::occurrences(p, &design)
            .unwrap()
            .1
            .iter()
            .flat_map(|o| o.nets.values().cloned())
            .next()
            .unwrap()
    }

    #[test]
    fn four_layers_carry_tracks_vias_and_through_pads() {
        let (_d, p0, _) = placed();
        let net = a_net(&p0);
        let extra = vec![
            (
                "boards/b/tracks/t1.json".to_owned(),
                format!(
                    r#"{{"format":2,"kind":"ryueda.track","layer":"in2","net":"{net}","points":[[1000000,1000000],[9000000,1000000]],"width_nm":250000}}"#
                ),
            ),
            (
                "boards/b/vias/v1.json".to_owned(),
                format!(
                    r#"{{"at":[9000000,1000000],"diameter_nm":600000,"drill_nm":300000,"format":2,"kind":"ryueda.via","net":"{net}","to":"in2"}}"#
                ),
            ),
            (
                "boards/b/vias/v2.json".to_owned(),
                format!(
                    r#"{{"at":[20000000,1000000],"diameter_nm":600000,"drill_nm":300000,"format":1,"kind":"ryueda.via","net":"{net}"}}"#
                ),
            ),
        ];
        let (_dir, p, id) = placed_on(4, &extra);
        let s = build(&p, &p.boards[&id]).unwrap();
        assert_eq!(s.n(), 4);
        let on = |l: &str, owner: &str| {
            s.layers
                .get(l)
                .is_some_and(|v| v.iter().any(|(o, _)| o == owner))
        };
        assert!(on("copper.in2", "b:track/t1") && !on("copper.top", "b:track/t1"));
        for (l, blind, through) in [
            ("copper.top", true, true),
            ("copper.in1", true, true),
            ("copper.in2", true, true),
            ("copper.bottom", false, true),
        ] {
            assert_eq!(on(l, "b:via/v1"), blind, "{l}");
            assert_eq!(on(l, "b:via/v2"), through, "{l}");
        }
        let v1 = s.vias.iter().find(|v| v.id.as_str() == "v1").unwrap();
        assert_eq!((v1.from, v1.to), (0, 2));
        let h1 = s.holes.iter().find(|h| h.owner == "b:via/v1").unwrap();
        assert_eq!(h1.span, (0, 2));
        // Surface pads stay on their side; nothing is unsupported.
        assert!(
            s.pads
                .iter()
                .all(|pd| pd.top && !pd.inner && !pd.bottom || pd.top && pd.inner && pd.bottom)
        );
        assert!(s.notices.is_empty(), "{:?}", s.notices);
        // On two layers the same board has only its two copper layers.
        let (_d2, p2, id2) = placed();
        let s2 = build(&p2, &p2.boards[&id2]).unwrap();
        assert_eq!(s2.n(), 2);
        assert!(s2.layers.keys().all(|k| !k.starts_with("copper.in")));
    }

    #[test]
    fn a_bottom_part_is_its_top_twin_mirrored_with_its_layers_swapped() {
        let (_d, p_top, id) = placed();
        let top = build(&p_top, &p_top.boards[&id]).unwrap();
        let occ = top.pads[0].occurrence.clone();
        let at = p_top.boards[&id].placements[&occ].item.as_ref().unwrap().at;
        let path = format!(
            "boards/b/placements/{}.json",
            crate::board::file::placement_name(&occ)
        );
        let flipped = format!(
            r#"{{"at":[{},{}],"format":2,"kind":"ryueda.placement","rotation_mdeg":0,"side":"bottom"}}"#,
            at[0], at[1]
        );
        let (_d2, p_bot, id2) = placed_with(&[(path, flipped)]);
        let bot = build(&p_bot, &p_bot.boards[&id2]).unwrap();
        let pads = |s: &BoardShapes| -> Vec<PadInfo> {
            s.pads
                .iter()
                .filter(|x| x.occurrence == occ)
                .cloned()
                .collect()
        };
        let (t, b) = (pads(&top), pads(&bot));
        assert_eq!(t.len(), b.len());
        for (x, y) in t.iter().zip(&b) {
            // Mirrored about the part's own y axis: x reflected about its origin.
            assert_eq!(y.at.x - at[0], -(x.at.x - at[0]), "{}", x.reference);
            assert_eq!(y.at.y, x.at.y);
            assert_eq!((y.top, y.bottom), (x.bottom, x.top));
            assert_eq!(y.mask[1].is_some(), x.mask[0].is_some());
            assert_eq!(y.net, x.net);
        }
        let owner = &t[0].reference[..t[0].reference.rfind('/').unwrap()];
        let has = |s: &BoardShapes, l: &str| {
            s.layers
                .get(l)
                .is_some_and(|v| v.iter().any(|(o, _)| o == owner))
        };
        for kind in ["copper", "mask", "paste", "silk", "fab", "courtyard"] {
            if has(&top, &format!("{kind}.top")) {
                assert!(
                    has(&bot, &format!("{kind}.bottom")) && !has(&bot, &format!("{kind}.top")),
                    "{kind}"
                );
            }
        }
        let c = bot.courtyards.iter().find(|c| c.occurrence == occ).unwrap();
        assert!(c.bottom);
        assert!(top.courtyards.iter().all(|c| !c.bottom));
        // Mirrored regions keep their orientation: counterclockwise outers.
        for (o, sh) in &bot.layers["copper.bottom"] {
            if o != owner {
                continue;
            }
            for part in &sh.parts {
                if let ryu_board::shape::Piece::Region(r) = &part.piece {
                    assert!(area2(&r.outer) > 0, "{o}");
                }
            }
        }
    }

    /// Twice a contour's signed area, by its edges' chords.
    fn area2(c: &ryu_board::geom::Contour) -> i128 {
        c.edges()
            .iter()
            .map(|e| {
                let (a, b) = (e.start(), e.end());
                i128::from(a.x) * i128::from(b.y) - i128::from(b.x) * i128::from(a.y)
            })
            .sum()
    }

    #[test]
    fn routes_are_drawn_on_their_layers_with_their_drills() {
        let (_d, p0, _) = placed();
        let net = a_net(&p0);
        let extra = vec![
            (
                "boards/b/tracks/t1.json".to_owned(),
                format!(
                    r#"{{"format":1,"kind":"ryueda.track","layer":"bottom","net":"{net}","points":[[1000000,1000000],[1000000,1000000],[9000000,1000000]],"width_nm":250000}}"#
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
        let s = build(&p, &p.boards[&id]).unwrap();
        assert_eq!(s.tracks.len(), 1);
        assert_eq!(s.tracks[0].layer, super::super::file::CopperLayer::Bottom);
        let on = |layer: &str, owner: &str| {
            s.layers
                .get(layer)
                .is_some_and(|l| l.iter().any(|(o, _)| o == owner))
        };
        assert!(on("copper.bottom", "b:track/t1"));
        assert!(!on("copper.top", "b:track/t1"));
        assert!(on("copper.top", "b:via/v1") && on("copper.bottom", "b:via/v1"));
        let via = s.holes.iter().find(|h| h.owner == "b:via/v1").unwrap();
        assert!(via.plated);
        assert_eq!(
            via.of,
            HoleOf::Via(crate::id::Id::parse("v1", "via").unwrap())
        );
        let hole = s.holes.iter().find(|h| h.owner == "b:hole/h1").unwrap();
        assert!(!hole.plated);
        // The repeated point is skipped: one segment.
        match &s.tracks[0].shape.parts[0].piece {
            Piece::Stroke(st) => assert_eq!(st.path.len(), 1),
            other => panic!("{other:?}"),
        }
        // Each pad knows its name and its mask openings.
        for pd in &s.pads {
            assert!(pd.reference.ends_with(&format!("/{}", pd.pad)), "{pd:?}");
            assert_eq!(pd.mask[0].is_some(), pd.top, "{pd:?}");
        }
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
    fn a_placements_field_places_override_the_footprints() {
        let (_d, p0, id) = placed();
        let design = crate::netlist::one_design(&p0, None).unwrap();
        let (_, occs) = occ::occurrences(&p0, &design).unwrap();
        let o = occs.iter().find(|o| o.footprint.is_some()).unwrap();
        let owner = format!("b:{}", o.label());
        let silk = |s: &BoardShapes| -> Vec<Shape> {
            s.layers
                .get("silk.top")
                .into_iter()
                .flatten()
                .filter(|(w, _)| *w == owner)
                .map(|(_, sh)| sh.clone())
                .collect()
        };
        let before = silk(&build(&p0, &p0.boards[&id]).unwrap());
        assert!(
            !before.is_empty(),
            "the fixture's part draws its designator"
        );
        let path = format!(
            "boards/b/placements/{}.json",
            super::super::file::placement_name(&o.reference)
        );
        let with = |fields: &str| {
            let text = format!(
                r#"{{"at":[5000000,10000000],"fields":{{"reference":{fields}}},"format":2,"kind":"ryueda.placement","rotation_mdeg":0}}"#
            );
            let (d, p, _) = placed_with(&[(path.clone(), text)]);
            let s = build(&p, &p.boards[&id]).unwrap();
            (d, silk(&s))
        };
        let (_d1, moved) = with(r#"{"at":[0,5000000]}"#);
        assert_eq!(moved.len(), before.len());
        assert_ne!(moved, before, "the designator moved");
        let (_d2, hidden) = with(r#"{"hidden":true}"#);
        assert!(hidden.len() < before.len(), "the designator hidden");
        let (_d3, again) = with("{}");
        assert_eq!(again, before, "an empty override changes nothing");
    }

    #[test]
    fn a_boards_keepouts_are_in_its_shapes() {
        let k = r#"{"contour":{"edges":[{"to":[2000000,0],"type":"line"},{"to":[2000000,2000000],"type":"line"},{"to":[0,2000000],"type":"line"},{"to":[0,0],"type":"line"}],"start":[0,0]},"format":1,"keeps":["vias"],"kind":"ryueda.keepout","layers":["top"]}"#;
        let (_d, p, id) = placed_with(&[("boards/b/keepouts/k1.json".into(), k.into())]);
        let s = build(&p, &p.boards[&id]).unwrap();
        let ko = s
            .keepouts
            .iter()
            .find(|x| x.reference == "b:keepout/k1")
            .unwrap();
        assert_eq!(ko.layers, vec![0]);
        assert_eq!(ko.keeps, vec![crate::footprint::Keep::Vias]);
        assert!(ko.part.is_none());
        assert_eq!(ko.shape.bounds().unwrap().max.x, 2_000_000);
    }

    #[test]
    fn a_boards_drawings_join_their_layers() {
        let d = r#"{"end":[3000000,1000000],"format":1,"kind":"ryueda.board-drawing","layer":"silk.top","start":[1000000,1000000],"type":"line","width_nm":150000}"#;
        let (_d, p, id) = placed_with(&[("boards/b/drawings/d1.json".into(), d.into())]);
        let s = build(&p, &p.boards[&id]).unwrap();
        assert!(
            s.layers["silk.top"]
                .iter()
                .any(|(o, _)| o == "b:drawing/d1")
        );
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

    #[test]
    fn pads_carry_their_own_pour_connections() {
        use crate::footprint::{Thermal, ZoneConnect};
        let (_dir, mut p, id) = placed();
        for lib in p.libraries.values_mut().flatten() {
            for f in lib.footprints.values_mut() {
                if let Ok(f) = f.item.as_mut() {
                    let mut pads = f.pads.values_mut();
                    if let Some(a) = pads.next() {
                        a.margins.zone_connect = Some(ZoneConnect::Solid);
                    }
                    if let Some(b) = pads.next() {
                        b.thermal = Some(Thermal {
                            spoke_nm: 300_000,
                            gap_nm: 400_000,
                            angle_mdeg: None,
                        });
                    }
                }
            }
        }
        let s = build(&p, &p.boards[&id]).unwrap();
        assert!(s.pads.iter().any(|x| x.connect == Some(ZoneConnect::Solid)));
        assert!(s.pads.iter().any(|x| x.thermal == Some((400_000, 300_000))));
    }
}
