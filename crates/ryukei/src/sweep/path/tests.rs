use super::*;
use crate::geom::{Circle, Frame, KnotVector, Line};
use core::f64::consts::{FRAC_PI_2, PI, TAU};

fn seg(key: u128, curve: Curve, range: [f64; 2]) -> PathSegment {
    PathSegment {
        key: SegmentKey(key),
        curve,
        range,
    }
}

fn line(from: Point3, dir: Vector3) -> Curve {
    Curve::Line(Line::new(from, dir).unwrap())
}

/// A quarter turn of radius `r` about `centre` (in the xy plane), starting
/// at angle `a` counterclockwise.
fn quarter(key: u128, centre: Point3, r: f64, a: f64) -> PathSegment {
    let f = Frame::new(
        centre,
        Vector3::new(0.0, 0.0, 1.0),
        Vector3::new(1.0, 0.0, 0.0),
    )
    .unwrap();
    seg(
        key,
        Curve::Circle(Circle::new(f, r).unwrap()),
        [a, a + FRAC_PI_2],
    )
}

/// Up `x` by 2, a quarter turn of radius 1 towards `y`, up `y` by 2.
pub(crate) fn bent() -> Path {
    Path {
        segments: vec![
            seg(
                1,
                line(Point3::ORIGIN, Vector3::new(1.0, 0.0, 0.0)),
                [0.0, 2.0],
            ),
            quarter(2, Point3::new(2.0, 1.0, 0.0), 1.0, -FRAC_PI_2),
            seg(
                3,
                line(Point3::new(3.0, 1.0, 0.0), Vector3::new(0.0, 1.0, 0.0)),
                [0.0, 2.0],
            ),
        ],
    }
}

/// A square of side 2 with corners rounded to radius 1, counterclockwise
/// in the xy plane from (1, 0).
pub(crate) fn rounded_square() -> Path {
    let x = Vector3::new(1.0, 0.0, 0.0);
    let y = Vector3::new(0.0, 1.0, 0.0);
    Path {
        segments: vec![
            seg(1, line(Point3::new(1.0, 0.0, 0.0), x), [0.0, 2.0]),
            quarter(2, Point3::new(3.0, 1.0, 0.0), 1.0, -FRAC_PI_2),
            seg(3, line(Point3::new(4.0, 1.0, 0.0), y), [0.0, 2.0]),
            quarter(4, Point3::new(3.0, 3.0, 0.0), 1.0, 0.0),
            seg(5, line(Point3::new(3.0, 4.0, 0.0), -x), [0.0, 2.0]),
            quarter(6, Point3::new(1.0, 3.0, 0.0), 1.0, FRAC_PI_2),
            seg(7, line(Point3::new(0.0, 3.0, 0.0), -y), [0.0, 2.0]),
            quarter(8, Point3::new(1.0, 1.0, 0.0), 1.0, PI),
        ],
    }
}

#[test]
fn chains_open_and_closed() {
    let p = ValidPath::new(&bent()).unwrap();
    assert!(!p.closed);
    let lengths = p.lengths();
    assert!((lengths[0] - 2.0).abs() <= 1e-12 && (lengths[1] - FRAC_PI_2).abs() <= 1e-12);
    let p = ValidPath::new(&rounded_square()).unwrap();
    assert!(p.closed);
    let total: f64 = p.lengths().iter().sum();
    assert!((total - (8.0 + TAU)).abs() <= 1e-11);
    // A whole circle is closed on its own.
    let f = Frame::WORLD;
    let circle = Path {
        segments: vec![seg(
            1,
            Curve::Circle(Circle::new(f, 2.0).unwrap()),
            [0.5, 0.5 + TAU],
        )],
    };
    assert!(ValidPath::new(&circle).unwrap().closed);
}

/// A clamped spline closing on itself tangentially is made periodic, with
/// the same points at the same parameters.
#[test]
fn a_closed_clamped_spline_is_made_periodic() {
    let k = KnotVector::clamped(3, &[0.0, 1.0, 2.0], &[4, 2, 4]).unwrap();
    let pts = vec![
        Point3::new(0.0, 0.0, 0.0),
        Point3::new(1.0, 0.0, 0.0),
        Point3::new(2.0, 1.0, 0.0),
        Point3::new(-1.0, 1.5, 0.5),
        Point3::new(-1.0, 0.0, 0.0),
        Point3::new(0.0, 0.0, 0.0),
    ];
    let n = NurbsCurve::new(k, pts, None).unwrap();
    let c = Curve::Nurbs(Arc::new(n.clone()));
    let p = ValidPath::new(&Path {
        segments: vec![seg(1, c, [0.0, 2.0])],
    })
    .unwrap();
    assert!(p.closed);
    let s = &p.segments[0];
    assert_eq!(s.curve.period(), Some(2.0));
    for t in [0.0, 0.3, 1.0, 1.7] {
        assert!(s.curve.point(t).distance(n.point(t)) <= 1e-12);
    }
}

#[test]
fn faults_are_refused_in_order() {
    let x = Vector3::new(1.0, 0.0, 0.0);
    let key = |e: Error| match e {
        Error::InvalidSweep(e) => e,
        e => panic!("{e:?}"),
    };
    assert_eq!(
        key(ValidPath::new(&Path { segments: vec![] }).unwrap_err()),
        SweepError::EmptyPath
    );
    let one = |s: PathSegment| ValidPath::new(&Path { segments: vec![s] });
    for range in [[1.0, 1.0], [1.0, 0.0], [0.0, 1e-9]] {
        assert_eq!(
            key(one(seg(9, line(Point3::ORIGIN, x), range)).unwrap_err()),
            SweepError::BadPathSegment {
                segment: SegmentKey(9)
            }
        );
    }
    let mut p = bent();
    p.segments[2].key = SegmentKey(1);
    assert_eq!(
        key(ValidPath::new(&p).unwrap_err()),
        SweepError::BadPathSegment {
            segment: SegmentKey(1)
        }
    );
    // A kink, and a gap.
    let mut p = bent();
    p.segments[2].curve = line(Point3::new(3.0, 1.0, 0.0), Vector3::new(0.1, 1.0, 0.0));
    assert_eq!(
        key(ValidPath::new(&p).unwrap_err()),
        SweepError::PathNotG1 { at: SegmentKey(3) }
    );
    let mut p = bent();
    p.segments[2].curve = line(
        Point3::new(3.0, 1.0 + 1e-6, 0.0),
        Vector3::new(0.0, 1.0, 0.0),
    );
    assert_eq!(
        key(ValidPath::new(&p).unwrap_err()),
        SweepError::PathNotG1 { at: SegmentKey(3) }
    );
    // A loop closing at a corner.
    let k = KnotVector::clamped(3, &[0.0, 1.0], &[4, 4]).unwrap();
    let pts = vec![
        Point3::ORIGIN,
        Point3::new(2.0, 0.0, 0.0),
        Point3::new(0.0, 2.0, 0.0),
        Point3::ORIGIN,
    ];
    let drop = Curve::Nurbs(Arc::new(NurbsCurve::new(k, pts, None).unwrap()));
    assert_eq!(
        key(one(seg(5, drop, [0.0, 1.0])).unwrap_err()),
        SweepError::PathNotG1 { at: SegmentKey(5) }
    );
    // A whole circle among other segments.
    let mut p = bent();
    p.segments[1].range = [0.0, TAU];
    assert_eq!(
        key(ValidPath::new(&p).unwrap_err()),
        SweepError::BadPathSegment {
            segment: SegmentKey(2)
        }
    );
}
