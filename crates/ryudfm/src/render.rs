//! `render`: layers of a board drawn as PNG or SVG by
//! `ryu_board::render`, the bytes after the response line.
//!
//! Layers are composited bottom up: in the order given, or by default the
//! viewed side's copper, solder mask and silk screen in stack order, then
//! every drill layer cut through to the background, then the profile's
//! outline. Each kind has a fixed colour on a fixed dark background. The
//! view frames the profile with a margin (else what is drawn), or the
//! window given; from below it is mirrored, as a person holding the board
//! sees it.
//!
//! With `merged` (or `net`, or `offset`), `render_copper` draws the board
//! step's copper as `copper::CopperBoard` connects it: see its comment.

use std::collections::{BTreeMap, BTreeSet};

use crate::copper::{CopperBoard, Key};
use crate::error::{Code, Error, Result};
use crate::inspect::{rect, step_index};
use crate::quantity;
use crate::units::Unit;
use ryu_board::geom::{Pen, Rect, Stroke};
use ryu_board::model::{Board, Layer, LayerKind, Polarity, Side as LayerSide};
use ryu_board::render::{self, Ink, MAX_SIDE, Scene, View};
use ryu_board::shape::{Part, Piece, Shape};
use ryu_copper::connect::{IslandId, Node};
use ryu_copper::layout::PointSide;
use ryu_session::rpc::Answer;
use serde::{Deserialize, Serialize};
use serde_json::json;

/// The long side of a render unless asked, in pixels.
pub const DEFAULT_SIZE: u32 = 1600;
/// The background every render is drawn on.
pub const BACKGROUND: [u8; 3] = [16, 20, 24];
/// The profile's outline.
pub const PROFILE: Ink = Ink::Paint {
    colour: [240, 200, 40],
    alpha: 255,
};

/// Which way the board is seen.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Side {
    #[default]
    Top,
    Bottom,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Format {
    #[default]
    Png,
    Svg,
}

/// What to draw.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Args {
    /// The step; the board step by default.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub step: Option<String>,
    /// The layers, bottom first; the viewed side's by default.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub layers: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub side: Option<Side>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub format: Option<Format>,
    /// `W` (the long side) or `WxH`, in pixels.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub size: Option<String>,
    /// `X0,Y0,X1,Y1`, lengths (bare numbers in the default unit).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub window: Option<String>,
    /// Copper merged, each island in its own colour (implied by `net` and
    /// `offset`).
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub merged: bool,
    /// That net's islands highlighted, the rest dimmed.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub net: Option<String>,
    /// The merged copper grown by this length (shrunk when negative).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub offset: Option<String>,
}

impl Args {
    /// Whether this asks for the board's copper as it connects.
    pub fn wants_copper(&self) -> bool {
        self.merged || self.net.is_some() || self.offset.is_some()
    }
}

/// How a layer is drawn: copper by side, mask, paste, silk, routs and
/// the rest each in a colour of their own; drills cut through.
pub fn ink(l: &Layer) -> Ink {
    let paint = |colour, alpha| Ink::Paint { colour, alpha };
    match &l.kind {
        k if k.is_copper() => match l.side {
            Some(LayerSide::Top) => paint([200, 130, 50], 230),
            Some(LayerSide::Bottom) => paint([70, 130, 210], 230),
            _ => paint([90, 170, 90], 180),
        },
        LayerKind::SolderMask => paint([30, 160, 90], 110),
        LayerKind::SolderPaste => paint([170, 170, 180], 200),
        LayerKind::SilkScreen => paint([235, 235, 235], 240),
        LayerKind::Drill => Ink::Cut,
        LayerKind::Rout => paint([230, 200, 60], 200),
        _ => paint([170, 110, 210], 160),
    }
}

/// The layers drawn by default, bottom first: the viewed side's copper,
/// solder mask and silk screen in stack order (the matrix lists the stack
/// top down), then every drill layer.
pub fn default_layers(b: &Board, side: Side) -> Vec<usize> {
    let want = match side {
        Side::Top => LayerSide::Top,
        Side::Bottom => LayerSide::Bottom,
    };
    let mut faces: Vec<usize> = (0..b.layers.len())
        .filter(|&i| {
            let l = &b.layers[i];
            l.side == Some(want)
                && (l.kind.is_copper()
                    || matches!(l.kind, LayerKind::SolderMask | LayerKind::SilkScreen))
        })
        .collect();
    faces.sort_by_key(|&i| b.layers[i].row);
    if side == Side::Top {
        faces.reverse();
    }
    faces.extend((0..b.layers.len()).filter(|&i| b.layers[i].kind == LayerKind::Drill));
    faces
}

/// An image's size: its long side, or both.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Size {
    Long(u32),
    Exact(u32, u32),
}

pub fn size(text: Option<&str>) -> Result<Size> {
    let bad = |t: &str| {
        Error::new(
            Code::InvalidParam,
            format!("{t:?} isn't a size: W or WxH, in pixels, 1 to {MAX_SIDE}"),
        )
        .with("param", "size")
    };
    let side = |t: &str, all: &str| -> Result<u32> {
        t.trim()
            .parse::<u32>()
            .ok()
            .filter(|n| (1..=MAX_SIDE).contains(n))
            .ok_or_else(|| bad(all))
    };
    match text {
        None => Ok(Size::Long(DEFAULT_SIZE)),
        Some(t) => match t.split_once(['x', 'X']) {
            Some((w, h)) => Ok(Size::Exact(side(w, t)?, side(h, t)?)),
            None => Ok(Size::Long(side(t, t)?)),
        },
    }
}

/// `r` grown by a fiftieth of its longer side each way (at least 1 nm).
fn margin(r: Rect) -> Rect {
    let m = ((r.max.x - r.min.x).max(r.max.y - r.min.y) / 50).max(1);
    Rect {
        min: r.min.offset(-m, -m),
        max: r.max.offset(m, m),
    }
}

/// The profile's outline, a pen `width` nanometres wide along each of its
/// contours.
fn outline(profile: &ryu_board::geom::Region, width: i64) -> Shape {
    Shape {
        parts: std::iter::once(&profile.outer)
            .chain(&profile.holes)
            .map(|c| Part {
                polarity: Polarity::Positive,
                piece: Piece::Stroke(Stroke {
                    path: c.edges().to_vec(),
                    pen: Pen::Round(width),
                }),
            })
            .collect(),
        ..Shape::default()
    }
}

/// Draws board `b` (`prefix` its ID in a project); bare lengths are in
/// `unit`.
pub fn render(b: &Board, prefix: Option<&str>, args: &Args, unit: Unit) -> Result<Answer> {
    let step = step_index(b, args.step.as_deref())?;
    let side = args.side.unwrap_or_default();
    let layers: Vec<usize> = if args.layers.is_empty() {
        default_layers(b, side)
    } else {
        args.layers
            .iter()
            .map(|n| {
                b.layer(n).ok_or_else(|| {
                    Error::new(Code::NotFound, format!("the board has no layer {n}"))
                        .with("layer", n)
                        .with(
                            "layers",
                            b.layers.iter().map(|l| l.name.as_str()).collect::<Vec<_>>(),
                        )
                })
            })
            .collect::<Result<_>>()?
    };
    b.build_shapes(step, &layers);
    let shapes: Vec<Vec<&Shape>> = layers
        .iter()
        .map(|&l| {
            b.layer_shapes(step, l)
                .into_iter()
                .filter_map(|s| s.as_ref().ok())
                .collect()
        })
        .collect();
    let (view, per_px, edge) = frame(
        b,
        step,
        render::bounds(shapes.iter().flatten().copied()),
        args,
        unit,
        side,
    )?;
    let mut scene = Scene {
        view,
        background: BACKGROUND,
        layers: layers
            .iter()
            .zip(shapes)
            .map(|(&l, shapes)| render::Layer {
                name: b.layers[l].name.clone(),
                ink: ink(&b.layers[l]),
                shapes,
            })
            .collect(),
    };
    if let Some(e) = &edge {
        scene.layers.push(render::Layer {
            name: "profile".into(),
            ink: PROFILE,
            shapes: vec![e],
        });
    }
    let format = args.format.unwrap_or_default();
    let result = json!({
        "board": prefix,
        "step": b.steps[step].name,
        "side": side,
        "format": format,
        "layers": layers.iter().map(|&l| b.layers[l].name.as_str()).collect::<Vec<_>>(),
        "profile": edge.is_some(),
        "width": view.width,
        "height": view.height,
        "window_nm": rect(view.window),
        "nm_per_px": per_px,
    });
    Ok(answer(&scene, format, result))
}

fn answer(scene: &Scene, format: Format, result: serde_json::Value) -> Answer {
    match format {
        Format::Png => Answer::bytes(result, "image/png", render::raster(scene).png()),
        Format::Svg => Answer::bytes(result, "image/svg+xml", render::svg(scene).into_bytes()),
    }
}

/// The view of a render: the window given, else the profile's box (else
/// what is drawn) with a margin; its nanometres per pixel; and the
/// profile's outline at a pixel and a half.
fn frame(
    b: &Board,
    step: usize,
    drawn: Option<Rect>,
    args: &Args,
    unit: Unit,
    side: Side,
) -> Result<(View, f64, Option<Shape>)> {
    let profile = b.steps[step].profile.as_ref();
    let window = match &args.window {
        Some(w) => quantity::window(w, unit)?,
        None => margin(profile.map(|p| p.bounds()).or(drawn).ok_or_else(|| {
            Error::new(
                Code::InvalidParam,
                "nothing to draw: the layers are empty and the step has no profile",
            )
            .with("param", "layers")
        })?),
    };
    let mirror = side == Side::Bottom;
    let view = match size(args.size.as_deref())? {
        Size::Long(n) => View::fitting(window, n, mirror),
        Size::Exact(w, h) => View {
            window,
            width: w,
            height: h,
            mirror,
        },
    };
    // Nanometres per pixel: the window fits the image.
    let per_px = ((window.max.x - window.min.x) as f64 / f64::from(view.width))
        .max((window.max.y - window.min.y) as f64 / f64::from(view.height));
    let edge = profile.map(|p| outline(p, (1.5 * per_px).ceil().max(1.0) as i64));
    Ok((view, per_px, edge))
}

/// The colours islands are drawn in, by their order.
pub const PALETTE: [[u8; 3]; 12] = [
    [230, 90, 70],
    [90, 170, 230],
    [120, 200, 90],
    [240, 190, 60],
    [180, 110, 220],
    [70, 200, 190],
    [240, 130, 180],
    [160, 160, 60],
    [100, 120, 240],
    [220, 140, 80],
    [90, 220, 140],
    [200, 200, 200],
];
/// Islands not of the net asked for.
pub const DIM: Ink = Ink::Paint {
    colour: [80, 80, 80],
    alpha: 160,
};
/// The net asked for.
pub const HIGHLIGHT: Ink = Ink::Paint {
    colour: [255, 230, 40],
    alpha: 255,
};

/// Polygons as a shape of regions.
fn shape_of(p: &ryu_copper::poly::Polygons) -> Shape {
    Shape::regions(
        Polarity::Positive,
        p.polygons.iter().filter_map(|g| {
            let outer = ryu_board::shape::polygon(&g.outer).ok()?;
            let holes = g
                .holes
                .iter()
                .filter_map(|h| ryu_board::shape::polygon(h).ok())
                .collect();
            Some(ryu_board::geom::Region { outer, holes })
        }),
    )
}

/// What a group of a merged render draws: an item's own shape, a drill
/// feature's, or a shape made here.
enum Drawn {
    Item(Key),
    Feature(usize, usize),
    Made(usize),
}

/// A render layer of a merged render: its name, ink and what it draws.
type Group = (String, Ink, Vec<Drawn>);

/// Copper layers of board `cb` merged, each island in a colour of
/// [`PALETTE`] by its order; with `net`, that net's islands in
/// [`HIGHLIGHT`] and the rest in [`DIM`]; with `offset`, each layer's
/// merged copper grown or shrunk, in the layer's own colour. Drill layers
/// cut through above. The board step only.
pub fn render_copper(
    cb: &CopperBoard,
    prefix: Option<&str>,
    args: &Args,
    unit: Unit,
) -> Result<Answer> {
    let b = &*cb.board;
    let step = cb.built.step;
    if args.step.as_deref().is_some_and(|s| s != cb.step_name()) {
        return Err(Error::new(
            Code::InvalidParam,
            "merged copper is drawn for the board step only",
        )
        .with("param", "step"));
    }
    let side = args.side.unwrap_or_default();
    let (copper, drills): (Vec<usize>, Vec<usize>) = if args.layers.is_empty() {
        let outer = cb.copper.outer(match side {
            Side::Top => PointSide::Top,
            Side::Bottom => PointSide::Bottom,
        });
        (
            outer.into_iter().collect(),
            (0..b.layers.len())
                .filter(|&i| b.layers[i].kind == LayerKind::Drill)
                .collect(),
        )
    } else {
        let (mut c, mut d) = (Vec::new(), Vec::new());
        for n in &args.layers {
            match b.layer(n) {
                Some(bl) if b.layers[bl].kind == LayerKind::Drill => d.push(bl),
                _ => c.push(cb.layer_of(n)?),
            }
        }
        (c, d)
    };
    let highlight: Option<BTreeSet<IslandId<Key>>> = match &args.net {
        None => None,
        Some(n) => {
            if !cb.built.layout.points.iter().any(|p| &p.net == n) {
                return Err(Error::new(
                    Code::NotFound,
                    format!("no net {n} in the board's netlist"),
                )
                .with("net", n));
            }
            Some(
                cb.nets
                    .iter()
                    .filter(|(_, s)| s.contains(n))
                    .map(|(id, _)| *id)
                    .collect(),
            )
        }
    };
    let offset = args
        .offset
        .as_deref()
        .map(|t| quantity::length(t, unit))
        .transpose()?;
    let islands = cb.copper.islands();
    let order: BTreeMap<IslandId<Key>, usize> =
        islands.iter().enumerate().map(|(i, x)| (x.id, i)).collect();
    let mut made: Vec<Shape> = Vec::new();
    let mut groups: Vec<Group> = Vec::new();
    let mut shown: BTreeSet<IslandId<Key>> = BTreeSet::new();
    for &l in &copper {
        let name = cb.layer_name(l).to_owned();
        if let Some(d) = offset {
            let m = cb.merged(l);
            let fo = ryu_copper::poly::FlattenOptions::new(cb.copper.options().bound());
            let g = match d.signum() {
                1 => ryu_copper::poly::grow(&m, d, &fo),
                -1 => ryu_copper::poly::shrink(&m, -d, &fo),
                _ => (*m).clone(),
            };
            made.push(shape_of(&g));
            groups.push((
                name,
                ink(&b.layers[cb.built.copper[l]]),
                vec![Drawn::Made(made.len() - 1)],
            ));
            continue;
        }
        // One group per colour, in the palette's order (or dim, then
        // highlight), each holding its islands' pieces in key order.
        let slots = if highlight.is_some() {
            2
        } else {
            PALETTE.len()
        };
        let mut by: Vec<Vec<Drawn>> = (0..slots).map(|_| Vec::new()).collect();
        let slot = |id: IslandId<Key>, shown: &mut BTreeSet<IslandId<Key>>| -> usize {
            let s = match &highlight {
                Some(h) => usize::from(h.contains(&id)),
                None => order[&id] % PALETTE.len(),
            };
            shown.insert(id);
            s
        };
        for (k, it) in cb.copper.items() {
            if it.layer != l {
                continue;
            }
            match cb.copper.resolved(k) {
                None => {
                    if let Some(id) = cb.copper.island_of(&Node::Piece(*k, 0)) {
                        by[slot(id, &mut shown)].push(Drawn::Item(*k));
                    }
                }
                Some(pieces) => {
                    for (i, g) in pieces.iter().enumerate() {
                        if let Some(id) = cb.copper.island_of(&Node::Piece(*k, i as u32)) {
                            made.push(shape_of(g));
                            by[slot(id, &mut shown)].push(Drawn::Made(made.len() - 1));
                        }
                    }
                }
            }
        }
        for (s, drawn) in by.into_iter().enumerate() {
            if drawn.is_empty() {
                continue;
            }
            let (label, ink) = match &highlight {
                Some(_) if s == 1 => (format!("{name} highlight"), HIGHLIGHT),
                Some(_) => (format!("{name} dim"), DIM),
                None => (
                    format!("{name} #{s}"),
                    Ink::Paint {
                        colour: PALETTE[s],
                        alpha: 230,
                    },
                ),
            };
            groups.push((label, ink, drawn));
        }
    }
    b.build_shapes(step, &drills);
    for &d in &drills {
        let n = b.steps[step].layers[d].features().len();
        groups.push((
            b.layers[d].name.clone(),
            Ink::Cut,
            (0..n).map(|i| Drawn::Feature(d, i)).collect(),
        ));
    }
    let resolve = |d: &Drawn| -> Option<&Shape> {
        match d {
            Drawn::Item(k) => cb.copper.item(k).map(|it| &it.shape),
            Drawn::Feature(l, i) => b.feature_shape(step, *l, *i).as_ref().ok(),
            Drawn::Made(i) => Some(&made[*i]),
        }
    };
    let layers: Vec<render::Layer> = groups
        .iter()
        .map(|(name, ink, drawn)| render::Layer {
            name: name.clone(),
            ink: *ink,
            shapes: drawn.iter().filter_map(resolve).collect(),
        })
        .collect();
    let (view, per_px, edge) = frame(
        b,
        step,
        render::bounds(layers.iter().flat_map(|l| l.shapes.iter().copied())),
        args,
        unit,
        side,
    )?;
    let mut scene = Scene {
        view,
        background: BACKGROUND,
        layers,
    };
    if let Some(e) = &edge {
        scene.layers.push(render::Layer {
            name: "profile".into(),
            ink: PROFILE,
            shapes: vec![e],
        });
    }
    let format = args.format.unwrap_or_default();
    let result = json!({
        "board": prefix,
        "step": cb.step_name(),
        "side": side,
        "format": format,
        "merged": true,
        "layers": copper
            .iter()
            .map(|&l| cb.layer_name(l))
            .chain(drills.iter().map(|&d| b.layers[d].name.as_str()))
            .collect::<Vec<_>>(),
        "islands": shown.len(),
        "net": args.net,
        "offset_nm": offset,
        "profile": edge.is_some(),
        "width": view.width,
        "height": view.height,
        "window_nm": rect(view.window),
        "nm_per_px": per_px,
    });
    Ok(answer(&scene, format, result))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn minimal() -> Board {
        let path =
            std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/corpus/minimal-clean.tgz");
        crate::odb::read_path(&path, &Default::default()).unwrap()
    }

    #[test]
    fn sizes() {
        assert_eq!(size(None).unwrap(), Size::Long(1600));
        assert_eq!(size(Some("800")).unwrap(), Size::Long(800));
        assert_eq!(size(Some("640x480")).unwrap(), Size::Exact(640, 480));
        for bad in ["0", "16385", "axb", "10x", ""] {
            assert_eq!(
                size(Some(bad)).unwrap_err().code,
                Code::InvalidParam,
                "{bad}"
            );
        }
    }

    #[test]
    fn default_layers_by_side() {
        let b = minimal();
        let names = |side| {
            default_layers(&b, side)
                .into_iter()
                .map(|i| (b.layers[i].name.clone(), b.layers[i].kind.clone()))
                .collect::<Vec<_>>()
        };
        let top = names(Side::Top);
        assert!(top.iter().any(|(_, k)| k.is_copper()));
        assert!(matches!(top.last().unwrap().1, LayerKind::Drill));
        // The top copper is drawn after (over) the mask below it isn't: the
        // matrix lists the stack top down, so the top view starts lowest.
        let bottom = names(Side::Bottom);
        assert_ne!(top, bottom);
    }

    #[test]
    fn renders_answer_their_bytes() {
        let b = minimal();
        let args = Args {
            size: Some("200".into()),
            ..Default::default()
        };
        let a = render(&b, Some("mc"), &args, Unit::Mm).unwrap();
        assert_eq!(a.result["board"], json!("mc"));
        let (w, h) = (
            a.result["width"].as_u64().unwrap(),
            a.result["height"].as_u64().unwrap(),
        );
        assert_eq!(w.max(h), 200);
        let png = a.binary.unwrap();
        assert_eq!(a.result["binary"]["length"], json!(png.len()));
        assert!(png.starts_with(b"\x89PNG"));
        let svg = render(
            &b,
            None,
            &Args {
                format: Some(Format::Svg),
                side: Some(Side::Bottom),
                ..args
            },
            Unit::Mm,
        )
        .unwrap();
        let text = String::from_utf8(svg.binary.unwrap()).unwrap();
        assert!(text.starts_with("<svg"));
        assert!(text.contains("data-layer=\"profile\""));
        let e = render(
            &b,
            None,
            &Args {
                layers: vec!["nope".into()],
                ..Default::default()
            },
            Unit::Mm,
        )
        .unwrap_err();
        assert_eq!(e.code, Code::NotFound);
        let w = render(
            &b,
            None,
            &Args {
                window: Some("0,0,10,5".into()),
                size: Some("100".into()),
                ..Default::default()
            },
            Unit::Mm,
        )
        .unwrap();
        assert_eq!(w.result["window_nm"], json!([0, 0, 10_000_000, 5_000_000]));
        assert_eq!(
            (w.result["width"].clone(), w.result["height"].clone()),
            (json!(100), json!(50))
        );
    }

    #[test]
    fn merged_copper_by_island() {
        let cb = crate::copper::CopperBoard::new(std::sync::Arc::new(minimal()));
        let args = Args {
            merged: true,
            size: Some("300".into()),
            ..Default::default()
        };
        let a = render_copper(&cb, Some("mc"), &args, Unit::Mm).unwrap();
        assert!(a.binary.unwrap().starts_with(b"\x89PNG"));
        assert_eq!(a.result["merged"], json!(true));
        assert_eq!(a.result["layers"][0], json!("top"));
        assert!(a.result["islands"].as_u64().unwrap() >= 2);
        let svg = render_copper(
            &cb,
            None,
            &Args {
                net: Some("GND".into()),
                format: Some(Format::Svg),
                ..args.clone()
            },
            Unit::Mm,
        )
        .unwrap();
        let text = String::from_utf8(svg.binary.unwrap()).unwrap();
        assert!(
            text.contains("data-layer=\"top highlight\""),
            "{}",
            &text[..200]
        );
        for o in ["0.1", "-0.05"] {
            let v = render_copper(
                &cb,
                None,
                &Args {
                    offset: Some(o.into()),
                    ..args.clone()
                },
                Unit::Mm,
            )
            .unwrap();
            assert!(v.result["offset_nm"].is_i64());
        }
        let bad = |a: Args| render_copper(&cb, None, &a, Unit::Mm).unwrap_err().code;
        assert_eq!(
            bad(Args {
                layers: vec!["top_overlay".into()],
                ..args.clone()
            }),
            Code::InvalidParam
        );
        assert_eq!(
            bad(Args {
                net: Some("NOPE".into()),
                ..args.clone()
            }),
            Code::NotFound
        );
        assert_eq!(
            bad(Args {
                step: Some("panel".into()),
                ..args
            }),
            Code::InvalidParam
        );
    }
}
