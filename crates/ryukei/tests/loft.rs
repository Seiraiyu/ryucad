//! Lofts end to end: solids that pass the checker and the self-
//! intersection check, mesh cleanly and enclose the volumes frustums and
//! prisms have, refusals by name, and names that survive edits.

use core::f64::consts::{PI, TAU};
use ryukei::check::check;
use ryukei::geom::{
    Circle2, Curve2, Frame, Frame2, Line2, Point2, Point3, Surface, UnitVector3, Vector2, Vector3,
};
use ryukei::mesh::{MeshParams, check_mesh, tessellate};
use ryukei::name::OpKey;
use ryukei::sweep::{
    LoftOptions, Loop, LoopKey, Profile, Segment, SegmentKey, Tangent, ValidProfile, loft,
};
use ryukei::topo::Body;
use ryukei::{Error, LoftError};
use ryukei_testkit::mesh::volume_and_area;

const K: OpKey = OpKey(0x10f7);

fn seg(key: u128, curve: Curve2, t0: f64, t1: f64) -> Segment {
    Segment {
        key: SegmentKey(key),
        curve,
        t0,
        t1,
    }
}

/// A square of side `s` centred on the origin, its loop keyed `loop_key`
/// and its segments from `first`.
fn square(loop_key: u128, first: u128, s: f64) -> Loop {
    let h = s / 2.0;
    let pts = [(-h, -h), (h, -h), (h, h), (-h, h)];
    let segments = (0..4)
        .map(|i| {
            let (p, q) = (pts[i], pts[(i + 1) % 4]);
            let (p, q) = (Point2::new(p.0, p.1), Point2::new(q.0, q.1));
            seg(
                first + i as u128,
                Curve2::Line2(Line2::through(p, q).unwrap()),
                0.0,
                p.distance(q),
            )
        })
        .collect();
    Loop {
        key: LoopKey(loop_key),
        segments,
    }
}

fn circle(loop_key: u128, key: u128, c: (f64, f64), r: f64) -> Loop {
    let f = Frame2::new(Point2::new(c.0, c.1), Vector2::new(1.0, 0.0), true).unwrap();
    Loop {
        key: LoopKey(loop_key),
        segments: vec![seg(
            key,
            Curve2::Circle2(Circle2::new(f, r).unwrap()),
            0.0,
            TAU,
        )],
    }
}

/// A section in the plane `z = h`.
fn at(h: f64, loops: Vec<Loop>) -> ValidProfile {
    ValidProfile::new(Profile {
        frame: Frame::new(
            Point3::new(0.0, 0.0, h),
            Vector3::new(0.0, 0.0, 1.0),
            Vector3::new(1.0, 0.0, 0.0),
        )
        .unwrap(),
        loops,
    })
    .unwrap()
}

fn volume(b: &Body) -> f64 {
    assert_eq!(check(b), []);
    let m = tessellate(b, &MeshParams::new(2e-4, 0.3)).unwrap();
    assert!(m.faults.is_empty(), "{:?}", m.faults);
    check_mesh(b, &m).unwrap();
    volume_and_area(&m).0
}

fn close(a: f64, b: f64, tol: f64) -> bool {
    (a - b).abs() <= tol * b.abs().max(1.0)
}

fn kinds(b: &Body) -> Vec<&'static str> {
    let mut out: Vec<&str> = b
        .faces()
        .map(|f| match b.surface(b.face(f).unwrap().surface).unwrap() {
            Surface::Plane(_) => "plane",
            Surface::Nurbs(_) => "nurbs",
            _ => "other",
        })
        .collect();
    out.sort_unstable();
    out
}

/// Two squares a unit apart, of sides 2 and 1, are a frustum; two circles
/// a cone's; three equal squares a prism.
#[test]
fn frustums_and_prisms() {
    let opts = LoftOptions::default();
    let (b, _) = loft(
        K,
        &[
            at(0.0, vec![square(1, 10, 2.0)]),
            at(1.0, vec![square(1, 20, 1.0)]),
        ],
        &opts,
    )
    .unwrap();
    assert_eq!(
        kinds(&b),
        ["nurbs", "nurbs", "nurbs", "nurbs", "plane", "plane"]
    );
    let v = volume(&b);
    assert!(close(v, 7.0 / 3.0, 1e-3), "{v}");
    let (b, _) = loft(
        K,
        &[
            at(0.0, vec![circle(1, 10, (0.0, 0.0), 1.0)]),
            at(2.0, vec![circle(1, 20, (0.0, 0.0), 0.5)]),
        ],
        &opts,
    )
    .unwrap();
    let v = volume(&b);
    let want = PI * 2.0 * (1.0 + 0.5 + 0.25) / 3.0;
    assert!(close(v, want, 2e-3), "{v} against {want}");
    let (b, _) = loft(
        K,
        &[
            at(0.0, vec![square(1, 10, 1.0)]),
            at(0.7, vec![square(1, 20, 1.0)]),
            at(2.0, vec![square(1, 30, 1.0)]),
        ],
        &opts,
    )
    .unwrap();
    let v = volume(&b);
    assert!(close(v, 2.0, 1e-3), "{v}");
}

/// A hole lofts with its outer loop, a cavity-free tube through.
#[test]
fn holes_loft_too() {
    let (b, _) = loft(
        K,
        &[
            at(
                0.0,
                vec![square(1, 10, 2.0), circle(2, 50, (0.0, 0.0), 0.5)],
            ),
            at(
                1.0,
                vec![square(1, 20, 2.0), circle(2, 60, (0.0, 0.0), 0.5)],
            ),
        ],
        &LoftOptions::default(),
    )
    .unwrap();
    let v = volume(&b);
    assert!(close(v, 4.0 - PI * 0.25, 2e-3), "{v}");
}

/// End tangents bend the skin out of the sections' frustum, keeping it at
/// the sections.
#[test]
fn tangents_shape_the_ends() {
    let z = UnitVector3::exact(Vector3::new(0.0, 0.0, 1.0)).unwrap();
    let opts = LoftOptions {
        closed: false,
        start: Some(Tangent {
            direction: z,
            weight: 1.0,
        }),
        end: Some(Tangent {
            direction: z,
            weight: 1.0,
        }),
    };
    let sections = [
        at(0.0, vec![square(1, 10, 2.0)]),
        at(1.0, vec![square(1, 20, 1.0)]),
    ];
    let (b, _) = loft(K, &sections, &opts).unwrap();
    let v = volume(&b);
    // Bulging out from the ruled frustum, never past the bigger prism.
    assert!(v > 7.0 / 3.0 && v < 4.0, "{v}");
}

/// Four squares round the z axis loft into a ring with no caps.
#[test]
#[cfg_attr(
    debug_assertions,
    ignore = "slow in a debug build: CI runs it in release"
)]
fn a_closed_loft_is_a_ring() {
    let sections: Vec<ValidProfile> = (0..4)
        .map(|k| {
            let a = TAU * f64::from(k) / 4.0;
            let (s, c) = ryukei::math::sin_cos(a);
            ValidProfile::new(Profile {
                frame: Frame::new(
                    Point3::new(3.0 * c, 3.0 * s, 0.0),
                    Vector3::new(-s, c, 0.0),
                    Vector3::new(c, s, 0.0),
                )
                .unwrap(),
                loops: vec![square(1, 10 * (k as u128 + 1), 1.0)],
            })
            .unwrap()
        })
        .collect();
    let opts = LoftOptions {
        closed: true,
        ..LoftOptions::default()
    };
    let (b, _) = loft(K, &sections, &opts).unwrap();
    assert_eq!(b.faces().count(), 4);
    let v = volume(&b);
    // About a square ring's: the skin swings a little outside the circle.
    assert!(close(v, TAU * 3.0, 0.1), "{v}");
}

fn refusal(r: Result<(Body, ryukei::name::Journal), Error>) -> LoftError {
    match r {
        Err(Error::InvalidLoft(e)) => e,
        r => panic!("{:?}", r.map(|_| ())),
    }
}

#[test]
fn refusals() {
    let opts = LoftOptions::default();
    let one = at(0.0, vec![square(1, 10, 1.0)]);
    assert_eq!(
        refusal(loft(K, std::slice::from_ref(&one), &opts)),
        LoftError::TooFewSections
    );
    let closed = LoftOptions {
        closed: true,
        ..opts
    };
    let two = [one.clone(), at(1.0, vec![square(1, 20, 1.0)])];
    assert_eq!(refusal(loft(K, &two, &closed)), LoftError::TooFewSections);
    // Another loop key, another segment count, the same plane.
    let other = at(1.0, vec![square(2, 20, 1.0)]);
    assert_eq!(
        refusal(loft(K, &[one.clone(), other], &opts)),
        LoftError::LoopMismatch {
            section: 1,
            loop_: LoopKey(1)
        }
    );
    let round = at(1.0, vec![circle(1, 20, (0.0, 0.0), 0.5)]);
    assert_eq!(
        refusal(loft(K, &[one.clone(), round], &opts)),
        LoftError::SegmentCountMismatch {
            section: 1,
            loop_: LoopKey(1)
        }
    );
    let flat = at(0.0, vec![square(1, 20, 2.0)]);
    assert_eq!(
        refusal(loft(K, &[one.clone(), flat], &opts)),
        LoftError::BadSection { section: 1 }
    );
    // A tangent in the section's plane, or one round a closed loft.
    let x = UnitVector3::exact(Vector3::new(1.0, 0.0, 0.0)).unwrap();
    let sideways = LoftOptions {
        start: Some(Tangent {
            direction: x,
            weight: 1.0,
        }),
        ..opts
    };
    assert_eq!(refusal(loft(K, &two, &sideways)), LoftError::BadTangent);
    // A closed loft of circles would be a tube with no edges.
    let rings: Vec<ValidProfile> = (0..3)
        .map(|k| {
            at(
                f64::from(k),
                vec![circle(1, 10 + k as u128, (0.0, 0.0), 0.5)],
            )
        })
        .collect();
    assert!(matches!(
        refusal(loft(K, &rings, &closed)),
        LoftError::ClosedTube { .. } | LoftError::SelfIntersecting { .. }
    ));
}

/// Resizing and moving sections renames nothing.
#[test]
fn names_survive_resizing() {
    let names = |b: &Body| {
        let mut out: Vec<_> = b
            .faces()
            .map(|f| b.face(f).unwrap().name)
            .chain(b.edges().map(|e| b.edge(e).unwrap().name))
            .chain(b.vertices().map(|v| b.vertex(v).unwrap().name))
            .collect();
        out.sort_unstable();
        out
    };
    let opts = LoftOptions::default();
    let (a, _) = loft(
        K,
        &[
            at(0.0, vec![square(1, 10, 2.0)]),
            at(1.0, vec![square(1, 20, 1.0)]),
        ],
        &opts,
    )
    .unwrap();
    let (c, _) = loft(
        K,
        &[
            at(0.5, vec![square(1, 10, 1.5)]),
            at(2.0, vec![square(1, 20, 0.7)]),
        ],
        &opts,
    )
    .unwrap();
    let all = names(&a);
    let mut dedup = all.clone();
    dedup.dedup();
    assert_eq!(dedup.len(), all.len());
    assert_eq!(names(&c), all);
}
