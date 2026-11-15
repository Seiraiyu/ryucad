//! Near boxes (design 669, plan 20.B): the part of a rung's box each
//! working face can need. A region's changed faces are joined into gaps
//! where they share a vertex; a working face is carried over its own box
//! and the boxes of the gaps it touches, grown by their reach (the same
//! rule as `region_box`, for one face's neighbourhood). Applied in
//! narrowed builds only, so first builds keep their bits.

use super::region::{Region, face_vertices};
use crate::geom::Box3;
use crate::intersect::face_box;
use crate::topo::{Body, FaceId, VertexId};
use crate::tweak::engine::Changes;
use std::collections::{BTreeMap, BTreeSet};

/// A region's changed faces in gaps: faces sharing a vertex are one gap
/// (plan 20.B, Decision 1). Each gap's box and its reach.
#[derive(Clone, Debug)]
#[cfg_attr(not(test), allow(dead_code))]
pub(crate) struct Gaps {
    /// Each changed face's gap.
    pub(crate) of: BTreeMap<FaceId, usize>,
    /// Each gap's box: its faces' boxes joined.
    pub(crate) boxes: Vec<Box3>,
    /// Each gap's reach: `retrim::reach` over its faces alone.
    pub(crate) reach: Vec<f64>,
}

impl Gaps {
    /// The gaps of `region`'s changed faces, numbered by their least face.
    #[cfg_attr(not(test), allow(dead_code))]
    pub(crate) fn of(body: &Body, changes: &Changes, region: &Region) -> Gaps {
        let mut at: BTreeMap<VertexId, Vec<FaceId>> = BTreeMap::new();
        for &f in &region.changed {
            for v in face_vertices(body, f) {
                at.entry(v).or_default().push(f);
            }
        }
        let mut up: BTreeMap<FaceId, FaceId> = region.changed.iter().map(|&f| (f, f)).collect();
        fn root(up: &BTreeMap<FaceId, FaceId>, mut f: FaceId) -> FaceId {
            while up[&f] != f {
                f = up[&f];
            }
            f
        }
        for fs in at.values() {
            for w in fs.windows(2) {
                let (a, b) = (root(&up, w[0]), root(&up, w[1]));
                if a != b {
                    let (keep, go) = if a < b { (a, b) } else { (b, a) };
                    up.insert(go, keep);
                }
            }
        }
        let mut index: BTreeMap<FaceId, usize> = BTreeMap::new();
        let mut members: Vec<BTreeSet<FaceId>> = Vec::new();
        let mut of = BTreeMap::new();
        for &f in &region.changed {
            let r = root(&up, f);
            let k = *index.entry(r).or_insert_with(|| {
                members.push(BTreeSet::new());
                members.len() - 1
            });
            members[k].insert(f);
            of.insert(f, k);
        }
        let boxes = members
            .iter()
            .map(|m| {
                m.iter()
                    .map(|&f| face_box(body, f))
                    .reduce(|a, b| a.union(&b))
                    .expect("a gap holds a face")
            })
            .collect();
        let reach = members
            .iter()
            .map(|m| super::reach_of(body, changes, &|f| m.contains(&f)))
            .collect();
        Gaps { of, boxes, reach }
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use super::super::region::region;
    use super::super::tests::block;
    use super::*;
    use crate::blend::{BlendKind, EdgeBlend, blend};
    use crate::geom::Surface;
    use crate::name::OpKey;

    /// The test block with every edge rounded 0.05, and its two corner
    /// balls nearest the origin and farthest from it.
    pub(crate) fn rounded() -> (Body, FaceId, FaceId) {
        let b = block();
        let edges: Vec<EdgeBlend> = b
            .edges()
            .map(|e| EdgeBlend {
                edge: b.edges[e].name,
                kind: BlendKind::Fillet { radius: 0.05 },
            })
            .collect();
        let (r, _) = blend(OpKey(0x20b0), &b, &edges).unwrap();
        let balls: Vec<FaceId> = r
            .faces()
            .filter(|&f| matches!(r.surfaces[r.faces[f].surface], Surface::Sphere(_)))
            .collect();
        assert_eq!(balls.len(), 8);
        let key = |f: &FaceId| {
            let m = crate::intersect::face_box(&r, *f).min();
            m.x + m.y + m.z
        };
        let low = *balls
            .iter()
            .min_by(|a, b| key(a).total_cmp(&key(b)))
            .unwrap();
        let high = *balls
            .iter()
            .max_by(|a, b| key(a).total_cmp(&key(b)))
            .unwrap();
        (r, low, high)
    }

    #[test]
    fn far_corners_are_two_gaps_each_with_its_own_reach() {
        let (r, low, high) = rounded();
        let changes = Changes {
            deleted: BTreeSet::from([low, high]),
            ..Default::default()
        };
        let region = region(&r, &BTreeSet::from([low, high]));
        let gaps = Gaps::of(&r, &changes, &region);
        assert_eq!(gaps.boxes.len(), 2);
        assert_ne!(gaps.of[&low], gaps.of[&high]);
        for f in [low, high] {
            let k = gaps.of[&f];
            let own = crate::intersect::face_box(&r, f);
            assert_eq!(gaps.boxes[k], own);
            // A deleted ball's gap is as wide as the ball's box.
            assert_eq!(
                gaps.reach[k].to_bits(),
                own.min().distance(own.max()).to_bits()
            );
            // The body's reach spans both corners: far more.
            assert!(super::super::reach(&r, &changes) > 5.0 * gaps.reach[k]);
        }
    }

    #[test]
    fn faces_sharing_a_vertex_are_one_gap() {
        let (r, low, _) = rounded();
        // The ball and an edge round beside it.
        let side = r
            .face_loops(low)
            .flat_map(|l| r.loop_fins(l))
            .flat_map(|fin| r.edge_fins(r.fins[fin].edge))
            .map(|x| r.loops[r.fins[x].loop_].face)
            .find(|&g| g != low)
            .unwrap();
        let changes = Changes {
            deleted: BTreeSet::from([low, side]),
            ..Default::default()
        };
        let region = region(&r, &BTreeSet::from([low, side]));
        let gaps = Gaps::of(&r, &changes, &region);
        assert_eq!(gaps.boxes.len(), 1);
        // One gap: the reach is the body's, bit for bit.
        assert_eq!(
            gaps.reach[0].to_bits(),
            super::super::reach(&r, &changes).to_bits()
        );
    }
}
