//! Curve–pipe intersection.
//!
//! A curve point `C(t)` lies on a pipe exactly when, for some spine
//! parameter `s`, it is in the plane normal to the spine at `s` and at the
//! pipe's radius from it:
//!
//! - `E₁(t, s) = (C(t) - c(s)) · c'(s) = 0`, and
//! - `E₂(t, s) = |C(t) - c(s)|² - r² = 0`.
//!
//! Cells of the `(t, s)` plane are enclosed in interval arithmetic (the
//! curve's and the spine's derivatives over them, narrowed by the mean
//! value form) and halved (along whichever parameter's width adds more to
//! the enclosures, so a curve lying along a section of the pipe needs
//! narrow cells across the spine only) until each is excluded (`E₁` or `E₂` of one
//! certified sign), holds exactly one solution (Krawczyk's test), or is
//! certified within the tolerance of the pipe: with `a = |E₁| / |c'|`,
//! how far `C` is along the spine from the normal plane, and `ρ` its
//! distance from the spine across it, `ρ² - r² = E₂ - a²`, so `C` is
//! within `sqrt(a² + ((|E₂| + a²) / r)²)` of the pipe's point in that
//! plane. What none of these settle is [`Failure::Uncertain`].
//!
//! Hits follow [`super::curve_surface`]'s rules: contacts closer than the
//! tolerance are one, a contact shorter than 100 tolerances is a touch,
//! hits within the tolerance of an end of the interval are at it, and
//! sides (the pipe's natural normal points away from its spine, so
//! `Positive` is outside) come from certified signs of `ρ² - r²`.

use super::Failure;
use super::cs::{CurveHit, Side};
use crate::geom::bound2::IVec3;
use crate::geom::{Curve, Pipe, Point3, Surface};
use crate::interval::Interval;
use crate::predicates::Sign;
use crate::tol::LINEAR_RESOLUTION;

/// A contact shorter than this many tolerances is a touch.
const TOUCH: f64 = 100.0;

/// The most times a cell of the `(t, s)` plane is halved, one parameter
/// at a time.
const MAX_DEPTH: u32 = 80;

/// The most cells a search may examine.
const MAX_CELLS: usize = 1 << 20;

type Iv = Interval;

fn dot(a: IVec3, b: IVec3) -> Iv {
    a[0] * b[0] + a[1] * b[1] + a[2] * b[2]
}

fn sub(a: IVec3, b: IVec3) -> IVec3 {
    [a[0] - b[0], a[1] - b[1], a[2] - b[2]]
}

fn signed(x: Iv) -> bool {
    x.lo() > 0.0 || x.hi() < 0.0
}

fn meet(a: Iv, b: Iv) -> Iv {
    Iv::new(a.lo().max(b.lo()), a.hi().min(b.hi())).unwrap_or(a)
}

/// Every place where `curve` over `[t0, t1]` meets the pipe `surface`, in
/// increasing parameter, as [`super::curve_surface`] gives them for an
/// analytic surface. Fails with `Unsupported` unless `surface` is a pipe,
/// and `Uncertain` where a stretch is neither certified apart from the
/// pipe nor within the tolerance of it.
pub fn curve_pipe(
    curve: &Curve,
    [t0, t1]: [f64; 2],
    surface: &Surface,
    tolerance: f64,
) -> Result<Vec<CurveHit>, Failure> {
    let Surface::Pipe(pipe) = surface else {
        return Err(Failure::Unsupported);
    };
    let s = Search {
        curve,
        pipe,
        range: [t0, t1],
        tolerance: tolerance.max(LINEAR_RESOLUTION),
    };
    let mut found = Vec::new();
    let mut budget = MAX_CELLS;
    for ts in s.t_cells() {
        for ss in s.s_cells() {
            s.search([ts, ss], 0, &mut budget, &mut found)?;
        }
    }
    // An end within tolerance of the pipe touches it.
    for t in [t0, t1] {
        if let Some(at) = s.near_point(t) {
            found.push(Candidate {
                t: [t, t],
                s: [at, at],
                simple: None,
            });
        }
    }
    let merged = s.merge(found);
    let mut out = Vec::with_capacity(merged.len());
    for (k, c) in merged.iter().enumerate() {
        let lo = if k == 0 { t0 } else { merged[k - 1].t[1] };
        let hi = merged.get(k + 1).map_or(t1, |n| n.t[0]);
        out.push(s.classify(c, [lo, hi])?);
    }
    Ok(out)
}

/// A stretch of the curve's parameter holding a hit: one simple
/// crossing (`Some(rising)`, from inside the pipe to outside when
/// rising), or a stretch within tolerance, with the stretch of spine
/// parameter its cells covered.
#[derive(Clone, Copy, Debug)]
struct Candidate {
    t: [f64; 2],
    s: [f64; 2],
    simple: Option<bool>,
}

/// Krawczyk's test for a zero of two functions over a cell, from their
/// values at its middle and their Jacobian over it: `Some(true)` for
/// exactly one, `Some(false)` for none, `None` undecided.
fn krawczyk(at: [Iv; 2], jac: [[Iv; 2]; 2], cell: [[f64; 2]; 2]) -> Option<bool> {
    let p = Iv::point;
    let mid = |x: Iv| (x.lo() + x.hi()) / 2.0;
    let [[t0, t1], [s0, s1]] = cell;
    let m = jac.map(|r| r.map(mid));
    let det = m[0][0] * m[1][1] - m[0][1] * m[1][0];
    if det == 0.0 || !det.is_finite() {
        return None;
    }
    let y = [
        [m[1][1] / det, -m[0][1] / det],
        [-m[1][0] / det, m[0][0] / det],
    ];
    let x = [(t0 + t1) / 2.0, (s0 + s1) / 2.0];
    let boxes = [Iv::new(t0, t1)? - p(x[0]), Iv::new(s0, s1)? - p(x[1])];
    let (mut inside, mut apart) = (true, false);
    for row in 0..2 {
        let mut k = p(x[row]) - (p(y[row][0]) * at[0] + p(y[row][1]) * at[1]);
        for (col, &b) in boxes.iter().enumerate() {
            let ident = if row == col { 1.0 } else { 0.0 };
            let e = p(ident) - (p(y[row][0]) * jac[0][col] + p(y[row][1]) * jac[1][col]);
            k = k + e * b;
        }
        let [lo, hi] = cell[row];
        inside &= k.lo() > lo && k.hi() < hi;
        apart |= k.hi() < lo || k.lo() > hi;
    }
    if apart {
        Some(false)
    } else if inside {
        Some(true)
    } else {
        None
    }
}

/// Enclosures of `E`, its Jacobian, and of `|c'|` from below, over a
/// cell.
struct Cell {
    e: [Iv; 2],
    jac: [[Iv; 2]; 2],
    speed: f64,
}

struct Search<'a> {
    curve: &'a Curve,
    pipe: &'a Pipe,
    range: [f64; 2],
    tolerance: f64,
}

impl Search<'_> {
    /// The first cuts of the curve's interval: sixteenths.
    fn t_cells(&self) -> Vec<[f64; 2]> {
        let [t0, t1] = self.range;
        (0..16)
            .map(|i| {
                let at = |k: u32| {
                    if k == 16 {
                        t1
                    } else {
                        t0 + (t1 - t0) * f64::from(k) / 16.0
                    }
                };
                [at(i), at(i + 1)]
            })
            .collect()
    }

    /// The first cuts of the spine's domain: its pieces, at most a 32nd
    /// of it each.
    fn s_cells(&self) -> Vec<[f64; 2]> {
        let [d0, d1] = self.pipe.spine().domain();
        let breaks: Vec<f64> = match self.pipe.spine() {
            Curve::Nurbs(n) => n.pieces().iter().map(|p| p.range[0]).chain([d1]).collect(),
            _ => vec![d0, d1],
        };
        let most = (d1 - d0) / 32.0;
        let mut out = Vec::new();
        for w in breaks.windows(2) {
            let n = ((w[1] - w[0]) / most).ceil().max(1.0) as usize;
            for i in 0..n {
                let at = |k: usize| {
                    if k == n {
                        w[1]
                    } else {
                        w[0] + (w[1] - w[0]) * k as f64 / n as f64
                    }
                };
                out.push([at(i), at(i + 1)]);
            }
        }
        out
    }

    /// The enclosures over a cell, without the mean value form.
    fn raw(&self, [rt, rs]: [[f64; 2]; 2]) -> Cell {
        let c = self.curve.jet_box(rt);
        let k = self.pipe.spine().jet_box(rs);
        let w = sub(c[0], k[0]);
        let r = Iv::point(self.pipe.radius());
        let speed2 = dot(k[1], k[1]);
        Cell {
            e: [dot(w, k[1]), dot(w, w) - r * r],
            jac: [
                [dot(c[1], k[1]), dot(w, k[2]) - speed2],
                [
                    Iv::point(2.0) * dot(w, c[1]),
                    Iv::point(-2.0) * dot(w, k[1]),
                ],
            ],
            speed: speed2.sqrt().map_or(0.0, Iv::lo),
        }
    }

    /// The enclosures over a cell, `E` narrowed by the mean value form
    /// about its middle.
    fn cell(&self, cell: [[f64; 2]; 2]) -> Cell {
        let mut out = self.raw(cell);
        let [[t0, t1], [s0, s1]] = cell;
        let (mt, ms) = ((t0 + t1) / 2.0, (s0 + s1) / 2.0);
        let m = self.raw([[mt, mt], [ms, ms]]);
        let (Some(dt), Some(ds)) = (Iv::new(t0 - mt, t1 - mt), Iv::new(s0 - ms, s1 - ms)) else {
            return out;
        };
        for k in 0..2 {
            out.e[k] = meet(out.e[k], m.e[k] + out.jac[k][0] * dt + out.jac[k][1] * ds);
        }
        out
    }

    /// A certified bound on the distance from the pipe of every curve
    /// point of the cell (see the module notes).
    fn reach(&self, c: &Cell) -> f64 {
        if c.speed <= 0.0 {
            return f64::INFINITY;
        }
        let a = c.e[0].mag() / c.speed;
        let r = self.pipe.radius();
        let across = (c.e[1].mag() + a * a) / r;
        (a * a + across * across).sqrt().next_up() * (1.0 + 1e-12) + super::SLACK
    }

    fn search(
        &self,
        cell: [[f64; 2]; 2],
        depth: u32,
        budget: &mut usize,
        out: &mut Vec<Candidate>,
    ) -> Result<(), Failure> {
        *budget = budget.checked_sub(1).ok_or(Failure::Uncertain)?;
        let c = self.cell(cell);
        if signed(c.e[0]) || signed(c.e[1]) {
            return Ok(());
        }
        if let Some(found) = self.krawczyk(cell, &c) {
            if let Some(hit) = found {
                out.push(hit);
            }
            return Ok(());
        }
        let [[t0, t1], [s0, s1]] = cell;
        if self.reach(&c) <= self.tolerance {
            out.push(Candidate {
                t: [t0, t1],
                s: [s0, s1],
                simple: None,
            });
            return Ok(());
        }
        if depth == MAX_DEPTH {
            return Err(Failure::Uncertain);
        }
        // Halve the parameter whose width adds more to the enclosures.
        let spread = |k: usize, w: f64| c.jac[0][k].mag().max(c.jac[1][k].mag()) * w;
        if spread(0, t1 - t0) >= spread(1, s1 - s0) {
            let mt = (t0 + t1) / 2.0;
            for t in [[t0, mt], [mt, t1]] {
                self.search([t, [s0, s1]], depth + 1, budget, out)?;
            }
        } else {
            let ms = (s0 + s1) / 2.0;
            for s in [[s0, ms], [ms, s1]] {
                self.search([[t0, t1], s], depth + 1, budget, out)?;
            }
        }
        Ok(())
    }

    /// Krawczyk's test on `E` over a cell: `Some(None)` for no solution,
    /// `Some(Some(hit))` for exactly one, a simple crossing whose way is
    /// certified (else it is left to the contact rule), `None`
    /// undecided.
    fn krawczyk(&self, cell: [[f64; 2]; 2], c: &Cell) -> Option<Option<Candidate>> {
        let [[t0, t1], [s0, s1]] = cell;
        let x = [(t0 + t1) / 2.0, (s0 + s1) / 2.0];
        let at = self.raw([[x[0], x[0]], [x[1], x[1]]]).e;
        if !krawczyk(at, c.jac, cell)? {
            return Some(None);
        }
        // Along the solution E₂ changes at the rate ∂E₂/∂t, since ∂E₂/∂s
        // is -2 E₁ = 0 there: rising from inside to outside.
        let slope = c.jac[1][0];
        if !signed(slope) {
            return None;
        }
        let t = self.root(cell);
        Some(Some(Candidate {
            t: [t, t],
            s: [s0, s1],
            simple: Some(slope.lo() > 0.0),
        }))
    }

    /// The parameter of a tangency in a contact: the one place in the
    /// stretch (widened by itself each way) where the curve's distance
    /// from the spine is stationary, `H = (C - c) · C' = 0` together with
    /// `E₁ = 0`, certified by Krawczyk's test over boxes halved down to
    /// a hundredth of the stretch.
    fn tangent(&self, c: &Candidate) -> Option<f64> {
        let (wt, ws) = (c.t[1] - c.t[0], c.s[1] - c.s[0]);
        let [lo, hi] = self.range;
        let cell = [
            [(c.t[0] - wt).max(lo), (c.t[1] + wt).min(hi)],
            [c.s[0] - ws, c.s[1] + ws],
        ];
        let enclose = |[[t0, t1], [s0, s1]]: [[f64; 2]; 2]| {
            let cv = self.curve.jet_box([t0, t1]);
            let k = self.pipe.spine().jet_box([s0, s1]);
            let w = sub(cv[0], k[0]);
            (
                [dot(w, k[1]), dot(w, cv[1])],
                [
                    [dot(cv[1], k[1]), dot(w, k[2]) - dot(k[1], k[1])],
                    [dot(cv[1], cv[1]) + dot(w, cv[2]), -dot(k[1], cv[1])],
                ],
            )
        };
        let mut stack = vec![(cell, 0u32)];
        let mut found = None;
        while let Some((box_, depth)) = stack.pop() {
            let ([e1, h], jac) = enclose(box_);
            if signed(e1) || signed(h) {
                continue;
            }
            let [[t0, t1], [s0, s1]] = box_;
            let (mt, ms) = ((t0 + t1) / 2.0, (s0 + s1) / 2.0);
            let (at, _) = enclose([[mt, mt], [ms, ms]]);
            match krawczyk(at, jac, box_) {
                Some(false) => continue,
                Some(true) if found.is_none() => found = Some(mt),
                Some(true) => return None,
                None if depth < 12 => {
                    for t in [[t0, mt], [mt, t1]] {
                        for s in [[s0, ms], [ms, s1]] {
                            stack.push(([t, s], depth + 1));
                        }
                    }
                }
                None => return None,
            }
        }
        found
    }

    /// The solution in a cell certified to hold one, by Newton's method
    /// on `E` from its middle, kept inside it.
    fn root(&self, [[t0, t1], [s0, s1]]: [[f64; 2]; 2]) -> f64 {
        let (mut t, mut s) = ((t0 + t1) / 2.0, (s0 + s1) / 2.0);
        for _ in 0..40 {
            let c = self.curve.jet3(t);
            let k = self.pipe.spine().jet3(s);
            let w = c[0] - k[0];
            let r = self.pipe.radius();
            let e = [w.dot(k[1]), w.dot(w) - r * r];
            let j = [
                [c[1].dot(k[1]), w.dot(k[2]) - k[1].dot(k[1])],
                [2.0 * w.dot(c[1]), -2.0 * w.dot(k[1])],
            ];
            let det = j[0][0] * j[1][1] - j[0][1] * j[1][0];
            if det == 0.0 || !det.is_finite() {
                break;
            }
            let dt = (j[1][1] * e[0] - j[0][1] * e[1]) / det;
            let ds = (j[0][0] * e[1] - j[1][0] * e[0]) / det;
            let (nt, ns) = ((t - dt).clamp(t0, t1), (s - ds).clamp(s0, s1));
            if nt == t && ns == s {
                break;
            }
            t = nt;
            s = ns;
        }
        t
    }

    /// The spine parameter the curve's point at `t` is certified within
    /// the tolerance of the pipe at, if it is.
    fn near_point(&self, t: f64) -> Option<f64> {
        let q = self.curve.point(t);
        let (s, _) = self.pipe.project(q).param;
        (self.reach(&self.raw([[t, t], [s, s]])) <= self.tolerance).then_some(s)
    }

    /// Whether the curve over `[a, b]` is certified within the tolerance
    /// of the pipe: each part of it near some stretch of spine.
    fn gap_within(&self, [a, b]: [f64; 2]) -> bool {
        let steps = 16;
        (0..steps).all(|i| {
            let at = |k: u32| a + (b - a) * f64::from(k) / f64::from(steps);
            let (x, y) = (at(i), at(i + 1));
            let (s, _) = self.pipe.project(self.curve.point((x + y) / 2.0)).param;
            let ds = 1e-6 * (1.0 + s.abs());
            self.reach(&self.cell([[x, y], [s - ds, s + ds]])) <= self.tolerance
        })
    }

    /// Candidates in order, with overlapping ones, and ones the curve
    /// between stays within the tolerance of the pipe, made one.
    fn merge(&self, mut found: Vec<Candidate>) -> Vec<Candidate> {
        found.sort_by(|a, b| a.t[0].total_cmp(&b.t[0]));
        let mut out: Vec<Candidate> = Vec::with_capacity(found.len());
        for c in found {
            if let Some(last) = out.last_mut() {
                let touching = c.t[0] <= last.t[1]
                    || self
                        .curve
                        .point(last.t[1])
                        .distance(self.curve.point(c.t[0]))
                        <= self.tolerance
                    || self.gap_within([last.t[1], c.t[0]]);
                if touching {
                    last.t[1] = last.t[1].max(c.t[1]);
                    last.s = [last.s[0].min(c.s[0]), last.s[1].max(c.s[1])];
                    last.simple = None;
                    continue;
                }
            }
            out.push(c);
        }
        out
    }

    /// The certified side of the pipe a point is on, by the sign of
    /// `ρ² - r² = E₂ - a²` against the spine point its projection gives.
    fn side_at(&self, q: Point3) -> Option<Sign> {
        let (s, _) = self.pipe.project(q).param;
        let k = self.pipe.spine().jet_box([s, s]);
        let qv = q.to_vector().to_array().map(Iv::point);
        let w = sub(qv, k[0]);
        let r = Iv::point(self.pipe.radius());
        let speed2 = dot(k[1], k[1]);
        let along = dot(w, k[1]);
        let a2 = (along * along).checked_div(speed2)?;
        (dot(w, w) - a2 - r * r).sign().filter(|&x| x != Sign::Zero)
    }

    /// The side just past `from` towards `limit`: the first certified
    /// sign, stepping out by doubling steps.
    fn side(&self, from: f64, limit: f64) -> Result<Side, Failure> {
        let span = limit - from;
        let mut step = span / 1024.0;
        for _ in 0..10 {
            match self.side_at(self.curve.point(from + step)) {
                Some(Sign::Positive) => return Ok(Side::Positive),
                Some(Sign::Negative) => return Ok(Side::Negative),
                _ => step *= 2.0,
            }
        }
        Err(Failure::Uncertain)
    }

    fn classify(&self, c: &Candidate, room: [f64; 2]) -> Result<CurveHit, Failure> {
        let [t0, t1] = self.range;
        let near =
            |a: f64, b: f64| self.curve.point(a).distance(self.curve.point(b)) <= self.tolerance;
        let [a, b] = c.t;
        let short = |x: f64, y: f64| {
            self.curve.point(x).distance(self.curve.point(y)) <= TOUCH * self.tolerance
        };
        let tangent = match c.simple {
            Some(_) => None,
            None => self.tangent(c),
        };
        let mid = tangent.unwrap_or((a + b) / 2.0);
        let point_like = c.simple.is_some()
            || tangent.is_some()
            || (0..=8).all(|k| short(mid, a + (b - a) * f64::from(k) / 8.0));
        if !point_like {
            let lo = if near(a, t0) { t0 } else { a };
            let hi = if near(b, t1) { t1 } else { b };
            return Ok(CurveHit::On { t: [lo, hi] });
        }
        let (at_start, at_end) = (near(mid, t0) || a <= t0, near(mid, t1) || b >= t1);
        let t = if at_start {
            t0
        } else if at_end {
            t1
        } else {
            mid
        };
        let (before, after) = match c.simple {
            Some(rising) if !at_start && !at_end => {
                let (neg, pos) = (Side::Negative, Side::Positive);
                if rising { (neg, pos) } else { (pos, neg) }
            }
            _ => (
                if at_start {
                    Side::Beyond
                } else {
                    self.side(a, room[0])?
                },
                if at_end {
                    Side::Beyond
                } else {
                    self.side(b, room[1])?
                },
            ),
        };
        Ok(CurveHit::Point {
            t,
            point: self.curve.point(t),
            before,
            after,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::geom::pipe::tests::{R, on_plane};
    use crate::geom::{Circle, Frame, Line, Vector3};
    use std::sync::Arc;

    fn tube() -> Surface {
        Surface::Pipe(Arc::new(on_plane()))
    }

    fn line(o: Point3, d: Vector3) -> Curve {
        Curve::Line(Line::new(o, d).unwrap())
    }

    #[test]
    fn a_line_through_the_tube() {
        let pipe = tube();
        let Surface::Pipe(p) = &pipe else {
            unreachable!()
        };
        // Straight down through the spine at s = 0.6: in at the top, out
        // at the bottom (the anchor plane z = 0).
        let c = p.spine().point(0.6);
        let l = line(Point3::new(c.x, c.y, 2.0), Vector3::new(0.0, 0.0, -1.0));
        let hits = curve_pipe(&l, [0.0, 2.5], &pipe, 1e-8).unwrap();
        assert_eq!(hits.len(), 2, "{hits:?}");
        let CurveHit::Point {
            t,
            before,
            after,
            point,
        } = hits[0]
        else {
            panic!("{hits:?}")
        };
        assert!((point.z - 2.0 * R).abs() <= 1e-9 && (t - (2.0 - 2.0 * R)).abs() <= 1e-9);
        assert_eq!((before, after), (Side::Positive, Side::Negative));
        let CurveHit::Point {
            t,
            before,
            after,
            point,
        } = hits[1]
        else {
            panic!("{hits:?}")
        };
        assert!(point.z.abs() <= 1e-9 && (t - 2.0).abs() <= 1e-9);
        assert_eq!((before, after), (Side::Negative, Side::Positive));
        // Stopping inside: one hit, and the end is inside.
        let hits = curve_pipe(&l, [0.0, 2.0 - R], &pipe, 1e-8).unwrap();
        assert_eq!(hits.len(), 1);
    }

    #[test]
    fn misses_touches_and_ends() {
        let pipe = tube();
        let Surface::Pipe(p) = &pipe else {
            unreachable!()
        };
        // A line well above the tube misses it.
        let above = line(Point3::new(-1.0, 0.0, 3.0), Vector3::new(1.0, 0.0, 0.0));
        assert_eq!(curve_pipe(&above, [0.0, 6.0], &pipe, 1e-8).unwrap(), []);
        // A horizontal line across the spine at s = 0.6, touching the top
        // of the tube.
        let c = p.spine().point(0.6);
        let t = p.spine().eval(0.6).d1;
        let across = Vector3::new(-t.y, t.x, 0.0).normalize().unwrap().get();
        let top = line(Point3::new(c.x, c.y, 2.0 * R) - across, across);
        let hits = curve_pipe(&top, [0.0, 2.0], &pipe, 1e-8).unwrap();
        assert_eq!(hits.len(), 1, "{hits:?}");
        let CurveHit::Point { before, after, .. } = hits[0] else {
            panic!("{hits:?}")
        };
        assert_eq!((before, after), (Side::Positive, Side::Positive));
        // A curve starting on the pipe: its end touches.
        let c = p.spine().point(1.3);
        let down = line(Point3::new(c.x, c.y, 2.0 * R), Vector3::new(0.0, 0.0, 1.0));
        let hits = curve_pipe(&down, [0.0, 1.0], &pipe, 1e-8).unwrap();
        assert_eq!(hits.len(), 1);
        let CurveHit::Point { t, before, .. } = hits[0] else {
            panic!("{hits:?}")
        };
        assert_eq!((t, before), (0.0, Side::Beyond));
        // Not a pipe.
        let plane = Surface::Plane(crate::geom::Plane::new(Frame::WORLD));
        assert_eq!(
            curve_pipe(&down, [0.0, 1.0], &plane, 1e-8),
            Err(Failure::Unsupported)
        );
    }

    #[test]
    fn a_section_lies_on_the_tube() {
        // The circle of the tube at s = 0.9 lies on it all along.
        let pipe = tube();
        let Surface::Pipe(p) = &pipe else {
            unreachable!()
        };
        let c = p.spine().point(0.9);
        let t = p.spine().eval(0.9).d1;
        let f = Frame::new(c, t, pipe.point(0.9, 0.0) - c).unwrap();
        let circle = Curve::Circle(Circle::new(f, R).unwrap());
        let hits = curve_pipe(&circle, [0.5, 2.5], &pipe, 1e-8).unwrap();
        assert_eq!(hits, [CurveHit::On { t: [0.5, 2.5] }]);
    }
}
