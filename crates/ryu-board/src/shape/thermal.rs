//! Thermals: a ring less its gaps.
//!
//! The gaps are channels `gap` wide with parallel sides, centred on rays
//! from the origin at `angle + k × 360° / spokes`, counterclockwise from
//! +x. A squared thermal's pieces end on the channels' sides; a rounded
//! one's pieces are its ring's middle line drawn with a round pen as wide
//! as the ring, ending where the pen's end clears the channel. An
//! open-cornered one (`s_tho`, `rc_tho`) cuts its rectangle's sides square
//! where each ray meets the rectangle: half the gap either side of a
//! side's middle for rays at multiples of 90°; at the corner of that
//! quadrant for odd multiples of 45°, leaving the ring's corner square
//! open, or half the gap from the corner if that is more.

use super::build::{circle, polygon, rect};
use super::ring::{Hit, Line, hit, in_order, sub_path};
use super::stroke::path_outline;
use super::symbols::{centred_rect, oval};
use super::{Shape, ShapeError};
use crate::geom::{Contour, Edge, Nm, Point, Region, div_round, half, span};
use crate::model::Polarity;
use crate::symbol::{Corners, Thermal, ThermalForm};

fn bad(why: &'static str) -> ShapeError {
    ShapeError::BadParameters(why)
}

/// The ray of gap `k`, in radians.
fn ray(t: &Thermal, k: u32) -> f64 {
    let mdeg = f64::from(t.angle.get()) + f64::from(k) * 360_000.0 / f64::from(t.spokes);
    mdeg * (std::f64::consts::PI / 180_000.0)
}

/// Each gap's two sides on `c`, at `±side` from its ray: right then left,
/// gap by gap, counterclockwise.
fn sides(c: &Contour, t: &Thermal, side: f64) -> Result<Vec<Hit>, ShapeError> {
    let mut hits = Vec::with_capacity(2 * t.spokes as usize);
    for k in 0..t.spokes {
        for s in [-side, side] {
            hits.push(
                hit(c, &Line::new(ray(t, k), s)).ok_or(bad("a thermal's gaps miss its ring"))?,
            );
        }
    }
    if !in_order(&hits) {
        return Err(bad("a thermal's gaps overlap"));
    }
    Ok(hits)
}

/// The ring between `outer` and `inner` cut along each gap's sides.
fn squared(outer: Contour, inner: Contour, t: &Thermal) -> Result<Vec<Region>, ShapeError> {
    if t.spokes == 0 {
        return Ok(vec![Region::ring(outer, inner)]);
    }
    let g = t.gap as f64 / 2.0;
    let (o, i) = (sides(&outer, t, g)?, sides(&inner, t, g)?);
    let n = t.spokes as usize;
    (0..n)
        .map(|k| {
            let (l, r) = (2 * k + 1, (2 * k + 2) % (2 * n));
            let mut edges = sub_path(&outer, &o[l], &o[r]);
            edges.push(Edge::line(o[r].at, i[r].at));
            edges.extend(
                sub_path(&inner, &i[l], &i[r])
                    .iter()
                    .rev()
                    .map(Edge::reversed),
            );
            edges.push(Edge::line(i[l].at, o[l].at));
            edges.retain(|e| e.start() != e.end() || matches!(e, Edge::Arc(_)));
            Contour::new(edges)
                .map(Region::solid)
                .map_err(|_| bad("a thermal's gaps overlap"))
        })
        .collect()
}

/// The middle line `mid` drawn with a round pen `2h` wide, cut where the
/// pen's ends clear each gap.
fn rounded(mid: Contour, h: Nm, t: &Thermal) -> Result<Vec<Region>, ShapeError> {
    let m = sides(&mid, t, t.gap as f64 / 2.0 + h as f64)?;
    let n = t.spokes as usize;
    (0..n)
        .map(|k| {
            let (l, r) = (2 * k + 1, (2 * k + 2) % (2 * n));
            path_outline(&sub_path(&mid, &m[l], &m[r]), h)
                .ok_or(bad("a thermal's pieces too small"))
        })
        .collect()
}

/// A rectangular ring `[x0, x1] × [y0, y1]`, `ring` wide, with its corners
/// open: pieces cut square to the sides.
fn open(x0: Nm, y0: Nm, x1: Nm, y1: Nm, ring: Nm, t: &Thermal) -> Result<Vec<Region>, ShapeError> {
    let n = t.spokes;
    if n == 0 {
        let outer = rect(x0, y0, x1, y1, &Corners::SHARP)?;
        return Ok(vec![Region::ring(
            outer,
            rect(x0 + ring, y0 + ring, x1 - ring, y1 - ring, &Corners::SHARP)?,
        )]);
    }
    if t.angle.get() % 45_000 != 0 || ![1, 2, 4, 8].contains(&n) {
        return Err(bad(
            "an open thermal's gaps must lie at multiples of 45 degrees",
        ));
    }
    let (w, h) = (x1 - x0, y1 - y0);
    let len = 2 * (w + h);
    // Counterclockwise along the outside from the bottom right corner.
    let corner = [0, h, h + w, 2 * h + w];
    let at = |s: Nm| -> (usize, Point) {
        let s = s.rem_euclid(len);
        if s < h {
            (0, Point::new(x1, y0 + s))
        } else if s < h + w {
            (1, Point::new(x1 - (s - h), y1))
        } else if s < 2 * h + w {
            (2, Point::new(x0, y1 - (s - h - w)))
        } else {
            (3, Point::new(x0 + (s - 2 * h - w), y0))
        }
    };
    let inward = |side: usize, p: Point| match side {
        0 => p.offset(-ring, 0),
        1 => p.offset(0, -ring),
        2 => p.offset(ring, 0),
        _ => p.offset(0, ring),
    };
    let inner_corner = [
        Point::new(x1 - ring, y0 + ring),
        Point::new(x1 - ring, y1 - ring),
        Point::new(x0 + ring, y1 - ring),
        Point::new(x0 + ring, y0 + ring),
    ];
    // Where each ray meets the outside, and whether that is a corner.
    let mut centres: Vec<(Nm, bool)> = (0..n)
        .map(|k| {
            let m = (i64::from(t.angle.get()) + i64::from(k) * 360_000 / i64::from(n)) % 360_000;
            match m {
                0 => (-y0, false),
                90_000 => (h + x1, false),
                180_000 => (h + w + y1, false),
                270_000 => (2 * h + w - x0, false),
                45_000 => (h, true),
                135_000 => (h + w, true),
                225_000 => (2 * h + w, true),
                _ => (0, true),
            }
        })
        .collect();
    centres.sort_unstable();
    // Half the gap either side of a side's middle; at a corner the ring's
    // corner square stays open whatever the gap.
    let g = t.gap;
    let (lo, hi) = (g / 2, g - g / 2);
    let back = |corner: bool, half: Nm| if corner { half.max(ring) } else { half };
    (0..n as usize)
        .map(|k| {
            let (ca, corner_a) = centres[k];
            let (cb, corner_b) = centres[(k + 1) % n as usize];
            let a = ca + back(corner_a, hi);
            let mut b = cb - back(corner_b, lo);
            if b <= a {
                b += len;
            }
            if b <= a || b - a >= len {
                return Err(bad("an open thermal's gaps overlap"));
            }
            let (sa, pa) = at(a);
            // The side `b` arrives along: a point on a corner belongs to
            // the side before it.
            let (sb, _) = at(b - 1);
            let pb = at(b).1;
            let between: Vec<usize> = (0..8)
                .filter_map(|j| {
                    let s = corner[j % 4] + len * (j as i64 / 4);
                    (a < s && s < b).then_some(j % 4)
                })
                .collect();
            let mut pts = vec![pa];
            pts.extend(between.iter().map(|&j| at(corner[j]).1));
            pts.push(pb);
            pts.push(inward(sb, pb));
            pts.extend(between.iter().rev().map(|&j| inner_corner[j]));
            pts.push(inward(sa, pa));
            polygon(&pts).map(Region::solid)
        })
        .collect()
}

pub(crate) fn thermal(t: &Thermal) -> Result<Shape, ShapeError> {
    if t.gap < 0 {
        return Err(bad("a thermal's gap can't be negative"));
    }
    let regions = match t.form {
        ThermalForm::RoundRounded { od, id } => {
            if id <= 0 || id >= od {
                return Err(bad("a thermal's hole must be inside it"));
            }
            if t.spokes == 0 {
                vec![Region::ring(
                    circle(Point::ORIGIN, half(od))?,
                    circle(Point::ORIGIN, half(id))?,
                )]
            } else {
                let mid = circle(Point::ORIGIN, div_round(i128::from(od + id), 4) as Nm)?;
                rounded(mid, div_round(i128::from(od - id), 4) as Nm, t)?
            }
        }
        ThermalForm::RoundSquared { od, id } => {
            if id <= 0 || id >= od {
                return Err(bad("a thermal's hole must be inside it"));
            }
            squared(
                circle(Point::ORIGIN, half(od))?,
                circle(Point::ORIGIN, half(id))?,
                t,
            )?
        }
        ThermalForm::Square {
            os,
            is,
            corners,
            open: o,
        } => {
            if is <= 0 || is >= os {
                return Err(bad("a thermal's hole must be inside it"));
            }
            if o {
                if !corners.is_sharp() {
                    return Err(bad("an open thermal has square corners"));
                }
                let (x0, x1) = span(os);
                open(x0, x0, x1, x1, (os - is) / 2, t)?
            } else {
                squared(
                    centred_rect(os, os, &corners)?,
                    centred_rect(is, is, &Corners::SHARP)?,
                    t,
                )?
            }
        }
        ThermalForm::SquareRound { os, id } => {
            if id <= 0 || id >= os {
                return Err(bad("a thermal's hole must be inside it"));
            }
            squared(
                centred_rect(os, os, &Corners::SHARP)?,
                circle(Point::ORIGIN, half(id))?,
                t,
            )?
        }
        ThermalForm::Line { os, is } => {
            if is <= 0 || is >= os {
                return Err(bad("a thermal's hole must be inside it"));
            }
            if t.spokes != 4 || t.angle.get() % 90_000 != 45_000 {
                return Err(bad("a line thermal has four gaps at its corners"));
            }
            let m = div_round(i128::from(os + is), 2) as Nm;
            rounded(
                centred_rect(m, m, &Corners::SHARP)?,
                div_round(i128::from(os - is), 4) as Nm,
                t,
            )?
        }
        ThermalForm::Rect {
            w,
            h,
            ring,
            corners,
            open: o,
        } => {
            if ring <= 0 || 2 * ring >= w.min(h) {
                return Err(bad("a thermal's ring wider than it"));
            }
            let ((x0, x1), (y0, y1)) = (span(w), span(h));
            if o {
                if !corners.is_sharp() {
                    return Err(bad("an open thermal has square corners"));
                }
                open(x0, y0, x1, y1, ring, t)?
            } else {
                let inner = rect(x0 + ring, y0 + ring, x1 - ring, y1 - ring, &Corners::SHARP)?;
                squared(rect(x0, y0, x1, y1, &corners)?, inner, t)?
            }
        }
        ThermalForm::Oval { w, h, ring }
        | ThermalForm::Oblong {
            w,
            h,
            ring,
            rounded: false,
        } => {
            if ring <= 0 || 2 * ring >= w.min(h) {
                return Err(bad("a thermal's ring wider than it"));
            }
            squared(oval(w, h)?, oval(w - 2 * ring, h - 2 * ring)?, t)?
        }
        ThermalForm::Oblong {
            w,
            h,
            ring,
            rounded: true,
        } => {
            if ring <= 0 || 2 * ring >= w.min(h) {
                return Err(bad("a thermal's ring wider than it"));
            }
            if t.spokes == 0 {
                vec![Region::ring(oval(w, h)?, oval(w - 2 * ring, h - 2 * ring)?)]
            } else {
                rounded(oval(w - ring, h - ring)?, half(ring), t)?
            }
        }
    };
    Ok(Shape::regions(Polarity::Positive, regions))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::geom::Mdeg;
    use crate::symbol::CornerSet;

    const PI: f64 = std::f64::consts::PI;

    fn th(form: ThermalForm, angle: i64, spokes: u32, gap: Nm) -> Thermal {
        Thermal {
            form,
            angle: Mdeg::new(angle),
            spokes,
            gap,
        }
    }

    fn area(t: &Thermal) -> f64 {
        thermal(t)
            .unwrap()
            .parts
            .iter()
            .map(|p| match &p.piece {
                super::super::Piece::Region(r) => {
                    assert_eq!(r.outer.orientation(), crate::geom::Dir::Ccw);
                    r.area2() / 2.0
                }
                _ => panic!(),
            })
            .sum()
    }

    #[test]
    fn squared_round_thermals_lose_their_gaps() {
        // od 2 mm, id 1 mm, four gaps 0.1 mm wide at 0°: the ring less
        // four channels, each a strip 0.1 mm wide through the ring.
        let t = th(
            ThermalForm::RoundSquared {
                od: 2_000_000,
                id: 1_000_000,
            },
            0,
            4,
            100_000,
        );
        let ring = PI * (1e12 - 0.25e12);
        // The part of a disc of radius r within 50 µm of the x axis, x > 0.
        let strip = |r: f64| 5e4 * ryu_math::sqrt(r * r - 2.5e9) + r * r * ryu_math::asin(5e4 / r);
        let channel = strip(1e6) - strip(5e5);
        // Rounding each point to the nanometre moves the area by about
        // the perimeter (some 10 mm) times a nanometre.
        assert!(
            (area(&t) - (ring - 4.0 * channel)).abs() < 1e7,
            "{}",
            area(&t)
        );
        assert_eq!(thermal(&t).unwrap().parts.len(), 4);
    }

    #[test]
    fn square_thermals_at_the_corners() {
        // A square ring 2000 outside, 1000 inside, gaps 100 wide along the
        // diagonals: each corner loses a diagonal channel.
        let t = th(
            ThermalForm::Square {
                os: 2000,
                is: 1000,
                corners: Corners::SHARP,
                open: false,
            },
            45_000,
            4,
            100,
        );
        let a = area(&t);
        assert!(
            a < 3_000_000.0 && a > 3_000_000.0 - 4.0 * 100.0 * 800.0,
            "{a}"
        );
        let rounded = th(
            ThermalForm::Square {
                os: 2000,
                is: 1000,
                corners: Corners::rounded(200, CornerSet::ALL),
                open: false,
            },
            0,
            4,
            100,
        );
        assert_eq!(thermal(&rounded).unwrap().parts.len(), 4);
    }

    #[test]
    fn open_corners_stay_open() {
        // Four bars, the ring's four corner squares open.
        let t = th(
            ThermalForm::Square {
                os: 2000,
                is: 1000,
                corners: Corners::SHARP,
                open: true,
            },
            45_000,
            4,
            100,
        );
        let s = thermal(&t).unwrap();
        assert_eq!(s.parts.len(), 4);
        assert_eq!(area(&t), 3_000_000.0 - 4.0 * 250_000.0);
        // A gap wider than twice the ring opens more.
        let wide = th(
            ThermalForm::Square {
                os: 2000,
                is: 1000,
                corners: Corners::SHARP,
                open: true,
            },
            45_000,
            4,
            1200,
        );
        assert_eq!(area(&wide), 4.0 * 800.0 * 500.0);
        // At 0° the gaps sit in the sides' middles: four L-shaped pieces.
        let mid = th(
            ThermalForm::Square {
                os: 2000,
                is: 1000,
                corners: Corners::SHARP,
                open: true,
            },
            0,
            4,
            100,
        );
        assert_eq!(area(&mid), 3_000_000.0 - 4.0 * 100.0 * 500.0);
        let rect = th(
            ThermalForm::Rect {
                w: 3000,
                h: 2000,
                ring: 400,
                corners: Corners::SHARP,
                open: true,
            },
            0,
            4,
            200,
        );
        assert_eq!(thermal(&rect).unwrap().parts.len(), 4);
    }

    #[test]
    fn rounded_and_line_thermals() {
        let t = th(
            ThermalForm::RoundRounded { od: 2000, id: 1000 },
            45_000,
            4,
            100,
        );
        assert_eq!(thermal(&t).unwrap().parts.len(), 4);
        let l = th(ThermalForm::Line { os: 2000, is: 1000 }, 45_000, 4, 100);
        assert_eq!(thermal(&l).unwrap().parts.len(), 4);
        assert!(thermal(&th(ThermalForm::Line { os: 2000, is: 1000 }, 0, 4, 100)).is_err());
        let o = th(
            ThermalForm::Oblong {
                w: 3000,
                h: 2000,
                ring: 400,
                rounded: true,
            },
            0,
            2,
            200,
        );
        assert_eq!(thermal(&o).unwrap().parts.len(), 2);
        let o = th(
            ThermalForm::Oval {
                w: 3000,
                h: 2000,
                ring: 400,
            },
            90_000,
            4,
            200,
        );
        assert_eq!(thermal(&o).unwrap().parts.len(), 4);
    }

    #[test]
    fn faults() {
        assert!(
            thermal(&th(
                ThermalForm::RoundSquared { od: 2000, id: 1000 },
                0,
                4,
                1500
            ))
            .is_err()
        );
        assert!(
            thermal(&th(
                ThermalForm::RoundSquared { od: 2000, id: 2000 },
                0,
                4,
                100
            ))
            .is_err()
        );
        assert!(
            thermal(&th(
                ThermalForm::Square {
                    os: 2000,
                    is: 1000,
                    corners: Corners::SHARP,
                    open: true
                },
                30_000,
                4,
                100
            ))
            .is_err()
        );
        let one = th(ThermalForm::RoundSquared { od: 2000, id: 1000 }, 0, 1, 100);
        assert_eq!(thermal(&one).unwrap().parts.len(), 1);
        let none = th(ThermalForm::RoundSquared { od: 2000, id: 1000 }, 0, 0, 100);
        assert_eq!(thermal(&none).unwrap().parts.len(), 1);
    }
}
