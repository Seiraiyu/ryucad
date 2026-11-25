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
    /// The board's pours as filled (ryueda 7.D).
    pub pours: Vec<super::pour::PourInfo>,
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

/// A board object (ryueda 7.E): what one file, or one placed occurrence,
/// adds to the board's shapes.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Obj {
    /// By occurrence reference (`s1/u1`).
    Placement(String),
    Track(crate::id::Id),
    Via(crate::id::Id),
    Hole(crate::id::Id),
    Keepout(crate::id::Id),
    Drawing(crate::id::Id),
}

/// What one object adds, in the order [`build`] adds it.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct ObjShapes {
    pub layers: Vec<(String, String, Shape)>,
    pub holes: Vec<HoleInfo>,
    pub pads: Vec<PadInfo>,
    pub tracks: Vec<TrackInfo>,
    pub vias: Vec<ViaInfo>,
    pub courtyards: Vec<CourtyardInfo>,
    pub keepouts: Vec<KeepoutInfo>,
    pub notices: Vec<Fault>,
}

/// What one placement adds.
type One = ObjShapes;

/// A board's objects' shapes (ryueda 7.E): each with the hash of what it
/// was built from, and the objects in [`build`]'s order.
#[derive(Clone, Debug, Default)]
pub struct Pieces {
    /// What every piece depends on: the board file, its preset and its
    /// copper count. A different `base` reuses nothing.
    pub base: [u8; 32],
    pub order: Vec<Obj>,
    pub by: BTreeMap<Obj, ([u8; 32], std::sync::Arc<ObjShapes>)>,
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
    assemble(p, b, &pieces(p, b, None)?, &[])
}

/// A piece's hash: BLAKE3 over its inputs, each length-prefixed.
fn hash_of(parts: &[&[u8]]) -> [u8; 32] {
    let mut h = blake3::Hasher::new();
    h.update(b"ryueda.board-piece.1");
    for p in parts {
        crate::design::piece(&mut h, p);
    }
    *h.finalize().as_bytes()
}

/// Anything serialisable, as JSON text, through one object-safe trait.
mod erased {
    pub trait Json {
        fn json(&self) -> String;
    }
    impl<T: serde::Serialize> Json for T {
        fn json(&self) -> String {
            serde_json::to_string(self).unwrap_or_default()
        }
    }
}

/// The board file, its preset and its copper count: what every piece is
/// built against.
fn base_of<'a>(
    p: &Project,
    b: &'a BoardFiles,
) -> Result<(&'a Board, std::sync::Arc<preset::Preset>, usize)> {
    let file = b.board.item.as_ref().map_err(Clone::clone)?;
    let preset = preset::resolve(p, &file.preset).ok_or_else(|| {
        Error::new(
            Code::InvalidDocument,
            format!("preset {} isn't known", file.preset),
        )
    })?;
    let n = super::stackup::copper_count(file);
    Ok((file, preset, n))
}

/// A file is left out of the shapes when it has an error fault (`check`
/// reports them).
fn fine(faults: &[Fault]) -> bool {
    faults.iter().all(|f| !f.is_error())
}

/// Each object's shapes, in [`build`]'s order. Objects whose inputs hash
/// as they did in `prev` keep `prev`'s pieces (Task 4 of ryueda 7.E).
pub fn pieces(p: &Project, b: &BoardFiles, prev: Option<&Pieces>) -> Result<Pieces> {
    let (file, preset, n) = base_of(p, b)?;
    let (_, occs) = occ::occurrences(p, &file.design)?;
    let board = b.id.to_string();
    let json = |v: &dyn erased::Json| v.json();
    let base = hash_of(&[
        b"base",
        json(file).as_bytes(),
        json(&*preset).as_bytes(),
        n.to_string().as_bytes(),
        board.as_bytes(),
    ]);
    let prev = prev.filter(|p| p.base == base);
    // What an object's hash says it was built from, and its old piece
    // when that is unchanged.
    let kept = |k: &Obj, h: &[u8; 32]| -> Option<std::sync::Arc<ObjShapes>> {
        let (g, piece) = prev?.by.get(k)?;
        (g == h).then(|| piece.clone())
    };
    let faults_of = |f: &[Fault]| -> String {
        f.iter()
            .map(|x| format!("{:?}{};", x.error.code, x.is_error()))
            .collect()
    };
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
    let mut out = Pieces {
        base,
        ..Pieces::default()
    };
    // Each object: its key, its hash, and its piece (kept or built).
    let put = |out: &mut Pieces, k: Obj, h: [u8; 32], piece: std::sync::Arc<ObjShapes>| {
        out.order.push(k.clone());
        out.by.insert(k, (h, piece));
    };
    let mut footprints: BTreeMap<String, [u8; 32]> = BTreeMap::new();
    let hashed: Vec<[u8; 32]> = placed
        .iter()
        .map(|(o, _, _)| {
            let pl = &b.placements[&o.reference];
            let fp = match (&o.footprint, &o.footprint_error) {
                (Some(f), _) => *footprints
                    .entry(o.footprint_ref.clone().unwrap_or_default())
                    .or_insert_with(|| hash_of(&[json(*f).as_bytes()])),
                (None, e) => hash_of(&[format!("{e:?}").as_bytes()]),
            };
            hash_of(&[
                b"placement",
                &base,
                json(pl.item.as_ref().ok().unwrap()).as_bytes(),
                &fp,
                o.reference.as_bytes(),
                format!("{:?}{:?}{:?}{}", o.refdes, o.value, o.nets, o.dnp).as_bytes(),
                o.footprint_ref.as_deref().unwrap_or_default().as_bytes(),
            ])
        })
        .collect();
    let parts: Vec<std::sync::Arc<One>> = placed
        .par_iter()
        .zip(&hashed)
        .map(|((o, place, fields), h)| {
            if let Some(piece) = kept(&Obj::Placement(o.reference.clone()), h) {
                return piece;
            }
            std::sync::Arc::new(match (&o.footprint, &o.footprint_error) {
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
            })
        })
        .collect();
    for (((o, _, _), one), h) in placed.iter().zip(parts).zip(hashed) {
        put(&mut out, Obj::Placement(o.reference.clone()), h, one);
    }
    // A file's object: its hash over its kind, its file and its faults.
    let file_hash = |kind: &[u8], j: String, f: &[Fault]| {
        hash_of(&[kind, &base, j.as_bytes(), faults_of(f).as_bytes()])
    };
    for (tid, t) in &b.tracks {
        let (Ok(track), true) = (&t.item, fine(&t.faults)) else {
            continue;
        };
        if track.layer.index(n).is_none() {
            continue;
        }
        let h = file_hash(b"track", json(track), &t.faults);
        if let Some(piece) = kept(&Obj::Track(tid.clone()), &h) {
            put(&mut out, Obj::Track(tid.clone()), h, piece);
            continue;
        }
        let shape = track_shape(track);
        let piece = ObjShapes {
            layers: vec![(
                track.layer.name(),
                format!("{board}:track/{tid}"),
                shape.clone(),
            )],
            tracks: vec![TrackInfo {
                id: tid.clone(),
                net: track.net.clone(),
                layer: track.layer,
                width_nm: track.width_nm,
                shape,
            }],
            ..ObjShapes::default()
        };
        put(
            &mut out,
            Obj::Track(tid.clone()),
            h,
            std::sync::Arc::new(piece),
        );
    }
    for (vid, v) in &b.vias {
        let (Ok(via), true) = (&v.item, fine(&v.faults)) else {
            continue;
        };
        let Some((from, to)) = via.span(n) else {
            continue;
        };
        let h = file_hash(b"via", json(via), &v.faults);
        if let Some(piece) = kept(&Obj::Via(vid.clone()), &h) {
            put(&mut out, Obj::Via(vid.clone()), h, piece);
            continue;
        }
        let at = Point::new(via.at[0], via.at[1]);
        let shape = disc(at, via.diameter_nm);
        let mut piece = ObjShapes::default();
        for layer in &super::stackup::names(n)[from..=to] {
            piece
                .layers
                .push((layer.clone(), format!("{board}:via/{vid}"), shape.clone()));
        }
        piece.holes.push(HoleInfo {
            owner: format!("{board}:via/{vid}"),
            shape: HoleShape::Round {
                at,
                d: via.drill_nm,
            },
            plated: true,
            of: HoleOf::Via(vid.clone()),
            span: (from, to),
        });
        piece.vias.push(ViaInfo {
            id: vid.clone(),
            from,
            to,
            net: via.net.clone(),
            at,
            drill_nm: via.drill_nm,
            diameter_nm: via.diameter_nm,
            shape,
        });
        put(
            &mut out,
            Obj::Via(vid.clone()),
            h,
            std::sync::Arc::new(piece),
        );
    }
    for (hid, h) in &b.holes {
        let (Ok(hole), true) = (&h.item, fine(&h.faults)) else {
            continue;
        };
        let hh = file_hash(b"hole", json(hole), &h.faults);
        if let Some(piece) = kept(&Obj::Hole(hid.clone()), &hh) {
            put(&mut out, Obj::Hole(hid.clone()), hh, piece);
            continue;
        }
        let piece = ObjShapes {
            holes: vec![HoleInfo {
                owner: format!("{board}:hole/{hid}"),
                shape: HoleShape::Round {
                    at: Point::new(hole.at[0], hole.at[1]),
                    d: hole.drill_nm,
                },
                plated: false,
                of: HoleOf::Hole(hid.clone()),
                span: (0, n - 1),
            }],
            ..ObjShapes::default()
        };
        put(
            &mut out,
            Obj::Hole(hid.clone()),
            hh,
            std::sync::Arc::new(piece),
        );
    }
    for (kid, k) in &b.keepouts {
        let (Ok(ko), true) = (&k.item, fine(&k.faults)) else {
            continue;
        };
        let Ok(c) = super::outline::contour(&ko.contour) else {
            continue;
        };
        let h = file_hash(b"keepout", json(ko), &k.faults);
        if let Some(piece) = kept(&Obj::Keepout(kid.clone()), &h) {
            put(&mut out, Obj::Keepout(kid.clone()), h, piece);
            continue;
        }
        let piece = ObjShapes {
            keepouts: vec![KeepoutInfo {
                reference: format!("{board}:keepout/{kid}"),
                part: None,
                layers: ko.indices(n),
                keeps: ko.keeps.clone(),
                shape: Shape::regions(Polarity::Positive, [Region::solid(super::outline::ccw(c))]),
            }],
            ..ObjShapes::default()
        };
        put(
            &mut out,
            Obj::Keepout(kid.clone()),
            h,
            std::sync::Arc::new(piece),
        );
    }
    for (did, d) in &b.drawings {
        let (Ok(g), true) = (&d.item, fine(&d.faults)) else {
            continue;
        };
        let h = file_hash(b"drawing", json(g), &d.faults);
        if let Some(piece) = kept(&Obj::Drawing(did.clone()), &h) {
            put(&mut out, Obj::Drawing(did.clone()), h, piece);
            continue;
        }
        let Some((shape, missing)) = super::drawing::shape(g) else {
            continue;
        };
        let mut piece = ObjShapes::default();
        if !missing.is_empty() {
            piece.notices.push(
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
        piece
            .layers
            .push((g.layer.clone(), format!("{board}:drawing/{did}"), shape));
        put(
            &mut out,
            Obj::Drawing(did.clone()),
            h,
            std::sync::Arc::new(piece),
        );
    }
    Ok(out)
}

/// The board's shapes from its pieces, in their order, then its pours
/// filled over them (`prev_pours`, the last fills, for Task 17's reuse)
/// and its outline.
pub fn assemble(
    p: &Project,
    b: &BoardFiles,
    ps: &Pieces,
    prev_pours: &[super::pour::PourInfo],
) -> Result<BoardShapes> {
    let (file, preset, n) = base_of(p, b)?;
    let board = b.id.to_string();
    let mut out = BoardShapes {
        copper_layers: n,
        ..BoardShapes::default()
    };
    let mut notices = Vec::new();
    for k in &ps.order {
        let piece = &ps.by[k].1;
        for (layer, owner, s) in &piece.layers {
            out.layers
                .entry(layer.clone())
                .or_default()
                .push((owner.clone(), s.clone()));
        }
        out.holes.extend(piece.holes.iter().cloned());
        out.pads.extend(piece.pads.iter().cloned());
        out.tracks.extend(piece.tracks.iter().cloned());
        out.vias.extend(piece.vias.iter().cloned());
        out.courtyards.extend(piece.courtyards.iter().cloned());
        out.keepouts.extend(piece.keepouts.iter().cloned());
        notices.extend(piece.notices.iter().cloned());
    }
    if !b.pours.is_empty() {
        let pours: Vec<(crate::id::Id, super::pour::Pour)> = b
            .pours
            .iter()
            .filter(|(_, l)| fine(&l.faults))
            .filter_map(|(id, l)| Some((id.clone(), l.item.as_ref().ok()?.clone())))
            .collect();
        let (region, edges) = outline(file);
        out.outline = region;
        out.edges = edges;
        let rules = super::rules::of(p, b)?;
        out.pours = super::pour::fill_all(&pours, &out, &rules, &preset, prev_pours);
        for f in &out.pours {
            let name = &super::stackup::names(n)[f.layer];
            for r in &f.regions {
                out.layers
                    .entry(name.clone())
                    .or_default()
                    .push((format!("{board}:pour/{}", f.id), r.clone()));
            }
        }
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
    use std::sync::Arc;

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
    fn a_board_is_its_pieces_assembled() {
        for (_d, p, id) in [placed(), placed_on(4, &[])] {
            let b = &p.boards[&id];
            let ps = pieces(&p, b, None).unwrap();
            assert!(ps.order.iter().any(|k| matches!(k, Obj::Placement(_))));
            assert_eq!(ps.order.len(), ps.by.len());
            assert_eq!(
                format!("{:?}", assemble(&p, b, &ps, &[]).unwrap()),
                format!("{:?}", build(&p, b).unwrap())
            );
        }
    }

    #[test]
    fn unchanged_objects_reuse_their_pieces() {
        let (_d, p, id) = placed();
        let b = &p.boards[&id];
        let first = pieces(&p, b, None).unwrap();
        let again = pieces(&p, b, Some(&first)).unwrap();
        for (k, (h, piece)) in &again.by {
            assert!(Arc::ptr_eq(piece, &first.by[k].1), "{k:?} built again");
            assert_eq!(h, &first.by[k].0);
        }
        // One placement moved: only its piece is new.
        let o = first
            .order
            .iter()
            .find_map(|k| match k {
                Obj::Placement(r) => Some(r.clone()),
                _ => None,
            })
            .unwrap();
        let path = format!(
            "boards/b/placements/{}.json",
            super::super::file::placement_name(&o)
        );
        let text =
            r#"{"at":[1000000,1000000],"format":1,"kind":"ryueda.placement","rotation_mdeg":0}"#;
        let (_d2, p2, _) = placed_with(&[(path, text.into())]);
        let b2 = &p2.boards[&id];
        let moved = pieces(&p2, b2, Some(&first)).unwrap();
        let fresh: Vec<&Obj> = moved
            .by
            .iter()
            .filter(|(k, (_, pc))| !Arc::ptr_eq(pc, &first.by[*k].1))
            .map(|(k, _)| k)
            .collect();
        assert_eq!(fresh, vec![&Obj::Placement(o)]);
        assert_eq!(
            format!("{:?}", assemble(&p2, b2, &moved, &[]).unwrap()),
            format!("{:?}", build(&p2, b2).unwrap())
        );
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

    #[test]
    fn a_whole_pour_fills_its_layer() {
        let (_d, p0, id) = placed();
        let net = a_net(&p0);
        let pour = super::super::pour::Pour::whole(&net, super::super::file::CopperLayer::Top);
        let text = crate::doc::to_string(&pour, super::super::file::POUR_KIND, 1);
        let (_d, p, _) = placed_with(&[("boards/b/pours/p1.json".into(), text)]);
        let s = build(&p, &p.boards[&id]).unwrap();
        assert_eq!(s.pours.len(), 1);
        assert!(!s.pours[0].regions.is_empty());
        assert!(
            s.layers["copper.top"]
                .iter()
                .any(|(owner, _)| owner == "b:pour/p1")
        );
        let built = super::super::layout::build("b", &s);
        let items: Vec<_> = built
            .layout
            .items
            .iter()
            .filter(|(k, _)| matches!(k, super::super::layout::Key::Pour { id, .. } if id == "p1"))
            .collect();
        assert_eq!(items.len(), s.pours[0].regions.len());
        assert!(
            items
                .iter()
                .all(|(_, it)| it.net.as_deref() == Some(net.as_str()))
        );
    }
}
