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

/// Working face `f`'s near box at `rung` (design 669, plan 20.B): `f`'s
/// box joined with the boxes of the gaps it touches (one holding a face
/// that shares a vertex with `f`, or `f` itself), grown by the largest of
/// their reaches (`reach`'s floor, ten resolutions, where it touches
/// none), doubled about its middle at rung 1 as the rung's box is, and
/// kept inside the rung's box `x` (already doubled at rung 1); `x` itself
/// where they don't meet.
#[cfg_attr(not(test), allow(dead_code))]
pub(crate) fn near_box(body: &Body, gaps: &Gaps, f: FaceId, x: &Box3, rung: usize) -> Box3 {
    let mut touched = BTreeSet::new();
    if let Some(&k) = gaps.of.get(&f) {
        touched.insert(k);
    }
    for v in face_vertices(body, f) {
        for e in body.vertex_edges(v) {
            for fin in body.edge_fins(e) {
                let g = body.loops[body.fins[fin].loop_].face;
                if let Some(&k) = gaps.of.get(&g) {
                    touched.insert(k);
                }
            }
        }
    }
    let mut b = face_box(body, f);
    let mut r: f64 = 10.0 * crate::tol::LINEAR_RESOLUTION;
    for k in touched {
        b = b.union(&gaps.boxes[k]);
        r = r.max(gaps.reach[k]);
    }
    let mut n = b.grow(r);
    if rung == 1 {
        n = super::body::doubled(&n);
    }
    within(&n, x).unwrap_or(*x)
}

/// Where boxes `a` and `x` overlap, or `None`.
#[cfg_attr(not(test), allow(dead_code))]
fn within(a: &Box3, x: &Box3) -> Option<Box3> {
    use crate::geom::Point3;
    let lo = Point3::new(
        a.min().x.max(x.min().x),
        a.min().y.max(x.min().y),
        a.min().z.max(x.min().z),
    );
    let hi = Point3::new(
        a.max().x.min(x.max().x),
        a.max().y.min(x.max().y),
        a.max().z.min(x.max().z),
    );
    (lo.x <= hi.x && lo.y <= hi.y && lo.z <= hi.z).then(|| Box3::new(lo, hi))
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

    /// The region's box for `changes`, as `retrim_with` makes it at rung 0.
    fn rung_box(r: &Body, changes: &Changes, region: &Region) -> Box3 {
        super::super::body::region_box(r, region, super::super::reach(r, changes))
    }

    fn holds(outer: &Box3, inner: &Box3) -> bool {
        let (a, b) = (outer.min(), outer.max());
        let (c, d) = (inner.min(), inner.max());
        a.x <= c.x && a.y <= c.y && a.z <= c.z && d.x <= b.x && d.y <= b.y && d.z <= b.z
    }

    #[test]
    fn a_face_beside_a_deleted_corner_is_carried_past_it_not_across_the_body() {
        let (r, low, high) = rounded();
        let changes = Changes {
            deleted: BTreeSet::from([low, high]),
            ..Default::default()
        };
        let region = region(&r, &BTreeSet::from([low, high]));
        let gaps = Gaps::of(&r, &changes, &region);
        let x = rung_box(&r, &changes, &region);
        let ball = crate::intersect::face_box(&r, low);
        let far = crate::intersect::face_box(&r, high);
        let mut beside = 0;
        for &f in region.faces.iter().filter(|f| !changes.deleted.contains(f)) {
            let n = near_box(&r, &gaps, f, &x, 0);
            // Inside the rung's box, holding the face's own box.
            assert!(holds(&x, &n), "{f:?}");
            assert!(holds(&n, &crate::intersect::face_box(&r, f)), "{f:?}");
            // A face round the low corner holds its ball, and stops well
            // short of the far corner's.
            if crate::intersect::face_box(&r, f).intersects(&ball.grow(1e-9)) {
                beside += 1;
                assert!(holds(&n, &ball), "{f:?}");
                assert!(!n.intersects(&far), "{f:?}: {n:?}");
            }
        }
        assert!(beside >= 3, "{beside}");
    }

    #[test]
    fn a_face_touching_no_gap_keeps_its_own_box() {
        let (r, low, _) = rounded();
        let changes = Changes {
            deleted: BTreeSet::from([low]),
            ..Default::default()
        };
        // Rung 2's region: grown by a ring.
        let region = region(&r, &BTreeSet::from([low])).grown(&r);
        let gaps = Gaps::of(&r, &changes, &region);
        let x = rung_box(&r, &changes, &region);
        let lone = region
            .faces
            .iter()
            .copied()
            .find(|&f| {
                face_vertices(&r, f)
                    .iter()
                    .all(|&v| !face_vertices(&r, low).contains(&v))
            })
            .expect("a face of the ring touching no gap");
        let n = near_box(&r, &gaps, lone, &x, 2);
        let own = crate::intersect::face_box(&r, lone);
        assert_eq!(n, own.grow(10.0 * crate::tol::LINEAR_RESOLUTION));
    }

    #[test]
    fn rung_one_doubles_a_near_box_inside_its_doubled_box() {
        let (r, low, high) = rounded();
        let changes = Changes {
            deleted: BTreeSet::from([low, high]),
            ..Default::default()
        };
        let region = region(&r, &BTreeSet::from([low, high]));
        let gaps = Gaps::of(&r, &changes, &region);
        let x = super::super::body::doubled(&rung_box(&r, &changes, &region));
        let f = region
            .faces
            .iter()
            .copied()
            .find(|f| !changes.deleted.contains(f))
            .unwrap();
        let (n0, n1) = (near_box(&r, &gaps, f, &x, 0), near_box(&r, &gaps, f, &x, 1));
        assert!(holds(&n1, &n0));
        assert!(holds(&x, &n1));
    }
}
