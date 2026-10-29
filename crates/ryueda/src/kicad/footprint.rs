//! `.kicad_mod` footprints (KiCad 6 to 9): every pad, graphic, keepout
//! and text field, in nanometres and millidegrees. KiCad's footprint y
//! axis points down and ryueda's up, so every y is negated: points,
//! drill offsets, a custom pad's primitives and a trapezoid's lean across
//! its height. Angles keep their sense (KiCad's turn counterclockwise as
//! drawn, as ryueda's do). Models are read as KiCad names them; the
//! import finds their files.

use super::sexpr::{self, Sexpr, unreadable};
use super::{Notes, hidden, yes};
use crate::error::{Code, Error, Result};
use crate::footprint::{
    Anchor, Corner, Drill, Field, Fields, Flag, Footprint, Graphic, Keep, Keepout, Margins, Mount,
    Pad, PadKind, PadProperty, PadShape, Shape, Thermal, ZoneConnect,
};
use crate::id::Id;
use crate::symbol::Point;
use std::collections::BTreeMap;

/// A 3D model as the footprint names it: its path as written (KiCad's
/// variables and all) and where it sits.
#[derive(Clone, Debug, PartialEq)]
pub struct ModelRef {
    pub path: String,
    pub offset_nm: [i64; 3],
    pub rotation_mdeg: [i64; 3],
    pub scale_ppm: [i64; 3],
}

/// A footprint file, read: the footprint (its models still to find) and
/// the models it names.
#[derive(Clone, Debug, PartialEq)]
pub struct Read {
    pub footprint: Footprint,
    pub models: Vec<ModelRef>,
}

/// A generated ID: `<prefix><n>`.
fn gen_id(prefix: &str, n: usize) -> Id {
    Id::parse(&format!("{prefix}{n}"), "footprint object").expect("generated IDs are IDs")
}

/// A point `(at x y …)`, `(start x y)`, `(xy x y)`, its y negated.
fn point(node: &Sexpr, notes: &mut Notes) -> Result<Point> {
    Ok([notes.mm_at(node, 0, "x")?, -notes.mm_at(node, 1, "y")?])
}

fn point_of(node: &Sexpr, name: &str, notes: &mut Notes) -> Result<Point> {
    match node.child(name) {
        Some(c) => point(c, notes),
        None => Err(unreadable(node.pos(), format!("({name} …) is missing"))),
    }
}

/// The angle `(at x y angle)` gives, in millidegrees.
fn angle(at: &Sexpr, notes: &mut Notes) -> Result<i64> {
    match at.items().get(2) {
        Some(a) if a.text().is_some_and(|t| t != "unlocked") => notes.deg(a),
        _ => Ok(0),
    }
}

/// `(pts …)`'s points; an arc in a polygon's outline can't be held.
fn points(node: &Sexpr, notes: &mut Notes) -> Result<Vec<Point>> {
    let pts = node
        .child("pts")
        .ok_or_else(|| unreadable(node.pos(), "(pts …) is missing"))?;
    pts.items()
        .iter()
        .map(|p| match p.head() {
            Some("xy") => point(p, notes),
            Some("arc") => Err(unreadable(
                p.pos(),
                "a polygon with an arc in its outline; ryueda's polygons are straight",
            )),
            _ => Err(unreadable(p.pos(), "a polygon's points are (xy x y)")),
        })
        .collect()
}

/// A drawn line's width: `(stroke (width w))` (KiCad 7 on) or `(width w)`.
fn width(node: &Sexpr, notes: &mut Notes) -> Result<i64> {
    let w = node
        .child("stroke")
        .and_then(|s| s.child("width"))
        .or_else(|| node.child("width"));
    match w {
        Some(w) => notes.mm_at(w, 0, "the width"),
        None => Ok(0),
    }
}

/// Whether a shape is filled: `(fill yes)`, `(fill solid)`.
fn filled(node: &Sexpr) -> bool {
    node.child("fill")
        .and_then(|f| f.items().first())
        .and_then(Sexpr::text)
        .is_some_and(|t| t == "yes" || t == "solid")
}

/// A drawn shape, `fp_…` on a layer or `gr_…` in a custom pad: `None`
/// for something that isn't one.
fn shape(node: &Sexpr, kind: &str, notes: &mut Notes) -> Result<Option<Shape>> {
    Ok(Some(match kind {
        "line" => Shape::Line {
            start: point_of(node, "start", notes)?,
            end: point_of(node, "end", notes)?,
            width_nm: width(node, notes)?,
        },
        "rect" => Shape::Rect {
            start: point_of(node, "start", notes)?,
            end: point_of(node, "end", notes)?,
            width_nm: width(node, notes)?,
            fill: filled(node),
        },
        "arc" => {
            if node.child("mid").is_none() {
                return Err(unreadable(node.pos(), "an arc without its middle point"));
            }
            Shape::Arc {
                start: point_of(node, "start", notes)?,
                mid: point_of(node, "mid", notes)?,
                end: point_of(node, "end", notes)?,
                width_nm: width(node, notes)?,
            }
        }
        "circle" => {
            let center = point_of(node, "center", notes)?;
            let end = point_of(node, "end", notes)?;
            Shape::Circle {
                center,
                radius_nm: distance(center, end, notes),
                width_nm: width(node, notes)?,
                fill: filled(node),
            }
        }
        "poly" => Shape::Polygon {
            points: points(node, notes)?,
            width_nm: width(node, notes)?,
            fill: filled(node),
        },
        "curve" => {
            let p: [Point; 4] = points(node, notes)?
                .try_into()
                .map_err(|_| unreadable(node.pos(), "a Bézier has four points"))?;
            Shape::Bezier {
                points: p,
                width_nm: width(node, notes)?,
            }
        }
        _ => return Ok(None),
    }))
}

/// The distance from `a` to `b`, rounded to a whole nanometre, noted
/// when it isn't one already.
fn distance(a: Point, b: Point, notes: &mut Notes) -> i64 {
    let d = |i: usize| i128::from(b[i]) - i128::from(a[i]);
    let s = (d(0) * d(0) + d(1) * d(1)) as u128;
    let mut r = s.isqrt();
    // (r + ½)² = r² + r + ¼, so s past r² + r rounds up; a square root
    // is never exactly a half.
    if s > r * r + r {
        r += 1;
    }
    if r * r != s {
        notes.rounded.insert(format!("radius {}", s.isqrt()));
    }
    r as i64
}

/// The text of a footprint `(fp_text …)` or `(property …)` as a field:
/// `None` when its layer has no equivalent.
fn field(node: &Sexpr, notes: &mut Notes) -> Result<Option<Field>> {
    let at = node
        .child("at")
        .ok_or_else(|| unreadable(node.pos(), "(at …) is missing"))?;
    let Some(layer) = one_layer(node, notes)? else {
        return Ok(None);
    };
    let font = node.child("effects").and_then(|e| e.child("font"));
    let (size, thickness) = match font {
        Some(f) => (
            match f.child("size") {
                // KiCad writes (size height width).
                Some(s) => [
                    notes.mm_at(s, 1, "the width")?,
                    notes.mm_at(s, 0, "the height")?,
                ],
                None => [1_000_000, 1_000_000],
            },
            match f.child("thickness") {
                Some(t) => notes.mm_at(t, 0, "the thickness")?,
                None => 0,
            },
        ),
        None => ([1_000_000, 1_000_000], 0),
    };
    Ok(Some(Field {
        layer: layer.to_owned(),
        at: point(at, notes)?,
        rotation_mdeg: angle(at, notes)?,
        size_nm: size,
        thickness_nm: thickness,
        hidden: hidden(node),
    }))
}

/// The one layer `(layer "L")` names: `None` (noted) when it has no
/// ryueda equivalent.
fn one_layer(node: &Sexpr, notes: &mut Notes) -> Result<Option<&'static str>> {
    let l = node
        .child("layer")
        .ok_or_else(|| unreadable(node.pos(), "(layer …) is missing"))?
        .arg(0, "the layer")?;
    match super::layers(l).as_deref() {
        Some([one]) => Ok(Some(one)),
        _ => {
            notes.unmapped.insert(l.to_owned());
            Ok(None)
        }
    }
}

/// `(layers …)`'s layers, those with no equivalent noted and left out.
fn many_layers(node: &Sexpr, notes: &mut Notes) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    for l in node.items().iter().filter_map(Sexpr::text) {
        match super::layers(l) {
            Some(ls) => {
                for x in ls {
                    if !out.iter().any(|o| o == x) {
                        out.push(x.to_owned());
                    }
                }
            }
            None => {
                notes.unmapped.insert(l.to_owned());
            }
        }
    }
    out
}

/// A margin setting, if `node` is one: true when taken.
fn margin(m: &mut Margins, node: &Sexpr, notes: &mut Notes) -> Result<bool> {
    let v = |notes: &mut Notes| notes.mm_at(node, 0, "the margin");
    match node.head() {
        Some("solder_mask_margin") => m.mask_margin_nm = Some(v(notes)?),
        Some("solder_paste_margin") => m.paste_margin_nm = Some(v(notes)?),
        Some("solder_paste_margin_ratio" | "solder_paste_ratio") => {
            let r = node
                .items()
                .first()
                .ok_or_else(|| unreadable(node.pos(), "the ratio is missing"))?;
            m.paste_ratio_ppm = Some(notes.scaled(r, 1_000_000)?);
        }
        Some("clearance") => m.clearance_nm = Some(v(notes)?),
        Some("zone_connect") => {
            m.zone_connect = Some(match node.arg(0, "the connection")? {
                "0" => ZoneConnect::None,
                "1" => ZoneConnect::Thermal,
                "2" => ZoneConnect::Solid,
                "3" => ZoneConnect::ThermalThrough,
                t => {
                    return Err(unreadable(
                        node.pos(),
                        format!("zone_connect {t} isn't KiCad's"),
                    ));
                }
            })
        }
        _ => return Ok(false),
    }
    Ok(true)
}

/// Reads a `.kicad_mod` file. A KiCad 5 `(module …)` is
/// `kicad_legacy_unsupported`.
pub fn read(text: &str, notes: &mut Notes) -> Result<Read> {
    let top = sexpr::parse(text)?;
    match top.head() {
        Some("footprint") => {}
        Some("module") => {
            return Err(Error::new(
                Code::KicadLegacyUnsupported,
                "a KiCad 5 footprint (module); open and save it in KiCad 6 or later",
            ));
        }
        _ => return Err(unreadable(top.pos(), "not a KiCad footprint (footprint …)")),
    }
    super::version(&top, super::FOOTPRINT_VERSION, notes)?;
    let mut f = Footprint {
        name: top.arg(0, "the footprint's name")?.to_owned(),
        description: String::new(),
        tags: Vec::new(),
        mount: Mount::Unspecified,
        flags: Vec::new(),
        margins: Margins::default(),
        pads: BTreeMap::new(),
        graphics: BTreeMap::new(),
        keepouts: BTreeMap::new(),
        fields: Fields::default(),
        models: Vec::new(),
    };
    let mut models = Vec::new();
    for node in top.items().iter().skip(1) {
        let Some(head) = node.head() else {
            // KiCad 6 writes `locked` and `placed` bare.
            if matches!(node.text(), Some("locked" | "placed")) {
                continue;
            }
            return Err(unreadable(node.pos(), "a list was expected"));
        };
        if margin(&mut f.margins, node, notes)? {
            continue;
        }
        match head {
            "layer" => {
                let l = node.arg(0, "the footprint's layer")?;
                if l != "F.Cu" {
                    return Err(unreadable(
                        node.pos(),
                        format!("a library footprint is on F.Cu, not {l}"),
                    ));
                }
            }
            "descr" => f.description = node.arg(0, "the description")?.to_owned(),
            "tags" => {
                f.tags = node
                    .arg(0, "the tags")?
                    .split_whitespace()
                    .map(str::to_owned)
                    .collect()
            }
            "attr" => attributes(&mut f, node, notes),
            "property" => {
                let (name, value) = (node.arg(0, "the name")?, node.arg(1, "the value")?);
                match name {
                    "Reference" => f.fields.reference = field(node, notes)?,
                    "Value" => f.fields.value = field(node, notes)?,
                    _ if value.is_empty() => {}
                    _ => notes.ignore(format!("property.{name}")),
                }
            }
            "fp_text" => match node.arg(0, "the text's kind")? {
                "reference" => f.fields.reference = field(node, notes)?,
                "value" => f.fields.value = field(node, notes)?,
                _ => {
                    if let Some(g) = user_text(node, notes)? {
                        let id = gen_id("g", f.graphics.len() + 1);
                        f.graphics.insert(id, g);
                    }
                }
            },
            "pad" => {
                let id = gen_id("p", f.pads.len() + 1);
                f.pads.insert(id, pad(node, notes)?);
            }
            "zone" => {
                if let Some(k) = keepout(node, notes)? {
                    let id = gen_id("k", f.keepouts.len() + 1);
                    f.keepouts.insert(id, k);
                }
            }
            "model" => models.push(model(node, notes)?),
            h if h.starts_with("fp_") => {
                let Some(s) = shape(node, &h[3..], notes)? else {
                    notes.ignore(h);
                    continue;
                };
                let Some(layer) = one_layer(node, notes)? else {
                    continue;
                };
                let id = gen_id("g", f.graphics.len() + 1);
                f.graphics.insert(
                    id,
                    Graphic {
                        layer: layer.to_owned(),
                        shape: s,
                    },
                );
            }
            // KiCad's bookkeeping.
            "version" | "generator" | "generator_version" | "uuid" | "tstamp" | "tedit"
            | "locked" | "placed" => {}
            "embedded_fonts" if !yes(node) => {}
            h => notes.ignore(h),
        }
    }
    Ok(Read {
        footprint: f,
        models,
    })
}

fn attributes(f: &mut Footprint, node: &Sexpr, notes: &mut Notes) {
    for a in node.items().iter().filter_map(Sexpr::text) {
        let flag = match a {
            "smd" => {
                f.mount = Mount::Smd;
                continue;
            }
            "through_hole" => {
                f.mount = Mount::Through;
                continue;
            }
            "board_only" | "virtual" => Flag::BoardOnly,
            "exclude_from_pos_files" => Flag::ExcludeFromPos,
            "exclude_from_bom" => Flag::ExcludeFromBom,
            "allow_missing_courtyard" => Flag::AllowMissingCourtyard,
            "allow_soldermask_bridges" => Flag::AllowSoldermaskBridges,
            "dnp" => Flag::Dnp,
            other => {
                notes.ignore(format!("attr.{other}"));
                continue;
            }
        };
        if !f.flags.contains(&flag) {
            f.flags.push(flag);
        }
    }
}

/// A user text `(fp_text user …)`: `None` off any equivalent layer.
fn user_text(node: &Sexpr, notes: &mut Notes) -> Result<Option<Graphic>> {
    let Some(fl) = field(node, notes)? else {
        return Ok(None);
    };
    Ok(Some(Graphic {
        layer: fl.layer,
        shape: Shape::Text {
            text: node.arg(1, "the text")?.to_owned(),
            at: fl.at,
            rotation_mdeg: fl.rotation_mdeg,
            size_nm: fl.size_nm,
            thickness_nm: fl.thickness_nm,
            hidden: fl.hidden,
        },
    }))
}

fn pad(node: &Sexpr, notes: &mut Notes) -> Result<Pad> {
    let number = node.arg(0, "the pad's number")?;
    let kind = match node.arg(1, "the pad's type")? {
        "smd" => PadKind::Smd,
        "thru_hole" => PadKind::Through,
        "connect" => PadKind::Connector,
        "np_thru_hole" => PadKind::Npth,
        t => {
            return Err(unreadable(
                node.pos(),
                format!("pad type {t:?} isn't KiCad's"),
            ));
        }
    };
    let form = node.arg(2, "the pad's shape")?;
    let at = node
        .child("at")
        .ok_or_else(|| unreadable(node.pos(), "(at …) is missing"))?;
    let size = node
        .child("size")
        .ok_or_else(|| unreadable(node.pos(), "(size …) is missing"))?;
    let mut p = Pad {
        number: (!number.is_empty()).then(|| number.to_owned()),
        kind,
        shape: PadShape::Rect,
        at: point(at, notes)?,
        rotation_mdeg: angle(at, notes)?,
        size: [
            notes.mm_at(size, 0, "the width")?,
            notes.mm_at(size, 1, "the height")?,
        ],
        drill: None,
        layers: Vec::new(),
        margins: Margins::default(),
        thermal: None,
        property: None,
        die_length_nm: None,
    };
    let ratio = |name: &str, notes: &mut Notes| -> Result<i64> {
        match node.child(name).and_then(|r| r.items().first()) {
            Some(r) => notes.scaled(r, 1_000_000),
            None => Ok(0),
        }
    };
    let corners: Vec<Corner> = match node.child("chamfer") {
        Some(c) => c
            .items()
            .iter()
            .filter_map(Sexpr::text)
            .map(|t| match t {
                "top_left" => Ok(Corner::TopLeft),
                "top_right" => Ok(Corner::TopRight),
                "bottom_left" => Ok(Corner::BottomLeft),
                "bottom_right" => Ok(Corner::BottomRight),
                t => Err(unreadable(c.pos(), format!("corner {t:?} isn't KiCad's"))),
            })
            .collect::<Result<_>>()?,
        None => Vec::new(),
    };
    p.shape = match form {
        "circle" => PadShape::Circle,
        "rect" | "roundrect" if !corners.is_empty() => PadShape::Chamfered {
            ratio_ppm: ratio("chamfer_ratio", notes)?,
            corners,
            round_ratio_ppm: if form == "roundrect" {
                ratio("roundrect_rratio", notes)?
            } else {
                0
            },
        },
        "rect" => PadShape::Rect,
        "oval" => PadShape::Oval,
        "roundrect" => PadShape::Roundrect {
            ratio_ppm: ratio("roundrect_rratio", notes)?,
        },
        "trapezoid" => {
            let d = node.child("rect_delta");
            let delta = match d {
                // The lean across the height turns with the y axis.
                Some(d) => [notes.mm_at(d, 0, "dx")?, -notes.mm_at(d, 1, "dy")?],
                None => [0, 0],
            };
            PadShape::Trapezoid { delta }
        }
        "custom" => {
            let anchor = match node
                .child("options")
                .and_then(|o| o.child("anchor"))
                .and_then(|a| a.items().first())
                .and_then(Sexpr::text)
            {
                Some("circle") => Anchor::Circle,
                _ => Anchor::Rect,
            };
            let mut primitives = Vec::new();
            if let Some(list) = node.child("primitives") {
                for g in list.items() {
                    let kind = g.head().and_then(|h| h.strip_prefix("gr_")).unwrap_or("");
                    match shape(g, kind, notes)? {
                        Some(s) => primitives.push(s),
                        None => notes.ignore(format!("pad.primitives.{}", g.head().unwrap_or("?"))),
                    }
                }
            }
            PadShape::Custom { anchor, primitives }
        }
        t => {
            return Err(unreadable(
                node.pos(),
                format!("pad shape {t:?} isn't KiCad's"),
            ));
        }
    };
    let (mut spoke, mut gap, mut turn) = (None, None, None);
    for c in node.items().iter().skip(3) {
        if margin(&mut p.margins, c, notes)? {
            continue;
        }
        match c.head() {
            Some("drill") => {
                if matches!(kind, PadKind::Through | PadKind::Npth) {
                    p.drill = Some(drill(c, notes)?);
                } else {
                    notes.ignore("pad.drill");
                }
            }
            Some("layers") => p.layers = many_layers(c, notes),
            Some("property") => {
                p.property = match c.arg(0, "the pad's property")? {
                    "pad_prop_bga" => Some(PadProperty::Bga),
                    "pad_prop_fiducial_glob" => Some(PadProperty::FiducialGlobal),
                    "pad_prop_fiducial_loc" => Some(PadProperty::FiducialLocal),
                    "pad_prop_testpoint" => Some(PadProperty::Testpoint),
                    "pad_prop_heatsink" => Some(PadProperty::Heatsink),
                    "pad_prop_castellated" => Some(PadProperty::Castellated),
                    "pad_prop_mechanical" => Some(PadProperty::Mechanical),
                    "none" => None,
                    other => {
                        notes.ignore(format!("pad.property.{other}"));
                        None
                    }
                }
            }
            Some("die_length") => p.die_length_nm = Some(notes.mm_at(c, 0, "the die length")?),
            Some("thermal_bridge_width" | "thermal_width") => {
                spoke = Some(notes.mm_at(c, 0, "the spoke's width")?)
            }
            Some("thermal_gap") => gap = Some(notes.mm_at(c, 0, "the thermal gap")?),
            Some("thermal_bridge_angle") => {
                turn = Some(match c.items().first() {
                    Some(a) => notes.deg(a)?,
                    None => return Err(unreadable(c.pos(), "the angle is missing")),
                })
            }
            // Taken above, or KiCad's bookkeeping.
            Some(
                "at" | "size" | "roundrect_rratio" | "chamfer_ratio" | "chamfer" | "rect_delta"
                | "primitives" | "uuid" | "tstamp" | "locked",
            ) => {}
            Some("options") => {
                let convex = c
                    .child("clearance")
                    .and_then(|x| x.items().first())
                    .and_then(Sexpr::text)
                    == Some("convexhull");
                if convex {
                    notes.ignore("pad.options.clearance");
                }
            }
            Some("remove_unused_layers") if !yes(c) => {}
            Some(h) => notes.ignore(format!("pad.{h}")),
            None => {
                if c.text() != Some("locked") {
                    return Err(unreadable(c.pos(), "a list was expected"));
                }
            }
        }
    }
    if spoke.is_some() || gap.is_some() || turn.is_some() {
        p.thermal = Some(Thermal {
            spoke_nm: spoke.unwrap_or(0),
            gap_nm: gap.unwrap_or(0),
            angle_mdeg: turn,
        });
    }
    Ok(p)
}

/// `(drill D)`, `(drill oval W H)`, each with an optional `(offset x y)`.
fn drill(node: &Sexpr, notes: &mut Notes) -> Result<Drill> {
    let offset = match node.child("offset") {
        Some(o) => point(o, notes)?,
        None => [0, 0],
    };
    let nums: Vec<&Sexpr> = node
        .items()
        .iter()
        .filter(|s| matches!(s, Sexpr::Atom(t, _) if t != "oval"))
        .collect();
    let first = nums
        .first()
        .ok_or_else(|| unreadable(node.pos(), "the drill's size is missing"))?;
    let w = notes.mm(first)?;
    if node.has_atom("oval") {
        let h = match nums.get(1) {
            Some(h) => notes.mm(h)?,
            None => w,
        };
        return Ok(Drill::Slot {
            size: [w, h],
            offset,
        });
    }
    Ok(Drill::Round {
        diameter_nm: w,
        offset,
    })
}

/// A keepout zone: `None` (noted) for a zone that keeps nothing out,
/// which a library has no use for.
fn keepout(node: &Sexpr, notes: &mut Notes) -> Result<Option<Keepout>> {
    let Some(k) = node.child("keepout") else {
        notes.ignore("zone");
        return Ok(None);
    };
    let mut keeps = Vec::new();
    for rule in k.items() {
        let (Some(what), Ok(how)) = (rule.head(), rule.arg(0, "the rule")) else {
            continue;
        };
        if how != "not_allowed" {
            continue;
        }
        match what {
            "tracks" => keeps.push(Keep::Tracks),
            "vias" => keeps.push(Keep::Vias),
            "pads" => keeps.push(Keep::Pads),
            "copperpour" => keeps.push(Keep::Pours),
            other => notes.ignore(format!("zone.keepout.{other}")),
        }
    }
    if keeps.is_empty() {
        notes.ignore("zone");
        return Ok(None);
    }
    let layers = match (node.child("layers"), node.child("layer")) {
        (Some(ls), _) => many_layers(ls, notes),
        (None, Some(l)) => many_layers(l, notes),
        (None, None) => Vec::new(),
    };
    let polygon = node
        .child("polygon")
        .ok_or_else(|| unreadable(node.pos(), "a keepout without its (polygon …)"))?;
    Ok(Some(Keepout {
        polygon: points(polygon, notes)?,
        keeps,
        layers,
    }))
}

/// `(model "path" (offset (xyz …)) (scale (xyz …)) (rotate (xyz …)))`.
/// KiCad's model frame is already y up (z out of the board), so its
/// numbers are kept as written.
fn model(node: &Sexpr, notes: &mut Notes) -> Result<ModelRef> {
    let xyz = |name: &str, factor: i64, default: i64, notes: &mut Notes| -> Result<[i64; 3]> {
        let Some(v) = node.child(name).and_then(|n| n.child("xyz")) else {
            return Ok([default; 3]);
        };
        let mut out = [0; 3];
        for (i, o) in out.iter_mut().enumerate() {
            let n = v
                .items()
                .get(i)
                .ok_or_else(|| unreadable(v.pos(), "(xyz x y z) has three numbers"))?;
            *o = notes.scaled(n, factor)?;
        }
        Ok(out)
    };
    for c in node.items().iter().skip(1) {
        match c.head() {
            Some("offset" | "scale" | "rotate") => {}
            Some("hide") if !yes(c) => {}
            Some(h) => notes.ignore(format!("model.{h}")),
            None => {}
        }
    }
    Ok(ModelRef {
        path: node.arg(0, "the model's path")?.to_owned(),
        offset_nm: xyz("offset", 1_000_000, 0, notes)?,
        rotation_mdeg: xyz("rotate", 1_000, 0, notes)?,
        scale_ppm: xyz("scale", 1_000_000, 1_000_000, notes)?,
    })
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;

    /// A footprint of our own, written as KiCad 6 writes them: every pad
    /// kind and shape, a keepout, user text, and a model named the
    /// KiCad 6 way.
    pub const SIX: &str = r#"(footprint "Every_Pad" (version 20211014) (generator pcbnew) (layer "F.Cu")
  (tedit 0) (descr "every pad") (tags "test pads")
  (attr smd exclude_from_bom)
  (solder_mask_margin 0.05) (clearance 0.2)
  (fp_text reference "REF**" (at 0 -3 0) (layer "F.SilkS") (effects (font (size 1 1.2) (thickness 0.15))))
  (fp_text value "Every_Pad" (at 0 3 90) (layer "F.Fab") hide (effects (font (size 1 1) (thickness 0.15))))
  (fp_text user "${REFERENCE}" (at 0 0) (layer "F.Fab") (effects (font (size 0.5 0.5) (thickness 0.08))))
  (fp_line (start -1 -2) (end 1 -2) (layer "F.SilkS") (width 0.12))
  (fp_rect (start -2.5 -2.5) (end 2.5 2.5) (layer "F.CrtYd") (width 0.05) (fill none))
  (fp_circle (center 0 0) (end 0.3 0.4) (layer "F.Fab") (width 0.1) (fill solid))
  (fp_arc (start -1 0) (mid 0 -1) (end 1 0) (layer "F.Fab") (width 0.1))
  (fp_poly (pts (xy 0 0) (xy 1 0) (xy 1 1)) (layer "Edge.Cuts") (width 0))
  (fp_line (start 0 0) (end 1 1) (layer "Rescue") (width 0.1))
  (pad "1" smd roundrect (at -1.5 0.5 90) (size 0.6 0.8) (layers "F.Cu" "F.Paste" "F.Mask") (roundrect_rratio 0.25) (solder_paste_margin_ratio -0.1))
  (pad "2" thru_hole oval (at 1.5 0.5) (size 1.2 2) (drill oval 0.6 1.2 (offset 0 0.1)) (layers *.Cu *.Mask) (zone_connect 2) (thermal_width 0.3) (thermal_gap 0.4))
  (pad "" np_thru_hole circle (at 0 0) (size 1 1) (drill 1) (layers *.Cu *.Mask))
  (pad "3" smd trapezoid (at 0 -1.5) (size 1 1) (rect_delta 0.2 0.1) (layers "F.Cu"))
  (pad "4" smd rect (at 0 1.5) (size 1 1) (chamfer_ratio 0.2) (chamfer top_left bottom_right) (layers "F.Cu") (die_length 0.5) (pinfunction "X"))
  (pad "5" smd custom (at 2 2) (size 0.5 0.5) (layers "F.Cu") (options (clearance outline) (anchor circle))
    (primitives (gr_poly (pts (xy 0 0) (xy 1 0) (xy 0 1)) (width 0) (fill yes))))
  (pad "6" connect circle (at 0 2.5) (size 0.5 0.5) (layers "F.Cu" "F.Mask") (property pad_prop_testpoint))
  (zone (net 0) (net_name "") (layers F&B.Cu) (hatch edge 0.5) (keepout (tracks not_allowed) (vias allowed) (pads not_allowed) (copperpour not_allowed) (footprints allowed))
    (polygon (pts (xy -1 -1) (xy 1 -1) (xy 1 1))))
  (model "${KICAD6_3DMODEL_DIR}/Test.3dshapes/Every_Pad.wrl" (offset (xyz 0 0 0.1)) (scale (xyz 1 1 1)) (rotate (xyz 0 0 -90)))
)
"#;

    #[test]
    fn a_kicad_6_footprint_reads_with_y_up() {
        let mut notes = Notes::default();
        let r = read(SIX, &mut notes).unwrap();
        let f = &r.footprint;
        assert_eq!((f.name.as_str(), f.mount), ("Every_Pad", Mount::Smd));
        assert_eq!(f.tags, ["test", "pads"]);
        assert_eq!(f.flags, [Flag::ExcludeFromBom]);
        assert_eq!(f.margins.mask_margin_nm, Some(50_000));
        let reference = f.fields.reference.as_ref().unwrap();
        assert_eq!(
            (reference.at, reference.size_nm),
            ([0, 3_000_000], [1_200_000, 1_000_000])
        );
        let value = f.fields.value.as_ref().unwrap();
        assert!(value.hidden && value.rotation_mdeg == 90_000);
        let p = |id: &str| &f.pads[&Id::parse(id, "p").unwrap()];
        assert_eq!(p("p1").at, [-1_500_000, -500_000]);
        assert_eq!(p("p1").rotation_mdeg, 90_000);
        assert_eq!(p("p1").shape, PadShape::Roundrect { ratio_ppm: 250_000 });
        assert_eq!(p("p1").margins.paste_ratio_ppm, Some(-100_000));
        assert_eq!(p("p1").layers, ["copper.top", "paste.top", "mask.top"]);
        assert_eq!(
            p("p2").drill,
            Some(Drill::Slot {
                size: [600_000, 1_200_000],
                offset: [0, -100_000]
            })
        );
        assert_eq!(p("p2").layers, ["copper.all", "mask.top", "mask.bottom"]);
        assert_eq!(
            p("p2").thermal,
            Some(Thermal {
                spoke_nm: 300_000,
                gap_nm: 400_000,
                angle_mdeg: None
            })
        );
        assert_eq!(p("p2").margins.zone_connect, Some(ZoneConnect::Solid));
        assert_eq!(
            (p("p3").number.as_deref(), p("p3").kind),
            (None, PadKind::Npth)
        );
        assert_eq!(
            p("p4").shape,
            PadShape::Trapezoid {
                delta: [200_000, -100_000]
            }
        );
        assert_eq!(
            p("p5").shape,
            PadShape::Chamfered {
                ratio_ppm: 200_000,
                corners: vec![Corner::TopLeft, Corner::BottomRight],
                round_ratio_ppm: 0
            }
        );
        assert_eq!(p("p5").die_length_nm, Some(500_000));
        let PadShape::Custom { anchor, primitives } = &p("p6").shape else {
            panic!("custom")
        };
        assert_eq!(*anchor, Anchor::Circle);
        assert!(
            matches!(&primitives[0], Shape::Polygon { points, fill: true, .. } if points[2] == [0, -1_000_000])
        );
        assert_eq!(
            (p("p7").kind, p("p7").property),
            (PadKind::Connector, Some(PadProperty::Testpoint))
        );
        let g = |id: &str| &f.graphics[&Id::parse(id, "g").unwrap()];
        assert!(matches!(&g("g1").shape, Shape::Text { text, .. } if text == "${REFERENCE}"));
        assert_eq!(
            g("g2").shape,
            Shape::Line {
                start: [-1_000_000, 2_000_000],
                end: [1_000_000, 2_000_000],
                width_nm: 120_000
            }
        );
        assert_eq!(g("g3").layer, "courtyard.top");
        assert!(matches!(
            g("g4").shape,
            Shape::Circle {
                radius_nm: 500_000,
                fill: true,
                ..
            }
        ));
        assert!(matches!(
            g("g5").shape,
            Shape::Arc {
                mid: [0, 1_000_000],
                ..
            }
        ));
        assert_eq!(g("g6").layer, "edge");
        assert_eq!(f.graphics.len(), 6);
        let k = &f.keepouts[&Id::parse("k1", "k").unwrap()];
        assert_eq!(k.keeps, [Keep::Tracks, Keep::Pads, Keep::Pours]);
        assert_eq!(k.layers, ["copper.top", "copper.bottom"]);
        assert_eq!(
            r.models,
            [ModelRef {
                path: "${KICAD6_3DMODEL_DIR}/Test.3dshapes/Every_Pad.wrl".into(),
                offset_nm: [0, 0, 100_000],
                rotation_mdeg: [0, 0, -90_000],
                scale_ppm: [1_000_000; 3],
            }]
        );
        assert_eq!(notes.unmapped.iter().collect::<Vec<_>>(), ["Rescue"]);
        assert_eq!(
            notes.ignored.iter().collect::<Vec<_>>(),
            ["pad.pinfunction"]
        );
        assert!(notes.rounded.is_empty());
        let faults: Vec<Code> = f.check().iter().map(|x| x.error.code).collect();
        assert!(faults.is_empty(), "{faults:?}");
    }

    #[test]
    fn what_a_footprint_cant_hold_is_refused() {
        let mut n = Notes::default();
        let module = "(module \"x\" (layer F.Cu))";
        assert_eq!(
            read(module, &mut n).unwrap_err().code,
            Code::KicadLegacyUnsupported
        );
        let back = "(footprint \"x\" (layer \"B.Cu\"))";
        assert_eq!(read(back, &mut n).unwrap_err().code, Code::KicadUnreadable);
        let arc = "(footprint \"x\" (layer \"F.Cu\") (fp_poly (pts (xy 0 0) (arc (start 0 0) (mid 1 1) (end 2 0))) (layer \"F.SilkS\")))";
        assert_eq!(read(arc, &mut n).unwrap_err().code, Code::KicadUnreadable);
        let odd = "(footprint \"x\" (layer \"F.Cu\") (fp_circle (center 0 0) (end 1 1) (layer \"F.Fab\") (width 0)))";
        let mut n = Notes::default();
        let r = read(odd, &mut n).unwrap();
        let g = r.footprint.graphics.values().next().unwrap();
        assert!(matches!(
            g.shape,
            Shape::Circle {
                radius_nm: 1_414_214,
                ..
            }
        ));
        assert_eq!(n.rounded.len(), 1);
    }
}
