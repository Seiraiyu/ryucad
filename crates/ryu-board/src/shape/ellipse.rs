//! The ellipse: the one standard symbol segments and circular arcs can't
//! describe exactly. Its shape is arcs through points of the ellipse,
//! within [`BOUND`] of it, marked approximate.

use super::{Shape, ShapeError};
use crate::geom::{Arc, Contour, Edge, Mdeg, Place, Point, Region, round_nm};
use crate::model::Polarity;

/// How far the arcs may lie from the ellipse, in nanometres.
pub const BOUND: i64 = 10;

/// What each arc is held to at its samples: the rest of the bound covers
/// rounding its ends and centre to the nanometre and the stretches between
/// samples.
const AIM: f64 = 4.0;
const SAMPLES: u32 = 16;
const MAX_DEPTH: u32 = 30;

pub(crate) fn ellipse(w: i64, h: i64) -> Result<Shape, ShapeError> {
    if w <= 0 || h <= 0 {
        return Err(ShapeError::BadParameters(
            "an ellipse needs a width and a height",
        ));
    }
    let (a, b) = (w as f64 / 2.0, h as f64 / 2.0);
    let at = |t: f64| {
        let (s, c) = ryu_math::sin_cos(t);
        Point::new(round_nm(a * c), round_nm(b * s))
    };
    // The first quarter, from (a, 0) to (0, b), counterclockwise.
    let mut quarter = Vec::new();
    split(&at, a, b, 0.0, std::f64::consts::FRAC_PI_2, 0, &mut quarter)?;
    // The other three quarters by exact mirrors and half turns.
    let mirror = Place {
        mirror: true,
        ..Place::IDENTITY
    };
    let second: Vec<Edge> = quarter
        .iter()
        .rev()
        .map(|e| mirror.edge(e).reversed())
        .collect();
    let half_turn = Place {
        rot: Mdeg::new(180_000),
        ..Place::IDENTITY
    };
    let mut edges = quarter.clone();
    edges.extend(second.iter().copied());
    edges.extend(quarter.iter().map(|e| half_turn.edge(e)));
    edges.extend(second.iter().map(|e| half_turn.edge(e)));
    let contour = Contour::new(edges)
        .map_err(|_| ShapeError::BadParameters("an ellipse too small to draw"))?;
    let mut shape = Shape::regions(Polarity::Positive, [Region::solid(contour)]);
    shape.approximate = Some(BOUND);
    Ok(shape)
}

fn split(
    at: &impl Fn(f64) -> Point,
    a: f64,
    b: f64,
    t0: f64,
    t1: f64,
    depth: u32,
    out: &mut Vec<Edge>,
) -> Result<(), ShapeError> {
    let (p0, p1) = (at(t0), at(t1));
    if p0 == p1 {
        return Ok(());
    }
    let tm = 0.5 * (t0 + t1);
    let edge = match Arc::through(p0, at(tm), p1) {
        Some(arc) => Edge::Arc(arc),
        None => Edge::line(p0, p1),
    };
    let worst = (1..SAMPLES)
        .map(|j| {
            let t = t0 + (t1 - t0) * f64::from(j) / f64::from(SAMPLES);
            let (s, c) = ryu_math::sin_cos(t);
            deviation(&edge, a * c, b * s)
        })
        .fold(0.0, f64::max);
    if worst <= AIM {
        out.push(edge);
        return Ok(());
    }
    if depth == MAX_DEPTH {
        return Err(ShapeError::BadParameters("an ellipse too thin to draw"));
    }
    split(at, a, b, t0, tm, depth + 1, out)?;
    split(at, a, b, tm, t1, depth + 1, out)
}

/// The distance from `(x, y)` to the edge's circle or line.
fn deviation(e: &Edge, x: f64, y: f64) -> f64 {
    match e {
        Edge::Arc(arc) => {
            let (dx, dy) = (x - arc.centre.x as f64, y - arc.centre.y as f64);
            (ryu_math::sqrt(dx * dx + dy * dy) - arc.radius()).abs()
        }
        Edge::Segment(s) => {
            let (ax, ay) = (s.a.x as f64, s.a.y as f64);
            let (ux, uy) = (s.b.x as f64 - ax, s.b.y as f64 - ay);
            ((x - ax) * uy - (y - ay) * ux).abs() / ryu_math::sqrt(ux * ux + uy * uy)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::geom::Dir;

    #[test]
    fn ellipses_hold_their_area_and_box() {
        let s = ellipse(3_000_000, 1_000_000).unwrap();
        assert_eq!(s.approximate, Some(BOUND));
        let super::super::Piece::Region(r) = &s.parts[0].piece else {
            panic!()
        };
        let pi = std::f64::consts::PI;
        let exact = pi * 1_500_000.0 * 500_000.0;
        // Within the bound times the perimeter.
        assert!((r.area2() / 2.0 - exact).abs() < 10.0 * 7_000_000.0);
        let b = s.bounds().unwrap();
        assert!((b.max.x - 1_500_000).abs() <= BOUND && (b.max.y - 500_000).abs() <= BOUND);
        assert!((b.min.x + 1_500_000).abs() <= BOUND && (b.min.y + 500_000).abs() <= BOUND);
        assert_eq!(r.outer.orientation(), Dir::Ccw);
    }

    #[test]
    fn every_arc_stays_near_the_ellipse() {
        let s = ellipse(2_000_000, 600_000).unwrap();
        let super::super::Piece::Region(r) = &s.parts[0].piece else {
            panic!()
        };
        for e in r.outer.edges() {
            let Edge::Arc(arc) = e else { continue };
            // The arc's middle, measured against the ellipse's equation.
            let (sx, sy) = (
                (arc.start.x - arc.centre.x) as f64,
                (arc.start.y - arc.centre.y) as f64,
            );
            let (ex, ey) = (
                (arc.end.x - arc.centre.x) as f64,
                (arc.end.y - arc.centre.y) as f64,
            );
            let (mx, my) = (sx + ex, sy + ey);
            let k = arc.radius() / ryu_math::sqrt(mx * mx + my * my);
            let (px, py) = (arc.centre.x as f64 + mx * k, arc.centre.y as f64 + my * k);
            let f = (px / 1_000_000.0) * (px / 1_000_000.0) + (py / 300_000.0) * (py / 300_000.0);
            // |f - 1| ≈ 2 d / (semi-axis), so d < 10 nm.
            assert!((f - 1.0).abs() * 300_000.0 / 2.0 < BOUND as f64, "{f}");
        }
    }

    #[test]
    fn circles_and_faults() {
        assert!(ellipse(0, 5).is_err());
        assert!(ellipse(1_000, 1_000).is_ok());
    }
}
