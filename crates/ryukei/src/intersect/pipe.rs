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
use crate::geom::{Pipe, Point3};
use crate::interval::Interval;
use crate::math;

/// A box of the pipe's parameter plane: `[s range, θ range]`.
pub(crate) type Cell = [[f64; 2]; 2];

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
    /// m)`, and likewise for the gradient from the Hessian.
    pub(crate) fn enclose(&self, cell: Cell) -> Option<Enclosure> {
        let j = self.jets(cell)?;
        let [[s0, s1], [t0, t1]] = cell;
        let (ms, mt) = ((s0 + s1) / 2.0, (t0 + t1) / 2.0);
        let m = self.jets([[ms, ms], [mt, mt]])?;
        let ds = Interval::new(s0, s1)? - Interval::point(ms);
        let dt = Interval::new(t0, t1)? - Interval::point(mt);
        let v = meet(j.v, m.v + j.d[0] * ds + j.d[1] * dt);
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
