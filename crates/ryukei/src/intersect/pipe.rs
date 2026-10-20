//! Where a pipe meets an analytic surface: the field `F = f ∘ P`.
//!
//! A pipe has no polynomial implicit form (its frame normalizes), so the
//! analytic surface's implicit form `f` goes over the pipe's
//! parameterization instead: `F(s, θ) = f(P(s, θ))`, whose zero set in the
//! pipe's parameter plane is the intersection. Both are written over jets
//! (see [`crate::geom::jet`]), so over a cell of the plane interval jets
//! give certified enclosures of `F`, its gradient and its Hessian, which
//! the mean value form about the cell's middle narrows. Along a cell's
//! side, `F`'s roots are isolated by those enclosures: a stretch where it
//! has one certified sign has none, and one where its derivative along the
//! side has one sign and its ends have opposite certified signs has
//! exactly one. A root rounding can't settle (at a corner, or where `F`
//! only touches zero) is [`Failure::Uncertain`].

use super::Failure;
use super::implicit::Implicit;
use crate::geom::jet::Jet;
use crate::geom::{Curve2, Pipe, Point3, Surface};
use crate::interval::Interval;
use crate::math;
use crate::predicates::Sign;
use core::f64::consts::TAU;

/// A box of the pipe's parameter plane: `[s range, θ range]`.
pub type Cell = [[f64; 2]; 2];

/// The deepest a stretch of a side is halved while isolating roots.
const ROOT_DEPTH: u32 = 48;

/// Enclosures of `F`, `[F_s, F_θ]` and `[F_ss, F_sθ, F_θθ]` over a cell.
#[derive(Clone, Copy, Debug)]
pub(crate) struct Enclosure {
    pub(crate) v: Interval,
    pub(crate) d: [Interval; 2],
    pub(crate) h: [Interval; 3],
}

/// A line of the parameter plane: `s` held while `θ` runs, or `θ` held
/// while `s` runs.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) enum Line {
    S(f64),
    T(f64),
}

impl Line {
    /// The cell of the line over `[a, b]` of the parameter that runs.
    fn cell(self, [a, b]: [f64; 2]) -> Cell {
        match self {
            Line::S(s) => [[s, s], [a, b]],
            Line::T(t) => [[a, b], [t, t]],
        }
    }

    /// Which derivative runs along the line.
    fn along(self) -> usize {
        match self {
            Line::S(_) => 1,
            Line::T(_) => 0,
        }
    }

    /// The parameters of the point `x` along the line.
    pub(crate) fn at(self, x: f64) -> (f64, f64) {
        match self {
            Line::S(s) => (s, x),
            Line::T(t) => (x, t),
        }
    }
}

/// The narrower of two enclosures of the same value; the first if
/// rounding left them apart.
fn meet(a: Interval, b: Interval) -> Interval {
    Interval::new(a.lo().max(b.lo()), a.hi().min(b.hi())).unwrap_or(a)
}

/// True when an interval has one certified sign.
pub(crate) fn signed(x: Interval) -> bool {
    x.lo() > 0.0 || x.hi() < 0.0
}

/// A pipe and an analytic surface's implicit form.
pub(crate) struct Field<'a> {
    pub(crate) pipe: &'a Pipe,
    pub(crate) imp: &'a Implicit,
}

impl Field<'_> {
    /// The interval jets of `F` over a cell; `None` where the pipe's frame
    /// can't be certified over it.
    fn jets(&self, cell: Cell) -> Option<Jet<Interval>> {
        let [p, _] = self.pipe.jet_box(cell)?;
        Some(self.imp.over(p))
    }

    /// `F` and its derivatives at a point, in floating point: for Newton's
    /// method, never for a decision.
    pub(crate) fn at(&self, s: f64, t: f64) -> Jet<f64> {
        let d = self.pipe.spine().jet3(s).map(|v| v.to_array());
        let (sin, cos) = math::sin_cos(t);
        let [p, _] = self
            .pipe
            .jets(d, cos, sin)
            .expect("a certified pipe has a frame on its spine");
        self.imp.over(p)
    }

    /// The pipe's point at `(s, θ)`.
    pub(crate) fn point(&self, s: f64, t: f64) -> Point3 {
        self.pipe.point(s, t)
    }

    /// A certified enclosure of `F` at a point.
    pub(crate) fn value(&self, s: f64, t: f64) -> Option<Interval> {
        Some(self.jets([[s, s], [t, t]])?.v)
    }

    /// Certified enclosures over a cell: the jets', narrowed by the mean
    /// value form about the cell's middle, `F(c) ⊂ F(m) + ∇F(cell) (c -
    /// m)`, and by the second order form `F(m) + ∇F(m) (c - m) + ½ (c -
    /// m)ᵀ H(cell) (c - m)`, and likewise for the gradient from the
    /// Hessian.
    pub(crate) fn enclose(&self, cell: Cell) -> Option<Enclosure> {
        let j = self.jets(cell)?;
        let [[s0, s1], [t0, t1]] = cell;
        let (ms, mt) = ((s0 + s1) / 2.0, (t0 + t1) / 2.0);
        let m = self.jets([[ms, ms], [mt, mt]])?;
        let ds = Interval::new(s0, s1)? - Interval::point(ms);
        let dt = Interval::new(t0, t1)? - Interval::point(mt);
        // The mean value form, and the second order form from the
        // gradient at the middle (tight) and the Hessian over the cell.
        let half = Interval::point(0.5);
        let second = m.v
            + m.d[0] * ds
            + m.d[1] * dt
            + half
                * (j.h[0] * ds.square()
                    + Interval::point(2.0) * j.h[1] * ds * dt
                    + j.h[2] * dt.square());
        let v = meet(meet(j.v, m.v + j.d[0] * ds + j.d[1] * dt), second);
        let fs = meet(j.d[0], m.d[0] + j.h[0] * ds + j.h[1] * dt);
        let ft = meet(j.d[1], m.d[1] + j.h[1] * ds + j.h[2] * dt);
        Some(Enclosure {
            v,
            d: [fs, ft],
            h: j.h,
        })
    }

    /// The roots of `F` along `line` over `[a, b]`, each isolated and then
    /// narrowed by bisection to where rounding can't tell its sign, in
    /// order along the line.
    pub(crate) fn roots(&self, line: Line, [a, b]: [f64; 2]) -> Result<Vec<f64>, Failure> {
        let mut out = Vec::new();
        self.isolate(line, [a, b], ROOT_DEPTH, &mut out)?;
        Ok(out)
    }

    fn isolate(
        &self,
        line: Line,
        [a, b]: [f64; 2],
        depth: u32,
        out: &mut Vec<f64>,
    ) -> Result<(), Failure> {
        let split = |out: &mut Vec<f64>| -> Result<(), Failure> {
            if depth == 0 {
                return Err(Failure::Uncertain);
            }
            let m = (a + b) / 2.0;
            if m <= a || m >= b {
                return Err(Failure::Uncertain);
            }
            self.isolate(line, [a, m], depth - 1, out)?;
            self.isolate(line, [m, b], depth - 1, out)
        };
        let Some(e) = self.enclose(line.cell([a, b])) else {
            return split(out);
        };
        if signed(e.v) {
            return Ok(());
        }
        if !signed(e.d[line.along()]) {
            return split(out);
        }
        let sign = |x: f64| {
            let (s, t) = line.at(x);
            self.value(s, t)
                .filter(|v| signed(*v))
                .map(|v| v.lo() > 0.0)
        };
        match (sign(a), sign(b)) {
            (Some(fa), Some(fb)) if fa == fb => Ok(()),
            (Some(fa), Some(_)) => {
                out.push(self.narrow(line, [a, b], fa, sign));
                Ok(())
            }
            // A root at an end of the stretch: on a corner of a cell.
            _ => Err(Failure::Uncertain),
        }
    }

    /// Bisection inside a bracket whose start has sign `fa`, until the
    /// middle's sign is uncertain or the bracket can't shrink.
    fn narrow(
        &self,
        _line: Line,
        [mut a, mut b]: [f64; 2],
        fa: bool,
        sign: impl Fn(f64) -> Option<bool>,
    ) -> f64 {
        loop {
            let m = (a + b) / 2.0;
            if m <= a || m >= b {
                return m;
            }
            match sign(m) {
                Some(x) if x == fa => a = m,
                Some(_) => b = m,
                None => return m,
            }
        }
    }
}

/// The most cells [`pipe_beside`] may examine.
const BESIDE_CELLS: usize = 1 << 16;

/// The deepest [`pipe_beside`] halves a cell, one parameter at a time.
const BESIDE_DEPTH: u32 = 64;

/// Which side of an analytic surface `other` a pipe face lies on, away
/// from a curve where the two touch (a fillet's rail, where it meets its
/// support tangentially, so that no intersection can be traced there).
///
/// `cell` is the face's box in the pipe's parameter plane, and `rail` the
/// touching curve's pcurve on the pipe over its parameter range. Every
/// part of the box lying wholly within `strip` of the rail is left out;
/// on the rest, `F = f ∘ P` must have one certified sign. A part is
/// within the strip when the pcurve's pieces (256 of them, by their
/// boxes) whose every `θ` lies within `strip / r` of every `θ` of the
/// part cover its `s` range between them: at every `s` of the part the
/// rail then passes (each piece is continuous), at an angle the part's
/// points are within `r |Δθ| ≤ strip` of. The parts are halved along
/// whichever parameter adds more to `F`'s enclosure, so the strip needs
/// narrow parts across it only.
///
/// `Ok(Some(sign))` is that sign (`Positive` on the side `other`'s
/// implicit form is positive: outside a cylinder, sphere or torus), or
/// `Zero` when the whole box is within the strip; `Ok(None)` when parts
/// of both signs are certified, so the pipe crosses `other` away from the
/// strip. Fails with `Unsupported` unless `pipe` is a pipe and `other`
/// analytic, and `Uncertain` where a part can't be signed.
pub fn pipe_beside(
    pipe: &Surface,
    other: &Surface,
    cell: Cell,
    (rail, [r0, r1]): (&Curve2, [f64; 2]),
    strip: f64,
) -> Result<Option<Sign>, Failure> {
    let (Surface::Pipe(p), Some(imp)) = (pipe, Implicit::of(other)) else {
        return Err(Failure::Unsupported);
    };
    let field = Field { pipe: p, imp: &imp };
    let pieces: Vec<[[f64; 2]; 2]> = (0..RAIL_PIECES)
        .map(|i| {
            let at = |k: u32| r0 + (r1 - r0) * f64::from(k) / f64::from(RAIL_PIECES);
            let b = rail.bounds([at(i), at(i + 1)]);
            [[b.min().x, b.max().x], [b.min().y, b.max().y]]
        })
        .collect();
    let w = strip / p.radius();
    let within = |[[s0, s1], [t0, t1]]: Cell| {
        // The pieces whose every angle is within w of the part's (a whole
        // turn either way too: pcurves unwrap across the seam), and
        // whether their `s` ranges cover the part's between them.
        let mut runs: Vec<[f64; 2]> = pieces
            .iter()
            .filter(|&&[ps, pt]| {
                ps[1] >= s0
                    && ps[0] <= s1
                    && [-TAU, 0.0, TAU]
                        .iter()
                        .any(|&k| pt[1] - w <= t0 + k && t1 + k <= pt[0] + w)
            })
            .map(|&[ps, _]| ps)
            .collect();
        runs.sort_by(|a, b| a[0].total_cmp(&b[0]));
        let mut reach = s0;
        for [a, b] in runs {
            if a > reach {
                break;
            }
            reach = reach.max(b);
        }
        reach >= s1
    };
    // Breadth first, so that where the pipe crosses `other`, both signs
    // turn up at coarse parts, before the parts along the crossing (which
    // never sign) use up the budget.
    let mut signs = [false; 2];
    let mut budget = BESIDE_CELLS;
    let mut queue = std::collections::VecDeque::from([(cell, 0u32)]);
    while let Some((c, depth)) = queue.pop_front() {
        budget = budget.checked_sub(1).ok_or(Failure::Uncertain)?;
        if within(c) {
            continue;
        }
        let e = field.enclose(c);
        if let Some(e) = e
            && signed(e.v)
        {
            signs[usize::from(e.v.lo() > 0.0)] = true;
            if signs == [true, true] {
                return Ok(None);
            }
            continue;
        }
        if depth == BESIDE_DEPTH {
            return Err(Failure::Uncertain);
        }
        let [[s0, s1], [t0, t1]] = c;
        let along_s = match e {
            Some(e) => e.d[0].mag() * (s1 - s0) >= e.d[1].mag() * (t1 - t0),
            None => true,
        };
        if along_s {
            let m = (s0 + s1) / 2.0;
            queue.push_back(([[s0, m], [t0, t1]], depth + 1));
            queue.push_back(([[m, s1], [t0, t1]], depth + 1));
        } else {
            let m = (t0 + t1) / 2.0;
            queue.push_back(([[s0, s1], [t0, m]], depth + 1));
            queue.push_back(([[s0, s1], [m, t1]], depth + 1));
        }
    }
    Ok(Some(match signs {
        [false, true] => Sign::Positive,
        [true, false] => Sign::Negative,
        _ => Sign::Zero,
    }))
}

/// How many pieces a rail's pcurve is cut into for [`pipe_beside`].
const RAIL_PIECES: u32 = 256;

#[cfg(test)]
mod tests {
    use super::*;
    use crate::geom::pipe::tests::{R, on_plane, round_cylinder};
    use crate::geom::{Cylinder, Frame, Plane, Sphere, Surface, Vector3};
    use core::f64::consts::PI;

    fn surfaces() -> Vec<Surface> {
        let tilted = Frame::new(
            Point3::new(1.5, 0.2, 0.1),
            Vector3::new(1.0, 0.3, 0.2),
            Vector3::new(0.0, 1.0, 0.0),
        )
        .unwrap();
        vec![
            Surface::Plane(Plane::new(tilted)),
            Surface::Sphere(Sphere::new(Frame::WORLD, 2.0).unwrap()),
            Surface::Cylinder(Cylinder::new(tilted, 0.6).unwrap()),
        ]
    }

    #[test]
    fn the_field_is_the_implicit_form_on_the_pipe() {
        for pipe in [on_plane(), round_cylinder()] {
            for s in surfaces() {
                let imp = Implicit::of(&s).unwrap();
                let f = Field {
                    pipe: &pipe,
                    imp: &imp,
                };
                let [d0, d1] = pipe.spine().domain();
                for i in 0..=10 {
                    for j in 0..=10 {
                        let u = d0 + (d1 - d0) * f64::from(i) / 10.0;
                        let t = -PI + 2.0 * PI * f64::from(j) / 10.0;
                        let p = f.point(u, t);
                        let at = f.at(u, t);
                        let (fv, g) = imp.eval_grad(p);
                        assert!((at.v - fv).abs() <= 1e-12 * (1.0 + fv.abs()));
                        assert!(f.value(u, t).unwrap().contains(fv) || (at.v - fv).abs() < 1e-12);
                        let e = Surface::Pipe(std::sync::Arc::new(pipe.clone())).eval(u, t);
                        assert!((at.d[0] - g.dot(e.du)).abs() <= 1e-10 * (1.0 + g.length()));
                        assert!((at.d[1] - g.dot(e.dv)).abs() <= 1e-10 * (1.0 + g.length()));
                    }
                }
            }
        }
    }

    #[test]
    fn enclosures_hold_over_cells() {
        let pipe = round_cylinder();
        for s in surfaces() {
            let imp = Implicit::of(&s).unwrap();
            let f = Field {
                pipe: &pipe,
                imp: &imp,
            };
            for cell in [
                [[0.0, 0.1], [-0.3, 0.1]],
                [[2.0, 2.05], [1.0, 1.4]],
                [[4.0, 4.08], [-PI, -2.5]],
            ] {
                let e = f.enclose(cell).unwrap();
                for i in 0..=8 {
                    for j in 0..=8 {
                        let u = cell[0][0] + (cell[0][1] - cell[0][0]) * f64::from(i) / 8.0;
                        let t = cell[1][0] + (cell[1][1] - cell[1][0]) * f64::from(j) / 8.0;
                        let at = f.at(u, t);
                        let slack = 1e-12 * (1.0 + at.v.abs());
                        let inside =
                            |x: Interval, v: f64| x.lo() - slack <= v && v <= x.hi() + slack;
                        assert!(inside(e.v, at.v), "{cell:?} at {u}, {t}");
                        assert!(inside(e.d[0], at.d[0]) && inside(e.d[1], at.d[1]));
                        for k in 0..3 {
                            assert!(inside(e.h[k], at.h[k]));
                        }
                    }
                }
            }
        }
    }

    #[test]
    fn apart_beside_a_rail() {
        // The tube on the plane z = 0 touches it along the spine's shadow,
        // the line θ = 0: the rest of the tube is above it.
        let pipe = on_plane();
        let rail = crate::geom::Curve2::Line2(
            crate::geom::Line2::new(
                crate::geom::Point2::new(0.0, 0.0),
                crate::geom::Vector2::new(1.0, 0.0),
            )
            .unwrap(),
        );
        let ps = Surface::Pipe(std::sync::Arc::new(pipe));
        let plane = Surface::Plane(Plane::new(Frame::WORLD));
        let whole = [[0.0, 2.0], [-PI, PI]];
        assert_eq!(
            pipe_beside(&ps, &plane, whole, (&rail, [-0.1, 2.1]), 1e-4),
            Ok(Some(Sign::Positive))
        );
        // A plane a little above cuts the tube outside the strip.
        let higher = Surface::Plane(Plane::new(
            Frame::new(
                Point3::new(0.0, 0.0, 0.01),
                Vector3::new(0.0, 0.0, 1.0),
                Vector3::new(1.0, 0.0, 0.0),
            )
            .unwrap(),
        ));
        assert_eq!(
            pipe_beside(&ps, &higher, whole, (&rail, [-0.1, 2.1]), 1e-4),
            Ok(None)
        );
        // With no strip to leave out, the touch can't be signed.
        assert_eq!(
            pipe_beside(&ps, &plane, whole, (&rail, [-0.1, 2.1]), 0.0),
            Err(Failure::Uncertain)
        );
        assert_eq!(
            pipe_beside(&plane, &ps, whole, (&rail, [-0.1, 2.1]), 1e-4),
            Err(Failure::Unsupported)
        );
    }

    #[test]
    fn roots_along_lines() {
        // The plane x = 1.5 across the pipe on the plane: along each line
        // of constant θ there is one root, where the pipe's point has
        // x = 1.5.
        let pipe = on_plane();
        let plane = Surface::Plane(Plane::new(
            Frame::new(
                Point3::new(1.5, 0.0, 0.0),
                Vector3::new(1.0, 0.0, 0.0),
                Vector3::new(0.0, 1.0, 0.0),
            )
            .unwrap(),
        ));
        let imp = Implicit::of(&plane).unwrap();
        let f = Field {
            pipe: &pipe,
            imp: &imp,
        };
        for t in [-3.0, -1.0, 0.1, 0.7, 2.9] {
            let roots = f.roots(Line::T(t), [0.0, 2.0]).unwrap();
            assert_eq!(roots.len(), 1, "{t}");
            assert!((f.point(roots[0], t).x - 1.5).abs() <= 1e-12);
        }
        // Along a line of constant s near the plane, the circle there
        // crosses it twice, or not at all.
        let s = 0.75;
        let c = pipe.spine().point(s);
        let roots = f.roots(Line::S(s), [-PI, PI]).unwrap();
        let expect = if (c.x - 1.5).abs() < R { 2 } else { 0 };
        assert_eq!(roots.len(), expect, "{c:?}");
        for r in roots {
            assert!((f.point(s, r).x - 1.5).abs() <= 1e-12);
        }
        // A root on the end of the stretch can't be settled.
        let t = 0.4;
        let at = f.roots(Line::T(t), [0.0, 2.0]).unwrap()[0];
        assert_eq!(f.roots(Line::T(t), [at, 2.0]), Err(Failure::Uncertain));
    }
}
