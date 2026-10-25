//! The solver's core, apart from sketches: what any system of equations
//! over f64 variables can be solved and diagnosed by. ryumate solves rigid
//! bodies with it, as ryusketch solves sketches.
//!
//! - [`dual`]: forward-mode dual numbers, for residuals written once with
//!   exact gradients.
//! - [`linalg`]: dense Cholesky and pivoted QR, with ranks and null spaces.
//! - [`blocks`]: the Dulmage-Mendelsohn decomposition of a component
//!   (Hopcroft-Karp matching, Tarjan's components).
//! - [`solve`]: damped least-norm Gauss-Newton on a component, whole or
//!   piece by piece.
//! - [`diagnose`]: degrees of freedom, null spaces and the minimal sets of
//!   equations that are redundant or in conflict.
//!
//! A system is anything that implements [`Equations`]: it numbers its
//! variables and equations, and says what each equation reads, its
//! residual and gradient, and how far it is from holding. Arithmetic is
//! f64 in a fixed order, with `ryukei::math` for every function, so a
//! system solves to the same bits on every platform.

pub mod blocks;
pub mod diagnose;
pub mod dual;
pub mod linalg;
pub mod solve;

/// The independent pieces of a system: variables and equations that reach
/// only each other, each by its index, ascending.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Component {
    pub vars: Vec<usize>,
    pub eqs: Vec<usize>,
}

/// The longest step a variable takes by default, in solver units: a system
/// scaled to fit in ±1 of them can't be carried off towards infinity,
/// where angle residuals shrink without anything holding.
pub const MAX_STEP: f64 = 0.5;

/// A system of equations over f64 variables, by index. Variables `x` are
/// held by the caller; equations read them.
pub trait Equations {
    /// The variables equation `eq` reads (repeats allowed).
    fn reads(&self, eq: usize) -> Vec<usize>;

    /// Equation `eq`'s residual at `x`: zero where it holds.
    fn residual(&self, eq: usize, x: &[f64]) -> f64;

    /// How far equation `eq` is from holding at `x`, in units of its
    /// tolerance: it holds at 1 or less. Infinite where it can't be judged
    /// (a direction read from a collapsed line).
    fn misfit(&self, eq: usize, x: &[f64]) -> f64;

    /// Adds equation `eq`'s gradient at `x` into `row`, at the column
    /// `col` gives each variable (none: left out), and returns its
    /// residual.
    fn gradient(
        &self,
        eq: usize,
        x: &[f64],
        col: &dyn Fn(usize) -> Option<usize>,
        row: &mut [f64],
    ) -> f64;

    /// Equation `eq`'s partial derivatives at `x`, one per variable it
    /// reads, in the order it first reads them.
    fn partials(&self, eq: usize, x: &[f64]) -> Vec<(usize, f64)>;

    /// What equation `eq` comes from, as the diagnosis names it: the
    /// caller's constraint or mate. `None` for an equation that belongs to
    /// the geometry itself (an arc keeping its end on its circle), which
    /// takes part in every set.
    fn group(&self, eq: usize) -> Option<u32>;

    /// The residual (in solver units) past which a dependent set of
    /// equations can't all hold to first order: a conflict.
    fn conflict_tol(&self) -> f64;

    /// The longest step variable `var` takes.
    fn max_step(&self, _var: usize) -> f64 {
        MAX_STEP
    }

    /// Variable `var` after a step left it at `value`: a radius stops at
    /// zero, say.
    fn clamp(&self, _var: usize, value: f64) -> f64 {
        value
    }
}
