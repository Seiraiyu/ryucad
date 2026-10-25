//! Stage 2: the Dulmage-Mendelsohn decomposition of a component, the
//! order its equations can be solved in.
//!
//! Equations and the variables they read form a bipartite graph. A
//! maximum matching pairs equations with variables; what it leaves
//! unmatched splits the component in three: an over-determined part
//! (equations no variable is left for: redundant or in conflict), an
//! under-determined part (variables no equation is left for: what can
//! still move), and a square part between them, which splits further, by
//! the strongly connected components of the matching's dependencies, into
//! square blocks that can be solved one after another. A dimensioned
//! polygon is a chain of 2 x 2 blocks, so a sketch of thousands of
//! entities solves in pieces of a few variables each.
//!
//! The decomposition is structural: it says which equations can be solved
//! for which variables, not whether they are independent at the solution
//! (two horizontal lines made parallel are dependent only by their
//! values). The solver still judges ranks numerically, block by block
//! (see [`super::diagnose`]).
//!
//! Everything iterates in index order with explicit stacks, so the same
//! system decomposes the same way everywhere, at any depth.

use super::{Component, Equations};

/// A component's pieces, in the order they are solved: `over`, then
/// `square` in order, then `under`. Each piece's equations read only its
/// own variables and those of pieces before it; `under`'s read any.
/// Variables and equations are the system's indices, ascending in each.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Blocks {
    /// More equations than the variables they reach.
    pub over: Component,
    /// Square blocks, each solvable once those before it are.
    pub square: Vec<Component>,
    /// More variables than equations: what is left free.
    pub under: Component,
}

impl Blocks {
    /// Every piece that has anything in it, in solving order.
    pub fn pieces(&self) -> impl Iterator<Item = &Component> {
        std::iter::once(&self.over)
            .chain(&self.square)
            .chain(std::iter::once(&self.under))
            .filter(|c| !c.vars.is_empty() || !c.eqs.is_empty())
    }
}

/// The variables (as indices into `comp.vars`) each of `comp`'s equations
/// reads, ascending, each once.
fn adjacency(sys: &impl Equations, comp: &Component) -> Vec<Vec<usize>> {
    comp.eqs
        .iter()
        .map(|&e| {
            let mut vs: Vec<usize> = sys
                .reads(e)
                .into_iter()
                .filter_map(|v| comp.vars.binary_search(&v).ok())
                .collect();
            vs.sort_unstable();
            vs.dedup();
            vs
        })
        .collect()
}

/// A maximum matching of rows (equations) to columns (variables), by
/// Hopcroft and Karp: in phases, a breadth-first search from the free rows
/// layers the rows by their distance along alternating paths, then a
/// depth-first search from each free row in turn (explicit stack, columns
/// in ascending order) augments along shortest paths, each row tried once
/// a phase. A few dozen phases match thousands of rows; augmenting from
/// one row at a time instead can walk a long chain end to end for each
/// row of it.
fn matching(rows: &[Vec<usize>], cols: usize) -> (Vec<Option<usize>>, Vec<Option<usize>>) {
    const FAR: usize = usize::MAX;
    let mut row_of: Vec<Option<usize>> = vec![None; cols];
    let mut col_of: Vec<Option<usize>> = vec![None; rows.len()];
    // A free column of its own for each row that has one.
    for (r, cs) in rows.iter().enumerate() {
        if let Some(&c) = cs.iter().find(|&&c| row_of[c].is_none()) {
            col_of[r] = Some(c);
            row_of[c] = Some(r);
        }
    }
    let mut layer = vec![FAR; rows.len()];
    let mut next = vec![0; rows.len()];
    loop {
        // Layers from the free rows; is a free column in reach?
        let mut queue = std::collections::VecDeque::new();
        for r in 0..rows.len() {
            layer[r] = if col_of[r].is_none() {
                queue.push_back(r);
                0
            } else {
                FAR
            };
        }
        let mut reach = false;
        while let Some(r) = queue.pop_front() {
            for &c in &rows[r] {
                match row_of[c] {
                    None => reach = true,
                    Some(m) if layer[m] == FAR => {
                        layer[m] = layer[r] + 1;
                        queue.push_back(m);
                    }
                    Some(_) => {}
                }
            }
        }
        if !reach {
            return (col_of, row_of);
        }
        next.fill(0);
        for r0 in 0..rows.len() {
            if col_of[r0].is_some() {
                continue;
            }
            // The rows on the path, and the column that led to each row
            // after the first.
            let mut stack = vec![r0];
            let mut via: Vec<usize> = Vec::new();
            while let Some(&r) = stack.last() {
                let Some(&c) = rows[r].get(next[r]) else {
                    // A dead end this phase.
                    layer[r] = FAR;
                    stack.pop();
                    via.pop();
                    continue;
                };
                next[r] += 1;
                match row_of[c] {
                    None => {
                        // Augment: every row on the path takes the column
                        // that led away from it.
                        via.push(c);
                        for (&row, &col) in stack.iter().zip(&via) {
                            col_of[row] = Some(col);
                            row_of[col] = Some(row);
                        }
                        break;
                    }
                    Some(m) if layer[m] != FAR && layer[m] == layer[r] + 1 => {
                        via.push(c);
                        stack.push(m);
                    }
                    Some(_) => {}
                }
            }
        }
    }
}

/// The strongly connected components of a directed graph on `0..n`
/// (Tarjan's, with an explicit stack), each emitted after every component
/// it reaches: dependencies first.
fn strong_components(edges: &[Vec<usize>]) -> Vec<Vec<usize>> {
    let n = edges.len();
    let mut index = vec![usize::MAX; n];
    let mut low = vec![0; n];
    let mut on_stack = vec![false; n];
    let mut stack: Vec<usize> = Vec::new();
    let mut out = Vec::new();
    let mut next = 0;
    for root in 0..n {
        if index[root] != usize::MAX {
            continue;
        }
        let mut calls: Vec<(usize, usize)> = vec![(root, 0)];
        index[root] = next;
        low[root] = next;
        next += 1;
        stack.push(root);
        on_stack[root] = true;
        while let Some(&mut (v, ref mut k)) = calls.last_mut() {
            if let Some(&w) = edges[v].get(*k) {
                *k += 1;
                if index[w] == usize::MAX {
                    index[w] = next;
                    low[w] = next;
                    next += 1;
                    stack.push(w);
                    on_stack[w] = true;
                    calls.push((w, 0));
                } else if on_stack[w] {
                    low[v] = low[v].min(index[w]);
                }
                continue;
            }
            calls.pop();
            if let Some(&(parent, _)) = calls.last() {
                low[parent] = low[parent].min(low[v]);
            }
            if low[v] == index[v] {
                let mut scc = Vec::new();
                loop {
                    let w = stack.pop().expect("v is on the stack");
                    on_stack[w] = false;
                    scc.push(w);
                    if w == v {
                        break;
                    }
                }
                scc.sort_unstable();
                out.push(scc);
            }
        }
    }
    out
}

/// Decomposes `comp` (see the module docs).
pub fn decompose(sys: &impl Equations, comp: &Component) -> Blocks {
    let rows = adjacency(sys, comp);
    let ncols = comp.vars.len();
    let mut cols: Vec<Vec<usize>> = vec![Vec::new(); ncols];
    for (r, vs) in rows.iter().enumerate() {
        for &c in vs {
            cols[c].push(r);
        }
    }
    let (col_of, row_of) = matching(&rows, ncols);
    // Over: what alternating paths reach from unmatched rows (row to any
    // of its columns, column to its matched row).
    let mut over_row = vec![false; rows.len()];
    let mut over_col = vec![false; ncols];
    let mut todo: Vec<usize> = (0..rows.len()).filter(|&r| col_of[r].is_none()).collect();
    for &r in &todo {
        over_row[r] = true;
    }
    while let Some(r) = todo.pop() {
        for &c in &rows[r] {
            if !over_col[c] {
                over_col[c] = true;
                let m = row_of[c].expect("a maximum matching leaves no free column here");
                if !over_row[m] {
                    over_row[m] = true;
                    todo.push(m);
                }
            }
        }
    }
    // Under: what alternating paths reach from unmatched columns (column
    // to any of its rows, row to its matched column).
    let mut under_row = vec![false; rows.len()];
    let mut under_col = vec![false; ncols];
    let mut todo: Vec<usize> = (0..ncols).filter(|&c| row_of[c].is_none()).collect();
    for &c in &todo {
        under_col[c] = true;
    }
    while let Some(c) = todo.pop() {
        for &r in &cols[c] {
            if !under_row[r] {
                under_row[r] = true;
                let m = col_of[r].expect("a maximum matching leaves no free row here");
                if !under_col[m] {
                    under_col[m] = true;
                    todo.push(m);
                }
            }
        }
    }
    let piece = |rs: &[usize]| {
        let mut eqs: Vec<usize> = rs.iter().map(|&r| comp.eqs[r]).collect();
        let mut vars: Vec<usize> = rs
            .iter()
            .filter_map(|&r| col_of[r])
            .map(|c| comp.vars[c])
            .collect();
        eqs.sort_unstable();
        vars.sort_unstable();
        Component { vars, eqs }
    };
    let gather = |row: &[bool], col: &[bool]| Component {
        vars: (0..ncols)
            .filter(|&c| col[c])
            .map(|c| comp.vars[c])
            .collect(),
        eqs: (0..rows.len())
            .filter(|&r| row[r])
            .map(|r| comp.eqs[r])
            .collect(),
    };
    let over = gather(&over_row, &over_col);
    let under = gather(&under_row, &under_col);
    // The square part: the matched rows in neither; a row depends on the
    // rows matched to the other columns it reads.
    let square: Vec<usize> = (0..rows.len())
        .filter(|&r| !over_row[r] && !under_row[r])
        .collect();
    let mut at = vec![usize::MAX; rows.len()];
    for (i, &r) in square.iter().enumerate() {
        at[r] = i;
    }
    let edges: Vec<Vec<usize>> = square
        .iter()
        .map(|&r| {
            rows[r]
                .iter()
                .filter_map(|&c| row_of[c])
                .filter(|&m| m != r && at[m] != usize::MAX)
                .map(|m| at[m])
                .collect()
        })
        .collect();
    let square = strong_components(&edges)
        .into_iter()
        .map(|scc| piece(&scc.iter().map(|&i| square[i]).collect::<Vec<_>>()))
        .collect();
    Blocks {
        over,
        square,
        under,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::sketch::tests::{line, point, rectangle};
    use crate::sketch::{Constraint as C, Entity, Sketch};
    use crate::system::{Scalar, System};

    fn sizes(b: &Blocks) -> (usize, usize, Vec<usize>, usize, usize) {
        (
            b.over.eqs.len(),
            b.over.vars.len(),
            b.square.iter().map(|s| s.vars.len()).collect(),
            b.under.eqs.len(),
            b.under.vars.len(),
        )
    }

    /// The system, and the pieces of its component with the most
    /// equations.
    fn only(s: &Sketch) -> (System, Blocks) {
        let sys = System::new(s);
        let main = sys
            .components
            .iter()
            .max_by_key(|c| c.eqs.len())
            .expect("a component")
            .clone();
        let b = decompose(&sys, &main);
        (sys, b)
    }

    #[test]
    fn matchings_are_maximum() {
        // Rows 0 and 1 both want column 0 first; row 1 can move to 1.
        let rows = vec![vec![0], vec![0, 1], vec![1, 2]];
        let (col_of, row_of) = matching(&rows, 3);
        assert_eq!(col_of, [Some(0), Some(1), Some(2)]);
        assert_eq!(row_of, [Some(0), Some(1), Some(2)]);
        // Three rows on two columns: one is left.
        let (col_of, _) = matching(&[vec![0, 1], vec![0, 1], vec![0, 1]], 2);
        assert_eq!(col_of.iter().filter(|c| c.is_none()).count(), 1);
    }

    #[test]
    fn components_come_dependencies_first() {
        // 0 -> 1 -> 2 -> 1, 3 alone, 4 -> 0.
        let edges = vec![vec![1], vec![2], vec![1], vec![], vec![0]];
        assert_eq!(
            strong_components(&edges),
            [vec![1, 2], vec![0], vec![3], vec![4]]
        );
        // A long chain doesn't overflow anything.
        let n = 100_000;
        let chain: Vec<Vec<usize>> = (0..n)
            .map(|i| if i + 1 < n { vec![i + 1] } else { vec![] })
            .collect();
        let sccs = strong_components(&chain);
        assert_eq!(sccs.len(), n);
        assert_eq!(sccs[0], [n - 1]);
    }

    #[test]
    fn a_fixed_corner_and_two_lengths_are_a_chain() {
        // The dimensioned rectangle of the solver's tests: square, in
        // pieces of one or two variables.
        let s = crate::solve::tests::dimensioned();
        let (_, b) = only(&s);
        let (oe, ov, sq, ue, uv) = sizes(&b);
        assert_eq!((oe, ov, ue, uv), (0, 0, 0, 0));
        assert_eq!(sq.iter().sum::<usize>(), 8);
        assert!(sq.iter().all(|&n| n <= 2), "{sq:?}");
    }

    #[test]
    fn free_and_over_parts() {
        // A horizontal line, its ends' heights: one equation, two
        // variables, under-determined.
        let (_, b) = only(&rectangle_with(&[C::Horizontal(11)]));
        let (oe, _, sq, ue, uv) = sizes(&b);
        assert_eq!((oe, ue, uv), (0, 1, 2));
        assert!(sq.is_empty());
        // A line's length given twice: two equations on its ends' four
        // variables, still under-determined structurally.
        let mut s = rectangle();
        s.entities.insert(
            1,
            Entity::Point {
                at: [0.0, 0.0],
                fixed: true,
            },
        );
        s.constraints.insert(1, C::Horizontal(11));
        s.constraints.insert(2, C::DistanceX(1, 2, 2.0));
        s.constraints.insert(3, C::Length(11, 2.0));
        let (_, b) = only(&s);
        // Point 2's y from the horizontal, its x from the distance: the
        // length has nothing left.
        assert_eq!((b.over.eqs.len(), b.over.vars.len()), (3, 2));
    }

    fn rectangle_with(cs: &[C]) -> Sketch {
        let mut s = rectangle();
        for (i, c) in cs.iter().enumerate() {
            s.constraints.insert(i as u32 + 1, c.clone());
        }
        s
    }

    #[test]
    fn every_equation_and_variable_is_in_one_piece() {
        let mut s = Sketch::default();
        for (id, x, y) in [(1, 0.0, 0.0), (2, 1.0, 0.1), (3, 2.0, 0.0), (4, 3.0, 1.0)] {
            s.entities.insert(id, point(x, y));
        }
        s.entities.insert(10, line(1, 2));
        s.entities.insert(11, line(2, 3));
        s.entities.insert(12, line(3, 4));
        s.constraints.insert(1, C::Fix(1));
        s.constraints.insert(2, C::Length(10, 1.0));
        s.constraints.insert(3, C::Horizontal(10));
        s.constraints.insert(4, C::Parallel(10, 11));
        s.constraints.insert(5, C::Perpendicular(11, 12));
        let sys = System::new(&s);
        assert_eq!(sys.components.len(), 1);
        let b = decompose(&sys, &sys.components[0]);
        let mut eqs: Vec<usize> = b.pieces().flat_map(|p| p.eqs.clone()).collect();
        let mut vars: Vec<usize> = b.pieces().flat_map(|p| p.vars.clone()).collect();
        eqs.sort_unstable();
        vars.sort_unstable();
        assert_eq!(eqs, sys.components[0].eqs);
        assert_eq!(vars, sys.components[0].vars);
        // Each square block reads only its own variables and those of the
        // over part and blocks before it.
        let mut known: Vec<usize> = b.over.vars.clone();
        for blk in &b.square {
            known.extend(&blk.vars);
            for &e in &blk.eqs {
                for a in &sys.eqs[e].args {
                    if let Scalar::Var(v) = a {
                        assert!(known.contains(v), "eq {e} reads {v} too early");
                    }
                }
            }
        }
    }
}
