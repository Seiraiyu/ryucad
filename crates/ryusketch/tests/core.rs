//! The solver's core from outside the crate: a system that isn't a
//! sketch, solved and diagnosed through `ryusketch::core` alone.

use ryusketch::core::dual::D;
use ryusketch::core::{Component, Equations, diagnose, solve};

/// A point `(x, y)` on the unit circle (equation 0, group 7) and on the
/// line `y = k x` (equation 1, group 8), plus optional extra equations.
struct Circle {
    k: f64,
    /// `x - c` for each `c`, in group 9 onwards.
    extra: Vec<f64>,
}

impl Circle {
    fn eval(&self, eq: usize, x: &[f64]) -> D<2> {
        let (a, b) = (D::<2>::input(x[0], 0), D::<2>::input(x[1], 1));
        match eq {
            0 => a * a + b * b - D::constant(1.0),
            1 => b - a.scale(self.k),
            n => a - D::constant(self.extra[n - 2]),
        }
    }
}

impl Equations for Circle {
    fn reads(&self, eq: usize) -> Vec<usize> {
        if eq < 2 { vec![0, 1] } else { vec![0] }
    }
    fn residual(&self, eq: usize, x: &[f64]) -> f64 {
        self.eval(eq, x).v
    }
    fn misfit(&self, eq: usize, x: &[f64]) -> f64 {
        self.eval(eq, x).v.abs() / 1e-12
    }
    fn gradient(
        &self,
        eq: usize,
        x: &[f64],
        col: &dyn Fn(usize) -> Option<usize>,
        row: &mut [f64],
    ) -> f64 {
        let d = self.eval(eq, x);
        for v in 0..2 {
            if let Some(j) = col(v) {
                row[j] += d.g[v];
            }
        }
        d.v
    }
    fn partials(&self, eq: usize, x: &[f64]) -> Vec<(usize, f64)> {
        let d = self.eval(eq, x);
        self.reads(eq).into_iter().map(|v| (v, d.g[v])).collect()
    }
    fn group(&self, eq: usize) -> Option<u32> {
        Some(7 + eq as u32)
    }
    fn conflict_tol(&self) -> f64 {
        1e-12
    }
}

fn all(sys: &Circle) -> Component {
    Component {
        vars: vec![0, 1],
        eqs: (0..2 + sys.extra.len()).collect(),
    }
}

#[test]
fn a_system_that_isnt_a_sketch_solves_and_is_diagnosed() {
    let sys = Circle {
        k: 1.0,
        extra: vec![],
    };
    let comp = all(&sys);
    let mut x = vec![0.9, 0.6];
    let (ok, steps, blocks) = solve::in_pieces(&sys, &comp, &mut x);
    assert!(ok && steps > 0);
    let h = std::f64::consts::FRAC_1_SQRT_2;
    assert!(
        (x[0] - h).abs() < 1e-12 && (x[1] - h).abs() < 1e-12,
        "{x:?}"
    );
    let part = diagnose::diagnose(&sys, &comp, &x, ok, blocks.as_ref());
    assert_eq!(part.dof, 0);
    assert!(part.sets.is_empty() && part.null.is_empty());
    // Held: the same bits, no steps.
    let held = x.clone();
    let (ok, steps, _) = solve::in_pieces(&sys, &comp, &mut x);
    assert!(ok);
    assert_eq!(steps, 0);
    assert_eq!(x, held);
}

#[test]
fn what_is_left_free_and_what_repeats_or_conflicts() {
    // The circle alone: one freedom, a motion round it.
    let sys = Circle {
        k: 1.0,
        extra: vec![],
    };
    let comp = Component {
        vars: vec![0, 1],
        eqs: vec![0],
    };
    let mut x = vec![0.6, 0.8];
    let (ok, _, blocks) = solve::in_pieces(&sys, &comp, &mut x);
    let part = diagnose::diagnose(&sys, &comp, &x, ok, blocks.as_ref());
    assert_eq!(part.dof, 1);
    let t = &part.null[0];
    assert!((t[0] * 0.6 + t[1] * 0.8).abs() < 1e-12, "tangent: {t:?}");
    // The line and x given again where they meet: redundant, together
    // with the two that fix it.
    let h = std::f64::consts::FRAC_1_SQRT_2;
    let sys = Circle {
        k: 1.0,
        extra: vec![h],
    };
    let mut x = vec![h, h];
    let comp = all(&sys);
    let (ok, _, blocks) = solve::in_pieces(&sys, &comp, &mut x);
    assert!(ok);
    let part = diagnose::diagnose(&sys, &comp, &x, ok, blocks.as_ref());
    assert_eq!(part.sets, [(vec![7, 8, 9], false)]);
    // Given elsewhere: a conflict, which doesn't converge.
    let sys = Circle {
        k: 1.0,
        extra: vec![0.2],
    };
    let mut x = vec![h, h];
    let comp = all(&sys);
    let (ok, _, blocks) = solve::in_pieces(&sys, &comp, &mut x);
    assert!(!ok);
    let part = diagnose::diagnose(&sys, &comp, &x, ok, blocks.as_ref());
    assert_eq!(part.sets.len(), 1);
    assert!(part.sets[0].1, "{:?}", part.sets);
}
