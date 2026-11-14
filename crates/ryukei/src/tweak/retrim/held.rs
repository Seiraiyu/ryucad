//! Held seams (design 642, #652): input edges along which two faces that
//! both stay where they were are tangent, all along or in places, with at
//! least one of them a working face. The table meets each such pair
//! along the edge (`intersect::known_seam`), and where one still refuses
//! the re-trim builds the rung again with stops at the seams whose ends
//! can't move (`iso_stops`).

use super::region::Region;
use crate::geom::{Curve2, Surface};
use crate::intersect::SeamKind;
use crate::topo::{Body, EdgeId, FaceId, FinId};
use crate::{Error, TweakCase, TweakError};
use std::collections::BTreeMap;

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

/// The direction a pcurve holds fixed and the value it holds it at
/// (design 642): `(0, u)` for a pcurve along `v` at `u`, `(1, v)` for one
/// along `u` at `v`. A `Line2` whose direction has that component zero,
/// or a `Nurbs2` whose control points all have that coordinate bit for
/// bit (so it lies on the iso-line, inside their hull; STEP files often
/// write iso pcurves as B-splines). `None` for anything else.
pub(crate) fn iso(c: &Curve2) -> Option<(usize, f64)> {
    match c {
        Curve2::Line2(l) => {
            let (o, d) = (l.origin(), l.dir());
            if d.x == 0.0 {
                Some((0, o.x))
            } else if d.y == 0.0 {
                Some((1, o.y))
            } else {
                None
            }
        }
        Curve2::Nurbs2(n) => {
            let mut pts = n.control_points();
            let first = pts.next()?;
            let (mut su, mut sv) = (true, true);
            for p in pts {
                su &= p.x.to_bits() == first.x.to_bits();
                sv &= p.y.to_bits() == first.y.to_bits();
            }
            if su {
                Some((0, first.x))
            } else if sv {
                Some((1, first.y))
            } else {
                None
            }
        }
        Curve2::Circle2(_) | Curve2::Ellipse2(_) => None,
    }
}

/// The margin a stop leaves past held seam `h` (design 642, Decision 5):
/// half of `known_seam`'s strip ([`crate::intersect::seam_strip`]) from
/// the two input faces' box and the edge's tolerance, no more than the
/// working faces' box and the tolerance `known_seam` takes, so the sliver
/// between the seam and the stop lies within its strip.
pub(crate) fn margin(body: &Body, h: &Held) -> f64 {
    let bx = crate::intersect::face_box(body, h.faces[0])
        .union(&crate::intersect::face_box(body, h.faces[1]));
    let size = (bx.max() - bx.min()).length();
    let tol = body.edges[h.edge]
        .tolerance
        .get()
        .max(crate::tol::LINEAR_RESOLUTION);
    0.5 * crate::intersect::seam_strip(size, tol)
}

/// Working face `f`'s parameter box `rect` on `surface` with its iso stops
/// (design 642). For each of `held` with fixed ends (`Frontier` or
/// `Fixed`) on `f` whose pcurve on `f` is iso ([`iso`]) at an end of
/// `f`'s own parameter box (its pcurves' span, `working::own_span`, within
/// the margin in parameters), where every fin of `f` whose pcurve runs
/// along that end (its span across lies within the margin of it) is such
/// a seam, the box ends the margin past the seam: `margin / L`, `L` the
/// certified bound on `|P_u|` or `|P_v|` over `rect`
/// ([`crate::intersect::first_bound`]), so the stop lies at most the
/// margin past it in model space. A box ending nearer keeps its end. A
/// periodic direction is stopped only where `rect` spans less than a
/// period (`delete_turn` narrowed it): a whole period has no end.
pub(crate) fn iso_stops(
    body: &Body,
    f: FaceId,
    held: &[Held],
    surface: &Surface,
    rect: [[f64; 2]; 2],
) -> [[f64; 2]; 2] {
    let stopping: BTreeMap<EdgeId, &Held> = held
        .iter()
        .filter(|h| h.kind != SeamKind::Running && h.faces.contains(&f))
        .map(|h| (h.edge, h))
        .collect();
    if stopping.is_empty() {
        return rect;
    }
    let (pu, pv) = surface.periods();
    let periods = [pu, pv];
    let (lu, lv) = crate::intersect::first_bound(surface, rect);
    let bound = [lu, lv];
    // `f`'s fins: each one's edge and fin.
    let fins: Vec<(EdgeId, FinId)> = body
        .face_loops(f)
        .flat_map(|l| body.loop_fins(l))
        .map(|fin| (body.fins[fin].edge, fin))
        .collect();
    let mut out = rect;
    for (&e, h) in &stopping {
        let Some(&(_, fin)) = fins.iter().find(|(x, _)| *x == e) else {
            continue;
        };
        let Some(pc) = body.fins[fin].pcurve else {
            continue;
        };
        let Some((k, c)) = iso(&body.curves2[pc.curve]) else {
            continue;
        };
        if periods[k].is_some_and(|p| rect[k][1] - rect[k][0] >= p) {
            continue;
        }
        if !(bound[k].is_finite() && bound[k] > 0.0) {
            continue;
        }
        let Some([lo, hi]) = super::working::own_span(body, f, k) else {
            continue;
        };
        let d = margin(body, h) / bound[k];
        let (high, low) = ((c - hi).abs() <= d, (c - lo).abs() <= d);
        // Neither end, or a box too thin to tell which.
        if high == low {
            continue;
        }
        // Every fin running along that end is a seam that stops.
        let whole = fins
            .iter()
            .all(|&(x, g)| match super::working::fin_span(body, g, k) {
                Some([a, b]) if (a - c).abs() <= d && (b - c).abs() <= d => {
                    stopping.contains_key(&x)
                }
                Some(_) => true,
                None => false,
            });
        if !whole {
            continue;
        }
        if high {
            out[k][1] = out[k][1].min(c + d);
        } else {
            out[k][0] = out[k][0].max(c - d);
        }
    }
    if (0..2).all(|k| out[k][0] < out[k][1]) {
        out
    } else {
        rect
    }
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
    #[test]
    fn iso_pcurves_are_lines_and_splines_along_one_parameter() {
        use crate::geom::{Curve2, KnotVector, Line2, NurbsCurve2, Point2, Vector2};
        let line = |o: (f64, f64), d: (f64, f64)| {
            Curve2::Line2(Line2::new(Point2::new(o.0, o.1), Vector2::new(d.0, d.1)).unwrap())
        };
        assert_eq!(iso(&line((0.25, 1.0), (0.0, -2.0))), Some((0, 0.25)));
        assert_eq!(iso(&line((0.25, 1.0), (3.0, 0.0))), Some((1, 1.0)));
        assert_eq!(iso(&line((0.25, 1.0), (1.0, 1.0))), None);
        let spline = |pts: Vec<Point2>| {
            let k = KnotVector::clamped(2, &[0.0, 1.0], &[3, 3]).unwrap();
            Curve2::Nurbs2(std::sync::Arc::new(NurbsCurve2::new(k, pts, None).unwrap()))
        };
        let v = 0.7;
        let on = spline(vec![
            Point2::new(0.0, v),
            Point2::new(0.4, v),
            Point2::new(1.0, v),
        ]);
        assert_eq!(iso(&on), Some((1, v)));
        // One control point an ulp off: not on the iso-line.
        let off = spline(vec![
            Point2::new(0.0, v),
            Point2::new(0.4, f64::from_bits(v.to_bits() + 1)),
            Point2::new(1.0, v),
        ]);
        assert_eq!(iso(&off), None);
    }
    /// The rod's run as its working face, and its held seam with the
    /// bend: as a rung's first build sees them.
    fn run_working() -> (
        Body,
        FaceId,
        Vec<Held>,
        crate::tweak::retrim::working::Working,
    ) {
        use crate::tweak::engine::Changes;
        use crate::tweak::retrim::body::region_box;
        use crate::tweak::retrim::working::{delete_turn, working};
        let (rod, rim) = crate::testing::bent_rod();
        let r = region(&rod, &BTreeSet::from([rim]));
        let (run, _) = runs(&rod, rim);
        let h = held(&rod, &r, &|f| r.faces.contains(&f) && f != rim);
        let changes = Changes {
            deleted: BTreeSet::from([rim]),
            ..Changes::default()
        };
        let reach = {
            let g = crate::intersect::face_box(&rod, rim);
            g.min().distance(g.max())
        };
        let bx = region_box(&rod, &r, reach);
        let mut w = working(&rod, run, None, rod.faces[run].sense, true, &bx).unwrap();
        for (k, t) in delete_turn(&rod, &changes, run, &w.surface)
            .into_iter()
            .enumerate()
        {
            if let Some(t) = t {
                w.rect[k] = t;
            }
        }
        (rod, run, h, w)
    }

    #[test]
    fn an_iso_stop_ends_the_run_half_a_strip_past_its_seam() {
        let (rod, run, h, w) = run_working();
        let stopped = iso_stops(&rod, run, &h, &w.surface, w.rect);
        // One end moved: the run's end at the bend, along its axis.
        let moved: Vec<(usize, usize)> = (0..2)
            .flat_map(|k| (0..2).map(move |j| (k, j)))
            .filter(|&(k, j)| stopped[k][j] != w.rect[k][j])
            .collect();
        assert_eq!(moved.len(), 1, "{stopped:?} {:?}", w.rect);
        let (k, j) = moved[0];
        // The seam's iso value on the run.
        let fin = rod
            .edge_fins(h[0].edge)
            .find(|&x| rod.loops[rod.fins[x].loop_].face == run)
            .unwrap();
        let pc = rod.fins[fin].pcurve.unwrap();
        let (dir, c) = iso(&rod.curves2[pc.curve]).expect("iso on the run");
        assert_eq!(dir, k);
        // In model space, past the seam by no more than the margin, and
        // by more than half of it (the run's |P_v| is 1).
        let margin = margin(&rod, &h[0]);
        let u = 0.5 * (stopped[1 - k][0] + stopped[1 - k][1]);
        let at = |t: f64| {
            if k == 0 {
                w.surface.point(t, u)
            } else {
                w.surface.point(u, t)
            }
        };
        let d = at(c).distance(at(stopped[k][j]));
        assert!(d > 0.5 * margin && d <= margin, "{d} {margin}");
        // And short of where the unstopped run reached.
        assert!(at(c).distance(at(w.rect[k][j])) > d);
    }

    #[test]
    fn a_seam_whose_ends_may_move_stops_nothing() {
        let (rod, run, mut h, w) = run_working();
        h[0].kind = SeamKind::Running;
        assert_eq!(iso_stops(&rod, run, &h, &w.surface, w.rect), w.rect);
        // No held seam on that end: nothing either.
        assert_eq!(iso_stops(&rod, run, &[], &w.surface, w.rect), w.rect);
    }
}
