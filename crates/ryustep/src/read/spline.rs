//! B-splines as files write them, made into knot vectors ryukei holds.
//!
//! A file's spline may be clamped (its end knots of multiplicity
//! `degree + 1`), or not: uniform splines, and the periodic form most
//! writers use for closed splines, whose last `degree` control points
//! repeat the first ones over knots that run on past the domain. ryukei
//! takes clamped and periodic knot vectors only, so:
//!
//! - a clamped spline is taken as it is;
//! - an unclamped one whose control points wrap is made periodic: its
//!   first `n - degree` control points over the distinct knots of its
//!   domain, accepted only where it evaluates to the file's spline at
//!   every knot and between;
//! - any other is clamped at its domain's ends by knot insertion
//!   (Boehm's), which changes no point of it.
//!
//! Control points are held as the file gives them (`x, y, z` and the
//! weight `w`), so those that pass through untouched keep their bits;
//! evaluation and knot insertion work on them made homogeneous
//! (`x w, y w, z w, w`), so rational splines are handled alike.

use ryukei::geom::KnotVector;

/// A spline's control net along one direction: columns of points (`x,
/// y, z, w`), each a curve over `flat` of degree `degree`.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct Raw {
    pub(crate) degree: usize,
    /// Every knot, repeated by its multiplicity.
    pub(crate) flat: Vec<f64>,
    /// `points[i][c]`: control point `i` of column `c`.
    pub(crate) points: Vec<Vec<[f64; 4]>>,
}

/// A knot vector ryukei holds, and the control points over it.
pub(crate) struct Made {
    pub(crate) knots: KnotVector,
    pub(crate) points: Vec<Vec<[f64; 4]>>,
}

/// Why a spline couldn't be made: its knots or degree are bad.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct Bad(pub(crate) ryukei::Error);

impl Raw {
    fn n(&self) -> usize {
        self.points.len()
    }

    /// The domain: `[flat[degree], flat[n]]`.
    fn domain(&self) -> [f64; 2] {
        [self.flat[self.degree], self.flat[self.n()]]
    }

    /// The distinct knots of `flat[from..=to]` and their multiplicities
    /// there.
    fn distinct(flat: &[f64]) -> (Vec<f64>, Vec<usize>) {
        let mut knots: Vec<f64> = Vec::new();
        let mut mults: Vec<usize> = Vec::new();
        for &k in flat {
            if knots.last() == Some(&k) {
                *mults.last_mut().expect("a knot") += 1;
            } else {
                knots.push(k);
                mults.push(1);
            }
        }
        (knots, mults)
    }

    /// The homogeneous point of column `c` at `t` in the domain, by de
    /// Boor.
    fn eval(&self, c: usize, t: f64) -> [f64; 4] {
        let p = self.degree;
        let n = self.n();
        let [d0, d1] = self.domain();
        let t = t.clamp(d0, d1);
        // The span: flat[k] <= t < flat[k + 1], p <= k < n.
        let mut k = p;
        while k + 1 < n && self.flat[k + 1] <= t {
            k += 1;
        }
        let mut d: Vec<[f64; 4]> = (0..=p).map(|j| hom(self.points[j + k - p][c])).collect();
        for r in 1..=p {
            for j in (r..=p).rev() {
                let i = j + k - p;
                let den = self.flat[i + p + 1 - r] - self.flat[i];
                let a = if den == 0.0 {
                    0.0
                } else {
                    (t - self.flat[i]) / den
                };
                let prev = d[j - 1];
                for (x, q) in d[j].iter_mut().enumerate() {
                    *q = (1.0 - a) * prev[x] + a * *q;
                }
            }
        }
        d[p]
    }

    /// Inserts knot `t` once (Boehm), in every column.
    fn insert(&mut self, t: f64) {
        let p = self.degree;
        let n = self.n();
        let mut k = p;
        while k + 1 < n && self.flat[k + 1] <= t {
            k += 1;
        }
        let cols = self.points[0].len();
        let mut out: Vec<Vec<[f64; 4]>> = Vec::with_capacity(n + 1);
        for i in 0..=n {
            if i + p <= k {
                out.push(self.points[i].clone());
            } else if i > k {
                out.push(self.points[i - 1].clone());
            } else {
                let den = self.flat[i + p] - self.flat[i];
                let a = if den == 0.0 {
                    0.0
                } else {
                    (t - self.flat[i]) / den
                };
                out.push(
                    (0..cols)
                        .map(|c| {
                            let (q, r) = (hom(self.points[i - 1][c]), hom(self.points[i][c]));
                            cart(core::array::from_fn(|x| (1.0 - a) * q[x] + a * r[x]))
                        })
                        .collect(),
                );
            }
        }
        self.flat.insert(k + 1, t);
        self.points = out;
    }

    /// The same spline clamped at its domain's ends.
    fn clamped(mut self) -> Self {
        let p = self.degree;
        let [d0, d1] = self.domain();
        let count = |r: &Raw, t: f64| r.flat.iter().filter(|&&k| k == t).count();
        while count(&self, d0) < p + 1 {
            self.insert(d0);
        }
        while count(&self, d1) < p + 1 {
            self.insert(d1);
        }
        // Drop what lies outside the domain: knots before the first d0
        // (and their control points) and after the last d1.
        let first = self.flat.iter().position(|&k| k == d0).expect("d0");
        let last = self.flat.iter().rposition(|&k| k == d1).expect("d1");
        let n_points = last - first - p;
        self.points = self.points[first..first + n_points].to_vec();
        self.flat = self.flat[first..=last].to_vec();
        self
    }

    /// The periodic spline the file's unclamped one is, if its control
    /// points wrap and it evaluates alike.
    fn periodic(&self) -> Option<Made> {
        let p = self.degree;
        let n = self.n();
        if n <= 2 * p {
            return None;
        }
        let m = n - p;
        let close = |a: &[f64; 4], b: &[f64; 4]| {
            let s = a.iter().chain(b).fold(1.0f64, |x, y| x.max(y.abs()));
            a.iter().zip(b).all(|(x, y)| (x - y).abs() <= 1e-12 * s)
        };
        for i in 0..p {
            if !self.points[i]
                .iter()
                .zip(&self.points[i + m])
                .all(|(a, b)| close(a, b))
            {
                return None;
            }
        }
        let [d0, d1] = self.domain();
        // One period's distinct knots, with the multiplicities they have
        // in the whole vector.
        let (all, all_m) = Self::distinct(&self.flat);
        let inside: Vec<usize> = (0..all.len())
            .filter(|&i| all[i] >= d0 && all[i] <= d1)
            .collect();
        let knots: Vec<f64> = inside.iter().map(|&i| all[i]).collect();
        let mut mults: Vec<usize> = inside.iter().map(|&i| all_m[i]).collect();
        let r = mults.len() - 1;
        let end = mults[0].max(mults[r]).min(p);
        mults[0] = end;
        mults[r] = end;
        let kv = KnotVector::periodic(p, &knots, &mults).ok()?;
        if kv.control_count() != m {
            return None;
        }
        // The periodic spline's control points: the file's first `m`,
        // turned so its numbering lines up with ryukei's, which is the
        // turn that evaluates alike. Accept it only where it is the same
        // spline at every knot and between.
        let cols = self.points[0].len();
        let samples = 4 * knots.len();
        let same = |made: &Made| {
            (0..cols).all(|c| {
                (0..=samples).all(|s| {
                    let t = d0 + (d1 - d0) * s as f64 / samples as f64;
                    close(&self.eval(c, t), &eval_periodic(made, c, t))
                })
            })
        };
        (0..m).find_map(|turn| {
            let made = Made {
                knots: kv.clone(),
                points: (0..m)
                    .map(|i| self.points[(i + turn) % m].clone())
                    .collect(),
            };
            same(&made).then_some(made)
        })
    }

    /// A clamped spline whose ends meet as a periodic one: the same
    /// knots with the ends' multiplicity `degree` (the joint stays where
    /// it was) and the last control point dropped; accepted only where it
    /// evaluates alike.
    fn closed_to_periodic(&self) -> Option<Made> {
        let p = self.degree;
        let n = self.n();
        let close = |a: &[f64; 4], b: &[f64; 4]| {
            let s = a.iter().chain(b).fold(1.0f64, |x, y| x.max(y.abs()));
            a.iter().zip(b).all(|(x, y)| (x - y).abs() <= 1e-12 * s)
        };
        if n < p + 2
            || !self.points[0]
                .iter()
                .zip(&self.points[n - 1])
                .all(|(a, b)| close(a, b))
        {
            return None;
        }
        let (knots, mut mults) = Self::distinct(&self.flat);
        let r = mults.len() - 1;
        mults[0] = p;
        mults[r] = p;
        let kv = KnotVector::periodic(p, &knots, &mults).ok()?;
        if kv.control_count() != n - 1 {
            return None;
        }
        let [d0, d1] = self.domain();
        let cols = self.points[0].len();
        let samples = 4 * knots.len();
        (0..n - 1).find_map(|turn| {
            let made = Made {
                knots: kv.clone(),
                points: (0..n - 1)
                    .map(|i| self.points[(i + turn) % (n - 1)].clone())
                    .collect(),
            };
            (0..cols)
                .all(|c| {
                    (0..=samples).all(|s| {
                        let t = d0 + (d1 - d0) * s as f64 / samples as f64;
                        close(&self.eval(c, t), &eval_periodic(&made, c, t))
                    })
                })
                .then_some(made)
        })
    }

    /// The spline as a surface's direction must be: as [`Raw::make`], and
    /// periodic where it closes, so that a seam across it can be taken
    /// away.
    pub(crate) fn make_closing(self) -> Result<Made, Bad> {
        let made = self.make()?;
        if made.knots.is_periodic() {
            return Ok(made);
        }
        let raw = Raw {
            degree: made.knots.degree(),
            flat: made.knots.flat().to_vec(),
            points: made.points,
        };
        Ok(raw.closed_to_periodic().unwrap_or(Made {
            knots: made.knots,
            points: raw.points,
        }))
    }

    /// The spline as ryukei holds it.
    pub(crate) fn make(self) -> Result<Made, Bad> {
        let p = self.degree;
        let n = self.n();
        if p == 0 || self.flat.len() != n + p + 1 || n < p + 1 {
            return Err(Bad(ryukei::Error::InvalidNurbs(
                ryukei::NurbsError::KnotMultiplicity,
            )));
        }
        if self.flat.windows(2).any(|w| w[0] > w[1] || w[0].is_nan()) {
            return Err(Bad(ryukei::Error::InvalidNurbs(
                ryukei::NurbsError::KnotOrder,
            )));
        }
        let clamped_ends = self.flat[..=p].iter().all(|&k| k == self.flat[0])
            && self.flat[n..].iter().all(|&k| k == self.flat[n + p]);
        if !clamped_ends && let Some(m) = self.periodic() {
            return Ok(m);
        }
        let raw = if clamped_ends { self } else { self.clamped() };
        let (knots, mults) = Self::distinct(&raw.flat);
        let kv = KnotVector::clamped(raw.degree, &knots, &mults).map_err(Bad)?;
        Ok(Made {
            knots: kv,
            points: raw.points,
        })
    }
}

/// A point and weight made homogeneous.
fn hom(p: [f64; 4]) -> [f64; 4] {
    [p[0] * p[3], p[1] * p[3], p[2] * p[3], p[3]]
}

/// A homogeneous point as a point and weight.
fn cart(h: [f64; 4]) -> [f64; 4] {
    [h[0] / h[3], h[1] / h[3], h[2] / h[3], h[3]]
}

/// The homogeneous point of column `c` of a periodic spline at `t`, by
/// its basis.
fn eval_periodic(m: &Made, c: usize, t: f64) -> [f64; 4] {
    let b = m.knots.basis(t);
    let p = m.knots.degree();
    let n = m.points.len();
    let mut out = [0.0; 4];
    for (r, &v) in b.values(0).iter().enumerate() {
        let q = hom(m.points[(b.span() - p + r) % n][c]);
        for x in 0..4 {
            out[x] += v * q[x];
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A curve's points as one column each.
    fn raw(degree: usize, flat: &[f64], points: &[[f64; 4]]) -> Raw {
        Raw {
            degree,
            flat: flat.to_vec(),
            points: points.iter().map(|p| vec![*p]).collect(),
        }
    }

    fn close(a: [f64; 4], b: [f64; 4]) -> bool {
        a.iter().zip(&b).all(|(x, y)| (x - y).abs() <= 1e-12)
    }

    #[test]
    fn a_clamped_spline_is_taken_as_it_is() {
        let r = raw(
            2,
            &[0.0, 0.0, 0.0, 1.0, 2.0, 2.0, 2.0],
            &[
                [0.0, 0.0, 0.0, 1.0],
                [1.0, 2.0, 0.0, 1.0],
                [3.0, 2.0, 0.0, 1.0],
                [4.0, 0.0, 0.0, 1.0],
            ],
        );
        let m = r.clone().make().unwrap();
        assert_eq!(m.knots.knots(), [0.0, 1.0, 2.0]);
        assert_eq!(m.knots.mults(), [3, 1, 3]);
        assert_eq!(m.points, r.points);
    }

    #[test]
    fn a_uniform_spline_is_clamped_without_moving() {
        // A uniform cubic: knots -3..=6, six control points, domain [0, 3].
        let flat: Vec<f64> = (-3..=6).map(f64::from).collect();
        let pts: Vec<[f64; 4]> = [0.0, 2.0, 1.0, 3.0, 0.5, 2.5]
            .iter()
            .enumerate()
            .map(|(i, &y)| [i as f64, y, 0.0, 1.0])
            .collect();
        let r = raw(3, &flat, &pts);
        let m = r.clone().make().unwrap();
        assert!(!m.knots.is_periodic());
        assert_eq!(m.knots.domain(), [0.0, 3.0]);
        let made = Raw {
            degree: 3,
            flat: m.knots.flat().to_vec(),
            points: m.points.clone(),
        };
        for s in 0..=30 {
            let t = s as f64 / 10.0;
            assert!(close(made.eval(0, t), r.eval(0, t)), "{t}");
        }
    }

    #[test]
    fn a_wrapped_spline_is_periodic() {
        // A periodic quadratic over four spans written unclamped: the first
        // two control points repeated at the end.
        let flat: Vec<f64> = (-2..=6).map(f64::from).collect();
        let base = [
            [1.0, 0.0, 0.0, 1.0],
            [0.0, 1.0, 0.0, 1.0],
            [-1.0, 0.0, 0.0, 1.0],
            [0.0, -1.0, 0.0, 1.0],
        ];
        let pts: Vec<[f64; 4]> = (0..6).map(|i| base[i % 4]).collect();
        let r = raw(2, &flat, &pts);
        let m = r.clone().make().unwrap();
        assert!(m.knots.is_periodic());
        assert_eq!(m.knots.period(), Some(4.0));
        assert_eq!(m.points.len(), 4);
        for s in 0..=40 {
            let t = s as f64 / 10.0;
            assert!(close(eval_periodic(&m, 0, t), r.eval(0, t)), "{t}");
        }
    }

    #[test]
    fn a_closed_clamped_direction_is_made_periodic() {
        // A circle of three arcs as OCCT writes it: clamped, ends meeting.
        let flat = [0.0, 0.0, 0.0, 1.0, 1.0, 2.0, 2.0, 3.0, 3.0, 3.0];
        let h = 0.5;
        let corner = |a: f64| {
            let (s, c) = ryukei::math::sin_cos(a);
            [2.0 * c * h, 2.0 * s * h, 0.0, h]
        };
        let on = |a: f64| {
            let (s, c) = ryukei::math::sin_cos(a);
            [c, s, 0.0, 1.0]
        };
        let third = 2.0 * core::f64::consts::PI / 3.0;
        let pts = [
            on(0.0),
            corner(third / 2.0),
            on(third),
            corner(1.5 * third),
            on(2.0 * third),
            corner(2.5 * third),
            on(0.0),
        ];
        let r = raw(2, &flat, &pts);
        let m = r.clone().make_closing().unwrap();
        assert!(m.knots.is_periodic());
        assert_eq!(m.points.len(), 6);
        // A curve keeps its clamped form.
        assert!(!r.make().unwrap().knots.is_periodic());
    }

    #[test]
    fn bad_knots_are_refused() {
        let r = raw(2, &[0.0, 1.0, 0.5, 2.0], &[[0.0; 4]]);
        assert!(r.make().is_err());
        let r = raw(
            2,
            &[0.0, 0.0, 1.0, 0.5, 2.0, 2.0],
            &[[0.0, 0.0, 0.0, 1.0]; 3],
        );
        assert!(r.make().is_err());
    }
}
