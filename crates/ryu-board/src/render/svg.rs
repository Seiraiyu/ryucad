//! The same scene as SVG: shapes as paths in integer nanometres (y
//! negated, and x too when seen from below), arcs as SVG arcs, a group per
//! layer at its opacity, and each run of negative parts a mask over what
//! the layer drew before it. The text is fixed by construction: layers and
//! parts in order, masks numbered in order, no time and nothing random.

use std::fmt::Write;

use super::flatten;
use super::{Ink, Map, Scene, View};
use crate::geom::{Arc, Contour, Dir, Edge, Nm, Pen, Point, round_nm};
use crate::model::Polarity;
use crate::shape::{Piece, Shape};

/// Writes `scene` as SVG.
pub fn svg(scene: &Scene) -> String {
    let v = scene.view;
    let mirror = v.mirror;
    let shown = v.visible();
    let (w, h) = (shown.max.x - shown.min.x, shown.max.y - shown.min.y);
    let vx = if mirror { -shown.max.x } else { shown.min.x };
    let vy = -shown.max.y;
    let mut out = String::new();
    writeln!(
        out,
        r#"<svg xmlns="http://www.w3.org/2000/svg" width="{}" height="{}" viewBox="{vx} {vy} {w} {h}">"#,
        v.width, v.height
    )
    .unwrap();
    // The background, and masks' white, are paths: browsers drew no
    // `rect` this large (Chrome's coordinates are 32-bit floats), but do
    // draw paths.
    let all = format!("M{vx} {vy} h{w} v{h} h-{w}Z");
    writeln!(out, r#"<path d="{all}" fill="{}"/>"#, hex(scene.background)).unwrap();
    let mut masks = 0usize;
    // Square pens and pens of their own are written as polygons, their
    // arcs as chords straying at most a quarter pixel, as the
    // rasterizer's do (but at least a nanometre).
    let tol = (0.25 / v.map(1.0).scale()).max(1.0);
    let map = Map::svg(mirror);
    for layer in &scene.layers {
        let (colour, opacity) = match layer.ink {
            Ink::Paint { colour, alpha } => (hex(colour), alpha),
            Ink::Cut => (hex(scene.background), 255),
        };
        let mut content = String::new();
        let mut negatives: Vec<String> = Vec::new();
        let close_mask = |content: &mut String, negatives: &mut Vec<String>, masks: &mut usize| {
            if negatives.is_empty() {
                return;
            }
            *masks += 1;
            let id = format!("m{masks}");
            let mut m = String::new();
            writeln!(
                m,
                r##"<mask id="{id}" maskUnits="userSpaceOnUse" x="{vx}" y="{vy}" width="{w}" height="{h}"><path d="{all}" fill="#fff"/>"##
            )
            .unwrap();
            for n in negatives.drain(..) {
                m.push_str(&n);
            }
            m.push_str("</mask>\n");
            *content = format!("{m}<g mask=\"url(#{id})\">\n{content}</g>\n");
        };
        for shape in layer.shapes.iter().filter(|s| View::shows(&shown, s)) {
            for part in &shape.parts {
                match part.polarity {
                    Polarity::Positive => {
                        close_mask(&mut content, &mut negatives, &mut masks);
                        content.push_str(&element(&part.piece, &colour, &map, tol));
                    }
                    Polarity::Negative => negatives.push(element(&part.piece, "#000", &map, tol)),
                }
            }
        }
        close_mask(&mut content, &mut negatives, &mut masks);
        let opacity = if opacity == 255 {
            String::new()
        } else {
            format!(" opacity=\"{}\"", alpha(opacity))
        };
        writeln!(
            out,
            "<g data-layer=\"{}\"{opacity}>\n{content}</g>",
            escape(&layer.name)
        )
        .unwrap();
    }
    out.push_str("</svg>\n");
    out
}

/// An opacity out of 255 as a decimal with three places.
fn alpha(a: u8) -> String {
    let thousandths = (u32::from(a) * 1000 + 127) / 255;
    format!("{}.{:03}", thousandths / 1000, thousandths % 1000)
}

fn hex(c: [u8; 3]) -> String {
    format!("#{:02x}{:02x}{:02x}", c[0], c[1], c[2])
}

fn escape(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
}

impl Map {
    /// Board nanometres as SVG user units: y negated, and x too from
    /// below.
    fn svg(mirror: bool) -> Map {
        Map {
            x0: 0,
            y0: 0,
            k: 1.0,
            dx: 0.0,
            dy: 0.0,
            mirror,
        }
    }
}

/// A point as SVG coordinates.
fn xy(p: Point, mirror: bool) -> (Nm, Nm) {
    (if mirror { -p.x } else { p.x }, -p.y)
}

/// The SVG arc command to `arc`'s end: its radius in whole nanometres,
/// the large arc flag past half a turn, and the sweep flag, 1 where the
/// arc turns with SVG's angles (y down: a counterclockwise arc on the
/// board turns that way only when x is negated too).
fn arc_to(d: &mut String, a: &Arc, to: Point, mirror: bool, large: bool) {
    let r = round_nm(a.radius()).max(1);
    let sweep = u8::from((a.dir == Dir::Ccw) == mirror);
    let (x, y) = xy(to, mirror);
    write!(d, " A{r} {r} 0 {} {sweep} {x} {y}", u8::from(large)).unwrap();
}

fn edge_to(d: &mut String, e: &Edge, mirror: bool) {
    match e {
        Edge::Segment(s) => {
            let (x, y) = xy(s.b, mirror);
            write!(d, " L{x} {y}").unwrap();
        }
        Edge::Arc(a) if a.is_degenerate() => {
            let (x, y) = xy(a.end, mirror);
            write!(d, " L{x} {y}").unwrap();
        }
        Edge::Arc(a) if a.is_full() => {
            // Two halves, through the point opposite the start.
            let far = Point::new(2 * a.centre.x - a.start.x, 2 * a.centre.y - a.start.y);
            arc_to(d, a, far, mirror, false);
            arc_to(d, a, a.end, mirror, false);
        }
        Edge::Arc(a) => arc_to(d, a, a.end, mirror, a.sweep() > std::f64::consts::PI),
    }
}

fn contour_d(d: &mut String, c: &Contour, mirror: bool) {
    let (x, y) = xy(c.edges()[0].start(), mirror);
    write!(d, "M{x} {y}").unwrap();
    for e in c.edges() {
        edge_to(d, e, mirror);
    }
    d.push('Z');
}

fn path_d(d: &mut String, path: &[Edge], mirror: bool) {
    let (x, y) = xy(path[0].start(), mirror);
    write!(d, "M{x} {y}").unwrap();
    for e in path {
        edge_to(d, e, mirror);
    }
}

/// Rings of image points as a path, rounded to whole nanometres.
fn rings_d(d: &mut String, rings: &[flatten::Ring]) {
    for ring in rings {
        for (i, p) in ring.iter().enumerate() {
            let c = if i == 0 { 'M' } else { 'L' };
            if i == 0 && !d.is_empty() {
                d.push(' ');
            }
            write!(d, "{c}{} {}", round_nm(p.0), round_nm(p.1)).unwrap();
            if i + 1 < ring.len() {
                d.push(' ');
            }
        }
        d.push('Z');
    }
}

/// One part as an SVG element painted `paint`.
fn element(piece: &Piece, paint: &str, map: &Map, tol: f64) -> String {
    let mirror = map.mirror;
    match piece {
        Piece::Region(r) => {
            let mut d = String::new();
            for (i, c) in std::iter::once(&r.outer).chain(&r.holes).enumerate() {
                if i > 0 {
                    d.push(' ');
                }
                contour_d(&mut d, c, mirror);
            }
            format!("<path d=\"{d}\" fill=\"{paint}\"/>\n")
        }
        Piece::Stroke(s) if matches!(s.pen, Pen::Round(_)) => {
            let Pen::Round(w) = s.pen else { unreachable!() };
            let mut d = String::new();
            path_d(&mut d, &s.path, mirror);
            format!(
                "<path d=\"{d}\" fill=\"none\" stroke=\"{paint}\" stroke-width=\"{w}\" stroke-linecap=\"round\" stroke-linejoin=\"round\"/>\n"
            )
        }
        piece => {
            // Square pens keep their sides along the axes, and pens of
            // their own their shape, which SVG's strokes don't: their
            // sweeps as polygons.
            let one = Shape {
                parts: vec![crate::shape::Part {
                    polarity: Polarity::Positive,
                    piece: piece.clone(),
                }],
                ..Shape::default()
            };
            let mut d = String::new();
            for flat in flatten::shape(&one, map, tol) {
                for fill in &flat.fills {
                    rings_d(&mut d, fill);
                }
            }
            format!("<path d=\"{d}\" fill=\"{paint}\"/>\n")
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::geom::{Rect, Region, Stroke};
    use crate::render::{Layer, View};
    use crate::shape::{circle, rect};
    use crate::symbol::Corners;

    fn scene<'a>(layers: Vec<Layer<'a>>, mirror: bool) -> Scene<'a> {
        Scene {
            view: View {
                window: Rect::new(Point::new(0, 0), Point::new(1000, 500)),
                width: 200,
                height: 100,
                mirror,
            },
            background: [16, 20, 24],
            layers,
        }
    }

    #[test]
    fn regions_arcs_and_masks() {
        let back = Shape::regions(
            Polarity::Positive,
            [Region::solid(
                rect(0, 0, 1000, 500, &Corners::SHARP).unwrap(),
            )],
        );
        let hole = Shape::regions(
            Polarity::Negative,
            [Region::solid(circle(Point::new(500, 250), 100).unwrap())],
        );
        let s = svg(&scene(
            vec![Layer {
                name: "top & co".into(),
                ink: Ink::Paint {
                    colour: [200, 130, 50],
                    alpha: 230,
                },
                shapes: vec![&back, &hole],
            }],
            false,
        ));
        assert_eq!(
            s,
            concat!(
                "<svg xmlns=\"http://www.w3.org/2000/svg\" width=\"200\" height=\"100\" viewBox=\"0 -500 1000 500\">\n",
                "<path d=\"M0 -500 h1000 v500 h-1000Z\" fill=\"#101418\"/>\n",
                "<g data-layer=\"top &amp; co\" opacity=\"0.902\">\n",
                "<mask id=\"m1\" maskUnits=\"userSpaceOnUse\" x=\"0\" y=\"-500\" width=\"1000\" height=\"500\"><path d=\"M0 -500 h1000 v500 h-1000Z\" fill=\"#fff\"/>\n",
                "<path d=\"M600 -250 A100 100 0 0 0 400 -250 A100 100 0 0 0 600 -250Z\" fill=\"#000\"/>\n",
                "</mask>\n",
                "<g mask=\"url(#m1)\">\n",
                "<path d=\"M1000 0 L1000 -500 L0 -500 L0 0 L1000 0Z\" fill=\"#c88232\"/>\n",
                "</g>\n",
                "</g>\n",
                "</svg>\n"
            )
        );
    }

    #[test]
    fn mirrored_arcs_keep_their_turn() {
        let c = Shape::regions(
            Polarity::Positive,
            [Region::solid(circle(Point::new(500, 250), 100).unwrap())],
        );
        let s = svg(&scene(
            vec![Layer {
                name: "drill".into(),
                ink: Ink::Cut,
                shapes: vec![&c],
            }],
            true,
        ));
        assert!(s.contains("viewBox=\"-1000 -500 1000 500\""), "{s}");
        assert!(
            s.contains("<path d=\"M-600 -250 A100 100 0 0 1 -400 -250 A100 100 0 0 1 -600 -250Z\" fill=\"#101418\"/>"),
            "{s}"
        );
    }

    #[test]
    fn pens_round_and_square() {
        let round = Shape {
            parts: vec![crate::shape::Part {
                polarity: Polarity::Positive,
                piece: Piece::Stroke(Stroke {
                    path: vec![Edge::line(Point::new(0, 0), Point::new(100, 0))],
                    pen: Pen::Round(20),
                }),
            }],
            ..Shape::default()
        };
        let square = Shape {
            parts: vec![crate::shape::Part {
                polarity: Polarity::Positive,
                piece: Piece::Stroke(Stroke {
                    path: vec![Edge::line(Point::new(0, 0), Point::new(100, 100))],
                    pen: Pen::Square(20),
                }),
            }],
            ..Shape::default()
        };
        let s = svg(&scene(
            vec![Layer {
                name: "silk".into(),
                ink: Ink::Paint {
                    colour: [255, 255, 255],
                    alpha: 255,
                },
                shapes: vec![&round, &square],
            }],
            false,
        ));
        assert!(s.contains("<path d=\"M0 0 L100 0\" fill=\"none\" stroke=\"#ffffff\" stroke-width=\"20\" stroke-linecap=\"round\" stroke-linejoin=\"round\"/>"), "{s}");
        assert!(
            s.contains("<path d=\"M-10 -10 L90 -110 L110 -110 L110 -90 L10 10 L-10 10Z\" fill=\"#ffffff\"/>"),
            "{s}"
        );
    }
}
