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

#[test]
#[cfg_attr(
    debug_assertions,
    ignore = "slow in a debug build: CI runs it in release"
)]
fn a_spline_box_offsets_as_the_box_does() {
    let (mut b, _) = primitive::block(K, 1.0, 0.6, 0.4, &Rigid::IDENTITY).unwrap();
    convert_body(&mut b).unwrap();
    let d = 0.1;
    let (o, _) = offset_body(OFF, &b, &by(d)).unwrap();
    near(volume(&o, 2e-4), grown_prism(0.6, 3.2, 0.4, d), 2e-3);
    let (o, _) = offset_body(OFF, &b, &by(-d)).unwrap();
    near(volume(&o, 1e-3), 0.8 * 0.4 * 0.2, 1e-6);
}

/// An extruded spline circle: its side is an extrusion, offset as an
/// [`ryukei::geom::Offset`]; its rims are spline circles, filled by pipes.
#[test]
#[cfg_attr(
    debug_assertions,
    ignore = "slow in a debug build: CI runs it in release"
)]
fn an_extruded_spline_cylinder_offsets() {
    let f = Frame2::new(Point2::ORIGIN, Vector2::new(1.0, 0.0), true).unwrap();
    let c = Circle2::new(f, 0.5).unwrap();
    let (n, r) = Curve2::Circle2(c).to_nurbs([0.0, 2.0 * PI]).unwrap();
    let [t0, t1] = [r.to_nurbs(0.0), r.to_nurbs(2.0 * PI)];
    let profile = ValidProfile::new(Profile {
        frame: Frame::WORLD,
        loops: vec![Loop {
            key: LoopKey(1),
            segments: vec![Segment {
                key: SegmentKey(1),
                curve: Curve2::Nurbs2(Arc::new(n)),
                t0,
                t1,
            }],
        }],
    })
    .unwrap();
    let (b, _) = extrude(K, &profile, Vector3::new(0.0, 0.0, 1.0), 0.0, 1.0).unwrap();
    let (r, h, d) = (0.5, 1.0, 0.1);
    let (o, _) = offset_body(OFF, &b, &by(d)).unwrap();
    assert!(o.faces().any(|f| matches!(
        o.surface(o.face(f).unwrap().surface),
        Some(Surface::Offset(_))
    )));
    let want = PI * (r + d) * (r + d) * h
        + 2.0 * PI * r * r * d
        + PI * PI * r * d * d
        + 4.0 / 3.0 * PI * d * d * d;
    near(volume(&o, 2e-4), want, 2e-3);
    let (o, _) = offset_body(OFF, &b, &by(-d)).unwrap();
    near(
        volume(&o, 2e-4),
        PI * (r - d) * (r - d) * (h - 2.0 * d),
        2e-3,
    );
}

/// A square of side `s` centred on the frame's origin.
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
#[cfg_attr(
    debug_assertions,
    ignore = "slow in a debug build: CI runs it in release"
)]
fn a_sweep_along_a_path_offsets() {
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
    let s = 0.3;
    let profile = ValidProfile::new(Profile {
        frame,
        loops: vec![square(s)],
    })
    .unwrap();
    let (b, _) = along(K, &profile, &path, &AlongOptions::default()).unwrap();
    let l = length(&path_curve, [0.0, 2.0]);
    let d = 0.03;
    let (o, _) = offset_body(OFF, &b, &by(d)).unwrap();
    near(volume(&o, 2e-4), grown_prism(s * s, 4.0 * s, l, d), 2e-3);
    assert!(matches!(
        offset_body(OFF, &b, &by(-d)),
        Err(Error::InvalidOffset(OffsetError::Uncertain { .. }))
    ));
}
