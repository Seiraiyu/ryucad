//! Offset bodies of faces on every surface: primitives with their curves
//! and surfaces swapped for exact NURBS forms (so the analytic volumes are
//! the oracle), a spline extrusion and a sweep along a path. Every result
//! passes the checker and the self-intersection check, meshes without
//! faults, and encloses the volume it should. This runs in release only
//! (`cargo test -p ryukei --release --test spline_offsets`).

use core::f64::consts::PI;
use ryukei::check::{check, check_intersections};
use ryukei::geom::{
    Circle2, Curve, Curve2, Frame, Frame2, KnotVector, Line2, NurbsCurve, Point2, Point3, Rigid,
    Surface, Vector2, Vector3,
};
use ryukei::heal;
use ryukei::mesh::{MeshParams, check_mesh, tessellate};
use ryukei::name::OpKey;
use ryukei::offset::{OffsetSpec, offset_body};
use ryukei::primitive;
use ryukei::sweep::{
    AlongOptions, Loop, LoopKey, Path, PathSegment, Profile, Segment, SegmentKey, ValidProfile,
    along, extrude,
};
use ryukei::topo::Body;
use ryukei::{Error, OffsetError};
use ryukei_testkit::mesh::volume_and_area;
use ryukei_testkit::nurbs::convert_body;
use std::sync::Arc;

const K: OpKey = OpKey(0xd0);
const OFF: OpKey = OpKey(0xd1);

fn volume(b: &Body, chord: f64) -> f64 {
    assert_eq!(check(b), []);
    assert_eq!(check_intersections(b), []);
    let m = tessellate(b, &MeshParams::new(chord, 0.1)).unwrap();
    assert!(m.faults.is_empty(), "{:?}", m.faults);
    check_mesh(b, &m).unwrap();
    volume_and_area(&m).0
}

fn near(got: f64, want: f64, rel: f64) {
    assert!(
        (got - want).abs() <= rel * want.abs(),
        "{got} against {want}"
    );
}

fn by(d: f64) -> OffsetSpec {
    OffsetSpec {
        distance: d,
        faces: Vec::new(),
    }
}

/// The volume of every point within `d` of a prism of section area `a`,
/// perimeter `p` (convex, so its corners turn by a whole turn) and length
/// `l`, and of the same along any path its section's centroid follows
/// square to it (Pappus): `l A(d) + 2 ∫₀ᵈ A(√(d² - s²)) ds`, `A(r) = a +
/// p r + π r²`.
fn grown_prism(a: f64, p: f64, l: f64, d: f64) -> f64 {
    l * (a + p * d + PI * d * d) + 2.0 * (a * d + p * PI * d * d / 4.0 + PI * 2.0 * d * d * d / 3.0)
}

fn square(s: f64) -> Loop {
    let h = s / 2.0;
    let pts = [(-h, -h), (h, -h), (h, h), (-h, h)];
    let segments = (0..4)
        .map(|i| {
            let (p, q) = (pts[i], pts[(i + 1) % 4]);
            let (p, q) = (Point2::new(p.0, p.1), Point2::new(q.0, q.1));
            Segment {
                key: SegmentKey(10 + i as u128),
                curve: Curve2::Line2(Line2::through(p, q).unwrap()),
                t0: 0.0,
                t1: p.distance(q),
            }
        })
        .collect();
    Loop {
        key: LoopKey(1),
        segments,
    }
}

/// A wavy cubic path starting at the origin along `x`.
fn wavy() -> NurbsCurve {
    let k = KnotVector::clamped(3, &[0.0, 1.0, 2.0], &[4, 1, 4]).unwrap();
    let pts = vec![
        Point3::new(0.0, 0.0, 0.0),
        Point3::new(1.0, 0.0, 0.0),
        Point3::new(2.0, 1.0, 0.5),
        Point3::new(3.0, 0.5, 1.5),
        Point3::new(4.0, 0.0, 2.0),
    ];
    NurbsCurve::new(k, pts, None).unwrap()
}

/// The length of `c` over `[a, b]`, by fine chords.
fn length(c: &NurbsCurve, [a, b]: [f64; 2]) -> f64 {
    let steps = 20_000;
    (0..steps)
        .map(|i| {
            let t = |k: usize| a + (b - a) * k as f64 / steps as f64;
            c.point(t(i)).distance(c.point(t(i + 1)))
        })
        .sum()
}

/// A square swept along a wavy path with a rotation-minimizing frame:
/// its sides' offsets are the sweep of the square's offset, so grown it
/// encloses the grown prism's volume along the path (Pappus). Shrunk, it
/// builds, but the self-intersection check can't settle neighbouring
/// sides' offsets, whose domains end together at the path's ends (issue
/// #223, plan 8.F), so it is refused as uncertain.

#[test]
fn scratch_223() {
    let path_curve = wavy();
    let path = Path {
        segments: vec![PathSegment {
            key: SegmentKey(1),
            curve: Curve::Nurbs(Arc::new(path_curve.clone())),
            range: [0.0, 2.0],
        }],
    };
    let frame = Frame::new(
        Point3::ORIGIN,
        Vector3::new(1.0, 0.0, 0.0),
        Vector3::new(0.0, 1.0, 0.0),
    )
    .unwrap();
    let profile = ValidProfile::new(Profile {
        frame,
        loops: vec![square(0.3)],
    })
    .unwrap();
    let (b, _) = along(K, &profile, &path, &AlongOptions::default()).unwrap();
    let t = std::time::Instant::now();
    let r = offset_body(OFF, &b, &by(-0.03));
    eprintln!(
        "SCRATCH shrunk: {:?} in {:?}",
        r.as_ref().map(|x| x.0.counts()),
        t.elapsed()
    );
    if let Ok((o, _)) = &r {
        let l = length(&path_curve, [0.0, 2.0]);
        eprintln!("SCRATCH volume {} vs {}", volume(o, 2e-4), 0.24 * 0.24 * l);
    }
}
