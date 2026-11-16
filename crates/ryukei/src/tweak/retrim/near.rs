//! Near boxes (design 669, plan 20.B): the part of a rung's box each
//! working face can need. A working face is carried over its own box and
//! the boxes of the changed faces it touches (shares a vertex with, or
//! is), grown by those faces' reach alone (the same rule as `region_box`,
//! for one face's neighbourhood). Applied in narrowed builds only, so
//! first builds keep their bits.
//!
//! Plan 20.B's Decision 1 grew a near box by the reach of the gaps it
//! touched (changed faces joined where they share a vertex). But
//! `delete_faces` grows each edge-connected set of deleted faces in its
//! own re-trim, so a delete's changed faces are always one gap and that
//! reach is the body's: the near box was the rung's box. The coxswain
//! chose the touched faces' reach instead (2026-11-15).

use super::region::{Region, face_vertices};
use crate::geom::Box3;
use crate::intersect::face_box;
use crate::topo::{Body, FaceId};
use crate::tweak::engine::Changes;
use std::collections::BTreeSet;

/// The changed faces of `region` that working face `f` touches: `f`
/// itself, and those sharing a vertex with it.
pub(crate) fn touched(body: &Body, region: &Region, f: FaceId) -> BTreeSet<FaceId> {
    let mut out = BTreeSet::new();
    if region.changed.contains(&f) {
        out.insert(f);
    }
    for v in face_vertices(body, f) {
        for e in body.vertex_edges(v) {
            for fin in body.edge_fins(e) {
                let g = body.loops[body.fins[fin].loop_].face;
                if region.changed.contains(&g) {
                    out.insert(g);
                }
            }
        }
    }
    out
}

/// Working face `f`'s near box at `rung` (design 669, plan 20.B): `f`'s
/// box joined with the boxes of the changed faces it touches
/// ([`touched`]), grown by their reach (`reach_of` over them alone;
/// `reach`'s floor, ten resolutions, where it touches none), doubled
/// about its middle at rung 1 as the rung's box is, and kept inside the
/// rung's box `x` (already doubled at rung 1); `x` itself where they
/// don't meet.
pub(crate) fn near_box(
    body: &Body,
    changes: &Changes,
    region: &Region,
    f: FaceId,
    x: &Box3,
    rung: usize,
) -> Box3 {
    let near = touched(body, region, f);
    let b = near
        .iter()
        .map(|&g| face_box(body, g))
        .fold(face_box(body, f), |a, b| a.union(&b));
    let r = super::reach_of(body, changes, &|g| near.contains(&g));
    let mut n = b.grow(r);
    if rung == 1 {
        n = super::body::doubled(&n);
    }
    within(&n, x).unwrap_or(*x)
}

/// Where boxes `a` and `x` overlap, or `None`.
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
    fn reach_of_every_face_is_the_bodys_reach() {
        let (r, low, high) = rounded();
        let changes = Changes {
            deleted: BTreeSet::from([low, high]),
            ..Default::default()
        };
        assert_eq!(
            super::super::reach_of(&r, &changes, &|_| true).to_bits(),
            super::super::reach(&r, &changes).to_bits()
        );
        // One ball alone: as wide as its box.
        let own = crate::intersect::face_box(&r, low);
        assert_eq!(
            super::super::reach_of(&r, &changes, &|f| f == low).to_bits(),
            own.min().distance(own.max()).to_bits()
        );
    }

    #[test]
    fn a_face_at_one_end_of_a_long_set_is_carried_past_that_end_only() {
        // One connected set spanning the block, as `delete_faces` re-trims
        // it: every blend face off the x = 0 end, and the low corner's
        // ball. The x = 0 side touches only that ball.
        let (r, low, _) = rounded();
        let blendy = |f: FaceId| !matches!(r.surfaces[r.faces[f].surface], Surface::Plane(_));
        let deleted: BTreeSet<FaceId> = r
            .faces()
            .filter(|&f| {
                blendy(f) && (f == low || crate::intersect::face_box(&r, f).min().x > 0.04)
            })
            .collect();
        let changes = Changes {
            deleted: deleted.clone(),
            ..Default::default()
        };
        let region = region(&r, &deleted);
        let end = r
            .faces()
            .find(|&f| !blendy(f) && crate::intersect::face_box(&r, f).max().x < 1e-9)
            .unwrap();
        assert!(region.faces.contains(&end));
        assert_eq!(touched(&r, &region, end), BTreeSet::from([low]));
        let x = rung_box(&r, &changes, &region);
        assert!(x.max().x > 1.5, "{x:?}");
        let n = near_box(&r, &changes, &region, end, &x, 0);
        // The ball's box (its sphere's, 0.1 deep) grown by its diagonal:
        // about 0.27, never the rung's 1.5.
        assert!(n.max().x < 0.3, "{n:?}");
        assert!(holds(&n, &crate::intersect::face_box(&r, end)));
        assert!(holds(&n, &crate::intersect::face_box(&r, low)));
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
        let x = rung_box(&r, &changes, &region);
        let ball = crate::intersect::face_box(&r, low);
        let far = crate::intersect::face_box(&r, high);
        let mut beside = 0;
        for &f in region.faces.iter().filter(|f| !changes.deleted.contains(f)) {
            let n = near_box(&r, &changes, &region, f, &x, 0);
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
        let n = near_box(&r, &changes, &region, lone, &x, 2);
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
        let x = super::super::body::doubled(&rung_box(&r, &changes, &region));
        let f = region
            .faces
            .iter()
            .copied()
            .find(|f| !changes.deleted.contains(f))
            .unwrap();
        let (n0, n1) = (
            near_box(&r, &changes, &region, f, &x, 0),
            near_box(&r, &changes, &region, f, &x, 1),
        );
        assert!(holds(&n1, &n0));
        assert!(holds(&x, &n1));
    }
}
