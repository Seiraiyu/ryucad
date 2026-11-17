//! `board render`: a board's layers drawn by `ryu-board`'s renderer, PNG
//! or SVG, in fixed colours per layer on a dark background (ryudfm's
//! colours), the view framing the outline with a millimetre's margin;
//! from below it is mirrored. With `net`, that net's copper is
//! highlighted and the rest dimmed.

use super::shapes::BoardShapes;
use crate::error::{Code, Error, Result};
use ryu_board::geom::Rect;
use ryu_board::render::{self, Ink, Layer, MAX_SIDE, Scene, View};
use ryu_board::shape::{Part, Piece, Shape};
use ryu_board::model::Polarity;
use serde_json::{Value, json};

pub const DEFAULT_SIZE: u32 = 1600;
pub const BACKGROUND: [u8; 3] = [16, 20, 24];
const MARGIN_NM: i64 = 1_000_000;

/// Every layer `board render` knows.
pub const LAYERS: [&str; 14] = [
    "copper.top", "copper.bottom", "mask.top", "mask.bottom", "paste.top", "paste.bottom",
    "silk.top", "silk.bottom", "fab.top", "fab.bottom", "courtyard.top", "courtyard.bottom",
    "drill", "outline",
];

pub fn ink(layer: &str) -> Ink {
    let paint = |colour, alpha| Ink::Paint { colour, alpha };
    match layer {
        "copper.top" => paint([200, 130, 50], 230),
        "copper.bottom" => paint([70, 130, 210], 230),
        "mask.top" | "mask.bottom" => paint([30, 160, 90], 110),
        "paste.top" | "paste.bottom" => paint([170, 170, 180], 200),
        "silk.top" | "silk.bottom" => paint([235, 235, 235], 240),
        "fab.top" | "fab.bottom" => paint([170, 110, 210], 160),
        "courtyard.top" | "courtyard.bottom" => paint([230, 90, 200], 160),
        "drill" => Ink::Cut,
        "outline" => paint([240, 200, 40], 255),
        _ => paint([170, 110, 210], 160),
    }
}

const DIM: Ink = Ink::Paint { colour: [80, 80, 80], alpha: 200 };
const HIGHLIGHT: Ink = Ink::Paint { colour: [255, 230, 40], alpha: 255 };

pub fn default_layers(bottom: bool) -> Vec<String> {
    let s = if bottom { "bottom" } else { "top" };
    vec![format!("copper.{s}"), format!("mask.{s}"), format!("silk.{s}"), "drill".into(), "outline".into()]
}

fn hole_shape(h: &ryu_copper::layout::HoleShape) -> Shape {
    use ryu_copper::layout::HoleShape as H;
    let region = match h {
        H::Round { at, d } => ryu_board::shape::circle(*at, ryu_board::geom::half(*d)).ok().map(ryu_board::geom::Region::solid),
        H::Slot { a, b, d } => ryu_board::shape::line_outline(*a, *b, ryu_board::geom::Pen::Round(*d)),
    };
    Shape::regions(Polarity::Positive, region)
}

/// The outline's boundary drawn a tenth of a millimetre wide.
fn outline_drawn(s: &BoardShapes) -> Shape {
    Shape {
        parts: s
            .edges
            .parts
            .iter()
            .map(|p| match &p.piece {
                Piece::Stroke(st) => Part {
                    polarity: Polarity::Positive,
                    piece: Piece::Stroke(ryu_board::geom::Stroke { path: st.path.clone(), pen: ryu_board::geom::Pen::Round(100_000) }),
                },
                other => Part { polarity: p.polarity, piece: other.clone() },
            })
            .collect(),
        ..Shape::default()
    }
}

/// What to draw.
pub struct Ask<'a> {
    pub layers: &'a [String],
    pub bottom: bool,
    pub net: Option<&'a str>,
    pub window: Option<Rect>,
    pub size: u32,
    pub svg: bool,
}

/// The picture's bytes and the answer's fields.
pub fn render(board: &str, s: &BoardShapes, ask: &Ask) -> Result<(Value, &'static str, Vec<u8>)> {
    if ask.size == 0 || ask.size > MAX_SIDE {
        return Err(Error::new(Code::InvalidParam, format!("--size is 1 to {MAX_SIDE} pixels")));
    }
    let layers: Vec<String> = if ask.layers.is_empty() { default_layers(ask.bottom) } else { ask.layers.to_vec() };
    if let Some(bad) = layers.iter().find(|l| !LAYERS.contains(&l.as_str())) {
        return Err(Error::new(Code::InvalidParam, format!("there is no layer {bad}; layers are {}", LAYERS.join(", "))).with("layer", bad));
    }
    if let Some(n) = ask.net
        && !s.pads.iter().any(|p| p.net.as_deref() == Some(n))
    {
        return Err(Error::new(Code::NotFound, format!("no placed pad is on net {n}")).with("net", n));
    }
    let holes: Vec<Shape> = s.holes.iter().map(|h| hole_shape(&h.shape)).collect();
    let outline = outline_drawn(s);
    // The net's pads' copper, per side.
    let net_copper = |top: bool| -> Vec<&Shape> {
        s.pads
            .iter()
            .filter(|p| ask.net.is_some() && p.net.as_deref() == ask.net)
            .filter(|p| if top { p.top } else { p.bottom })
            .map(|p| &p.copper)
            .collect()
    };
    let mut scene_layers = Vec::new();
    for l in &layers {
        let shapes: Vec<&Shape> = match l.as_str() {
            "drill" => holes.iter().collect(),
            "outline" => vec![&outline],
            other => s.layers.get(other).map(|v| v.iter().map(|(_, sh)| sh).collect()).unwrap_or_default(),
        };
        let copper = l.starts_with("copper.");
        let ink = if ask.net.is_some() && copper { DIM } else { ink(l) };
        scene_layers.push(Layer { name: l.clone(), ink, shapes });
        if ask.net.is_some() && copper {
            scene_layers.push(Layer { name: format!("{l}:net"), ink: HIGHLIGHT, shapes: net_copper(l == "copper.top") });
        }
    }
    let window = ask.window.unwrap_or_else(|| {
        let b = s.outline.bounds().or_else(|| render::bounds(s.layers.values().flatten().map(|(_, sh)| sh)));
        let b = b.unwrap_or(Rect::new(ryu_board::geom::Point::ORIGIN, ryu_board::geom::Point::new(1_000_000, 1_000_000)));
        Rect { min: b.min.offset(-MARGIN_NM, -MARGIN_NM), max: b.max.offset(MARGIN_NM, MARGIN_NM) }
    });
    let view = View::fitting(window, ask.size, ask.bottom);
    let scene = Scene { view, background: BACKGROUND, layers: scene_layers };
    let (mime, bytes) = if ask.svg {
        ("image/svg+xml", render::svg(&scene).into_bytes())
    } else {
        ("image/png", render::raster(&scene).png())
    };
    let v = json!({
        "board": format!("board/{board}"),
        "layers": layers,
        "side": if ask.bottom { "bottom" } else { "top" },
        "net": ask.net,
        "width": view.width,
        "height": view.height,
        "window": [window.min.x, window.min.y, window.max.x, window.max.y],
    });
    Ok((v, mime, bytes))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn renders_are_the_same_bytes_on_any_thread_count() {
        let s = BoardShapes {
            outline: Shape::regions(Polarity::Positive, [ryu_board::geom::Region::solid(ryu_board::shape::rect(0, 0, 10_000_000, 5_000_000, &ryu_board::symbol::Corners::SHARP).unwrap())]),
            ..BoardShapes::default()
        };
        let ask = Ask { layers: &[], bottom: false, net: None, window: None, size: 200, svg: false };
        let run = |n| rayon::ThreadPoolBuilder::new().num_threads(n).build().unwrap().install(|| render("b", &s, &ask).unwrap().2);
        assert_eq!(run(1), run(8));
        let ask = Ask { size: 0, ..ask };
        assert_eq!(render("b", &s, &ask).unwrap_err().code, Code::InvalidParam);
    }
}
