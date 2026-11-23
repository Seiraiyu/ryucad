//! Partial blend deletes (design 919, plan 23.C): what `delete_blends`
//! picks, deleted set by set and then unit by unit, keeping what refuses.

use super::Recognition;
use crate::Error;
use crate::name::Name;
use std::collections::BTreeSet;

/// A partial delete's report: what the units' indices point into, the
/// units deleted and those kept, each by least face name.
#[derive(Clone, Debug, PartialEq)]
pub struct PartialDelete {
    pub recognition: Recognition,
    pub deleted: Vec<BlendUnit>,
    pub kept: Vec<KeptUnit>,
}

/// The smallest group `delete_blends` deletes together: one picked blend
/// closed over its chain and the corners it reaches. `blends` index
/// [`Recognition::blends`]; `faces` are sorted.
#[derive(Clone, Debug, PartialEq)]
pub struct BlendUnit {
    pub blends: Vec<usize>,
    pub faces: Vec<Name>,
}

/// A unit kept, with the last error it refused with.
#[derive(Clone, Debug, PartialEq)]
pub struct KeptUnit {
    pub unit: BlendUnit,
    pub cause: Error,
}

/// The units of `picked`: each picked blend not yet in a unit closed
/// alone (`remake::close`), by least face name.
pub(crate) fn units(rec: &Recognition, picked: &BTreeSet<usize>) -> Vec<BlendUnit> {
    let mut seen = BTreeSet::new();
    let mut out = Vec::new();
    for &i in picked {
        if seen.contains(&i) {
            continue;
        }
        let (blends, faces) = super::remake::close(rec, BTreeSet::from([i]));
        seen.extend(blends.iter().copied());
        out.push(BlendUnit {
            blends: blends.into_iter().collect(),
            faces: faces.into_iter().collect(),
        });
    }
    out.sort_by_key(|u| u.faces[0]);
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::blend::{BlendKind, EdgeBlend, blend};
    use crate::geom::Rigid;
    use crate::name::{FaceRole, OpKey, Role};
    use crate::primitive;
    use crate::topo::Body;
    use crate::tweak::RecogniseOptions;

    /// A block 1 × 0.6 × 0.4 with two edges that share no vertex rounded
    /// 0.05: the top's front edge and the bottom's back edge.
    pub(crate) fn two_rounds() -> Body {
        let k = OpKey(0x23c0);
        let (b, _) = primitive::block(k, 1.0, 0.6, 0.4, &Rigid::IDENTITY).unwrap();
        let face = |r| Name::derive(k, Role::Face(r), &[]);
        let edge = |a, c| {
            let (a, c) = (face(a), face(c));
            b.edges()
                .find(|&e| {
                    let n: Vec<Name> = b
                        .edge_fins(e)
                        .map(|fin| b.faces[b.loops[b.fins[fin].loop_].face].name)
                        .collect();
                    n.contains(&a) && n.contains(&c)
                })
                .map(|e| b.edges[e].name)
                .unwrap()
        };
        let edges = [
            EdgeBlend {
                edge: edge(FaceRole::PlusZ, FaceRole::MinusY),
                kind: BlendKind::Fillet { radius: 0.05 },
            },
            EdgeBlend {
                edge: edge(FaceRole::MinusZ, FaceRole::PlusY),
                kind: BlendKind::Fillet { radius: 0.05 },
            },
        ];
        blend(OpKey(0x23c1), &b, &edges).unwrap().0
    }

    /// Design 919, plan 23.C: units partition the closed picks, each the
    /// closure of its least blend alone, by least face name.
    #[test]
    fn units_partition_the_closed_picks() {
        let b = two_rounds();
        let rec = crate::tweak::recognise_blends(&b, &RecogniseOptions::default()).unwrap();
        assert_eq!(rec.blends.len(), 2);
        let picked: BTreeSet<usize> = (0..rec.blends.len()).collect();
        let us = units(&rec, &picked);
        assert_eq!(us.len(), 2);
        let (_, all) = super::super::remake::close(&rec, picked);
        let mut seen = BTreeSet::new();
        for u in &us {
            for f in &u.faces {
                assert!(seen.insert(*f), "{f:?} in two units");
            }
        }
        assert_eq!(seen, all);
        assert!(us.windows(2).all(|w| w[0].faces[0] < w[1].faces[0]));
    }
}
