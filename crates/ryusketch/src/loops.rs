//! The closed loops a profile can be made of: curves that aren't
//! construction geometry, joined at shared points (coincident points are
//! one). A point where exactly two curves end joins them; anywhere else a
//! chain is open. Loops are found by which points curves share, never by
//! where they are; whether loops cross is for the kernel to certify.

use crate::sketch::{Entity, Id, Sketch};
use std::collections::{BTreeMap, BTreeSet};

/// A curve of a loop, walked from its start to its end, or `reversed`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Segment {
    pub entity: Id,
    pub reversed: bool,
}

/// A closed loop, from its smallest curve onwards.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Loop {
    pub segments: Vec<Segment>,
}

/// Curves that join but don't close: their entities, and the points where
/// they stop or branch (each named by its smallest point).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Chain {
    pub entities: Vec<Id>,
    pub ends: Vec<Id>,
}

/// The loops and open chains of `sketch`, with `group` naming the merged
/// point each point belongs to.
pub(crate) fn find(sketch: &Sketch, group: &BTreeMap<Id, Id>) -> (Vec<Loop>, Vec<Chain>) {
    let mut loops = Vec::new();
    // Each line and arc as (curve, start group, end group).
    let mut ends: BTreeMap<Id, (Id, Id)> = BTreeMap::new();
    for (&id, e) in &sketch.entities {
        if e.construction() {
            continue;
        }
        match *e {
            Entity::Circle { .. } => loops.push(Loop {
                segments: vec![Segment {
                    entity: id,
                    reversed: false,
                }],
            }),
            Entity::Line { a, b, .. } => {
                ends.insert(id, (group[&a], group[&b]));
            }
            Entity::Arc { start, end, .. } => {
                ends.insert(id, (group[&start], group[&end]));
            }
            Entity::Point { .. } => {}
        }
    }
    let mut at: BTreeMap<Id, Vec<Id>> = BTreeMap::new();
    for (&c, &(s, e)) in &ends {
        at.entry(s).or_default().push(c);
        at.entry(e).or_default().push(c);
    }
    let mut chains = Vec::new();
    let mut seen = BTreeSet::new();
    for &first in ends.keys() {
        if seen.contains(&first) {
            continue;
        }
        // The curves joined to `first`.
        let mut part = BTreeSet::from([first]);
        let mut todo = vec![first];
        while let Some(c) = todo.pop() {
            let (s, e) = ends[&c];
            for g in [s, e] {
                for &d in &at[&g] {
                    if part.insert(d) {
                        todo.push(d);
                    }
                }
            }
        }
        seen.extend(part.iter().copied());
        let groups: BTreeSet<Id> = part.iter().flat_map(|c| [ends[c].0, ends[c].1]).collect();
        let odd: Vec<Id> = groups
            .iter()
            .copied()
            .filter(|g| at[g].len() != 2)
            .collect();
        if !odd.is_empty() {
            chains.push(Chain {
                entities: part.into_iter().collect(),
                ends: odd,
            });
            continue;
        }
        // Every point joins two curves: walk round from `first`.
        let mut segments = vec![Segment {
            entity: first,
            reversed: false,
        }];
        let (mut prev, mut here) = (first, ends[&first].1);
        loop {
            let next = at[&here]
                .iter()
                .copied()
                .find(|&d| d != prev)
                .unwrap_or(prev);
            if next == first {
                break;
            }
            let (s, e) = ends[&next];
            let reversed = s != here;
            segments.push(Segment {
                entity: next,
                reversed,
            });
            here = if reversed { s } else { e };
            prev = next;
        }
        loops.push(Loop { segments });
    }
    loops.sort_by_key(|l| l.segments[0].entity);
    (loops, chains)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::sketch::Constraint;
    use crate::sketch::tests::{line, point, rectangle};
    use crate::solve::solve;

    fn seg(entity: Id, reversed: bool) -> Segment {
        Segment { entity, reversed }
    }

    #[test]
    fn a_rectangle_is_one_loop() {
        let r = solve(&rectangle()).unwrap().report;
        assert_eq!(
            r.loops,
            [Loop {
                segments: vec![
                    seg(11, false),
                    seg(12, false),
                    seg(13, false),
                    seg(14, false)
                ]
            }]
        );
        assert!(r.open.is_empty());
    }

    #[test]
    fn segments_run_either_way() {
        let mut s = rectangle();
        s.entities.insert(12, line(3, 2));
        s.entities.insert(13, line(4, 3));
        let r = solve(&s).unwrap().report;
        assert_eq!(
            r.loops[0].segments,
            [seg(11, false), seg(12, true), seg(13, true), seg(14, false)]
        );
    }

    #[test]
    fn coincident_ends_join_and_construction_is_left_out() {
        let mut s = rectangle();
        // Line 14 ends at its own copy of point 1, made coincident.
        s.entities.insert(5, point(0.0, 0.0));
        s.entities.insert(14, line(4, 5));
        s.constraints.insert(1, Constraint::Coincident(5, 1));
        s.entities.insert(6, point(1.0, 0.5));
        s.entities.insert(
            15,
            Entity::Line {
                a: 1,
                b: 6,
                construction: true,
            },
        );
        let r = solve(&s).unwrap().report;
        assert_eq!(r.loops.len(), 1);
        assert_eq!(r.loops[0].segments.len(), 4);
    }

    #[test]
    fn open_and_branching_chains() {
        let mut s = rectangle();
        s.entities.remove(&14);
        let r = solve(&s).unwrap().report;
        assert!(r.loops.is_empty());
        assert_eq!(
            r.open,
            [Chain {
                entities: vec![11, 12, 13],
                ends: vec![1, 4]
            }]
        );
        let mut t = rectangle();
        t.entities.insert(5, point(1.0, 0.5));
        t.entities.insert(15, line(1, 5));
        let r = solve(&t).unwrap().report;
        assert_eq!(r.open[0].ends, [1, 5]);
        assert_eq!(r.open[0].entities, [11, 12, 13, 14, 15]);
    }

    #[test]
    fn circles_and_lenses() {
        let mut s = Sketch::default();
        for (id, x, y) in [(1, 0.0, 0.0), (2, 2.0, 0.0), (3, 1.0, -1.0), (4, 5.0, 0.0)] {
            s.entities.insert(id, point(x, y));
        }
        s.entities.insert(10, line(1, 2));
        s.entities.insert(
            11,
            Entity::Arc {
                center: 3,
                start: 1,
                end: 2,
                construction: false,
            },
        );
        s.entities.insert(
            12,
            Entity::Circle {
                center: 4,
                r: 1.0,
                fixed: false,
                construction: false,
            },
        );
        let r = solve(&s).unwrap().report;
        assert_eq!(
            r.loops,
            [
                Loop {
                    segments: vec![seg(10, false), seg(11, true)]
                },
                Loop {
                    segments: vec![seg(12, false)]
                }
            ]
        );
    }
}
