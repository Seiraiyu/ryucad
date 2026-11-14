//! Held seams (design 642, #652): input edges along which two faces that
//! both stay where they were are tangent, all along or in places, with at
//! least one of them a working face. The table meets each such pair
//! along the edge (`intersect::known_seam`), and where one still refuses
//! the re-trim builds the rung again with stops at the seams whose ends
//! can't move (`iso_stops`).

use super::region::Region;
use crate::intersect::SeamKind;
use crate::topo::{Body, EdgeId, FaceId, FinId};
use crate::{Error, TweakCase, TweakError};

/// A held seam: an input edge, the faces of its two fins in that order,
/// and whether its ends can move.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct Held {
    pub(crate) edge: EdgeId,
    pub(crate) faces: [FaceId; 2],
    pub(crate) kind: SeamKind,
}

/// Whether the faces of edge `e` are tangent along it all along or in
/// places: `classify::tangent` is `Ok(true)` or refuses `PartlySmooth`
/// (design 642, Decision 1). A transversal edge (`Ok(false)`) is met by a
/// trace; an uncertain bend isn't held.
pub(crate) fn held_tangent(body: &Body, e: EdgeId) -> bool {
    matches!(
        crate::tweak::classify::tangent(body, e),
        Ok(true)
            | Err(Error::InvalidTweak(TweakError::Unsupported {
                case: TweakCase::PartlySmooth,
                ..
            }))
    )
}

/// The held seams of a re-trim of `body` over `region`, in edge order:
/// every edge with two fins, both with pcurves, on two faces that both
/// stay (outside the region, or `still`: in it and left as they are),
/// at least one a region face (a working face), tangent along it all
/// along or in places ([`held_tangent`]). Each is `Frontier` where one
/// face is outside, `Fixed` where every face round both its ends stays,
/// else `Running`.
pub(crate) fn held(body: &Body, region: &Region, still: &dyn Fn(FaceId) -> bool) -> Vec<Held> {
    let stays = |f: FaceId| !region.faces.contains(&f) || still(f);
    let face = |fin: FinId| body.loops[body.fins[fin].loop_].face;
    let mut out = Vec::new();
    for e in body.edges() {
        let fins: Vec<FinId> = body.edge_fins(e).collect();
        let [fa, fb] = fins[..] else { continue };
        if body.fins[fa].pcurve.is_none() || body.fins[fb].pcurve.is_none() {
            continue;
        }
        let (a, b) = (face(fa), face(fb));
        if a == b || !stays(a) || !stays(b) {
            continue;
        }
        let (wa, wb) = (region.faces.contains(&a), region.faces.contains(&b));
        if !wa && !wb {
            continue;
        }
        if !held_tangent(body, e) {
            continue;
        }
        let kind = if !(wa && wb) {
            SeamKind::Frontier
        } else if ends_stay(body, e, &stays) {
            SeamKind::Fixed
        } else {
            SeamKind::Running
        };
        out.push(Held {
            edge: e,
            faces: [a, b],
            kind,
        });
    }
    out
}

/// Whether every face round each end of edge `e` stays (a closed edge's
/// one vertex is its only end).
fn ends_stay(body: &Body, e: EdgeId, stays: &dyn Fn(FaceId) -> bool) -> bool {
    let x = &body.edges[e];
    [x.start, x.end].into_iter().all(|v| {
        body.vertex_edges(v).into_iter().all(|g| {
            body.edge_fins(g)
                .all(|fin| stays(body.loops[body.fins[fin].loop_].face))
        })
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::geom::Surface;
    use crate::tweak::retrim::region::region;
    use std::collections::BTreeSet;

    fn is(b: &Body, f: FaceId, pick: fn(&Surface) -> bool) -> bool {
        pick(&b.surfaces[b.faces[f].surface])
    }

    /// The rod's two cylinders: the run next to the rim first.
    fn runs(rod: &Body, rim: FaceId) -> (FaceId, FaceId) {
        let next_to = |f: FaceId, g: FaceId| {
            rod.face_loops(f).flat_map(|l| rod.loop_fins(l)).any(|fin| {
                rod.edge_fins(rod.fins[fin].edge)
                    .any(|x| rod.loops[rod.fins[x].loop_].face == g)
            })
        };
        let cyls: Vec<FaceId> = rod
            .faces()
            .filter(|&f| is(rod, f, |s| matches!(s, Surface::Cylinder(_))))
            .collect();
        let run = *cyls.iter().find(|&&c| next_to(c, rim)).unwrap();
        let other = *cyls.iter().find(|&&c| c != run).unwrap();
        (run, other)
    }

    fn bend(rod: &Body, rim: FaceId) -> FaceId {
        rod.faces()
            .find(|&f| f != rim && is(rod, f, |s| matches!(s, Surface::Torus(_))))
            .unwrap()
    }

    #[test]
    fn a_straight_run_holds_a_frontier_seam_at_its_bend() {
        let (rod, rim) = crate::testing::bent_rod();
        let r = region(&rod, &BTreeSet::from([rim]));
        let (run, _) = runs(&rod, rim);
        let t = bend(&rod, rim);
        assert!(r.faces.contains(&run) && !r.faces.contains(&t));
        let still = |f: FaceId| r.faces.contains(&f) && f != rim;
        let h = held(&rod, &r, &still);
        // The run against the bend, which stays outside: a frontier
        // seam. Nothing else: the cap meets only the deleted rim.
        assert_eq!(h.len(), 1, "{h:?}");
        assert_eq!(h[0].kind, SeamKind::Frontier);
        let mut faces = h[0].faces;
        faces.sort();
        let mut want = [run, t];
        want.sort();
        assert_eq!(faces, want);
    }

    #[test]
    fn a_seam_between_two_working_faces_is_fixed_where_its_ends_stay() {
        // The region grown once takes the bend in: the run against the
        // bend is fixed (a closed circle, every face round it stays), the
        // bend against the far run a frontier seam.
        let (rod, rim) = crate::testing::bent_rod();
        let r = region(&rod, &BTreeSet::from([rim])).grown(&rod);
        let (run, far) = runs(&rod, rim);
        let t = bend(&rod, rim);
        assert!(r.faces.contains(&t) && !r.faces.contains(&far));
        let still = |f: FaceId| r.faces.contains(&f) && f != rim;
        let h = held(&rod, &r, &still);
        let kind = |x: FaceId, y: FaceId| {
            h.iter()
                .find(|s| s.faces.contains(&x) && s.faces.contains(&y))
                .map(|s| s.kind)
        };
        assert_eq!(kind(run, t), Some(SeamKind::Fixed), "{h:?}");
        assert_eq!(kind(t, far), Some(SeamKind::Frontier), "{h:?}");
    }

    #[test]
    fn a_round_beside_a_changed_top_runs_and_transversal_edges_hold_nothing() {
        // 18.C's rounded block, its top changed: the round is tangent to
        // the two sides it joins, each seam ending at the top, which
        // changes, so running; the sides' corners and the bottom's
        // edges are transversal and hold nothing.
        use crate::blend::{BlendKind, EdgeBlend, blend};
        use crate::name::FaceRole;
        use crate::tweak::retrim::tests::{K, block, face_of};
        let b0 = block();
        let (px, py) = (face_of(&b0, FaceRole::PlusX), face_of(&b0, FaceRole::PlusY));
        let edge = b0
            .edges()
            .find(|&e| {
                let fs: Vec<FaceId> = b0
                    .edge_fins(e)
                    .map(|fin| b0.loops[b0.fins[fin].loop_].face)
                    .collect();
                fs.contains(&px) && fs.contains(&py)
            })
            .unwrap();
        let (b, _) = blend(
            K,
            &b0,
            &[EdgeBlend {
                edge: b0.edges[edge].name,
                kind: BlendKind::Fillet { radius: 0.1 },
            }],
        )
        .unwrap();
        let top = face_of(&b, FaceRole::PlusZ);
        let r = region(&b, &BTreeSet::from([top]));
        let h = held(&b, &r, &|f| r.faces.contains(&f) && f != top);
        assert_eq!(h.len(), 2, "{h:?}");
        assert!(h.iter().all(|s| s.kind == SeamKind::Running), "{h:?}");
        // Nothing stays but the bottom: no held seam.
        assert!(held(&b, &r, &|_| false).is_empty());
    }
}
