//! The equations constraints become: a residual that is zero when the
//! constraint holds, and its gradient. Residuals are written once, over
//! forward-mode dual numbers ([`crate::core::dual`]), so every Jacobian
//! entry is the exact derivative of the arithmetic that computes the
//! residual, with no derivative written by hand.
//!
//! Lengths are in the solver's scaled units; residuals of angle-like forms
//! (parallel, perpendicular, angle) are sines, which at the sketch's size
//! measure the same as lengths.

/// The most inputs one equation reads.
pub(crate) const SLOTS: usize = 12;

/// A value and its derivatives with respect to an equation's inputs.
pub(crate) type D = crate::core::dual::D<SLOTS>;

/// Shorter than this (in scaled units) a direction is treated as this
/// long, so a collapsed line gives a finite residual, not a NaN.
const TINY: f64 = 1e-12;

fn len(x: D, y: D) -> D {
    (x * x + y * y).sqrt()
}

/// A length that is never zero, for dividing by.
fn safe_len(x: D, y: D) -> D {
    let l = len(x, y);
    if l.v < TINY { D::constant(TINY) } else { l }
}

/// Where a radius comes from: a circle's own variable (one input), or an
/// arc's center and start point (four inputs).
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) enum Rad {
    Var,
    Arc,
}

impl Rad {
    pub fn slots(self) -> usize {
        match self {
            Rad::Var => 1,
            Rad::Arc => 4,
        }
    }

    fn read(self, x: &[D]) -> D {
        match self {
            Rad::Var => x[0],
            Rad::Arc => len(x[2] - x[0], x[3] - x[1]),
        }
    }
}

/// The shape of an equation; `x` below are its inputs in order, points as
/// two inputs (`p` is `x[0], x[1]`).
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) enum Form {
    /// `[a]`: `a - c`.
    Offset { c: f64 },
    /// `[a, b]`: `b - a - c`.
    Diff { c: f64 },
    /// `[a, b, p]` scalars: `p - (a + b) / 2`.
    Mid,
    /// `[p, q]`: `|q - p| - c`.
    Dist { c: f64 },
    /// `[p, q, R]`: `|q - p| - radius`.
    DistRad { r: Rad },
    /// `[p, q, s, t]`: `|q - p| - |t - s|`.
    DistDist,
    /// `[a, b, p]`: the signed distance of `p` from the line through `a`
    /// and `b` (positive to its left), times `sign`, minus `c`.
    LineDist { sign: f64, c: f64 },
    /// `[a, b, p, R]`: `sign` times the signed distance of `p` from the
    /// line, minus a radius.
    LineDistRad { sign: f64, r: Rad },
    /// `[a, b, p, q]`: the distance of the middle of `p` and `q` from the
    /// line through `a` and `b`.
    MidLine,
    /// `[a, b, p, q]`: how far `q - p` reaches along the line's direction.
    Along,
    /// `[a, b, c, d]`: the sine of the angle between `b - a` and `d - c`.
    Cross,
    /// `[a, b, c, d]`: the cosine of that angle.
    Dot,
    /// `[a, b, c, d]`: the sine of that angle minus `angle`, written with
    /// its cosine and sine.
    Angle { cos: f64, sin: f64 },
    /// `[p, s, t]`: the distance of `p` from the line through `s` and `t`
    /// (`s`, `t` and `p` collinear: two arcs meeting tangentially at `p`).
    Collinear,
    /// `[c1, c2, R1, R2]`: `|c2 - c1| - (outer * r1 + sign * r2)`. Circles
    /// touching outside have both 1; inside, the larger radius has 1 and
    /// the smaller -1.
    Circles {
        r1: Rad,
        r2: Rad,
        sign: f64,
        outer: f64,
    },
    /// `[R]`: `k * radius - c`.
    Radius { r: Rad, k: f64, c: f64 },
    /// `[R1, R2]`: `r1 - r2`.
    Radii { r1: Rad, r2: Rad },
    /// `[p]`: `(p - p0) · t`, which pins a point's motion along `t`.
    Toward { p0: [f64; 2], t: [f64; 2] },
}

impl Form {
    /// How many inputs it reads.
    pub fn slots(&self) -> usize {
        match *self {
            Form::Offset { .. } => 1,
            Form::Diff { .. } => 2,
            Form::Mid => 3,
            Form::Dist { .. } => 4,
            Form::DistRad { r } => 4 + r.slots(),
            Form::DistDist => 8,
            Form::LineDist { .. } => 6,
            Form::LineDistRad { r, .. } => 6 + r.slots(),
            Form::MidLine | Form::Along | Form::Cross | Form::Dot | Form::Angle { .. } => 8,
            Form::Collinear => 6,
            Form::Circles { r1, r2, .. } => 4 + r1.slots() + r2.slots(),
            Form::Radius { r, .. } => r.slots(),
            Form::Radii { r1, r2 } => r1.slots() + r2.slots(),
            Form::Toward { .. } => 2,
        }
    }

    /// The residual of inputs `x` (as many as [`slots`](Self::slots)).
    pub fn eval(&self, x: &[D]) -> D {
        debug_assert_eq!(x.len(), self.slots());
        let p = |i: usize| (x[i], x[i + 1]);
        let cross = |(ax, ay): (D, D), (bx, by): (D, D)| ax * by - ay * bx;
        let dot = |(ax, ay): (D, D), (bx, by): (D, D)| ax * bx + ay * by;
        let sub = |(ax, ay): (D, D), (bx, by): (D, D)| (ax - bx, ay - by);
        // The signed distance of point `q` from the line through `a`, `b`.
        let line_dist = |a, b, q| {
            let d = sub(b, a);
            cross(d, sub(q, a)) / safe_len(d.0, d.1)
        };
        // Sine and cosine of the angle from `b - a` to `d - c`.
        let turn = || {
            let (u, w) = (sub(p(2), p(0)), sub(p(6), p(4)));
            let n = safe_len(u.0, u.1) * safe_len(w.0, w.1);
            (cross(u, w) / n, dot(u, w) / n)
        };
        match *self {
            Form::Offset { c } => x[0] - D::constant(c),
            Form::Diff { c } => x[1] - x[0] - D::constant(c),
            Form::Mid => x[2] - (x[0] + x[1]).scale(0.5),
            Form::Dist { c } => {
                let d = sub(p(2), p(0));
                len(d.0, d.1) - D::constant(c)
            }
            Form::DistRad { r } => {
                let d = sub(p(2), p(0));
                len(d.0, d.1) - r.read(&x[4..])
            }
            Form::DistDist => {
                let (d, e) = (sub(p(2), p(0)), sub(p(6), p(4)));
                len(d.0, d.1) - len(e.0, e.1)
            }
            Form::LineDist { sign, c } => line_dist(p(0), p(2), p(4)).scale(sign) - D::constant(c),
            Form::LineDistRad { sign, r } => {
                line_dist(p(0), p(2), p(4)).scale(sign) - r.read(&x[6..])
            }
            Form::MidLine => {
                let m = ((x[4] + x[6]).scale(0.5), (x[5] + x[7]).scale(0.5));
                line_dist(p(0), p(2), m)
            }
            Form::Along => {
                let d = sub(p(2), p(0));
                dot(sub(p(6), p(4)), d) / safe_len(d.0, d.1)
            }
            Form::Cross => turn().0,
            Form::Dot => turn().1,
            Form::Angle { cos, sin } => {
                let (s, c) = turn();
                s.scale(cos) - c.scale(sin)
            }
            Form::Collinear => line_dist(p(2), p(4), p(0)),
            Form::Circles {
                r1,
                r2,
                sign,
                outer,
            } => {
                let d = sub(p(2), p(0));
                let a = r1.read(&x[4..]);
                let b = r2.read(&x[4 + r1.slots()..]);
                len(d.0, d.1) - (a.scale(outer) + b.scale(sign))
            }
            Form::Radius { r, k, c } => r.read(x).scale(k) - D::constant(c),
            Form::Radii { r1, r2 } => r1.read(x) - r2.read(&x[r1.slots()..]),
            Form::Toward { p0, t } => {
                (x[0] - D::constant(p0[0])).scale(t[0]) + (x[1] - D::constant(p0[1])).scale(t[1])
            }
        }
    }

    /// The lines whose directions it reads, by where each starts among its
    /// inputs: a line is the points at `i` and `i + 2`.
    pub fn lines(&self) -> &'static [usize] {
        match *self {
            Form::LineDist { .. } | Form::LineDistRad { .. } | Form::MidLine | Form::Along => &[0],
            Form::Cross | Form::Dot | Form::Angle { .. } => &[0, 4],
            Form::Collinear => &[2],
            _ => &[],
        }
    }

    /// The residual and its gradient at `values` (one per input).
    pub fn residual(&self, values: &[f64]) -> D {
        let x: Vec<D> = values
            .iter()
            .enumerate()
            .map(|(k, &v)| D::input(v, k))
            .collect();
        self.eval(&x)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use ryukei::math;
    use ryukei_testkit::Lcg;

    fn forms() -> Vec<Form> {
        let mut out = vec![
            Form::Offset { c: 0.3 },
            Form::Diff { c: -0.2 },
            Form::Mid,
            Form::Dist { c: 0.5 },
            Form::DistDist,
            Form::LineDist { sign: -1.0, c: 0.1 },
            Form::MidLine,
            Form::Along,
            Form::Cross,
            Form::Dot,
            Form::Angle {
                cos: math::cos(0.4),
                sin: math::sin(0.4),
            },
            Form::Collinear,
            Form::Toward {
                p0: [0.1, 0.2],
                t: [0.6, 0.8],
            },
        ];
        for r in [Rad::Var, Rad::Arc] {
            out.push(Form::DistRad { r });
            out.push(Form::LineDistRad { sign: 1.0, r });
            out.push(Form::Radius { r, k: 2.0, c: 0.3 });
            for r2 in [Rad::Var, Rad::Arc] {
                out.push(Form::Circles {
                    r1: r,
                    r2,
                    sign: -1.0,
                    outer: 1.0,
                });
                out.push(Form::Radii { r1: r, r2 });
            }
        }
        out
    }

    #[test]
    fn every_form_fits_its_slots() {
        for f in forms() {
            assert!(f.slots() <= SLOTS, "{f:?}");
        }
    }

    /// Gradients against central differences at random inputs.
    #[test]
    fn gradients_match_differences() {
        let mut g = Lcg::new(0x6772_6164);
        for f in forms() {
            for _ in 0..50 {
                let x: Vec<f64> = (0..f.slots()).map(|_| g.next_f64(-1.0, 1.0)).collect();
                let d = f.residual(&x);
                assert!(d.v.is_finite(), "{f:?}");
                for k in 0..f.slots() {
                    let h = 1e-6;
                    let (mut lo, mut hi) = (x.clone(), x.clone());
                    lo[k] -= h;
                    hi[k] += h;
                    let fd = (f.residual(&hi).v - f.residual(&lo).v) / (2.0 * h);
                    let err = (fd - d.g[k]).abs();
                    assert!(
                        err <= 1e-5 * (1.0 + fd.abs()),
                        "{f:?} slot {k}: {} vs {fd}",
                        d.g[k]
                    );
                }
            }
        }
    }

    #[test]
    fn residuals_vanish_where_they_hold() {
        let zero = |f: Form, x: &[f64]| {
            let r = f.residual(x).v;
            assert!(r.abs() < 1e-15, "{f:?}: {r}");
        };
        // A unit square's diagonal, its side, and a point on its edge.
        zero(Form::Dist { c: 1.0 }, &[0.0, 0.0, 1.0, 0.0]);
        zero(
            Form::LineDist { sign: 1.0, c: 0.5 },
            &[0.0, 0.0, 2.0, 0.0, 1.0, 0.5],
        );
        zero(
            Form::LineDist { sign: -1.0, c: 0.5 },
            &[0.0, 0.0, 2.0, 0.0, 1.0, -0.5],
        );
        zero(Form::Cross, &[0.0, 0.0, 1.0, 1.0, 3.0, 3.0, 5.0, 5.0]);
        zero(Form::Dot, &[0.0, 0.0, 1.0, 0.0, 3.0, 3.0, 3.0, 5.0]);
        let q = core::f64::consts::FRAC_PI_4;
        zero(
            Form::Angle {
                cos: math::cos(q),
                sin: math::sin(q),
            },
            &[0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 2.0, 2.0],
        );
        zero(Form::Mid, &[1.0, 3.0, 2.0]);
        zero(Form::MidLine, &[0.0, 0.0, 1.0, 0.0, 0.5, 1.0, 0.5, -1.0]);
        zero(Form::Along, &[0.0, 0.0, 1.0, 0.0, 0.5, 1.0, 0.5, -1.0]);
        zero(Form::Collinear, &[1.0, 1.0, 0.0, 0.0, 3.0, 3.0]);
        zero(
            Form::DistRad { r: Rad::Arc },
            &[0.0, 0.0, 0.0, 2.0, 0.0, 0.0, 2.0, 0.0],
        );
        // Circles of radius 1 and 2, centers 3 apart, touch outside; 1
        // apart, inside.
        let outside = Form::Circles {
            r1: Rad::Var,
            r2: Rad::Var,
            sign: 1.0,
            outer: 1.0,
        };
        zero(outside, &[0.0, 0.0, 3.0, 0.0, 1.0, 2.0]);
        let inside = Form::Circles {
            r1: Rad::Var,
            r2: Rad::Var,
            sign: 1.0,
            outer: -1.0,
        };
        zero(inside, &[0.0, 0.0, 1.0, 0.0, 1.0, 2.0]);
        zero(
            Form::Radius {
                r: Rad::Var,
                k: 2.0,
                c: 3.0,
            },
            &[1.5],
        );
    }

    #[test]
    fn collapsed_lines_stay_finite() {
        for f in [
            Form::Cross,
            Form::Dot,
            Form::Along,
            Form::LineDist { sign: 1.0, c: 0.0 },
        ] {
            let x = vec![0.5; f.slots()];
            let d = f.residual(&x);
            assert!(
                d.v.is_finite() && d.g.iter().all(|g| g.is_finite()),
                "{f:?}"
            );
        }
        let d = Form::Dist { c: 0.0 }.residual(&[1.0, 1.0, 1.0, 1.0]);
        assert!(d.g.iter().all(|g| g.is_finite()));
    }
}
