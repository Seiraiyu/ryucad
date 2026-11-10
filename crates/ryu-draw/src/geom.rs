//! Paths of exact curves: lines, circular and elliptic arcs, and Bézier
//! pieces (polynomial or rational, any degree).
//!
//! PDF and SVG draw only lines and cubic Béziers, so [`Path::cubics`]
//! gives every segment as those: lines as lines, polynomial pieces of
//! degree 3 or less exactly (degree elevation), arcs as Béziers of a
//! quarter turn at most (within about 3e-4 of the radius, every writer
//! alike), and rational or higher-degree pieces halved until a cubic
//! through their ends and end tangents lies within [`FIT`] of them at
//! sixteen points. DXF keeps every kind exactly (plan 9.B).

/// A point in the drawing's unit, y up.
pub type P = [f64; 2];

/// How far, in the drawing's unit, a cubic standing for a rational or
/// higher-degree piece may lie from it: a micrometre in millimetres.
pub const FIT: f64 = 1e-3;

/// One segment of a path, from where the path is to its end.
#[derive(Clone, Debug, PartialEq)]
pub enum Segment {
    Line(P),
    /// About `centre` of `radius`, from angle `from` turning by `sweep`
    /// (counterclockwise when positive), in radians.
    Arc {
        centre: P,
        radius: f64,
        from: f64,
        sweep: f64,
    },
    /// `centre + cos t · a + sin t · b` for `t` from `from` by `sweep`:
    /// `a` and `b` are conjugate semi-axes.
    Ellipse {
        centre: P,
        a: P,
        b: P,
        from: f64,
        sweep: f64,
    },
    /// A Bézier piece: its control points after the path's current point
    /// (so `points.len()` is its degree), with `weights` for every control
    /// point, the current one first, when it is rational.
    Bezier {
        points: Vec<P>,
        weights: Option<Vec<f64>>,
    },
}

/// A path: a start, its segments, and whether it closes back to the
/// start.
#[derive(Clone, Debug, PartialEq)]
pub struct Path {
    pub start: P,
    pub segments: Vec<Segment>,
    pub closed: bool,
}

/// A cubic's three control points after its start.
pub type Cubic = [P; 3];

/// One piece of a path as PDF and SVG draw it.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Piece {
    Line(P),
    Cubic(Cubic),
}

fn add(a: P, b: P) -> P {
    [a[0] + b[0], a[1] + b[1]]
}

fn sub(a: P, b: P) -> P {
    [a[0] - b[0], a[1] - b[1]]
}

fn mul(a: P, k: f64) -> P {
    [a[0] * k, a[1] * k]
}

/// The cubic Béziers of the arc about `centre` of radius `r` from angle
/// `from` turning by `sweep`, each a quarter turn at most, as `(control,
/// control, end)`. ryueda's PDF bytes depend on this arithmetic: keep it.
pub fn arc_curves(centre: P, r: f64, from: f64, sweep: f64) -> Vec<Cubic> {
    let n = (sweep.abs() / (std::f64::consts::FRAC_PI_2))
        .ceil()
        .max(1.0) as usize;
    let step = sweep / n as f64;
    let k = 4.0 / 3.0 * ryu_math::tan(step / 4.0);
    let at = |t: f64| {
        let (s, c) = ryu_math::sin_cos(t);
        ([centre[0] + r * c, centre[1] + r * s], [-s, c])
    };
    (0..n)
        .map(|i| {
            let (t0, t1) = (from + step * i as f64, from + step * (i + 1) as f64);
            let ((p0, d0), (p1, d1)) = (at(t0), at(t1));
            [
                [p0[0] + k * r * d0[0], p0[1] + k * r * d0[1]],
                [p1[0] - k * r * d1[0], p1[1] - k * r * d1[1]],
                p1,
            ]
        })
        .collect()
}

impl Segment {
    /// Where the segment ends.
    pub fn end(&self) -> P {
        match self {
            Segment::Line(p) => *p,
            Segment::Arc {
                centre,
                radius,
                from,
                sweep,
            } => {
                let (s, c) = ryu_math::sin_cos(from + sweep);
                [centre[0] + radius * c, centre[1] + radius * s]
            }
            Segment::Ellipse {
                centre,
                a,
                b,
                from,
                sweep,
            } => {
                let (s, c) = ryu_math::sin_cos(from + sweep);
                add(*centre, add(mul(*a, c), mul(*b, s)))
            }
            Segment::Bezier { points, .. } => *points.last().unwrap_or(&[0.0, 0.0]),
        }
    }
}

/// De Casteljau over homogeneous points `(w x, w y, w)` at `t`.
fn casteljau(h: &[[f64; 3]], t: f64) -> (Vec<[f64; 3]>, Vec<[f64; 3]>) {
    let mut row = h.to_vec();
    let mut left = vec![row[0]];
    let mut right = vec![row[row.len() - 1]];
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
        left.push(row[0]);
        right.push(row[row.len() - 1]);
    }
    right.reverse();
    (left, right)
}

fn point_of(h: &[[f64; 3]], t: f64) -> P {
    let (l, _) = casteljau(h, t);
    let e = l[l.len() - 1];
    [e[0] / e[2], e[1] / e[2]]
}

/// The cubic standing for the homogeneous Bézier `h` from `p0`: through
/// its ends with its end tangents' directions, the tangents' lengths a
/// third of its ends' derivatives.
fn hermite(h: &[[f64; 3]]) -> Cubic {
    let n = (h.len() - 1) as f64;
    let at = |i: usize| [h[i][0] / h[i][2], h[i][1] / h[i][2]];
    let last = h.len() - 1;
    let (p0, p1) = (at(0), at(last));
    // A rational Bézier's end derivative is n w1/w0 (P1 − P0).
    let d0 = mul(sub(at(1), p0), n * h[1][2] / h[0][2]);
    let d1 = mul(sub(p1, at(last - 1)), n * h[last - 1][2] / h[last][2]);
    [add(p0, mul(d0, 1.0 / 3.0)), sub(p1, mul(d1, 1.0 / 3.0)), p1]
}

fn cubic_at(p0: P, c: &Cubic, t: f64) -> P {
    let u = 1.0 - t;
    let (a, b, cc, d) = (u * u * u, 3.0 * u * u * t, 3.0 * u * t * t, t * t * t);
    [
        a * p0[0] + b * c[0][0] + cc * c[1][0] + d * c[2][0],
        a * p0[1] + b * c[0][1] + cc * c[1][1] + d * c[2][1],
    ]
}

/// Cubics within [`FIT`] of the homogeneous Bézier `h`, halved at most 12
/// times.
fn fit(h: &[[f64; 3]], depth: u32, out: &mut Vec<Piece>) {
    let c = hermite(h);
    let p0 = [h[0][0] / h[0][2], h[0][1] / h[0][2]];
    let close = (1..16).all(|i| {
        let t = f64::from(i) / 16.0;
        let (a, b) = (point_of(h, t), cubic_at(p0, &c, t));
        ryu_math::hypot(a[0] - b[0], a[1] - b[1]) <= FIT
    });
    if close || depth >= 12 {
        out.push(Piece::Cubic(c));
        return;
    }
    let (l, r) = casteljau(h, 0.5);
    fit(&l, depth + 1, out);
    fit(&r, depth + 1, out);
}

impl Path {
    /// A path of straight lines through `points`.
    pub fn polyline(points: &[P], closed: bool) -> Path {
        Path {
            start: points.first().copied().unwrap_or([0.0, 0.0]),
            segments: points.iter().skip(1).map(|p| Segment::Line(*p)).collect(),
            closed,
        }
    }

    /// A whole circle, counterclockwise from its rightmost point.
    pub fn circle(centre: P, radius: f64) -> Path {
        Path {
            start: [centre[0] + radius, centre[1]],
            segments: vec![Segment::Arc {
                centre,
                radius,
                from: 0.0,
                sweep: 2.0 * std::f64::consts::PI,
            }],
            closed: true,
        }
    }

    /// Every segment as lines and cubics, in order.
    pub fn cubics(&self) -> Vec<Piece> {
        let mut out = Vec::new();
        let mut at = self.start;
        for s in &self.segments {
            match s {
                Segment::Line(p) => out.push(Piece::Line(*p)),
                Segment::Arc {
                    centre,
                    radius,
                    from,
                    sweep,
                } => {
                    out.extend(
                        arc_curves(*centre, *radius, *from, *sweep)
                            .into_iter()
                            .map(Piece::Cubic),
                    );
                }
                Segment::Ellipse {
                    centre,
                    a,
                    b,
                    from,
                    sweep,
                } => {
                    // The unit circle's Béziers mapped by the affine map
                    // taking it to the ellipse.
                    let map = |q: P| add(*centre, add(mul(*a, q[0]), mul(*b, q[1])));
                    for c in arc_curves([0.0, 0.0], 1.0, *from, *sweep) {
                        out.push(Piece::Cubic(c.map(map)));
                    }
                }
                Segment::Bezier { points, weights } => {
                    let polynomial = weights
                        .as_ref()
                        .is_none_or(|w| w.iter().all(|x| *x == w[0]));
                    match (points.len(), polynomial) {
                        (0, _) => {}
                        (1, true) => out.push(Piece::Line(points[0])),
                        (2, true) => {
                            let (c, e) = (points[0], points[1]);
                            out.push(Piece::Cubic([
                                add(at, mul(sub(c, at), 2.0 / 3.0)),
                                add(e, mul(sub(c, e), 2.0 / 3.0)),
                                e,
                            ]));
                        }
                        (3, true) => out.push(Piece::Cubic([points[0], points[1], points[2]])),
                        _ => {
                            let ones = vec![1.0; points.len() + 1];
                            let w = weights.as_ref().unwrap_or(&ones);
                            let h: Vec<[f64; 3]> = std::iter::once(at)
                                .chain(points.iter().copied())
                                .zip(w)
                                .map(|(p, w)| [p[0] * w, p[1] * w, *w])
                                .collect();
                            fit(&h, 0, &mut out);
                        }
                    }
                }
            }
            at = s.end();
        }
        out
    }

    /// A box holding the path: its points' and every cubic's control
    /// points'.
    pub fn bounds(&self) -> [P; 2] {
        let mut lo = self.start;
        let mut hi = self.start;
        let mut grow = |p: P| {
            lo = [lo[0].min(p[0]), lo[1].min(p[1])];
            hi = [hi[0].max(p[0]), hi[1].max(p[1])];
        };
        for piece in self.cubics() {
            match piece {
                Piece::Line(p) => grow(p),
                Piece::Cubic(c) => c.into_iter().for_each(&mut grow),
            }
        }
        [lo, hi]
    }

    /// The path moved by `by`.
    pub fn moved(&self, by: P) -> Path {
        self.map(|p| add(p, by), |v| v)
    }

    /// The path under the affine map `x` (points) and its linear part `v`
    /// (vectors), which must keep orientation for arcs to stay arcs.
    fn map(&self, x: impl Fn(P) -> P, v: impl Fn(P) -> P) -> Path {
        Path {
            start: x(self.start),
            segments: self
                .segments
                .iter()
                .map(|s| match s {
                    Segment::Line(p) => Segment::Line(x(*p)),
                    Segment::Arc {
                        centre,
                        radius,
                        from,
                        sweep,
                    } => Segment::Arc {
                        centre: x(*centre),
                        radius: *radius,
                        from: *from,
                        sweep: *sweep,
                    },
                    Segment::Ellipse {
                        centre,
                        a,
                        b,
                        from,
                        sweep,
                    } => Segment::Ellipse {
                        centre: x(*centre),
                        a: v(*a),
                        b: v(*b),
                        from: *from,
                        sweep: *sweep,
                    },
                    Segment::Bezier { points, weights } => Segment::Bezier {
                        points: points.iter().map(|p| x(*p)).collect(),
                        weights: weights.clone(),
                    },
                })
                .collect(),
            closed: self.closed,
        }
    }

    /// The path scaled by `k > 0` about the origin, then moved by `by`.
    pub fn scaled(&self, k: f64, by: P) -> Path {
        let mut out = self.map(|p| add(mul(p, k), by), |v| mul(v, k));
        for s in &mut out.segments {
            if let Segment::Arc { radius, .. } = s {
                *radius *= k;
            }
        }
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn close(a: P, b: P) -> bool {
        ryu_math::hypot(a[0] - b[0], a[1] - b[1]) < 1e-9
    }

    #[test]
    fn segments_end_where_they_say() {
        let arc = Segment::Arc {
            centre: [1.0, 1.0],
            radius: 2.0,
            from: 0.0,
            sweep: std::f64::consts::FRAC_PI_2,
        };
        assert!(close(arc.end(), [1.0, 3.0]));
        let e = Segment::Ellipse {
            centre: [0.0, 0.0],
            a: [3.0, 0.0],
            b: [0.0, 1.0],
            from: 0.0,
            sweep: std::f64::consts::PI,
        };
        assert!(close(e.end(), [-3.0, 0.0]));
    }

    #[test]
    fn low_degree_polynomials_are_exact_cubics() {
        // A quadratic through (0,0), control (1,2), end (2,0): its middle
        // is (1, 1), and so is its elevated cubic's.
        let p = Path {
            start: [0.0, 0.0],
            segments: vec![Segment::Bezier {
                points: vec![[1.0, 2.0], [2.0, 0.0]],
                weights: None,
            }],
            closed: false,
        };
        let [Piece::Cubic(c)] = p.cubics()[..] else {
            panic!()
        };
        assert!(close(cubic_at([0.0, 0.0], &c, 0.5), [1.0, 1.0]));
    }

    #[test]
    fn a_rational_quarter_circle_fits_within_the_tolerance() {
        let s = std::f64::consts::FRAC_1_SQRT_2;
        let p = Path {
            start: [10.0, 0.0],
            segments: vec![Segment::Bezier {
                points: vec![[10.0, 10.0], [0.0, 10.0]],
                weights: Some(vec![1.0, s, 1.0]),
            }],
            closed: false,
        };
        let pieces = p.cubics();
        let mut at = p.start;
        for piece in &pieces {
            let Piece::Cubic(c) = piece else { panic!() };
            for i in 0..=16 {
                let q = cubic_at(at, c, f64::from(i) / 16.0);
                let r = ryu_math::hypot(q[0], q[1]);
                assert!((r - 10.0).abs() <= 2.0 * FIT, "{r}");
            }
            at = c[2];
        }
        assert!(close(at, [0.0, 10.0]));
    }

    #[test]
    fn bounds_hold_a_circle() {
        let [lo, hi] = Path::circle([0.0, 0.0], 1.0).bounds();
        assert!(lo[0] <= -1.0 && lo[1] <= -1.0 && hi[0] >= 1.0 && hi[1] >= 1.0);
        assert!(hi[0] < 1.4);
    }
}
