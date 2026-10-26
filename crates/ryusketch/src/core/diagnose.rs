//! What a solved component tells its caller beyond its variables: the
//! degrees of freedom left and the motions they allow, and which groups of
//! equations are redundant or in conflict, as small sets an agent can act
//! on.
//!
//! [`whole`] reads these from the Jacobian at the solution, as SolveSpace
//! does: its rank gives the freedom left and its null space what moves; a
//! dependency among its rows (its left null space) is a set of groups
//! that say the same thing, redundant if they all hold and conflicting if
//! they can't.
//!
//! [`by_pieces`] reads them from a component's pieces
//! ([`super::blocks`]) when they are clean: the over part of full column
//! rank, every square block of full rank, the under part of full row
//! rank. The Jacobian is then block triangular with those pieces on its
//! diagonal, so its null space is the under part's and its left null
//! space the over part's, and each is read from its piece alone. Ranks
//! are judged against the whole Jacobian's scale, as [`whole`] judges
//! them. A piece that isn't clean (a dependency only the values show)
//! sends the component back to [`whole`], and so, in a component small
//! enough to read whole ([`WHOLE_LIMIT`]), do clean pieces whose
//! coupling could make the whole nearly singular where none of them is:
//! a point's distance from a line a few microns long reads that line's
//! ends a million times as strongly as the point, and the whole Jacobian
//! finds the line's own constraints dependent on the distance.
//!
//! Each reading decides a rank once: the left null space has as many
//! vectors as the rows less the rank the null space was read with, so the
//! freedom and the dependent sets never disagree about it.

use super::blocks::Blocks;
use super::linalg::{Mat, qr, qr_against};
use super::solve::jacobian;
use super::{Component, Equations};
use std::collections::{BTreeMap, BTreeSet};

/// Diagonal entries of `R` smaller than this, relative to the first, count
/// as zero.
pub const RANK_TOL: f64 = 1e-8;
/// Entries of a unit null vector smaller than this are not part of it.
pub const SUPPORT_TOL: f64 = 1e-6;
/// How far, in multiples of [`RANK_TOL`] times the whole Jacobian's
/// scale, a bound on the smallest singular value of each pair of coupled
/// pieces must keep clear of it for the pieces to be read alone.
pub const COUPLING_MARGIN: f64 = 10.0;
/// Components of at most this many variables whose pieces are coupled too
/// strongly to be read alone are read whole. Larger ones keep their
/// pieces' reading: reading them whole costs what stage 1 does (about a
/// second at a thousand variables, growing with the cube), and the bound
/// is loose on long chains (a polygon of 10,000 entities clears it only
/// 26 times over). So a larger component keeps the pieces' reading
/// however strongly they are coupled, and in rare degenerate sketches its
/// diagnosis may differ from stage 1's.
pub const WHOLE_LIMIT: usize = 1000;

/// What one component's diagnosis found.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Part {
    /// Degrees of freedom left.
    pub dof: usize,
    /// The variables the null space is over, ascending.
    pub vars: Vec<usize>,
    /// A basis of the motions left, each a unit vector over `vars`; every
    /// other variable of the component stays put.
    pub null: Vec<Vec<f64>>,
    /// Minimal dependent sets of groups, each with whether it conflicts.
    pub sets: Vec<(Vec<u32>, bool)>,
}

impl Part {
    /// The variables some motion left moves.
    pub fn support(&self) -> BTreeSet<usize> {
        let mut out = BTreeSet::new();
        for v in &self.null {
            for (j, &var) in self.vars.iter().enumerate() {
                if v[j].abs() > SUPPORT_TOL {
                    out.insert(var);
                }
            }
        }
        out
    }
}

/// Diagnoses `comp` at the solution `x`: from its pieces when it was
/// solved in them and they are clean, else whole. `ok` says whether it
/// converged.
pub fn diagnose(
    sys: &impl Equations,
    comp: &Component,
    x: &[f64],
    ok: bool,
    blocks: Option<&Blocks>,
) -> Part {
    blocks
        .and_then(|b| by_pieces(sys, comp, b, x, ok))
        .unwrap_or_else(|| whole(sys, comp, x, ok))
}

/// The largest column norm of `comp`'s Jacobian at `x`: what its pivoted
/// QR takes as its scale.
fn norms(sys: &impl Equations, comp: &Component, x: &[f64]) -> f64 {
    let mut cols: BTreeMap<usize, f64> = BTreeMap::new();
    for &e in &comp.eqs {
        for (v, g) in sys.partials(e, x) {
            if comp.vars.binary_search(&v).is_ok() {
                *cols.entry(v).or_default() += g * g;
            }
        }
    }
    let col_max = cols.values().fold(0.0, |m: f64, c| m.max(*c));
    ryukei::math::sqrt(col_max)
}

/// A component's dependent sets, each with whether it conflicts, from a
/// Jacobian `jac` of rank `rank`.
fn sets(
    sys: &impl Equations,
    comp: &Component,
    jac: &Mat,
    r: &[f64],
    ok: bool,
    rank: usize,
) -> Vec<(Vec<u32>, bool)> {
    let tol = sys.conflict_tol();
    dependent_sets(sys, comp, jac, rank)
        .into_iter()
        .map(|set| {
            let rows = rows_of(sys, comp, &set);
            let conflict = !ok && inconsistent(&jac.rows_of(&rows), &pick(r, &rows), tol);
            (set, conflict)
        })
        .collect()
}

/// A component read from its whole Jacobian.
pub fn whole(sys: &impl Equations, comp: &Component, x: &[f64], ok: bool) -> Part {
    let (jac, r) = jacobian(sys, comp, x);
    let f = qr(&jac, RANK_TOL);
    Part {
        dof: comp.vars.len() - f.rank,
        vars: comp.vars.clone(),
        null: f.null_space(),
        sets: sets(sys, comp, &jac, &r, ok, f.rank),
    }
}

/// A component read from its pieces, when they are clean (see the module
/// docs); `None` when they aren't.
pub fn by_pieces(
    sys: &impl Equations,
    comp: &Component,
    b: &Blocks,
    x: &[f64],
    ok: bool,
) -> Option<Part> {
    let cols = norms(sys, comp, x);
    let factor = |m: &Mat| qr_against(m, RANK_TOL, Some(cols));
    let mut part = Part::default();
    // Each piece with rows and columns, and the smallest diagonal entry of
    // its `R` that counts for its rank.
    let mut floors: Vec<(&Component, f64)> = Vec::new();
    if !b.over.eqs.is_empty() {
        let (jac, r) = jacobian(sys, &b.over, x);
        let f = factor(&jac);
        let n = b.over.vars.len();
        if f.rank < n {
            return None;
        }
        if n > 0 {
            floors.push((&b.over, f.diagonal(n - 1)));
        }
        part.sets = sets(sys, &b.over, &jac, &r, ok, n);
    }
    for blk in &b.square {
        let f = factor(&jacobian(sys, blk, x).0);
        let n = blk.vars.len();
        if f.rank < n {
            return None;
        }
        if n > 0 {
            floors.push((blk, f.diagonal(n - 1)));
        }
    }
    if !b.under.vars.is_empty() {
        let f = factor(&jacobian(sys, &b.under, x).0);
        let m = b.under.eqs.len();
        if f.rank < m {
            return None;
        }
        if m > 0 {
            floors.push((&b.under, f.diagonal(m - 1)));
        }
        part.dof = b.under.vars.len() - f.rank;
        part.vars = b.under.vars.clone();
        part.null = f.null_space();
    }
    if comp.vars.len() <= WHOLE_LIMIT && !coupled_well(sys, comp, &floors, x, cols) {
        return None;
    }
    Some(part)
}

/// Whether pieces that are each clean stay so together. A piece's rows
/// also read the columns of pieces before it; for one piece `D_i` with
/// smallest singular value about `a_i` (its `floor`) reading a piece
/// `D_j`'s columns through `B_ij`, the two together keep a smallest
/// singular value of at least `a_i a_j / (a_i + a_j + |B_ij|)`. Every
/// coupled pair must keep that clear of the whole's rank tolerance by
/// [`COUPLING_MARGIN`]; a longer chain of strong couplings isn't bounded.
fn coupled_well(
    sys: &impl Equations,
    comp: &Component,
    floors: &[(&Component, f64)],
    x: &[f64],
    cols: f64,
) -> bool {
    // The piece of each of the component's variables.
    let mut piece = vec![usize::MAX; comp.vars.len()];
    for (i, (p, _)) in floors.iter().enumerate() {
        for v in &p.vars {
            if let Ok(k) = comp.vars.binary_search(v) {
                piece[k] = i;
            }
        }
    }
    let mut coupling: BTreeMap<(usize, usize), f64> = BTreeMap::new();
    for (i, (p, _)) in floors.iter().enumerate() {
        for &e in &p.eqs {
            for (v, g) in sys.partials(e, x) {
                if let Ok(k) = comp.vars.binary_search(&v) {
                    let j = piece[k];
                    if j != i && j != usize::MAX {
                        *coupling.entry((i, j)).or_default() += g * g;
                    }
                }
            }
        }
    }
    let tol = RANK_TOL * COUPLING_MARGIN * cols;
    coupling.into_iter().all(|((i, j), b2)| {
        let (a, c) = (floors[i].1, floors[j].1);
        a * c > tol * (a + c + ryukei::math::sqrt(b2))
    })
}

fn pick(r: &[f64], rows: &[usize]) -> Vec<f64> {
    rows.iter().map(|&i| r[i]).collect()
}

/// The rows (within the component) of the groups in `set`, and of every
/// equation that belongs to no group.
fn rows_of(sys: &impl Equations, comp: &Component, set: &[u32]) -> Vec<usize> {
    comp.eqs
        .iter()
        .enumerate()
        .filter(|(_, e)| sys.group(**e).is_none_or(|g| set.contains(&g)))
        .map(|(i, _)| i)
        .collect()
}

/// Whether `rows` of a Jacobian depend on each other.
fn dependent(m: &Mat) -> bool {
    qr(m, RANK_TOL).rank < m.rows
}

/// Whether residuals `r` of dependent rows `m` can't all be zero to first
/// order: some combination of the rows vanishes while the same combination
/// of the residuals doesn't.
fn inconsistent(m: &Mat, r: &[f64], tol: f64) -> bool {
    qr(&m.transpose(), RANK_TOL).null_space().iter().any(|y| {
        let s: f64 = y.iter().zip(r).map(|(a, b)| a * b).sum();
        s.abs() > tol
    })
}

/// The reduced echelon form of the span of `vectors` (each over a
/// component's rows), pivoting from the last row back: one vector per
/// pivot row, 1 there and 0 at every other pivot. It depends on the span
/// alone, not on the basis it came in, so a whole and a piecewise reading
/// name the same sets, and each set is a group with the earlier ones it
/// repeats: `{d1, d9}` for a ninth dimension that says what the first
/// does.
pub fn echelon(mut vectors: Vec<Vec<f64>>) -> Vec<Vec<f64>> {
    let rows = vectors.first().map_or(0, Vec::len);
    let mut done = 0;
    for j in (0..rows).rev() {
        if done == vectors.len() {
            break;
        }
        // The largest entry at row `j` among the vectors not yet pivoted.
        let (best, size) = (done..vectors.len())
            .map(|v| (v, vectors[v][j].abs()))
            .fold((done, -1.0), |a, b| if b.1 > a.1 { b } else { a });
        if size <= SUPPORT_TOL {
            continue;
        }
        vectors.swap(done, best);
        let p = vectors[done][j];
        for x in &mut vectors[done] {
            *x /= p;
        }
        let pivot = vectors[done].clone();
        for (v, other) in vectors.iter_mut().enumerate() {
            let f = other[j];
            if v != done && f != 0.0 {
                for (x, q) in other.iter_mut().zip(&pivot) {
                    *x -= f * q;
                }
            }
        }
        done += 1;
    }
    vectors.truncate(done);
    vectors
}

/// Minimal sets of groups whose rows depend on each other, one per vector
/// of the left null space's echelon form (as many as `jac`'s rows less
/// `rank`, the rank its null space was read with), each cut down until
/// removing any of its groups makes it independent.
fn dependent_sets(sys: &impl Equations, comp: &Component, jac: &Mat, rank: usize) -> Vec<Vec<u32>> {
    let mut out = BTreeSet::new();
    for y in echelon(qr(&jac.transpose(), 0.0).with_rank(rank).null_space()) {
        let mut set: Vec<u32> = y
            .iter()
            .enumerate()
            .filter(|(_, w)| w.abs() > SUPPORT_TOL)
            .filter_map(|(i, _)| sys.group(comp.eqs[i]))
            .collect::<BTreeSet<u32>>()
            .into_iter()
            .collect();
        let mut k = 0;
        while k < set.len() {
            let mut fewer = set.clone();
            fewer.remove(k);
            if dependent(&jac.rows_of(&rows_of(sys, comp, &fewer))) {
                set = fewer;
            } else {
                k += 1;
            }
        }
        if !set.is_empty() {
            out.insert(set);
        }
    }
    out.into_iter().collect()
}

#[cfg(test)]
mod tests {
    #[test]
    fn echelon_forms_depend_on_the_span_alone() {
        // Two bases of one plane in three rows.
        let a = vec![vec![1.0, 1.0, 0.0], vec![0.0, 1.0, 1.0]];
        let b = vec![vec![1.0, 2.0, 1.0], vec![-1.0, 0.0, 1.0]];
        let (ea, eb) = (super::echelon(a), super::echelon(b));
        assert_eq!(ea.len(), 2);
        for (u, w) in ea.iter().zip(&eb) {
            for (x, y) in u.iter().zip(w) {
                assert!((x - y).abs() < 1e-12, "{ea:?} vs {eb:?}");
            }
        }
        // Pivots on the last rows: each the later row with the first.
        assert_eq!(ea[0], [-1.0, 0.0, 1.0]);
        assert_eq!(ea[1], [1.0, 1.0, 0.0]);
    }
}
