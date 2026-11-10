//! Hatching: the straight lines that fill a region at an angle and a
//! spacing, by the even-odd rule over its boundary paths, and lines kept
//! inside a circle (a detail view's).
//!
//! The region is every path given, each closed back to its start: a
//! section cap's outer loop and its holes, in any order or sense. Each
//! hatch line `n · p = c` (`n` square to the lines, `c` a whole number of
//! spacings and a half from the origin, so neighbouring regions' hatches
//! line up) is met with every segment: a line in closed form, an arc or an
//! elliptic arc by the arc cosine, a Bézier piece by subdividing its
//! numerator's Bernstein coefficients until each holds one sign change.
//! A segment crosses where the sign of `n · p - c` changes along it, a
//! zero counting as positive, so a vertex on the line is crossed once by
//! the two segments meeting there, or twice (a touch), never once each
//! way by rounding. The crossings along the line, sorted, are paired
//! first with second, third with fourth. Plain `f64` through `ryu_math`:
//! the same bits everywhere.

use crate::geom::{P, Path, Segment};

/// The most hatch lines one region takes: a spacing that small is a
/// mistake, not a drawing.
pub const MAX_LINES: usize = 100_000;

fn dot(a: P, b: P) -> f64 {
    a[0] * b[0] + a[1] * b[1]
}

/// The sign rule: zero is positive.
fn side(f: f64) -> bool {
    f >= 0.0
}

/// Where a segment from `at` crosses `n · p = c`: the points, in order
/// along the segment.
fn crossings(at: P, s: &Segment, n: P, c: f64, out: &mut Vec<P>) {
    let f = |p: P| dot(n, p) - c;
    match s {
        Segment::Line(b) => {
            let (fa, fb) = (f(at), f(*b));
            if side(fa) != side(fb) {
                let t = fa / (fa - fb);
                out.push([at[0] + t * (b[0] - at[0]), at[1] + t * (b[1] - at[1])]);
            }
        }
        Segment::Arc {
            centre,
            radius,
            from,
            sweep,
        } => conic(
            *centre,
            [*radius, 0.0],
            [0.0, *radius],
            *from,
            *sweep,
            n,
            c,
            out,
        ),
        Segment::Ellipse {
            centre,
            a,
            b,
            from,
            sweep,
        } => conic(*centre, *a, *b, *from, *sweep, n, c, out),
        Segment::Bezier { points, weights } => {
            let ones = vec![1.0; points.len() + 1];
            let w = weights.as_ref().unwrap_or(&ones);
            let h: Vec<[f64; 3]> = std::iter::once(at)
                .chain(points.iter().copied())
                .zip(w)
                .map(|(p, w)| [p[0] * w, p[1] * w, *w])
                .collect();
            bezier(&h, n, c, out);
        }
    }
}

/// `centre + cos t · a + sin t · b` for `t` from `from` by `sweep`.
#[allow(clippy::too_many_arguments)]
fn conic(centre: P, a: P, b: P, from: f64, sweep: f64, n: P, c: f64, out: &mut Vec<P>) {
    let at = |t: f64| {
        let (s, co) = ryu_math::sin_cos(t);
        [
            centre[0] + co * a[0] + s * b[0],
            centre[1] + co * a[1] + s * b[1],
        ]
    };
    let f = |t: f64| dot(n, at(t)) - c;
    let (alpha, beta, k) = (dot(n, a), dot(n, b), dot(n, centre) - c);
    let r = ryu_math::hypot(alpha, beta);
    // Roots of k + r cos(t - phi) strictly inside the run, in its order.
    let mut roots = Vec::new();
    if r > 0.0 && (k / r).abs() < 1.0 {
        let phi = ryu_math::atan2(beta, alpha);
        let w = ryu_math::acos(-k / r);
        let (lo, hi) = (from.min(from + sweep), from.max(from + sweep));
        let tau = 2.0 * std::f64::consts::PI;
        for base in [phi + w, phi - w] {
            let mut t = base + tau * ((lo - base) / tau).ceil();
            while t < hi {
                if t > lo {
                    roots.push(t);
                }
                t += tau;
            }
        }
        roots.sort_by(f64::total_cmp);
        if sweep < 0.0 {
            roots.reverse();
        }
    }
    signs(from, from + sweep, &roots, f, at, out);
}

/// The crossings of a curve over `t0` to `t1` whose function `f` has
/// roots `roots` in between (in order from `t0`): where the sign at `t0`,
/// in each stretch between roots and at `t1` changes.
fn signs(
    t0: f64,
    t1: f64,
    roots: &[f64],
    f: impl Fn(f64) -> f64,
    at: impl Fn(f64) -> P,
    out: &mut Vec<P>,
) {
    let mut cuts = Vec::with_capacity(roots.len() + 2);
    cuts.push(t0);
    cuts.extend_from_slice(roots);
    cuts.push(t1);
    let mut last = side(f(t0));
    for (i, w) in cuts.windows(2).enumerate() {
        let mid = side(f((w[0] + w[1]) / 2.0));
        if mid != last {
            // Changed at this stretch's start: `t0`, or the root before it.
            out.push(at(cuts[i]));
            last = mid;
        }
    }
    if side(f(t1)) != last {
        out.push(at(t1));
    }
}

/// A homogeneous Bézier's point at `t`.
fn bezier_at(h: &[[f64; 3]], t: f64) -> P {
    let mut row = h.to_vec();
    while row.len() > 1 {
        row = row
            .windows(2)
            .map(|w| {
                [
                    w[0][0] + t * (w[1][0] - w[0][0]),
                    w[0][1] + t * (w[1][1] - w[0][1]),
                    w[0][2] + t * (w[1][2] - w[0][2]),
                ]
            })
            .collect();
    }
    [row[0][0] / row[0][2], row[0][1] / row[0][2]]
}

/// A scalar Bernstein polynomial's value at `t`.
fn bernstein_at(d: &[f64], t: f64) -> f64 {
    let mut row = d.to_vec();
    while row.len() > 1 {
        row = row.windows(2).map(|w| w[0] + t * (w[1] - w[0])).collect();
    }
    row[0]
}

/// The roots in `(lo, hi)` of the Bernstein polynomial `d` over that
/// stretch of the whole piece's `[0, 1]`.
fn bernstein_roots(d: &[f64], lo: f64, hi: f64, depth: u32, out: &mut Vec<f64>) {
    let changes = d.windows(2).filter(|w| side(w[0]) != side(w[1])).count();
    if changes == 0 {
        return;
    }
    if changes == 1 || depth >= 40 {
        // One root: bisect on the sign of the value (the control polygon
        // changes sign once, so the polynomial does once).
        let (mut a, mut b) = (0.0, 1.0);
        let fa = side(d[0]);
        for _ in 0..60 {
            let m = (a + b) / 2.0;
            if side(bernstein_at(d, m)) == fa {
                a = m;
            } else {
                b = m;
            }
        }
        let t = lo + (hi - lo) * (a + b) / 2.0;
        if t > lo && t < hi {
            out.push(t);
        }
        return;
    }
    // Halve: de Casteljau on the coefficients.
    let mut row = d.to_vec();
    let mut left = vec![row[0]];
    let mut right = vec![row[row.len() - 1]];
    while row.len() > 1 {
        row = row.windows(2).map(|w| (w[0] + w[1]) / 2.0).collect();
        left.push(row[0]);
        right.push(row[row.len() - 1]);
    }
    right.reverse();
    let m = (lo + hi) / 2.0;
    bernstein_roots(&left, lo, m, depth + 1, out);
    bernstein_roots(&right, m, hi, depth + 1, out);
}

fn bezier(h: &[[f64; 3]], n: P, c: f64, out: &mut Vec<P>) {
    // The numerator of n · p - c: weights are positive, so its sign is
    // the function's.
    let d: Vec<f64> = h
        .iter()
        .map(|q| n[0] * q[0] + n[1] * q[1] - c * q[2])
        .collect();
    let mut roots = Vec::new();
    bernstein_roots(&d, 0.0, 1.0, 0, &mut roots);
    roots.sort_by(f64::total_cmp);
    roots.dedup();
    signs(
        0.0,
        1.0,
        &roots,
        |t| bernstein_at(&d, t),
        |t| bezier_at(h, t),
        out,
    );
}

/// The hatch lines filling the region `boundary` bounds (even-odd), at
/// `angle` (radians, from the x axis) and `spacing` apart, each from where
/// it enters the region to where it leaves, in order of `n · p` then along
/// the line. Empty for a spacing that isn't positive, or more lines than
/// [`MAX_LINES`].
pub fn lines(boundary: &[Path], angle: f64, spacing: f64) -> Vec<[P; 2]> {
    if spacing.is_nan() || spacing <= 0.0 || boundary.is_empty() {
        return Vec::new();
    }
    let (s, co) = ryu_math::sin_cos(angle);
    let (u, n) = ([co, s], [-s, co]);
    let mut lo = f64::INFINITY;
    let mut hi = f64::NEG_INFINITY;
    for p in boundary {
        let [a, b] = p.bounds();
        for q in [a, [a[0], b[1]], b, [b[0], a[1]]] {
            lo = lo.min(dot(n, q));
            hi = hi.max(dot(n, q));
        }
    }
    let first = (lo / spacing - 0.5).ceil();
    let last = (hi / spacing - 0.5).floor();
    if last.is_nan() || first.is_nan() || last < first || last - first >= MAX_LINES as f64 {
        return Vec::new();
    }
    let mut out = Vec::new();
    let mut k = first;
    while k <= last {
        let c = (k + 0.5) * spacing;
        let mut hits = Vec::new();
        for p in boundary {
            let mut at = p.start;
            for seg in &p.segments {
                crossings(at, seg, n, c, &mut hits);
                at = seg.end();
            }
            if p.closed && at != p.start {
                crossings(at, &Segment::Line(p.start), n, c, &mut hits);
            }
        }
        hits.sort_by(|a, b| dot(u, *a).total_cmp(&dot(u, *b)));
        for pair in hits.as_chunks::<2>().0 {
            // A touch at a vertex enters and leaves at one point.
            if pair[0] != pair[1] {
                out.push([pair[0], pair[1]]);
            }
        }
        k += 1.0;
    }
    out
}

/// The parts of `lines` inside the circle about `centre` of `radius`.
pub fn clip_to_circle(lines: &[[P; 2]], centre: P, radius: f64) -> Vec<[P; 2]> {
    let mut out = Vec::new();
    for [a, b] in lines {
        let d = [b[0] - a[0], b[1] - a[1]];
        let m = [a[0] - centre[0], a[1] - centre[1]];
        // |m + t d|² = r²: t² dd + 2 t md + mm - r² = 0.
        let (dd, md, mm) = (dot(d, d), dot(m, d), dot(m, m) - radius * radius);
        let disc = md * md - dd * mm;
        if dd == 0.0 || disc <= 0.0 {
            continue;
        }
        let q = disc.sqrt();
        let (t0, t1) = (((-md - q) / dd).max(0.0), ((-md + q) / dd).min(1.0));
        if t0 < t1 {
            let at = |t: f64| [a[0] + t * d[0], a[1] + t * d[1]];
            out.push([at(t0), at(t1)]);
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::f64::consts::{FRAC_PI_4, PI};

    fn length(l: &[[P; 2]]) -> f64 {
        l.iter()
            .map(|[a, b]| ryu_math::hypot(b[0] - a[0], b[1] - a[1]))
            .sum()
    }

    #[test]
    fn a_square_is_filled_edge_to_edge() {
        let sq = Path::polyline(&[[0.0, 0.0], [10.0, 0.0], [10.0, 10.0], [0.0, 10.0]], true);
        let l = lines(&[sq], 0.0, 1.0);
        // Lines y = 0.5, 1.5, … 9.5, each 10 long.
        assert_eq!(l.len(), 10);
        assert!((length(&l) - 100.0).abs() < 1e-12);
        assert_eq!(l[0], [[0.0, 0.5], [10.0, 0.5]]);
    }

    #[test]
    fn a_hole_is_left_out_in_either_sense() {
        let sq = Path::polyline(&[[0.0, 0.0], [10.0, 0.0], [10.0, 10.0], [0.0, 10.0]], true);
        let hole = Path::circle([5.0, 5.0], 2.0);
        let l = lines(&[sq.clone(), hole], 0.0, 0.01);
        // The hatched length over the spacing is the area: 100 - 4π.
        let area = length(&l) * 0.01;
        assert!((area - (100.0 - 4.0 * PI)).abs() < 1e-2, "{area}");
        let back = Path {
            start: [7.0, 5.0],
            segments: vec![Segment::Arc {
                centre: [5.0, 5.0],
                radius: 2.0,
                from: 0.0,
                sweep: -2.0 * PI,
            }],
            closed: true,
        };
        let r = lines(&[sq, back], 0.0, 0.01);
        assert_eq!(l.len(), r.len());
        assert!((length(&l) - length(&r)).abs() < 1e-9);
    }

    #[test]
    fn a_line_through_a_vertex_crosses_it_once() {
        // A diamond: the line y = 0.5 passes through no vertex, but with
        // a spacing of 1 at 0° the lines y = 0.5 cross it; a diamond with
        // a vertex at y = 0.5 is crossed there once.
        let d = Path::polyline(&[[0.0, 0.5], [2.0, -1.5], [4.0, 0.5], [2.0, 2.5]], true);
        let l = lines(&[d], 0.0, 1.0);
        let at: Vec<f64> = l.iter().map(|s| s[0][1]).collect();
        assert_eq!(at, vec![-0.5, 0.5, 1.5]);
        // Through the two side vertices: the full width.
        assert!((l[1][1][0] - l[1][0][0] - 4.0).abs() < 1e-12);
    }

    #[test]
    fn an_ellipse_and_a_rational_circle_hatch_to_their_areas() {
        let e = Path {
            start: [3.0, 0.0],
            segments: vec![Segment::Ellipse {
                centre: [0.0, 0.0],
                a: [3.0, 0.0],
                b: [0.0, 1.0],
                from: 0.0,
                sweep: 2.0 * PI,
            }],
            closed: true,
        };
        let a = length(&lines(&[e], FRAC_PI_4, 0.005)) * 0.005;
        assert!((a - 3.0 * PI).abs() < 1e-2, "{a}");
        // A circle of radius 10 as four rational quadratics.
        let w = std::f64::consts::FRAC_1_SQRT_2;
        let q = |c: P, e: P| -> Vec<Segment> {
            vec![Segment::Bezier {
                points: vec![c, e],
                weights: Some(vec![1.0, w, 1.0]),
            }]
        };
        let mut segs = Vec::new();
        segs.extend(q([10.0, 10.0], [0.0, 10.0]));
        segs.extend(q([-10.0, 10.0], [-10.0, 0.0]));
        segs.extend(q([-10.0, -10.0], [0.0, -10.0]));
        segs.extend(q([10.0, -10.0], [10.0, 0.0]));
        let c = Path {
            start: [10.0, 0.0],
            segments: segs,
            closed: true,
        };
        let a = length(&lines(&[c], 0.3, 0.01)) * 0.01;
        assert!((a - 100.0 * PI).abs() < 5e-2, "{a}");
    }

    #[test]
    fn lines_are_kept_inside_a_circle() {
        let l = clip_to_circle(
            &[[[-10.0, 0.0], [10.0, 0.0]], [[0.0, 5.0], [1.0, 5.0]]],
            [0.0, 0.0],
            2.0,
        );
        assert_eq!(l.len(), 1);
        assert!((l[0][0][0] + 2.0).abs() < 1e-12 && (l[0][1][0] - 2.0).abs() < 1e-12);
    }

    #[test]
    fn nothing_for_a_bad_spacing() {
        let sq = Path::polyline(&[[0.0, 0.0], [1.0, 0.0], [1.0, 1.0]], true);
        assert!(lines(std::slice::from_ref(&sq), 0.0, 0.0).is_empty());
        assert!(lines(&[sq], 0.0, 1e-9).is_empty());
    }
}
