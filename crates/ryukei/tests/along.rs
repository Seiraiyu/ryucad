//! Sweeps along paths end to end: solids that pass the checker and the
//! self-intersection check, mesh cleanly and enclose the volume Pappus's
//! theorem gives, refusals by name, and names that survive edits.

use core::f64::consts::{FRAC_PI_2, PI, TAU};
use ryukei::check::check;
use ryukei::geom::{
    Circle, Circle2, Curve, Curve2, Frame, Frame2, FrameLaw, KnotVector, Line, Line2, NurbsCurve,
    Point2, Point3, Surface, UnitVector3, Vector2, Vector3,
};
use ryukei::mesh::{MeshParams, check_mesh, tessellate};
use ryukei::name::{Event, OpKey};
use ryukei::sweep::{
    AlongOptions, Loop, LoopKey, Path, PathSegment, Profile, Segment, SegmentKey, ValidProfile,
    along,
};
use ryukei::topo::Body;
use ryukei::{Error, SweepError};
use ryukei_testkit::mesh::volume_and_area;
use std::sync::Arc;

const K: OpKey = OpKey(0xa1);

fn seg(key: u128, curve: Curve2, t0: f64, t1: f64) -> Segment {
    Segment {
        key: SegmentKey(key),
        curve,
        t0,
        t1,
    }
}

/// A square of side `s` centred on the frame's origin, keyed from 10.
fn square(s: f64) -> Loop {
    let h = s / 2.0;
    let pts = [(-h, -h), (h, -h), (h, h), (-h, h)];
    let segments = (0..4)
        .map(|i| {
            let (p, q) = (pts[i], pts[(i + 1) % 4]);
            let (p, q) = (Point2::new(p.0, p.1), Point2::new(q.0, q.1));
            seg(
                10 + i as u128,
                Curve2::Line2(Line2::through(p, q).unwrap()),
                0.0,
                p.distance(q),
            )
        })
        .collect();
    Loop {
        key: LoopKey(1),
        segments,
    }
}

/// A circle of radius `r` about `c`, keyed `key`.
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

/// The plane across `x` at the origin: profiles start there, heading
/// along `x`.
fn across() -> Frame {
    Frame::new(
        Point3::ORIGIN,
        Vector3::new(1.0, 0.0, 0.0),
        Vector3::new(0.0, 1.0, 0.0),
    )
    .unwrap()
}

fn profile(loops: Vec<Loop>) -> ValidProfile {
    ValidProfile::new(Profile {
        frame: across(),
        loops,
    })
    .unwrap()
}

fn part(key: u128, curve: Curve, range: [f64; 2]) -> PathSegment {
    PathSegment {
        key: SegmentKey(key),
        curve,
        range,
    }
}

fn line(from: Point3, dir: Vector3) -> Curve {
    Curve::Line(Line::new(from, dir).unwrap())
}

/// Along `x` by 2, a quarter turn of radius 1 towards `y`, along `y` by
/// `last`: `4 + π/2` long with `last` 2.
fn bent(last: f64) -> Path {
    let f = Frame::new(
        Point3::new(2.0, 1.0, 0.0),
        Vector3::new(0.0, 0.0, 1.0),
        Vector3::new(1.0, 0.0, 0.0),
    )
    .unwrap();
    Path {
        segments: vec![
            part(
                1,
                line(Point3::ORIGIN, Vector3::new(1.0, 0.0, 0.0)),
                [0.0, 2.0],
            ),
            part(
                2,
                Curve::Circle(Circle::new(f, 1.0).unwrap()),
                [-FRAC_PI_2, 0.0],
            ),
            part(
                3,
                line(Point3::new(3.0, 1.0, 0.0), Vector3::new(0.0, 1.0, 0.0)),
                [0.0, last],
            ),
        ],
    }
}

/// A wavy cubic starting at the origin along `x`, and its length.
fn wavy() -> (Path, f64) {
    let k = KnotVector::clamped(3, &[0.0, 1.0, 2.0], &[4, 1, 4]).unwrap();
    let pts = vec![
        Point3::new(0.0, 0.0, 0.0),
        Point3::new(1.0, 0.0, 0.0),
        Point3::new(2.0, 1.0, 0.5),
        Point3::new(3.0, 0.5, 1.5),
        Point3::new(4.0, 0.0, 2.0),
    ];
    let n = NurbsCurve::new(k, pts, None).unwrap();
    // The length by fine sampling.
    let mut len = 0.0;
    let steps = 20_000;
    for i in 0..steps {
        let (a, b) = (
            2.0 * f64::from(i) / f64::from(steps),
            2.0 * f64::from(i + 1) / f64::from(steps),
        );
        len += n.point(a).distance(n.point(b));
    }
    let path = Path {
        segments: vec![part(1, Curve::Nurbs(Arc::new(n)), [0.0, 2.0])],
    };
    (path, len)
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
            Surface::Cylinder(_) => "cylinder",
            Surface::Torus(_) => "torus",
            Surface::Extrusion(_) => "extrusion",
            Surface::Revolution(_) => "revolution",
            Surface::Sweep(_) => "sweep",
            _ => "other",
        })
        .collect();
    out.sort_unstable();
    out
}

/// Round a bend, a tube's sides are a cylinder, a torus and a cylinder,
/// and a square bar's the planes and surfaces of revolution an extrude
/// and a revolve make. Both enclose the profile's area times the path's
/// length.
#[test]
fn round_a_bend_the_legs_are_analytic() {
    let len = 4.0 + FRAC_PI_2;
    let opts = AlongOptions::default();
    let (tube, _) = along(
        K,
        &profile(vec![circle(1, 1, (0.0, 0.0), 0.3)]),
        &bent(2.0),
        &opts,
    )
    .unwrap();
    assert_eq!(
        kinds(&tube),
        ["cylinder", "cylinder", "plane", "plane", "torus"]
    );
    let v = volume(&tube);
    assert!(close(v, PI * 0.09 * len, 2e-3), "{v}");
    let (bar, _) = along(K, &profile(vec![square(0.4)]), &bent(2.0), &opts).unwrap();
    assert_eq!(bar.faces().count(), 2 + 3 * 4);
    let v = volume(&bar);
    assert!(close(v, 0.16 * len, 2e-3), "{v}");
}

/// Along a spline, the profile's sides are sweeps; with the profile at
/// right angles to the path and centred on it, the volume is its area
/// times the path's length, twisted or not.
#[test]
#[cfg_attr(
    debug_assertions,
    ignore = "slow in a debug build: CI runs it in release"
)]
fn along_a_spline_the_sides_are_sweeps() {
    let (path, len) = wavy();
    for twist in [0.0, PI] {
        let opts = AlongOptions {
            twist,
            ..AlongOptions::default()
        };
        let (b, _) = along(K, &profile(vec![square(0.3)]), &path, &opts).unwrap();
        assert_eq!(
            kinds(&b),
            ["plane", "plane", "sweep", "sweep", "sweep", "sweep"]
        );
        let v = volume(&b);
        assert!(
            close(v, 0.09 * len, 2e-3),
            "{twist}: {v} against {}",
            0.09 * len
        );
    }
    let (b, _) = along(
        K,
        &profile(vec![circle(1, 1, (0.0, 0.0), 0.2)]),
        &path,
        &AlongOptions::default(),
    )
    .unwrap();
    let v = volume(&b);
    assert!(close(v, PI * 0.04 * len, 2e-3), "{v}");
}

/// A square growing to twice its side along a line is a frustum.
#[test]
#[cfg_attr(
    debug_assertions,
    ignore = "slow in a debug build: CI runs it in release"
)]
fn scaled_along_a_line_is_a_frustum() {
    let path = Path {
        segments: vec![part(
            1,
            line(Point3::ORIGIN, Vector3::new(1.0, 0.0, 0.0)),
            [0.0, 3.0],
        )],
    };
    let opts = AlongOptions {
        scale: 2.0,
        ..AlongOptions::default()
    };
    let (b, _) = along(K, &profile(vec![square(0.5)]), &path, &opts).unwrap();
    let v = volume(&b);
    let want = 0.25 * 3.0 * (1.0 + 2.0 + 4.0) / 3.0;
    assert!(close(v, want, 2e-3), "{v} against {want}");
}

/// Holes go along too: a square with a round hole and a square one round
/// the bend, and along the spline.
#[test]
#[cfg_attr(
    debug_assertions,
    ignore = "slow in a debug build: CI runs it in release"
)]
fn holes_go_along() {
    let h = 0.08;
    let small = |key: u128, first: u128, c: (f64, f64)| {
        let pts = [
            (c.0 - h, c.1 - h),
            (c.0 - h, c.1 + h),
            (c.0 + h, c.1 + h),
            (c.0 + h, c.1 - h),
        ];
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
            key: LoopKey(key),
            segments,
        }
    };
    let p = profile(vec![
        square(0.6),
        circle(2, 50, (-0.15, 0.0), 0.08),
        small(3, 60, (0.15, 0.0)),
    ]);
    let area = 0.36 - PI * 0.0064 - 4.0 * h * h;
    let opts = AlongOptions::default();
    let (b, _) = along(K, &p, &bent(2.0), &opts).unwrap();
    let v = volume(&b);
    assert!(close(v, area * (4.0 + FRAC_PI_2), 2e-3), "{v}");
    let (path, len) = wavy();
    let p = profile(vec![square(0.3), circle(2, 50, (-0.07, 0.0), 0.04)]);
    let (b, _) = along(K, &p, &path, &opts).unwrap();
    let v = volume(&b);
    assert!(close(v, (0.09 - PI * 0.0016) * len, 2e-3), "{v}");
}

/// A binormal law keeps `z` in the profile's plane: up a ramp, the
/// profile stays upright.
#[test]
#[cfg_attr(
    debug_assertions,
    ignore = "slow in a debug build: CI runs it in release"
)]
fn a_binormal_law_keeps_its_direction() {
    let k = KnotVector::clamped(2, &[0.0, 1.0], &[3, 3]).unwrap();
    let pts = vec![
        Point3::ORIGIN,
        Point3::new(2.0, 0.0, 0.0),
        Point3::new(3.0, 2.0, 1.0),
    ];
    let path = Path {
        segments: vec![part(
            1,
            Curve::Nurbs(Arc::new(NurbsCurve::new(k, pts, None).unwrap())),
            [0.0, 1.0],
        )],
    };
    let z = UnitVector3::exact(Vector3::new(0.0, 0.0, 1.0)).unwrap();
    let opts = AlongOptions {
        law: FrameLaw::Binormal(z),
        ..AlongOptions::default()
    };
    let (b, _) = along(K, &profile(vec![square(0.2)]), &path, &opts).unwrap();
    assert!(volume(&b) > 0.0);
}

fn refusal(r: Result<(Body, ryukei::name::Journal), Error>) -> SweepError {
    match r {
        Err(Error::InvalidSweep(e)) => e,
        r => panic!("{:?}", r.map(|_| ())),
    }
}

#[test]
fn refusals() {
    let opts = AlongOptions::default();
    let round = profile(vec![circle(1, 1, (0.0, 0.0), 0.3)]);
    // The profile's plane along the path.
    let sideways = Path {
        segments: vec![part(
            1,
            line(Point3::ORIGIN, Vector3::new(0.0, 1.0, 0.0)),
            [0.0, 1.0],
        )],
    };
    assert_eq!(
        refusal(along(K, &round, &sideways, &opts)),
        SweepError::ProfileAlongPath
    );
    // Wider than the bend.
    let wide = profile(vec![circle(1, 1, (0.0, 0.0), 1.5)]);
    assert_eq!(
        refusal(along(K, &wide, &bent(2.0), &opts)),
        SweepError::PathTooCurved {
            segment: SegmentKey(2)
        }
    );
    // A binormal along the path's start.
    let x = UnitVector3::exact(Vector3::new(1.0, 0.0, 0.0)).unwrap();
    let (path, _) = wavy();
    let binormal = AlongOptions {
        law: FrameLaw::Binormal(x),
        ..opts
    };
    assert_eq!(
        refusal(along(K, &round, &path, &binormal)),
        SweepError::FrameUndefined {
            segment: SegmentKey(1)
        }
    );
    // Scales that aren't, and a twist that isn't finite.
    for scale in [0.0, -1.0, f64::NAN] {
        let o = AlongOptions { scale, ..opts };
        assert_eq!(
            refusal(along(K, &round, &bent(2.0), &o)),
            SweepError::BadScale
        );
    }
    let o = AlongOptions {
        twist: f64::INFINITY,
        ..opts
    };
    assert!(matches!(
        along(K, &round, &bent(2.0), &o),
        Err(Error::NonFinite)
    ));
    // A path coming back through itself: along x, three quarters round,
    // then down across the start.
    let f = Frame::new(
        Point3::new(0.0, 1.0, 0.0),
        Vector3::new(0.0, 0.0, 1.0),
        Vector3::new(1.0, 0.0, 0.0),
    )
    .unwrap();
    let looped = Path {
        segments: vec![
            part(
                1,
                line(Point3::new(-3.0, 0.0, 0.0), Vector3::new(1.0, 0.0, 0.0)),
                [0.0, 3.0],
            ),
            part(
                2,
                Curve::Circle(Circle::new(f, 1.0).unwrap()),
                [-FRAC_PI_2, PI],
            ),
            part(
                3,
                line(Point3::new(-1.0, 1.0, 0.0), Vector3::new(0.0, -1.0, 0.0)),
                [0.0, 3.0],
            ),
        ],
    };
    let frame = Frame::new(
        Point3::new(-3.0, 0.0, 0.0),
        Vector3::new(1.0, 0.0, 0.0),
        Vector3::new(0.0, 1.0, 0.0),
    )
    .unwrap();
    let thin = ValidProfile::new(Profile {
        frame,
        loops: vec![circle(1, 1, (0.0, 0.0), 0.2)],
    })
    .unwrap();
    assert!(matches!(
        refusal(along(K, &thin, &looped, &opts)),
        SweepError::SelfIntersecting { .. }
    ));
}

/// Names come from the keys: a longer path, a longer last leg, a twist
/// or a scale rename nothing, and the journal creates every name once.
#[test]
#[cfg_attr(
    debug_assertions,
    ignore = "slow in a debug build: CI runs it in release"
)]
fn names_survive_edits() {
    let p = profile(vec![square(0.4)]);
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
    let (a, journal) = along(K, &p, &bent(2.0), &AlongOptions::default()).unwrap();
    let all = names(&a);
    let mut dedup = all.clone();
    dedup.dedup();
    assert_eq!(dedup.len(), all.len(), "names are unique");
    let created: Vec<_> = journal
        .events()
        .iter()
        .filter_map(|e| match e {
            Event::Created(n) => Some(*n),
            _ => None,
        })
        .collect();
    assert_eq!(created.len(), all.len());
    let (b, _) = along(K, &p, &bent(3.5), &AlongOptions::default()).unwrap();
    assert_eq!(names(&b), all);
    let twisted = AlongOptions {
        twist: 0.5,
        scale: 1.2,
        ..AlongOptions::default()
    };
    let (c, _) = along(K, &p, &bent(2.0), &twisted).unwrap();
    assert_eq!(names(&c), all);
}

/// A rectangle 2 by 3 with corners rounded to radius 0.5,
/// counterclockwise in the xy plane from the origin, `10 + π` long.
fn rounded_rectangle() -> Path {
    let x = Vector3::new(1.0, 0.0, 0.0);
    let y = Vector3::new(0.0, 1.0, 0.0);
    let quarter = |key: u128, c: (f64, f64), a: f64| {
        let f = Frame::new(Point3::new(c.0, c.1, 0.0), Vector3::new(0.0, 0.0, 1.0), x).unwrap();
        part(
            key,
            Curve::Circle(Circle::new(f, 0.5).unwrap()),
            [a, a + FRAC_PI_2],
        )
    };
    Path {
        segments: vec![
            part(1, line(Point3::ORIGIN, x), [0.0, 2.0]),
            quarter(2, (2.0, 0.5), -FRAC_PI_2),
            part(3, line(Point3::new(2.5, 0.5, 0.0), y), [0.0, 3.0]),
            quarter(4, (2.0, 3.5), 0.0),
            part(5, line(Point3::new(2.0, 4.0, 0.0), -x), [0.0, 2.0]),
            quarter(6, (0.0, 3.5), FRAC_PI_2),
            part(7, line(Point3::new(-0.5, 3.5, 0.0), -y), [0.0, 3.0]),
            quarter(8, (0.0, 0.5), PI),
        ],
    }
}

/// A crooked closed spline, and its length.
fn crooked() -> (Path, f64) {
    let k = KnotVector::periodic(3, &[0.0, 1.0, 2.0, 3.0, 4.0], &[1, 1, 1, 1, 1]).unwrap();
    let pts = vec![
        Point3::new(-4.0, 0.0, 0.0),
        Point3::new(0.0, -4.0, 0.0),
        Point3::new(4.0, 0.0, 2.0),
        Point3::new(0.0, 5.0, 1.0),
    ];
    let n = NurbsCurve::new(k, pts, None).unwrap();
    let mut len = 0.0;
    let steps = 40_000;
    for i in 0..steps {
        let (a, b) = (
            4.0 * f64::from(i) / f64::from(steps),
            4.0 * f64::from(i + 1) / f64::from(steps),
        );
        len += n.point(a).distance(n.point(b));
    }
    let path = Path {
        segments: vec![part(1, Curve::Nurbs(Arc::new(n)), [0.0, 4.0])],
    };
    (path, len)
}

/// A profile across a path's start.
fn across_start(path: &Path, loops: Vec<Loop>) -> ValidProfile {
    let s = &path.segments[0];
    let p = s.curve.point(s.range[0]);
    let t = s.curve.eval(s.range[0]).d1;
    let up = if t.z.abs() < 0.9 {
        Vector3::new(0.0, 0.0, 1.0)
    } else {
        Vector3::new(1.0, 0.0, 0.0)
    };
    ValidProfile::new(Profile {
        frame: Frame::new(p, t, up.cross(t)).unwrap(),
        loops,
    })
    .unwrap()
}

/// Round a rounded rectangle, tubes and bars close up: the sides are the
/// analytic ones, there are no caps, and the volume is the profile's area
/// times the path's length.
#[test]
#[cfg_attr(
    debug_assertions,
    ignore = "slow in a debug build: CI runs it in release"
)]
fn round_a_closed_path() {
    let path = rounded_rectangle();
    let len = 10.0 + PI;
    let opts = AlongOptions::default();
    let tube = across_start(&path, vec![circle(1, 1, (0.0, 0.0), 0.3)]);
    let (b, _) = along(K, &tube, &path, &opts).unwrap();
    assert_eq!(b.faces().count(), 8);
    let v = volume(&b);
    assert!(close(v, PI * 0.09 * len, 2e-3), "{v}");
    let bar = across_start(&path, vec![square(0.4)]);
    let (b, _) = along(K, &bar, &path, &opts).unwrap();
    assert_eq!(b.faces().count(), 32);
    let v = volume(&b);
    assert!(close(v, 0.16 * len, 2e-3), "{v}");
}

/// A hollow bar round a closed path: the hole is a cavity.
#[test]
#[cfg_attr(
    debug_assertions,
    ignore = "slow in a debug build: CI runs it in release"
)]
fn a_closed_path_round_a_hole_makes_a_cavity() {
    let path = rounded_rectangle();
    let p = across_start(&path, vec![square(0.4), circle(2, 50, (0.0, 0.0), 0.1)]);
    let (b, _) = along(K, &p, &path, &AlongOptions::default()).unwrap();
    assert_eq!(b.shells().count(), 2);
    let v = volume(&b);
    assert!(close(v, (0.16 - PI * 0.01) * (10.0 + PI), 2e-3), "{v}");
}

/// Round a whole circle a bar is a revolve's ring; round a crooked spline,
/// a tube is one face with no edges and a bar four bands, the frame
/// twisted back by its turn round the loop.
#[test]
#[cfg_attr(
    debug_assertions,
    ignore = "slow in a debug build: CI runs it in release"
)]
fn round_one_closed_curve() {
    let f = Frame::new(
        Point3::new(0.0, 2.0, 0.0),
        Vector3::new(0.0, 0.0, 1.0),
        Vector3::new(0.0, -1.0, 0.0),
    )
    .unwrap();
    let ring = Path {
        segments: vec![part(
            1,
            Curve::Circle(Circle::new(f, 2.0).unwrap()),
            [0.0, TAU],
        )],
    };
    let opts = AlongOptions::default();
    let (b, _) = along(K, &across_start(&ring, vec![square(0.4)]), &ring, &opts).unwrap();
    assert_eq!(b.faces().count(), 4);
    assert!(close(volume(&b), 0.16 * TAU * 2.0, 2e-3));
    let (path, len) = crooked();
    let (b, _) = along(
        K,
        &across_start(&path, vec![circle(1, 1, (0.0, 0.0), 0.2)]),
        &path,
        &opts,
    )
    .unwrap();
    assert_eq!((b.faces().count(), b.edges().count()), (1, 0));
    let v = volume(&b);
    assert!(close(v, PI * 0.04 * len, 2e-3), "{v}");
    let (b, _) = along(K, &across_start(&path, vec![square(0.3)]), &path, &opts).unwrap();
    assert_eq!(b.faces().count(), 4);
    let v = volume(&b);
    assert!(close(v, 0.09 * len, 2e-3), "{v}");
}

#[test]
fn closed_paths_need_whole_turns_and_no_scale() {
    let path = rounded_rectangle();
    let p = across_start(&path, vec![circle(1, 1, (0.0, 0.0), 0.3)]);
    for opts in [
        AlongOptions {
            twist: 1.0,
            ..AlongOptions::default()
        },
        AlongOptions {
            scale: 1.5,
            ..AlongOptions::default()
        },
    ] {
        assert_eq!(refusal(along(K, &p, &path, &opts)), SweepError::NotClosing);
    }
}

/// A sweep moved anywhere meshes as it did where it was made: its
/// bounds in axes it lies askew to are looser, and the mesher's estimate
/// halves the cells it would otherwise refuse the mesh over. This tube,
/// turned so, was once estimated at 7 × 10⁷ triangles against 6 × 10³.
#[test]
fn a_moved_sweep_meshes_as_it_was() {
    use ryukei::geom::Rigid;
    let s = 1.034_525_452_114_363_9;
    let k = KnotVector::clamped(3, &[0.0, 1.0, 2.0], &[4, 1, 4]).unwrap();
    let pts = vec![
        Point3::new(-s, 0.0, -0.2),
        Point3::new(-s / 2.0, 0.0, -0.2),
        Point3::new(0.0, 0.451_282_741_343_790_65, 0.0),
        Point3::new(s / 2.0, 0.2, 0.4),
        Point3::new(s, 0.0, 0.5),
    ];
    let path = Path {
        segments: vec![part(
            1,
            Curve::Nurbs(Arc::new(NurbsCurve::new(k, pts, None).unwrap())),
            [0.0, 2.0],
        )],
    };
    let start = ValidProfile::new(Profile {
        frame: Frame::new(
            Point3::new(-s, 0.0, -0.2),
            Vector3::new(1.0, 0.0, 0.0),
            Vector3::new(0.0, 1.0, 0.0),
        )
        .unwrap(),
        loops: vec![circle(1, 1, (0.0, 0.0), 0.15)],
    })
    .unwrap();
    let (tube, _) = along(K, &start, &path, &AlongOptions::default()).unwrap();
    let params = MeshParams::new(1e-2, 0.3);
    let here = tessellate(&tube, &params).unwrap();
    let axis = Vector3::new(
        0.649_630_727_172_793_2,
        0.587_694_000_248_602_9,
        0.755_507_613_145_529_2,
    );
    let place = Rigid::translation(Vector3::new(
        0.585_285_370_217_062_6,
        -0.059_118_452_082_203_12,
        -0.799_841_173_833_175_8,
    ))
    .then_after(&Rigid::rotation(
        axis.normalize().unwrap(),
        1.752_589_502_558_485,
    ));
    let moved = tube.transformed(&place).unwrap();
    let m = tessellate(&moved, &params).unwrap();
    assert!(m.faults.is_empty(), "{:?}", m.faults);
    let (a, b) = (
        here.welded_triangles().count(),
        m.welded_triangles().count(),
    );
    assert!(b <= 2 * a && a <= 2 * b, "{a} against {b}");
}
