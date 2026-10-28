//! Shapes of the standard symbols, in their own coordinates.

use super::build::{circle, polygon, rect};
use super::{Shape, ShapeError, ellipse, thermal};
use crate::geom::{Contour, Mdeg, Nm, Place, Point, Region, div_round, half, span};
use crate::model::Polarity;
use crate::symbol::{Corners, StandardSymbol};

fn bad(why: &'static str) -> ShapeError {
    ShapeError::BadParameters(why)
}

fn positive(regions: Vec<Region>) -> Shape {
    Shape::regions(Polarity::Positive, regions)
}

/// An oval `w` by `h` centred on the origin: a circle when they are
/// equal, else a stadium along the longer side.
pub(crate) fn oval(w: Nm, h: Nm) -> Result<Contour, ShapeError> {
    if w <= 0 || h <= 0 {
        return Err(bad("an oval needs a width and a height"));
    }
    if h > w {
        // The tall oval is the wide one turned a quarter, exactly.
        let wide = oval(h, w)?;
        return Ok(Place {
            rot: Mdeg::new(90_000),
            ..Place::IDENTITY
        }
        .contour(&wide));
    }
    let r = half(h);
    let (x0, x1) = span(w);
    let (a, b) = (x0 + r, x1 - r);
    if a >= b {
        return circle(Point::new(div_round(i128::from(x0 + x1), 2) as Nm, 0), r);
    }
    use crate::geom::{Arc, Dir, Edge};
    Ok(Contour::new(vec![
        Edge::line(Point::new(a, -r), Point::new(b, -r)),
        Edge::Arc(Arc {
            start: Point::new(b, -r),
            end: Point::new(b, r),
            centre: Point::new(b, 0),
            dir: Dir::Ccw,
        }),
        Edge::line(Point::new(b, r), Point::new(a, r)),
        Edge::Arc(Arc {
            start: Point::new(a, r),
            end: Point::new(a, -r),
            centre: Point::new(a, 0),
            dir: Dir::Ccw,
        }),
    ])
    .expect("closed by construction"))
}

/// A rectangle `w` by `h` centred on the origin.
pub(crate) fn centred_rect(w: Nm, h: Nm, corners: &Corners) -> Result<Contour, ShapeError> {
    let (x0, x1) = span(w);
    let (y0, y1) = span(h);
    rect(x0, y0, x1, y1, corners)
}

/// The corners of a ring's inside: rounded by what is left of the
/// outside's radius once the ring's width is taken off, so the ring keeps
/// its width round the corner.
fn inner_corners(c: &Corners, width: Nm) -> Corners {
    if c.round > width {
        Corners::rounded(c.round - width, c.rounded)
    } else {
        Corners::SHARP
    }
}

/// A standard symbol's shape in its own coordinates: one or more positive
/// regions, or nothing for `null`.
pub fn symbol_shape(s: &StandardSymbol) -> Result<Shape, ShapeError> {
    use StandardSymbol as S;
    Ok(match *s {
        S::Round { d } => positive(vec![Region::solid(circle(Point::ORIGIN, half(d))?)]),
        S::Square { s } => positive(vec![Region::solid(centred_rect(s, s, &Corners::SHARP)?)]),
        S::Rect { w, h, corners } => positive(vec![Region::solid(centred_rect(w, h, &corners)?)]),
        S::Oval { w, h } => positive(vec![Region::solid(oval(w, h)?)]),
        S::Diamond { w, h } => {
            let ((x0, x1), (y0, y1)) = (span(w), span(h));
            positive(vec![Region::solid(polygon(&[
                Point::new(x1, 0),
                Point::new(0, y1),
                Point::new(x0, 0),
                Point::new(0, y0),
            ])?)])
        }
        S::Octagon { w, h, r } => {
            if r < 0 || 2 * r > w.min(h) {
                return Err(bad("an octagon's corners larger than its sides"));
            }
            let ((x0, x1), (y0, y1)) = (span(w), span(h));
            positive(vec![Region::solid(polygon(&[
                Point::new(x1, y0 + r),
                Point::new(x1, y1 - r),
                Point::new(x1 - r, y1),
                Point::new(x0 + r, y1),
                Point::new(x0, y1 - r),
                Point::new(x0, y0 + r),
                Point::new(x0 + r, y0),
                Point::new(x1 - r, y0),
            ])?)])
        }
        S::DonutRound { od, id } => {
            if id <= 0 || id >= od {
                return Err(bad("a donut's hole must be inside it"));
            }
            positive(vec![Region::ring(
                circle(Point::ORIGIN, half(od))?,
                circle(Point::ORIGIN, half(id))?,
            )])
        }
        S::DonutSquare { od, id, corners } => {
            if id <= 0 || id >= od {
                return Err(bad("a donut's hole must be inside it"));
            }
            let inner = inner_corners(&corners, (od - id) / 2);
            positive(vec![Region::ring(
                centred_rect(od, od, &corners)?,
                centred_rect(id, id, &inner)?,
            )])
        }
        S::DonutSquareRound { od, id } => {
            if id <= 0 || id >= od {
                return Err(bad("a donut's hole must be inside it"));
            }
            positive(vec![Region::ring(
                centred_rect(od, od, &Corners::SHARP)?,
                circle(Point::ORIGIN, half(id))?,
            )])
        }
        S::DonutRect { w, h, lw, corners } => {
            if lw <= 0 || 2 * lw >= w.min(h) {
                return Err(bad("a donut's ring wider than it"));
            }
            let inner = inner_corners(&corners, lw);
            positive(vec![Region::ring(
                centred_rect(w, h, &corners)?,
                centred_rect(w - 2 * lw, h - 2 * lw, &inner)?,
            )])
        }
        S::DonutOval { w, h, lw } => {
            if lw <= 0 || 2 * lw >= w.min(h) {
                return Err(bad("a donut's ring wider than it"));
            }
            positive(vec![Region::ring(
                oval(w, h)?,
                oval(w - 2 * lw, h - 2 * lw)?,
            )])
        }
        S::HexagonH { w, h, r } => {
            if r < 0 || 2 * r > w {
                return Err(bad("a hexagon's corners wider than it"));
            }
            let ((x0, x1), (y0, y1)) = (span(w), span(h));
            positive(vec![Region::solid(polygon(&[
                Point::new(x1, 0),
                Point::new(x1 - r, y1),
                Point::new(x0 + r, y1),
                Point::new(x0, 0),
                Point::new(x0 + r, y0),
                Point::new(x1 - r, y0),
            ])?)])
        }
        S::HexagonV { w, h, r } => {
            if r < 0 || 2 * r > h {
                return Err(bad("a hexagon's corners taller than it"));
            }
            let ((x0, x1), (y0, y1)) = (span(w), span(h));
            positive(vec![Region::solid(polygon(&[
                Point::new(0, y1),
                Point::new(x0, y1 - r),
                Point::new(x0, y0 + r),
                Point::new(0, y0),
                Point::new(x1, y0 + r),
                Point::new(x1, y1 - r),
            ])?)])
        }
        S::ButterflyRound { d } => {
            let r = half(d);
            if r <= 0 {
                return Err(bad("a butterfly needs a size"));
            }
            use crate::geom::{Arc, Dir, Edge};
            // Quadrants 2 and 4: the upper left and lower right quarters.
            let quarter = |a: Point, b: Point| {
                Contour::new(vec![
                    Edge::line(Point::ORIGIN, a),
                    Edge::Arc(Arc {
                        start: a,
                        end: b,
                        centre: Point::ORIGIN,
                        dir: Dir::Ccw,
                    }),
                    Edge::line(b, Point::ORIGIN),
                ])
                .expect("closed by construction")
            };
            positive(vec![
                Region::solid(quarter(Point::new(0, r), Point::new(-r, 0))),
                Region::solid(quarter(Point::new(0, -r), Point::new(r, 0))),
            ])
        }
        S::ButterflySquare { s } => {
            let (lo, hi) = span(s);
            if lo >= 0 || hi <= 0 {
                return Err(bad("a butterfly needs a size"));
            }
            positive(vec![
                Region::solid(rect(lo, 0, 0, hi, &Corners::SHARP)?),
                Region::solid(rect(0, lo, hi, 0, &Corners::SHARP)?),
            ])
        }
        S::Triangle { base, h } => {
            let ((x0, x1), (y0, y1)) = (span(base), span(h));
            let apex = div_round(i128::from(x0 + x1), 2) as Nm;
            positive(vec![Region::solid(polygon(&[
                Point::new(x0, y0),
                Point::new(x1, y0),
                Point::new(apex, y1),
            ])?)])
        }
        S::HalfOval { w, h } => {
            // Flat on the left, a half circle on the right.
            let r = half(h);
            let (x0, x1) = span(w);
            let c = x1 - r;
            if r <= 0 || c < x0 {
                return Err(bad("a half oval shorter than its radius"));
            }
            use crate::geom::{Arc, Dir, Edge};
            let mut edges = vec![];
            if c > x0 {
                edges.push(Edge::line(Point::new(x0, -r), Point::new(c, -r)));
            }
            edges.push(Edge::Arc(Arc {
                start: Point::new(c, -r),
                end: Point::new(c, r),
                centre: Point::new(c, 0),
                dir: Dir::Ccw,
            }));
            if c > x0 {
                edges.push(Edge::line(Point::new(c, r), Point::new(x0, r)));
            }
            edges.push(Edge::line(Point::new(x0, r), Point::new(x0, -r)));
            positive(vec![Region::solid(
                Contour::new(edges).expect("closed by construction"),
            )])
        }
        S::Ellipse { w, h } => ellipse::ellipse(w, h)?,
        S::Moire {
            ring_width,
            ring_gap,
            rings,
            line_width,
            line_length,
            line_angle,
        } => moire(
            ring_width,
            ring_gap,
            rings,
            line_width,
            line_length,
            line_angle,
        )?,
        S::Hole { d, .. } => positive(vec![Region::solid(circle(Point::ORIGIN, half(d))?)]),
        S::Null { .. } => Shape::default(),
        S::Thermal(t) => thermal::thermal(&t)?,
        S::Trapezoid { w, h, dx, dy } => {
            if w <= 0 || h <= 0 || dx.abs() >= h || dy.abs() >= w {
                return Err(bad("a trapezoid's delta larger than its size"));
            }
            let ((x0, x1), (y0, y1)) = (span(w), span(h));
            let (hx, hy) = (half(dx), half(dy));
            positive(vec![Region::solid(polygon(&[
                Point::new(x0 + hy, y0 - hx),
                Point::new(x1 - hy, y0 + hx),
                Point::new(x1 + hy, y1 - hx),
                Point::new(x0 - hy, y1 + hx),
            ])?)])
        }
    })
}

/// A moiré: `rings` rings `ring_width` wide, the outermost reaching half
/// the line length out and each next one `ring_width + ring_gap` further
/// in (a ring reaching the centre is a disc), and two lines `line_width`
/// wide and `line_length` long through the centre, square to each other,
/// the first at `line_angle`.
fn moire(rw: Nm, rg: Nm, rings: u32, lw: Nm, ll: Nm, la: Mdeg) -> Result<Shape, ShapeError> {
    if rw <= 0 || rg < 0 || lw <= 0 || ll <= 0 {
        return Err(bad("a moire needs ring and line sizes"));
    }
    let mut regions = Vec::new();
    let outer = half(ll);
    for k in 0..i64::from(rings) {
        let r = outer - k * (rw + rg);
        if r <= 0 {
            break;
        }
        let o = circle(Point::ORIGIN, r)?;
        regions.push(if r - rw > 0 {
            Region::ring(o, circle(Point::ORIGIN, r - rw)?)
        } else {
            Region::solid(o)
        });
    }
    let bar = centred_rect(ll, lw, &Corners::SHARP)?;
    for turn in [0, 90_000] {
        let p = Place {
            rot: Mdeg::new(i64::from(la.get()) + turn),
            ..Place::IDENTITY
        };
        regions.push(Region::solid(p.contour(&bar)));
    }
    Ok(positive(regions))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::symbol::CornerSet;

    fn area(s: &StandardSymbol) -> f64 {
        let shape = symbol_shape(s).unwrap();
        shape
            .parts
            .iter()
            .map(|p| match &p.piece {
                super::super::Piece::Region(r) => r.area2() / 2.0,
                _ => panic!("regions only"),
            })
            .sum()
    }

    const PI: f64 = std::f64::consts::PI;

    #[test]
    fn areas_in_closed_form() {
        use StandardSymbol as S;
        assert!((area(&S::Round { d: 2000 }) - PI * 1e6).abs() < 1e-6);
        assert_eq!(area(&S::Square { s: 3 }), 9.0);
        assert_eq!(
            area(&S::Rect {
                w: 20,
                h: 10,
                corners: Corners::SHARP
            }),
            200.0
        );
        assert!((area(&S::Oval { w: 30, h: 10 }) - (200.0 + PI * 25.0)).abs() < 1e-9);
        assert!((area(&S::Oval { w: 10, h: 30 }) - (200.0 + PI * 25.0)).abs() < 1e-9);
        assert_eq!(area(&S::Diamond { w: 20, h: 10 }), 100.0);
        assert_eq!(area(&S::Octagon { w: 20, h: 10, r: 2 }), 200.0 - 4.0 * 2.0);
        assert!((area(&S::DonutRound { od: 20, id: 10 }) - PI * 75.0).abs() < 1e-9);
        assert_eq!(
            area(&S::DonutSquare {
                od: 20,
                id: 10,
                corners: Corners::SHARP
            }),
            300.0
        );
        assert!((area(&S::DonutSquareRound { od: 20, id: 10 }) - (400.0 - PI * 25.0)).abs() < 1e-9);
        assert_eq!(
            area(&S::DonutRect {
                w: 20,
                h: 10,
                lw: 2,
                corners: Corners::SHARP
            }),
            200.0 - 96.0
        );
        assert_eq!(area(&S::HexagonH { w: 20, h: 10, r: 5 }), 200.0 - 50.0);
        assert_eq!(area(&S::HexagonV { w: 10, h: 20, r: 5 }), 200.0 - 50.0);
        assert!((area(&S::ButterflyRound { d: 20 }) - PI * 50.0).abs() < 1e-9);
        assert_eq!(area(&S::ButterflySquare { s: 20 }), 200.0);
        assert_eq!(area(&S::Triangle { base: 20, h: 10 }), 100.0);
        assert!((area(&S::HalfOval { w: 30, h: 10 }) - (250.0 + PI * 12.5)).abs() < 1e-9);
        assert_eq!(
            area(&S::Trapezoid {
                w: 20,
                h: 10,
                dx: 4,
                dy: 0
            }),
            200.0
        );
        assert!(symbol_shape(&S::Null { ext: 3 }).unwrap().parts.is_empty());
    }

    #[test]
    fn rounded_donuts_keep_their_width() {
        let c = Corners::rounded(6, CornerSet::ALL);
        // Outer 20 × 20 with r 6, inner 10 × 10 with r 1.
        let expect = (400.0 - 4.0 * (36.0 - PI * 9.0)) - (100.0 - 4.0 * (1.0 - PI / 4.0));
        assert!(
            (area(&StandardSymbol::DonutSquare {
                od: 20,
                id: 10,
                corners: c
            }) - expect)
                .abs()
                < 1e-9
        );
    }

    #[test]
    fn faults() {
        use StandardSymbol as S;
        for s in [
            S::Round { d: 0 },
            S::Rect {
                w: 10,
                h: 0,
                corners: Corners::SHARP,
            },
            S::Octagon { w: 10, h: 10, r: 6 },
            S::DonutRound { od: 10, id: 10 },
            S::DonutRect {
                w: 10,
                h: 10,
                lw: 5,
                corners: Corners::SHARP,
            },
            S::HalfOval { w: 4, h: 10 },
            S::Trapezoid {
                w: 10,
                h: 10,
                dx: 10,
                dy: 0,
            },
        ] {
            assert!(
                matches!(symbol_shape(&s), Err(ShapeError::BadParameters(_))),
                "{s:?}"
            );
        }
    }

    #[test]
    fn moire_rings_and_lines() {
        let s = symbol_shape(&StandardSymbol::Moire {
            ring_width: 5,
            ring_gap: 10,
            rings: 4,
            line_width: 4,
            line_length: 100,
            line_angle: Mdeg::ZERO,
        })
        .unwrap();
        // Rings at 50, 35, 20 and a disc of 5, then two lines.
        assert_eq!(s.parts.len(), 6);
        let b = s.bounds().unwrap();
        assert_eq!((b.min, b.max), (Point::new(-50, -50), Point::new(50, 50)));
    }
}
