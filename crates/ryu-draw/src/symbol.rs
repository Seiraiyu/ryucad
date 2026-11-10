//! Drafting symbols as strokes (design 417): the diameter, counterbore,
//! countersink and depth signs of hole callouts, the 14 GD&T
//! characteristics of ASME Y14.5 and ISO 1101, circled material
//! conditions, and text mixed with them. A symbol is drawn, never taken
//! from a font, so every writer and reader shows it alike; letters are
//! Inter's.
//!
//! Each symbol is a few strokes in em units of the text it stands in: x
//! from 0 to its advance, its baseline at 0, Inter's capitals 0.727 high.

use crate::font;
use crate::geom::{P, Path, Segment};
use crate::item::{Anchor, Item};
use crate::style::{Colour, Stroke};

/// A symbol drawn as strokes.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Symbol {
    /// ⌀
    Diameter,
    /// ⌴
    Counterbore,
    /// ⌵
    Countersink,
    /// ↧
    Depth,
    Straightness,
    Flatness,
    Circularity,
    Cylindricity,
    ProfileLine,
    ProfileSurface,
    Angularity,
    Perpendicularity,
    Parallelism,
    Position,
    Concentricity,
    Symmetry,
    CircularRunout,
    TotalRunout,
    /// A letter in a circle: Ⓜ, Ⓛ, Ⓟ, …
    Circled(char),
}

/// How tall Inter's capitals stand, in em.
pub const CAP: f64 = 0.727;

/// A piece of text: words, or a symbol.
#[derive(Clone, Debug, PartialEq)]
pub enum Run {
    Text(String),
    Symbol(Symbol),
}

fn line(a: P, b: P) -> Path {
    Path::polyline(&[a, b], false)
}

fn poly(points: &[P], closed: bool) -> Path {
    Path::polyline(points, closed)
}

fn circle(c: P, r: f64) -> Path {
    Path::circle(c, r)
}

fn arc(c: P, r: f64, from: f64, sweep: f64) -> Path {
    let (s, co) = ryu_math::sin_cos(from);
    Path {
        start: [c[0] + r * co, c[1] + r * s],
        segments: vec![Segment::Arc {
            centre: c,
            radius: r,
            from,
            sweep,
        }],
        closed: false,
    }
}

impl Symbol {
    /// Its width in em, the space after it included.
    pub fn advance(self) -> f64 {
        match self {
            Symbol::Diameter => 0.78,
            Symbol::Counterbore | Symbol::Countersink => 0.8,
            Symbol::Depth => 0.6,
            Symbol::Circled(_) => 0.9,
            _ => 0.9,
        }
    }

    /// Its strokes in em, from its baseline's start.
    fn strokes(self) -> Vec<Path> {
        use std::f64::consts::PI;
        let m = CAP / 2.0;
        match self {
            Symbol::Diameter => vec![
                circle([0.36, m], 0.29),
                line([0.06, -0.04], [0.66, CAP + 0.04]),
            ],
            Symbol::Counterbore => vec![poly(
                &[[0.08, CAP], [0.08, 0.1], [0.72, 0.1], [0.72, CAP]],
                false,
            )],
            Symbol::Countersink => vec![poly(&[[0.06, CAP], [0.4, 0.08], [0.74, CAP]], false)],
            Symbol::Depth => vec![
                line([0.3, CAP + 0.05], [0.3, 0.02]),
                poly(&[[0.12, 0.22], [0.3, 0.02], [0.48, 0.22]], false),
                line([0.06, 0.02], [0.54, 0.02]),
            ],
            Symbol::Straightness => vec![line([0.1, m], [0.8, m])],
            Symbol::Flatness => vec![poly(
                &[[0.1, 0.2], [0.62, 0.2], [0.8, 0.55], [0.28, 0.55]],
                true,
            )],
            Symbol::Circularity => vec![circle([0.45, m], 0.3)],
            Symbol::Cylindricity => vec![
                circle([0.45, m], 0.22),
                line([0.12, 0.02], [0.4, 0.72]),
                line([0.5, 0.0], [0.78, 0.7]),
            ],
            Symbol::ProfileLine => vec![arc([0.45, 0.15], 0.33, 0.0, PI)],
            Symbol::ProfileSurface => vec![
                arc([0.45, 0.15], 0.33, 0.0, PI),
                line([0.12, 0.15], [0.78, 0.15]),
            ],
            Symbol::Angularity => vec![poly(&[[0.8, 0.1], [0.1, 0.1], [0.72, 0.6]], false)],
            Symbol::Perpendicularity => vec![
                line([0.1, 0.08], [0.8, 0.08]),
                line([0.45, 0.08], [0.45, CAP]),
            ],
            Symbol::Parallelism => vec![
                line([0.12, 0.05], [0.42, CAP]),
                line([0.48, 0.05], [0.78, CAP]),
            ],
            Symbol::Position => vec![
                circle([0.45, m], 0.22),
                line([0.45, m - 0.36], [0.45, m + 0.36]),
                line([0.09, m], [0.81, m]),
            ],
            Symbol::Concentricity => vec![circle([0.45, m], 0.32), circle([0.45, m], 0.16)],
            Symbol::Symmetry => vec![
                line([0.08, m], [0.82, m]),
                line([0.24, m - 0.2], [0.66, m - 0.2]),
                line([0.24, m + 0.2], [0.66, m + 0.2]),
            ],
            Symbol::CircularRunout => vec![
                line([0.15, 0.05], [0.72, 0.62]),
                poly(&[[0.48, 0.66], [0.75, 0.66], [0.75, 0.39]], false),
            ],
            Symbol::TotalRunout => vec![
                line([0.05, 0.05], [0.85, 0.05]),
                line([0.1, 0.05], [0.45, 0.62]),
                line([0.45, 0.05], [0.8, 0.62]),
                poly(&[[0.27, 0.6], [0.47, 0.66], [0.52, 0.45]], false),
                poly(&[[0.62, 0.6], [0.82, 0.66], [0.87, 0.45]], false),
            ],
            Symbol::Circled(_) => vec![circle([0.45, m], 0.42)],
        }
    }

    /// What it draws with its baseline's start at `at`, its em `size`,
    /// turned by `angle` radians about `at`.
    pub fn items(self, at: P, size: f64, angle: f64, stroke: &Stroke) -> Vec<Item> {
        let (s, c) = ryu_math::sin_cos(angle);
        let place = |p: Path| -> Path {
            // Scaled about the origin, turned, moved: arcs stay arcs (a
            // turn keeps orientation), their angles turned too.
            let turn = |q: P| {
                [
                    at[0] + size * (c * q[0] - s * q[1]),
                    at[1] + size * (s * q[0] + c * q[1]),
                ]
            };
            Path {
                start: turn(p.start),
                segments: p
                    .segments
                    .iter()
                    .map(|g| match g {
                        Segment::Line(q) => Segment::Line(turn(*q)),
                        Segment::Arc {
                            centre,
                            radius,
                            from,
                            sweep,
                        } => Segment::Arc {
                            centre: turn(*centre),
                            radius: radius * size,
                            from: from + angle,
                            sweep: *sweep,
                        },
                        other => other.clone(),
                    })
                    .collect(),
                closed: p.closed,
            }
        };
        let mut out: Vec<Item> = self
            .strokes()
            .into_iter()
            .map(|p| Item::Path {
                path: place(p),
                stroke: Some(stroke.clone()),
                fill: None,
            })
            .collect();
        if let Symbol::Circled(l) = self {
            // The letter in the circle, its capital centred on it.
            let x = 0.45 * size;
            let y = (CAP / 2.0 - CAP * 0.62 / 2.0) * size;
            out.push(Item::Text {
                text: l.to_string(),
                at: [at[0] + c * x - s * y, at[1] + s * x + c * y],
                size: size * 0.62,
                angle,
                anchor: Anchor::Middle,
                colour: stroke.colour,
            });
        }
        out
    }
}

/// How wide `runs` are at em `size`.
pub fn width(runs: &[Run], size: f64) -> f64 {
    runs.iter()
        .map(|r| match r {
            Run::Text(t) => font::width(t, size),
            Run::Symbol(s) => s.advance() * size,
        })
        .sum()
}

/// `runs` drawn on one line: `at` on their baseline at `anchor`, turned by
/// `angle` about `at`; text in `colour`, symbols stroked by `stroke`.
pub fn draw(
    runs: &[Run],
    at: P,
    size: f64,
    angle: f64,
    anchor: Anchor,
    colour: Colour,
    stroke: &Stroke,
) -> Vec<Item> {
    let w = width(runs, size);
    let mut x = match anchor {
        Anchor::Start => 0.0,
        Anchor::Middle => -w / 2.0,
        Anchor::End => -w,
    };
    let (s, c) = ryu_math::sin_cos(angle);
    let mut out = Vec::new();
    for r in runs {
        let here = [at[0] + c * x, at[1] + s * x];
        match r {
            Run::Text(t) if !t.is_empty() => {
                out.push(Item::Text {
                    text: t.clone(),
                    at: here,
                    size,
                    angle,
                    anchor: Anchor::Start,
                    colour,
                });
                x += font::width(t, size);
            }
            Run::Text(_) => {}
            Run::Symbol(sym) => {
                let mut st = stroke.clone();
                st.colour = colour;
                out.extend(sym.items(here, size, angle, &st));
                x += sym.advance() * size;
            }
        }
    }
    out
}

/// A filled arrowhead: its point at `tip`, pointing along unit `dir`,
/// `length` long and `width` wide at its back.
pub fn arrow(tip: P, dir: P, length: f64, width: f64, colour: Colour) -> Item {
    let back = [tip[0] - dir[0] * length, tip[1] - dir[1] * length];
    let n = [-dir[1] * width / 2.0, dir[0] * width / 2.0];
    Item::Path {
        path: Path::polyline(
            &[
                tip,
                [back[0] + n[0], back[1] + n[1]],
                [back[0] - n[0], back[1] - n[1]],
            ],
            true,
        ),
        stroke: None,
        fill: Some(colour),
    }
}

/// A filled dot of `radius` at `at`: where a leader ends on a surface.
pub fn dot(at: P, radius: f64, colour: Colour) -> Item {
    Item::Path {
        path: Path::circle(at, radius),
        stroke: None,
        fill: Some(colour),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::style::{LineKind, Standard};

    #[test]
    fn every_symbol_stays_in_its_box() {
        let all = [
            Symbol::Diameter,
            Symbol::Counterbore,
            Symbol::Countersink,
            Symbol::Depth,
            Symbol::Straightness,
            Symbol::Flatness,
            Symbol::Circularity,
            Symbol::Cylindricity,
            Symbol::ProfileLine,
            Symbol::ProfileSurface,
            Symbol::Angularity,
            Symbol::Perpendicularity,
            Symbol::Parallelism,
            Symbol::Position,
            Symbol::Concentricity,
            Symbol::Symmetry,
            Symbol::CircularRunout,
            Symbol::TotalRunout,
            Symbol::Circled('M'),
        ];
        let stroke = Standard::Iso.stroke(LineKind::Thin);
        for s in all {
            let items = s.items([0.0, 0.0], 10.0, 0.0, &stroke);
            assert!(!items.is_empty(), "{s:?}");
            for it in items {
                let [lo, hi] = it.bounds();
                assert!(
                    lo[0] >= -0.5 && hi[0] <= 10.0 * s.advance() + 0.5,
                    "{s:?} {lo:?} {hi:?}"
                );
                assert!(lo[1] >= -1.0 && hi[1] <= 8.5, "{s:?} {lo:?} {hi:?}");
            }
        }
    }

    #[test]
    fn runs_advance_by_text_and_symbols() {
        let runs = [
            Run::Text("4× ".into()),
            Run::Symbol(Symbol::Diameter),
            Run::Text("6".into()),
        ];
        let w = width(&runs, 3.5);
        let expect = font::width("4× ", 3.5) + 0.78 * 3.5 + font::width("6", 3.5);
        assert_eq!(w, expect);
        let stroke = Standard::Asme.stroke(LineKind::Thin);
        let items = draw(
            &runs,
            [10.0, 0.0],
            3.5,
            0.0,
            Anchor::Middle,
            Colour::BLACK,
            &stroke,
        );
        let texts: Vec<&Item> = items
            .iter()
            .filter(|i| matches!(i, Item::Text { .. }))
            .collect();
        assert_eq!(texts.len(), 2);
        let Item::Text { at, .. } = texts[0] else {
            panic!()
        };
        assert!((at[0] - (10.0 - w / 2.0)).abs() < 1e-12);
    }

    #[test]
    fn a_turned_symbol_turns_its_arcs() {
        let stroke = Standard::Iso.stroke(LineKind::Thin);
        let items =
            Symbol::ProfileLine.items([0.0, 0.0], 10.0, std::f64::consts::FRAC_PI_2, &stroke);
        let Item::Path { path, .. } = &items[0] else {
            panic!()
        };
        let Segment::Arc { from, centre, .. } = path.segments[0] else {
            panic!()
        };
        assert!((from - std::f64::consts::FRAC_PI_2).abs() < 1e-12);
        assert!((centre[0] + 1.5).abs() < 1e-9 && (centre[1] - 4.5).abs() < 1e-9);
    }

    #[test]
    fn inter_has_the_signs_text_keeps() {
        // ±, × and ° are Inter's glyphs; ⌀, ⌴, ⌵ and ↧ are drawn here.
        for c in ['±', '×', '°'] {
            assert!(font::width(&c.to_string(), 1.0) > 0.3, "{c}");
        }
    }

    #[test]
    fn an_arrow_points_where_it_is_told() {
        let Item::Path { path, fill, .. } = arrow([0.0, 0.0], [1.0, 0.0], 3.0, 1.0, Colour::BLACK)
        else {
            panic!()
        };
        assert_eq!(fill, Some(Colour::BLACK));
        assert!(path.closed);
        assert_eq!(path.bounds(), [[-3.0, -0.5], [0.0, 0.5]]);
    }
}
