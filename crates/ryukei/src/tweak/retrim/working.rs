//! Working faces' surfaces: each region face's surface (its new one, or
//! its own) carried past its domain where a bounded surface needs to
//! reach the region box, and the parameter box covering the box.

use crate::geom::{Box3, Surface};
use crate::intersect::{carried_by, face_rect};
use crate::topo::{Body, FaceId, Sense};
use crate::tweak::engine::Changes;
use crate::{Error, TweakError};

/// One working face's surface, parameter box and sense.
#[derive(Clone, Debug)]
pub(crate) struct Working {
    pub(crate) surface: Surface,
    pub(crate) rect: [[f64; 2]; 2],
    pub(crate) sense: Sense,
    /// Whether the surface could be carried where the region needs it: a
    /// piece reaching the side of a face that couldn't is
    /// `ExtensionUnsupported`, not `RegionTooSmall`.
    pub(crate) extendable: bool,
    /// The working face's sheet where a plane stop cut it (plan 19.B):
    /// one face, its every edge a side. `None`: the rect's sheet.
    pub(crate) cut: Option<Body>,
}

/// Whether `s` is a bounded surface `carried_by` takes.
fn carriable(s: &Surface) -> bool {
    matches!(
        s,
        Surface::Nurbs(_) | Surface::Extrusion(_) | Surface::Revolution(_)
    )
}

/// Face `f`'s working surface: `new` where the tweak gives one, else its
/// own, under `sense`. `same_params` says the surface keeps the old one's
/// parameters (no new surface, a rigid motion or an offset), so the face's
/// own parameter box can be read from its pcurves. The parameter box holds
/// that box and the feet of `region`'s corners, grown by a sixteenth each
/// way, within the domain; a periodic direction is a whole period. A
/// bounded surface is first carried past each end of its domain its face
/// comes near, by three times the region's diagonal.
pub(crate) fn working(
    body: &Body,
    f: FaceId,
    new: Option<&Surface>,
    sense: Sense,
    same_params: bool,
    region: &Box3,
) -> Result<Working, Error> {
    let name = body.faces[f].name;
    let mut surface = new
        .cloned()
        .unwrap_or_else(|| body.surfaces[body.faces[f].surface].clone());
    let base = if same_params {
        face_rect(body, f)
    } else {
        None
    };
    let diagonal = region.min().distance(region.max());
    let mut extendable = true;
    if carriable(&surface) {
        let domain = surface.domain();
        let (pu, pv) = surface.periods();
        let rect0 = base.unwrap_or(domain);
        let mid = surface.eval(
            (rect0[0][0] + rect0[0][1]) / 2.0,
            (rect0[1][0] + rect0[1][1]) / 2.0,
        );
        let speed = [mid.du.length(), mid.dv.length()];
        let mut m = crate::tweak::extend::margins(rect0, domain, [pu, pv], speed, diagonal);
        // No farther than the domain's own length each way: a polynomial
        // carried tens of its lengths on is no surface the region needs
        // (NIST FTC 07: v carried from a few units to [-37, 20], its
        // working sheet's sides fitted for 63 s and refused).
        for (mk, dk) in m.iter_mut().zip(domain) {
            let len = dk[1] - dk[0];
            for e in mk.iter_mut() {
                *e = e.min(len);
            }
        }
        if m != [[0.0; 2]; 2] {
            match carried_by(&surface, m) {
                Some(c) => surface = c,
                None => extendable = false,
            }
        }
    } else if !matches!(
        surface,
        Surface::Plane(_)
            | Surface::Cylinder(_)
            | Surface::Cone(_)
            | Surface::Sphere(_)
            | Surface::Torus(_)
    ) {
        // Pipes, sweeps, offsets and bent surfaces aren't carried.
        extendable = false;
    }
    let domain = surface.domain();
    let (pu, pv) = surface.periods();
    let periods = [pu, pv];
    let mut want = base.unwrap_or([[f64::INFINITY, f64::NEG_INFINITY]; 2]);
    for c in region.corners() {
        let (u, v) = surface.project(c).param;
        want[0] = [want[0][0].min(u), want[0][1].max(u)];
        want[1] = [want[1][0].min(v), want[1][1].max(v)];
    }
    let mut rect = [[0.0; 2]; 2];
    for k in 0..2 {
        if let Some(p) = periods[k] {
            rect[k] = [domain[k][0], domain[k][0] + p];
            continue;
        }
        let [a, b] = want[k];
        if !(a.is_finite() && b.is_finite() && a < b) {
            return Err(TweakError::ExtensionUnsupported { face: name }.into());
        }
        let g = (b - a) / 16.0;
        rect[k] = [(a - g).max(domain[k][0]), (b + g).min(domain[k][1])];
    }
    Ok(Working {
        surface,
        rect,
        sense,
        extendable,
        cut: None,
    })
}

/// In a delete (no face moved or given a new surface), the turn each
/// periodic direction of kept face `f`'s working face needs, where less
/// than a whole period: the span of its own pcurves, grown by a sixteenth
/// each way, when every edge it shares with a deleted face runs that
/// whole span (bounding it the other way, as a pocket floor's arc bounds
/// the corner cylinder above it), so it never has to grow round its axis
/// across one. Carried a whole turn, a pocket's quarter-turn corner
/// cylinders put their back three quarters across the pocket, and the
/// walk's choices among those pieces passed `CHOICES` (NIST FTC 08's
/// pocket floor, #653).
///
/// In a narrowed build (`circular`, design 669) the span is read round
/// the circle ([`own_hull`]), so a quarter face whose pcurves straddle
/// the period's seam is carried a quarter turn and a little more, not a
/// whole one; elsewhere it is [`own_span`], as before.
pub(crate) fn delete_turn(
    body: &Body,
    changes: &Changes,
    f: FaceId,
    surface: &Surface,
    circular: bool,
) -> [Option<[f64; 2]>; 2] {
    let (pu, pv) = surface.periods();
    let periods = [pu, pv];
    let mut out = [None; 2];
    if !changes.motion.is_empty() || !changes.surface.is_empty() || changes.deleted.contains(&f) {
        return out;
    }
    for k in 0..2 {
        let Some(p) = periods[k] else { continue };
        let span = if circular {
            own_hull(body, f, k, p)
        } else {
            own_span(body, f, k)
        };
        let Some([a, b]) = span else {
            continue;
        };
        let g = (b - a) / 16.0;
        if b - a + 2.0 * g >= p {
            continue;
        }
        let mut capped = true;
        for l in body.face_loops(f) {
            for fin in body.loop_fins(l) {
                let fi = &body.fins[fin];
                let by_deleted = body.edge_fins(fi.edge).any(|x| {
                    changes
                        .deleted
                        .contains(&body.loops[body.fins[x].loop_].face)
                });
                if !by_deleted {
                    continue;
                }
                let Some([c, d]) = fin_span(body, fin, k) else {
                    capped = false;
                    continue;
                };
                if d - c < b - a - g {
                    capped = false;
                }
            }
        }
        if capped {
            out[k] = Some([a - g, b + g]);
        }
    }
    out
}

/// The span of fin `fin`'s pcurve in direction `k`.
pub(crate) fn fin_span(body: &Body, fin: crate::topo::FinId, k: usize) -> Option<[f64; 2]> {
    let fi = &body.fins[fin];
    let pc = fi.pcurve?;
    let [s0, s1] = body.edges[fi.edge].interval.map(|t| pc.param(t));
    let b = body.curves2[pc.curve].bounds([s0.min(s1), s0.max(s1)]);
    Some(if k == 0 {
        [b.min().x, b.max().x]
    } else {
        [b.min().y, b.max().y]
    })
}

/// The span of face `f`'s pcurves in direction `k`; `None` for a face
/// with a loop that is a lone vertex, or a pcurve missing.
pub(crate) fn own_span(body: &Body, f: FaceId, k: usize) -> Option<[f64; 2]> {
    let mut t = [f64::INFINITY, f64::NEG_INFINITY];
    for l in body.face_loops(f) {
        if body.loops[l].vertex.is_some() {
            return None;
        }
        for fin in body.loop_fins(l) {
            let [lo, hi] = fin_span(body, fin, k)?;
            t = [t[0].min(lo), t[1].max(hi)];
        }
    }
    (t[0] < t[1]).then_some(t)
}

/// Face `f`'s own span in periodic direction `k`, of period `p`, read
/// round the circle (design 669): each fin's span moved by whole periods
/// to start within one period of the plain span's start (as `seam_for`
/// moves a pcurve into its face's box), then the largest gap between
/// them round the circle left out; the hull is the arc that remains, as
/// `[a, b]` with `b` past `a`. Where it is no shorter than [`own_span`]'s,
/// it is that span, bit for bit, so a face whose pcurves don't straddle
/// the period's seam reads as before. `None` where `own_span` is.
pub(crate) fn own_hull(body: &Body, f: FaceId, k: usize, p: f64) -> Option<[f64; 2]> {
    let plain = own_span(body, f, k)?;
    let base = plain[0];
    let mut arcs: Vec<[f64; 2]> = Vec::new();
    for l in body.face_loops(f) {
        for fin in body.loop_fins(l) {
            let [lo, hi] = fin_span(body, fin, k)?;
            let shift = -((lo - base) / p).floor() * p;
            arcs.push([lo + shift, hi + shift]);
        }
    }
    arcs.sort_by(|x, y| x[0].total_cmp(&y[0]).then(x[1].total_cmp(&y[1])));
    // The largest gap between the arcs, with the arc after it and how far
    // they reached before it; then the gap that wraps round past the last.
    let mut reach = arcs[0][1];
    let mut gap: Option<(f64, usize, f64)> = None;
    for (i, a) in arcs.iter().enumerate().skip(1) {
        if a[0] > reach && gap.is_none_or(|g| a[0] - reach > g.0) {
            gap = Some((a[0] - reach, i, reach));
        }
        reach = reach.max(a[1]);
    }
    let wrap = arcs[0][0] + p - reach;
    let hull = match gap {
        Some((g, i, end)) if g > wrap => [arcs[i][0], end + p],
        _ => [arcs[0][0], reach],
    };
    Some(if hull[1] - hull[0] < plain[1] - plain[0] {
        hull
    } else {
        plain
    })
}

/// Whether a face moved by `motion` keeps its surface's parameters.
pub(crate) fn same_params(motion: Option<&crate::tweak::Motion>) -> bool {
    matches!(
        motion,
        None | Some(crate::tweak::Motion::Rigid(_)) | Some(crate::tweak::Motion::Offset(_))
    )
}

#[cfg(test)]
pub(crate) mod tests {
    use super::super::tests::{K, block, face_of};
    use super::*;
    use crate::geom::Point3;
    use crate::name::FaceRole;

    fn region_box() -> Box3 {
        Box3::new(Point3::new(-0.5, -0.5, -0.5), Point3::new(1.5, 1.1, 0.9))
    }

    #[test]
    fn a_plane_covers_the_box_as_it_sees_it() {
        let b = block();
        let top = face_of(&b, FaceRole::PlusZ);
        let w = working(&b, top, None, b.faces[top].sense, true, &region_box()).unwrap();
        assert!(w.extendable);
        // Every corner of the box projects inside the rect.
        for c in region_box().corners() {
            let (u, v) = w.surface.project(c).param;
            assert!(u >= w.rect[0][0] && u <= w.rect[0][1], "{u} {:?}", w.rect);
            assert!(v >= w.rect[1][0] && v <= w.rect[1][1], "{v} {:?}", w.rect);
        }
    }

    #[test]
    fn a_cylinder_takes_its_whole_turn() {
        let (c, _) = crate::primitive::cylinder(
            super::super::tests::K,
            0.1,
            0.3,
            &crate::geom::Rigid::IDENTITY,
        )
        .unwrap();
        let side = c
            .faces()
            .find(|&f| matches!(c.surfaces[c.faces[f].surface], Surface::Cylinder(_)))
            .unwrap();
        let w = working(&c, side, None, c.faces[side].sense, true, &region_box()).unwrap();
        let p = core::f64::consts::TAU;
        assert!((w.rect[0][1] - w.rect[0][0] - p).abs() < 1e-12);
        assert!(w.rect[1][0] <= -0.5 && w.rect[1][1] >= 0.9);
    }

    #[test]
    fn a_spline_top_is_carried_past_its_domain() {
        // A spline patch of the block's top plane, as a sheet of one face.
        let b = block();
        let top = face_of(&b, FaceRole::PlusZ);
        let plane = &b.surfaces[b.faces[top].surface];
        let (n, _) = plane.to_nurbs([[0.1, 0.5], [0.1, 0.3]]).unwrap();
        let s = Surface::Nurbs(std::sync::Arc::new(n));
        let (sheet, _) = crate::sheet::from_surface(K, &s, s.domain()).unwrap();
        let f = sheet.faces().next().unwrap();
        let before = sheet.surfaces[sheet.faces[f].surface].domain();
        let w = working(&sheet, f, None, sheet.faces[f].sense, true, &region_box()).unwrap();
        let after = w.surface.domain();
        assert!(w.extendable);
        assert!(after[0][0] < before[0][0] && after[0][1] > before[0][1]);
        assert!(after[1][0] < before[1][0] && after[1][1] > before[1][1]);
    }

    #[test]
    fn a_spline_is_carried_no_farther_than_its_length() {
        // The spline top of `a_spline_top_is_carried_past_its_domain`,
        // 0.4 × 0.2, in a region box a thousand times its size.
        let b = block();
        let top = face_of(&b, FaceRole::PlusZ);
        let plane = &b.surfaces[b.faces[top].surface];
        let (n, _) = plane.to_nurbs([[0.1, 0.5], [0.1, 0.3]]).unwrap();
        let s = Surface::Nurbs(std::sync::Arc::new(n));
        let (sheet, _) = crate::sheet::from_surface(K, &s, s.domain()).unwrap();
        let f = sheet.faces().next().unwrap();
        let before = sheet.surfaces[sheet.faces[f].surface].domain();
        let huge = Box3::new(
            Point3::new(-200.0, -200.0, -200.0),
            Point3::new(200.0, 200.0, 200.0),
        );
        let w = working(&sheet, f, None, sheet.faces[f].sense, true, &huge).unwrap();
        let after = w.surface.domain();
        for k in 0..2 {
            let len = before[k][1] - before[k][0];
            assert!(
                after[k][1] - after[k][0] <= 3.0 * len + 1e-12,
                "{k}: {after:?}"
            );
        }
    }

    #[test]
    fn a_deletes_quarter_cylinder_is_carried_less_than_a_turn() {
        // A block with every edge rounded: a vertical edge's quarter
        // cylinder, its corner balls deleted, needs a quarter turn and a
        // little more, as a pocket's corner cylinder does (NIST FTC 08).
        use crate::blend::{BlendKind, EdgeBlend, blend};
        let b = block();
        let edges: Vec<EdgeBlend> = b
            .edges()
            .map(|e| EdgeBlend {
                edge: b.edges[e].name,
                kind: BlendKind::Fillet { radius: 0.05 },
            })
            .collect();
        let (r, _) = blend(crate::name::OpKey(3), &b, &edges).unwrap();
        let is = |f: FaceId, pick: fn(&Surface) -> bool| pick(&r.surfaces[r.faces[f].surface]);
        let cyl = r
            .faces()
            .find(|&f| is(f, |s| matches!(s, Surface::Cylinder(_))))
            .unwrap();
        let mut changes = Changes {
            deleted: r
                .faces()
                .filter(|&f| is(f, |s| matches!(s, Surface::Sphere(_))))
                .collect(),
            ..Default::default()
        };
        let surface = &r.surfaces[r.faces[cyl].surface];
        let [a, z] = delete_turn(&r, &changes, cyl, surface, false)[0].unwrap();
        let quarter = core::f64::consts::FRAC_PI_2;
        assert!(z - a >= quarter && z - a < 1.5 * quarter, "{a} {z}");
        // Its pcurves don't straddle the period's seam: read round the
        // circle, its turn is the same, bit for bit (design 669).
        assert_eq!(
            delete_turn(&r, &changes, cyl, surface, true),
            delete_turn(&r, &changes, cyl, surface, false)
        );
        // A deleted side, along the cylinder's generator, takes the
        // whole turn: the cylinder may have to grow round to meet it.
        let side = r
            .face_loops(cyl)
            .flat_map(|l| r.loop_fins(l))
            .flat_map(|fin| r.edge_fins(r.fins[fin].edge))
            .map(|x| r.loops[r.fins[x].loop_].face)
            .find(|&g| is(g, |s| matches!(s, Surface::Plane(_))))
            .unwrap();
        let mut sided = changes.clone();
        sided.deleted.insert(side);
        assert_eq!(delete_turn(&r, &sided, cyl, surface, false), [None; 2]);
        // A face moved anywhere takes the whole turn.
        changes
            .motion
            .insert(cyl, crate::tweak::Motion::Offset(0.01));
        assert_eq!(delete_turn(&r, &changes, cyl, surface, false), [None; 2]);
    }

    /// A unit cylinder's quarter sheet over u in [0, π/2], its side at
    /// u = 0 stored a period up at 2π, as a file writes a quarter whose
    /// pcurves straddle the period's seam (FTC 07's runs, #652). Shared
    /// with `held`'s and `retrim`'s tests.
    pub(crate) fn straddling_quarter() -> (Body, FaceId) {
        use crate::geom::{Cylinder, Frame, Vector2, Vector3};
        use core::f64::consts::{FRAC_PI_2, TAU};
        let frame = Frame::new(
            Point3::new(0.0, 0.0, 0.0),
            Vector3::new(0.0, 0.0, 1.0),
            Vector3::new(1.0, 0.0, 0.0),
        )
        .unwrap();
        let s = Surface::Cylinder(Cylinder::new(frame, 1.0).unwrap());
        let (mut b, _) = crate::sheet::from_surface(K, &s, [[0.0, FRAC_PI_2], [0.0, 1.0]]).unwrap();
        let f = b.faces().next().unwrap();
        let fins: Vec<_> = b.face_loops(f).flat_map(|l| b.loop_fins(l)).collect();
        let mut moved = 0;
        for fin in fins {
            let Some([lo, hi]) = fin_span(&b, fin, 0) else {
                continue;
            };
            // The iso side at u = 0: moved one period up.
            if lo == 0.0 && hi == 0.0 {
                let pc = b.fins[fin].pcurve.unwrap();
                let c = crate::intersect::shifted(&b.curves2[pc.curve], Vector2::new(TAU, 0.0))
                    .unwrap();
                b.curves2[pc.curve] = c;
                moved += 1;
            }
        }
        assert_eq!(moved, 1, "the side at u = 0");
        (b, f)
    }

    #[test]
    fn a_straddling_quarter_reads_as_a_quarter_round_the_circle() {
        use core::f64::consts::{FRAC_PI_2, PI, TAU};
        let (b, f) = straddling_quarter();
        // Read plainly, its pcurves span the whole turn.
        let [lo, hi] = own_span(&b, f, 0).unwrap();
        assert!(hi - lo > 1.5 * PI, "{lo} {hi}");
        // Round the circle, a quarter: [0, π/2], or the same arc a
        // period on, whichever way the side at 2π falls.
        let [a, z] = own_hull(&b, f, 0, TAU).unwrap();
        assert!((z - a - FRAC_PI_2).abs() < 1e-12, "{a} {z}");
        assert!(a.abs() < 1e-12 || (a - TAU).abs() < 1e-12, "{a} {z}");
    }

    #[test]
    fn a_face_that_doesnt_straddle_reads_as_before() {
        use crate::blend::{BlendKind, EdgeBlend, blend};
        // The rounded block's quarter cylinders, balls and planes (as in
        // `a_deletes_quarter_cylinder_is_carried_less_than_a_turn`).
        let b = block();
        let edges: Vec<EdgeBlend> = b
            .edges()
            .map(|e| EdgeBlend {
                edge: b.edges[e].name,
                kind: BlendKind::Fillet { radius: 0.05 },
            })
            .collect();
        let (r, _) = blend(crate::name::OpKey(3), &b, &edges).unwrap();
        let mut periodic = 0;
        for f in r.faces() {
            let s = &r.surfaces[r.faces[f].surface];
            let (pu, pv) = s.periods();
            for (k, p) in [pu, pv].into_iter().enumerate() {
                let Some(p) = p else { continue };
                periodic += 1;
                let plain = own_span(&r, f, k);
                let hull = own_hull(&r, f, k, p);
                assert_eq!(
                    plain.map(|s| s.map(f64::to_bits)),
                    hull.map(|s| s.map(f64::to_bits)),
                    "{f:?} {k}"
                );
            }
        }
        assert!(periodic > 0);
    }
}
