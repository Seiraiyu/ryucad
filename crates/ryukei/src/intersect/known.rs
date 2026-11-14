//! Known seams (design 551, plan 18.C, #652): two faces met along a curve
//! their surfaces are known to share, with no trace along it.
//!
//! The re-trim carries kept faces past their edges. Two kept faces that
//! were tangent along an edge of the input have surfaces that still hold
//! its curve (a carry extends a surface without changing it), so their
//! working faces meet tangentially along it, where no trace can certify
//! a crossing (FTC 07 and FTC 10: 2 to 26M units a pair, then
//! `Uncertain`). Here that curve is the faces' branch:
//!
//! - run on along its own parameter over the box the faces share (a
//!   line's whole stretch in it, a circle's or an ellipse's period),
//!   where both pcurves are lines in the parameter plane, so their affine
//!   maps still hold;
//! - certified within [`SEAM_SLACK`] times the edge's tolerance of both
//!   surfaces through its pcurves ([`pcurve_certified`]);
//! - with one face certified to keep off the other's surface outside a
//!   strip round it as wide as surfaces meeting tangentially stay within
//!   the touch rule's reach of each other, `sqrt(2 × size × reach)`
//!   ([`face_beside_rails`]): the self-intersection check's rule for faces
//!   tangent along an edge they share.
//!
//! The branch is then cut where the faces' edges meet the other surface
//! and placed in both, as a traced one is ([`placed`]). Anything else
//! withholds the rule: the caller traces the pair as before.

use super::Failure;
use super::beside::face_beside_rails;
use super::face::{FaceMeet, face_box, max_tolerance, overlap, placed, surface_of};
use super::fit::pcurve_certified;
use super::ss::Branch;
use crate::geom::{Box3, Curve, Curve2, Surface};
use crate::tol::LINEAR_RESOLUTION;
use crate::topo::{Body, FaceId};
use crate::work::Meter;
use std::collections::BTreeMap;

/// A curve two faces are known to meet along: an edge's curve over its
/// interval and its tolerance, and its pcurves on the first face's
/// surface and the second's, each with the affine map from the curve's
/// parameter to its own (as [`crate::topo::Pcurve`]).
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct Seam {
    pub(crate) curve: Curve,
    pub(crate) interval: [f64; 2],
    pub(crate) tolerance: f64,
    pub(crate) pcurves: [(Curve2, [f64; 2]); 2],
}

/// Known seams by pair of one body's faces, each pair and each seam's
/// pcurves in face order.
pub(crate) type Seams = BTreeMap<(FaceId, FaceId), Vec<Seam>>;

/// The touch rule's reach, in tolerances (`check::intersect`'s `TOUCH`).
const TOUCH: f64 = 100.0;

/// How far a seam run on past its edge may stray from either surface, in
/// the edge's tolerances, for the rule to hold.
const SEAM_SLACK: f64 = 10.0;

/// The part limits each face's search apart is tried under, the other's
/// between: the check's `apart_beside` ladder.
const LADDER: [usize; 3] = [1 << 9, 1 << 12, super::beside::CELLS];

/// How face `a.1` of body `a.0` and face `b.1` of body `b.0` meet, given
/// curves their surfaces are known to share (`seams`, each with its
/// pcurves on `a`'s surface first): met along those curves where one face
/// is certified to keep off the other's surface beside them, charging
/// `m`. Fails with `Uncertain` where it can't be.
pub(crate) fn known_seam(
    a: (&Body, FaceId),
    b: (&Body, FaceId),
    seams: &[Seam],
    m: &Meter,
) -> Result<FaceMeet, Failure> {
    m.charge(1)?;
    if seams.is_empty() {
        return Err(Failure::Uncertain);
    }
    let (sa, sb) = (surface_of(a)?, surface_of(b)?);
    let slack = max_tolerance(a).max(max_tolerance(b));
    let (ba, bb) = (face_box(a.0, a.1), face_box(b.0, b.1));
    let Some(region) = overlap(&ba, &bb, slack) else {
        return Ok(FaceMeet::apart());
    };
    let mut branches = Vec::with_capacity(seams.len());
    for s in seams {
        branches.push(branch(s, [sa, sb], &region, m).ok_or(Failure::Uncertain)?);
    }
    let tolerance = branches.iter().map(|br| br.tolerance).fold(slack, f64::max);
    let size = {
        let bx = ba.union(&bb);
        (bx.max() - bx.min()).length()
    };
    let strip = crate::math::sqrt(2.0 * size * TOUCH * tolerance);
    let apart = {
        let rails = |k: usize| -> Vec<(&Curve2, [f64; 2])> {
            branches
                .iter()
                .map(|br| {
                    let (c2, map) = &br.pcurves[k];
                    let [s0, s1] = br.range.map(|t| map[0] + map[1] * t);
                    (c2, [s0.min(s1), s0.max(s1)])
                })
                .collect()
        };
        let (ra, rb) = (rails(0), rails(1));
        let keeps_off = |f: (&Body, FaceId), other: &Surface, r: &[(&Curve2, [f64; 2])], cells| {
            face_beside_rails(f.0, f.1, other, r, strip, cells, m).is_ok()
        };
        // Each face's search under a growing part limit, the other's
        // between: the face that keeps off usually does in a few parts.
        LADDER
            .into_iter()
            .any(|cells| keeps_off(a, sb, &ra, cells) || keeps_off(b, sa, &rb, cells))
    };
    if !apart {
        return Err(Failure::Uncertain);
    }
    placed([a, b], [sa, sb], branches, Vec::new(), slack, m)
}

/// The seam as a tangent branch run on over `region` (see [`run_on`]),
/// its tolerance the edge's or what its pcurves are certified within,
/// whichever is more. `None` where a pcurve isn't certified within
/// [`SEAM_SLACK`] edge tolerances.
fn branch(s: &Seam, surfaces: [&Surface; 2], region: &Box3, m: &Meter) -> Option<Branch> {
    let range = run_on(s, region);
    let mut tolerance = s.tolerance.max(LINEAR_RESOLUTION);
    let target = SEAM_SLACK * tolerance;
    for (k, surface) in surfaces.into_iter().enumerate() {
        let (c2, map) = &s.pcurves[k];
        let bound = pcurve_certified(surface, &s.curve, range, c2, *map, target, m)?;
        if bound > target {
            return None;
        }
        tolerance = tolerance.max(bound);
    }
    Some(Branch {
        curve: s.curve.clone(),
        range,
        tangent: true,
        tolerance,
        pcurves: s.pcurves.clone(),
    })
}

/// The seam's curve run on past its interval where both pcurves are
/// lines in the parameter plane (unbounded, so their affine maps hold):
/// a line over the stretch of it inside `region`, a circle or an ellipse
/// over a whole period from the interval's start. Anything else, or a
/// line missing the region, keeps the interval.
fn run_on(s: &Seam, region: &Box3) -> [f64; 2] {
    if !s.pcurves.iter().all(|(c, _)| matches!(c, Curve2::Line2(_))) {
        return s.interval;
    }
    match &s.curve {
        Curve::Line(l) => {
            let (o, d) = (l.origin(), l.dir().get());
            let (lo, hi) = (region.min(), region.max());
            let mut t = [f64::NEG_INFINITY, f64::INFINITY];
            for (p, q, a, b) in [
                (o.x, d.x, lo.x, hi.x),
                (o.y, d.y, lo.y, hi.y),
                (o.z, d.z, lo.z, hi.z),
            ] {
                // A direction square to this axis: the slab doesn't bound
                // the line's parameter.
                if q == 0.0 {
                    continue;
                }
                let (t0, t1) = ((a - p) / q, (b - p) / q);
                t = [t[0].max(t0.min(t1)), t[1].min(t0.max(t1))];
            }
            if !(t[0].is_finite() && t[1].is_finite()) || t[0] >= t[1] {
                return s.interval;
            }
            [t[0].min(s.interval[0]), t[1].max(s.interval[1])]
        }
        c => match c.period() {
            Some(p) => [s.interval[0], s.interval[0] + p],
            None => s.interval,
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::geom::{
        Circle, Cylinder, Frame, Line, Line2, Plane, Point2, Point3, Torus, UnitVector3, Vector2,
        Vector3,
    };
    use crate::name::OpKey;
    use crate::work::Meter;
    use core::f64::consts::PI;

    const MM: f64 = 1e-3;
    const K: OpKey = OpKey(0x18c4);

    fn sheet(s: &Surface, rect: [[f64; 2]; 2]) -> Body {
        crate::sheet::from_surface(K, s, rect).unwrap().0
    }

    fn only(b: &Body) -> FaceId {
        b.faces().next().unwrap()
    }

    fn line2(o: (f64, f64), d: (f64, f64)) -> Curve2 {
        Curve2::Line2(Line2::new(Point2::new(o.0, o.1), Vector2::new(d.0, d.1)).unwrap())
    }

    /// A plane through z = -r - `drop`, turned `tilt` about the x axis,
    /// as a sheet x, y ∈ [-2, 2] × [-1, 1] mm; a cylinder of radius r =
    /// 1 mm about the x axis as a sheet over the bottom of its tube, u ∈
    /// [π - 1, π + 1], x ∈ [-3, 3] mm; and the seam y = 0, z = -r over x
    /// ∈ [-0.5, 0.5] mm, with its pcurves (u = x on the plane; u = π, v =
    /// x on the cylinder), as a re-trim would record it.
    fn plane_and_cylinder(tilt: f64, drop: f64) -> (Body, Body, Seam) {
        let r = MM;
        let (s, c) = crate::math::sin_cos(tilt);
        let plane = Surface::Plane(Plane::new(
            Frame::new(
                Point3::new(0.0, 0.0, -r - drop),
                Vector3::new(0.0, -s, c),
                Vector3::new(1.0, 0.0, 0.0),
            )
            .unwrap(),
        ));
        let axis = Frame::new(
            Point3::ORIGIN,
            Vector3::new(1.0, 0.0, 0.0),
            Vector3::new(0.0, 0.0, 1.0),
        )
        .unwrap();
        let cyl = Surface::Cylinder(Cylinder::new(axis, r).unwrap());
        let a = sheet(&plane, [[-2.0 * MM, 2.0 * MM], [-MM, MM]]);
        let b = sheet(&cyl, [[PI - 1.0, PI + 1.0], [-3.0 * MM, 3.0 * MM]]);
        let seam = Seam {
            curve: Curve::Line(Line::from_unit(Point3::new(0.0, 0.0, -r), UnitVector3::X).unwrap()),
            interval: [-0.5 * MM, 0.5 * MM],
            tolerance: LINEAR_RESOLUTION,
            pcurves: [
                (line2((0.0, 0.0), (1.0, 0.0)), [0.0, 1.0]),
                (line2((PI, 0.0), (0.0, 1.0)), [0.0, 1.0]),
            ],
        };
        (a, b, seam)
    }

    #[test]
    fn a_tangent_seam_is_met_along_its_line_to_the_faces_ends() {
        let (a, b, seam) = plane_and_cylinder(0.0, 0.0);
        let m = Meter::free(); // unmetered: a test
        let meet = known_seam((&a, only(&a)), (&b, only(&b)), &[seam], &m).unwrap();
        let FaceMeet::Meet {
            stretches,
            contacts,
        } = meet
        else {
            panic!("not coincident")
        };
        assert!(contacts.is_empty(), "{contacts:?}");
        assert_eq!(stretches.len(), 1, "{stretches:?}");
        let s = &stretches[0];
        assert!(s.tangent);
        // Run on to where the plane's face ends, x = ±2 mm.
        let [p0, p1] = s.range.map(|t| s.curve.point(t));
        assert!((p0.x.min(p1.x) + 2.0 * MM).abs() <= 1e-9, "{p0:?}");
        assert!((p0.x.max(p1.x) - 2.0 * MM).abs() <= 1e-9, "{p1:?}");
        // Against sampling: on both surfaces within its tolerance.
        for i in 0..=8 {
            let t = s.range[0] + (s.range[1] - s.range[0]) * f64::from(i) / 8.0;
            let p = s.curve.point(t);
            assert!((p.z + MM).abs() <= s.tolerance + 1e-15, "{p:?}");
            let rho = crate::math::sqrt(p.y * p.y + p.z * p.z);
            assert!((rho - MM).abs() <= s.tolerance + 1e-15, "{p:?}");
        }
        // And the faces meet nowhere else: the plane's points off the
        // line lie outside the tube.
        for i in 1..=10 {
            let y = 0.1 * MM * f64::from(i);
            let rho = crate::math::sqrt(y * y + MM * MM);
            assert!(rho > MM);
        }
    }

    #[test]
    fn a_seam_crossing_at_an_angle_is_withheld() {
        // Turned 0.3 rad about the line, the plane crosses the cylinder
        // along it and along a second line: no face keeps off the other.
        let (a, b, seam) = plane_and_cylinder(0.3, 0.0);
        let m = Meter::free(); // unmetered: a test
        assert!(known_seam((&a, only(&a)), (&b, only(&b)), &[seam], &m).is_err());
    }

    #[test]
    fn a_seam_off_a_surface_is_withheld() {
        // 10 µm up into the tube, the plane crosses the cylinder but
        // doesn't hold the seam: its pcurve is not certified within ten
        // tolerances. (10 µm under the tube, the faces' boxes would share
        // nothing and the faces would be apart.)
        let (a, b, seam) = plane_and_cylinder(0.0, -1e-5);
        let m = Meter::free(); // unmetered: a test
        assert!(known_seam((&a, only(&a)), (&b, only(&b)), &[seam], &m).is_err());
    }

    /// The meeting's bits, pinned in the crate (design 551's
    /// `known_seams` golden; `known_seam` isn't public, Decision 5): the
    /// plane and cylinder's stretches, their ranges, tolerances and
    /// places, FNV-1a over their bits. Unit tests run on every CI
    /// platform. After a deliberate change, set the constant to the value
    /// the failure prints, and review why it moved.
    #[test]
    fn known_seams_hash() {
        const KNOWN_SEAMS_HASH: u64 = 0x7c00_c46b_b666_9f22;
        let (a, b, seam) = plane_and_cylinder(0.0, 0.0);
        let m = Meter::free(); // unmetered: a test
        let FaceMeet::Meet {
            stretches,
            contacts,
        } = known_seam((&a, only(&a)), (&b, only(&b)), &[seam], &m).unwrap()
        else {
            panic!("not coincident")
        };
        let mut h: u64 = 0xcbf2_9ce4_8422_2325;
        let mut eat = |x: u64| {
            for byte in x.to_le_bytes() {
                h ^= u64::from(byte);
                h = h.wrapping_mul(0x0100_0000_01b3);
            }
        };
        eat(stretches.len() as u64);
        eat(contacts.len() as u64);
        for s in &stretches {
            eat(s.range[0].to_bits());
            eat(s.range[1].to_bits());
            eat(s.tolerance.to_bits());
            for p in &s.places {
                eat(u64::from(matches!(p, crate::intersect::Place::Inside)));
            }
        }
        assert_eq!(h, KNOWN_SEAMS_HASH, "known_seams hash: {h:#018x}");
    }

    /// FTC 07's kind (#652): a pipe-like round's straight run, a cylinder
    /// of radius 2 mm along y through (10, 0, 0) mm, carried past the
    /// circle where it meets its corner torus (major 10 mm, minor 2 mm,
    /// about z), which is carried back past it. Past the seam the
    /// cylinder enters the tube on the bend's inner side and leaves it on
    /// the outer, so the two cross along curves leaving the circle and
    /// neither keeps off the other: withheld (Decision 4).
    #[test]
    fn a_cylinder_carried_past_its_torus_seam_is_withheld() {
        let (big, small) = (10.0 * MM, 2.0 * MM);
        let torus = Surface::Torus(Torus::new(Frame::WORLD, big, small).unwrap());
        let cyl = Surface::Cylinder(
            Cylinder::new(
                Frame::new(
                    Point3::new(big, 0.0, 0.0),
                    Vector3::new(0.0, 1.0, 0.0),
                    Vector3::new(1.0, 0.0, 0.0),
                )
                .unwrap(),
                small,
            )
            .unwrap(),
        );
        let (td, cd) = (torus.domain(), cyl.domain());
        assert_eq!(cd[0], [0.0, 2.0 * PI], "the pcurve below assumes it");
        let a = sheet(&torus, [[-0.5, 0.5], td[1]]);
        let b = sheet(&cyl, [cd[0], [-6.0 * MM, 6.0 * MM]]);
        // The tube's circle at u = 0, on the torus's v: from +x towards +z.
        let circle = Circle::new(
            Frame::new(
                Point3::new(big, 0.0, 0.0),
                Vector3::new(0.0, -1.0, 0.0),
                Vector3::new(1.0, 0.0, 0.0),
            )
            .unwrap(),
            small,
        )
        .unwrap();
        let seam = Seam {
            curve: Curve::Circle(circle),
            interval: [0.0, PI / 2.0],
            tolerance: LINEAR_RESOLUTION,
            pcurves: [
                (line2((0.0, 0.0), (0.0, 1.0)), [0.0, 1.0]),
                // On the cylinder, u = 2π - t.
                (line2((2.0 * PI, 0.0), (-1.0, 0.0)), [0.0, 1.0]),
            ],
        };
        // Against sampling: 1 mm past the seam, the cylinder's outer
        // point lies outside the tube and its inner point inside.
        let off_tube = |p: Point3| {
            let rho = crate::math::sqrt(p.x * p.x + p.y * p.y);
            crate::math::sqrt((rho - big) * (rho - big) + p.z * p.z) - small
        };
        assert!(off_tube(Point3::new(big + small, MM, 0.0)) > 0.0);
        assert!(off_tube(Point3::new(big - small, MM, 0.0)) < 0.0);
        let m = Meter::free(); // unmetered: a test
        assert!(known_seam((&a, only(&a)), (&b, only(&b)), &[seam], &m).is_err());
    }
}
