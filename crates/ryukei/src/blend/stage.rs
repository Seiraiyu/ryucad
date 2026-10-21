//! Stages: blends built one after another where a corner needs it.
//!
//! At a rolled corner (see [`Class::Rolled`]) one fillet is built through
//! the corner first, as if the other two edges were not blended: it ends
//! on the third face there, in an end curve. The other two then run on
//! into that end curve, tangent to both at the earlier fillet's rails,
//! and the end curve is blended with them, the same size: the smaller
//! ball rolls over the earlier fillet. So the blend is built in stages,
//! each an ordinary blend of the body the stage before left; a chain
//! waits for every chain built first at one of its corners, and the two
//! chains rolling over a corner are built together.
//!
//! The journal of the whole blend is composed from the stages': names a
//! stage made and a later one took away never appear, and a vertex a
//! rolled corner took away is replaced by the faces that replaced its end
//! curve as well.

use super::chain::{Chain, Class};
use super::spec::Spec;
use crate::name::{Event, Journal, Name};
use crate::topo::{Body, VertexId};
use crate::{BlendError, Error};
use std::collections::{BTreeMap, BTreeSet};

/// A rolled corner.
#[derive(Clone, Debug)]
pub(crate) struct Rolled {
    pub(crate) vertex: VertexId,
    /// The spec built first, and the two that roll over it.
    pub(crate) first: usize,
    pub(crate) later: [usize; 2],
}

/// Each spec's stage, from 0, and the rolled corners. Fails with
/// `Unsupported` naming a rolled corner whose chains must wait for each
/// other.
pub(crate) fn stages(
    body: &Body,
    specs: &[Spec],
    chains: &[Chain],
) -> Result<(Vec<usize>, Vec<Rolled>), Error> {
    let mut chain_of = vec![0; specs.len()];
    for (ci, c) in chains.iter().enumerate() {
        for &(k, _) in &c.edges {
            chain_of[k] = ci;
        }
    }
    let mut seen: BTreeSet<VertexId> = BTreeSet::new();
    let mut rolled: Vec<Rolled> = Vec::new();
    for c in chains {
        for (&v, class) in c.vertices.iter().zip(&c.classes) {
            let Class::Rolled { first } = *class else {
                continue;
            };
            if !seen.insert(v) {
                continue;
            }
            let at = |e| specs.iter().position(|s| s.edge == e).expect("blended");
            let others: Vec<usize> = body
                .vertex_edges(v)
                .into_iter()
                .filter(|&e| e != first)
                .map(at)
                .collect();
            rolled.push(Rolled {
                vertex: v,
                first: at(first),
                later: [others[0], others[1]],
            });
        }
    }
    rolled.sort_by_key(|r| body.vertices[r.vertex].name);
    // Chains rolling over one corner go together: groups of chains.
    let mut group: Vec<usize> = (0..chains.len()).collect();
    fn root(group: &mut [usize], mut i: usize) -> usize {
        while group[i] != i {
            group[i] = group[group[i]];
            i = group[i];
        }
        i
    }
    for r in &rolled {
        let (a, b) = (
            root(&mut group, chain_of[r.later[0]]),
            root(&mut group, chain_of[r.later[1]]),
        );
        group[a.max(b)] = a.min(b);
    }
    let n = chains.len();
    let roots: Vec<usize> = (0..n).map(|i| root(&mut group, i)).collect();
    // Each group waits for the groups built first at its corners.
    let mut after: BTreeMap<usize, BTreeSet<usize>> = BTreeMap::new();
    let mut waits = vec![0usize; n];
    for r in &rolled {
        let (a, b) = (roots[chain_of[r.first]], roots[chain_of[r.later[0]]]);
        let at = body.vertices[r.vertex].name;
        if a == b {
            return Err(BlendError::Unsupported { at }.into());
        }
        if after.entry(a).or_default().insert(b) {
            waits[b] += 1;
        }
    }
    let mut level = vec![0usize; n];
    let mut ready: Vec<usize> = (0..n).filter(|&g| roots[g] == g && waits[g] == 0).collect();
    let mut done = 0;
    while let Some(g) = ready.pop() {
        done += 1;
        for &h in after.get(&g).into_iter().flatten() {
            level[h] = level[h].max(level[g] + 1);
            waits[h] -= 1;
            if waits[h] == 0 {
                ready.push(h);
            }
        }
    }
    if done < (0..n).filter(|&g| roots[g] == g).count() {
        let at = body.vertices[rolled[0].vertex].name;
        return Err(BlendError::Unsupported { at }.into());
    }
    let stage = (0..specs.len())
        .map(|k| level[roots[chain_of[k]]])
        .collect();
    Ok((stage, rolled))
}

/// The journal of the whole blend from its stages' journals, in order.
/// `before` holds the input's names; `aliases` maps an end curve a later
/// stage blended to the vertex its rolled corner took away.
pub(crate) fn compose(
    before: &BTreeSet<Name>,
    journals: &[Journal],
    aliases: &BTreeMap<Name, Name>,
) -> Journal {
    let mut created: BTreeSet<Name> = BTreeSet::new();
    let mut modified: BTreeSet<Name> = BTreeSet::new();
    let mut replaced: BTreeMap<Name, Vec<Name>> = BTreeMap::new();
    for j in journals {
        for e in j.events() {
            match e {
                Event::Created(n) => {
                    created.insert(*n);
                }
                Event::Modified(n) => {
                    if before.contains(n) {
                        modified.insert(*n);
                    }
                }
                Event::Replaced { old, new } => {
                    let to = if before.contains(old) {
                        Some(*old)
                    } else {
                        created.remove(old);
                        aliases.get(old).copied()
                    };
                    if let Some(o) = to {
                        replaced.entry(o).or_default().extend(new);
                    }
                }
                _ => unreachable!("a blend reports {e:?}"),
            }
        }
    }
    let mut out = Journal::new();
    for (old, mut new) in replaced {
        new.sort();
        new.dedup();
        modified.remove(&old);
        out.push(Event::Replaced { old, new });
    }
    for n in created {
        out.push(Event::Created(n));
    }
    for n in modified {
        out.push(Event::Modified(n));
    }
    out
}
