//! Random profiles swept: every result passes the checker and the
//! self-intersection check, meshes with no faults and encloses the volume
//! the profile's area says it should.

use core::f64::consts::TAU;
use proptest::prelude::*;
use ryukei::check::{Fault, check, check_intersections};
use ryukei::geom::{
    Axis, Circle2, Curve2, Frame, Frame2, Line2, Point2, Point3, Rigid, Surface, Vector2, Vector3,
};
use ryukei::mesh::{MeshParams, check_mesh, tessellate};
use ryukei::name::OpKey;
use ryukei::sweep::{Loop, LoopKey, Profile, Segment, SegmentKey, ValidProfile, extrude, revolve};
use ryukei::topo::Body;
use ryukei_testkit::mesh::volume_and_area;

/// A rectangle or a circle, as a loop.
#[derive(Clone, Debug)]
enum Shape {
    Rect { x: f64, y: f64, w: f64, h: f64 },
    Circle { x: f64, y: f64, r: f64 },
}

impl Shape {
    fn to_loop(&self, key: u128) -> Loop {
        let segments = match *self {
            Shape::Rect { x, y, w, h } => {
                let pts = [(x, y), (x + w, y), (x + w, y + h), (x, y + h)];
                (0..4)
                    .map(|i| {
                        let (a, b) = (pts[i], pts[(i + 1) % 4]);
                        let (p, q) = (Point2::new(a.0, a.1), Point2::new(b.0, b.1));
                        Segment {
                            key: SegmentKey(key * 16 + i as u128),
                            curve: Curve2::Line2(Line2::through(p, q).unwrap()),
                            t0: 0.0,
                            t1: p.distance(q),
                        }
                    })
                    .collect()
            }
            Shape::Circle { x, y, r } => {
                let f = Frame2::new(Point2::new(x, y), Vector2::new(1.0, 0.0), true).unwrap();
                vec![Segment {
                    key: SegmentKey(key * 16),
                    curve: Curve2::Circle2(Circle2::new(f, r).unwrap()),
                    t0: 0.0,
                    t1: TAU,
                }]
            }
        };
        Loop {
            key: LoopKey(key),
            segments,
        }
    }

    /// Area and the centroid's x.
    fn area(&self) -> (f64, f64) {
        match *self {
            Shape::Rect { x, w, h, .. } => (w * h, x + w / 2.0),
            Shape::Circle { x, r, .. } => (core::f64::consts::PI * r * r, x),
        }
    }
}

/// An outer shape in the box `[1, 4] × [-2, 2]` and maybe a hole well
/// inside it.
fn lump() -> impl Strategy<Value = (Shape, Option<Shape>)> {
    let rect = (1.0f64..1.5, -2.0f64..-1.0, 1.5f64..2.5, 2.0f64..3.0)
        .prop_map(|(x, y, w, h)| Shape::Rect { x, y, w, h });
    let circle =
        (2.2f64..2.8, -0.5f64..0.5, 1.0f64..1.2).prop_map(|(x, y, r)| Shape::Circle { x, y, r });
    (prop_oneof![rect, circle], any::<bool>(), 0u8..2).prop_map(|(outer, hole, kind)| {
        let (cx, cy) = match outer {
            Shape::Rect { x, y, w, h } => (x + w / 2.0, y + h / 2.0),
            Shape::Circle { x, y, .. } => (x, y),
        };
        let inner = match kind {
            0 => Shape::Circle {
                x: cx,
                y: cy,
                r: 0.4,
            },
            _ => Shape::Rect {
                x: cx - 0.3,
                y: cy - 0.3,
                w: 0.6,
                h: 0.5,
            },
        };
        (outer, hole.then_some(inner))
    })
}

fn frame(tilt: f64, turn: f64) -> Frame {
    let r = Rigid::rotation(Vector3::new(1.0, 2.0, 0.5).normalize().unwrap(), turn);
    let z = r.apply_vector(Vector3::new(tilt, 0.0, 1.0));
    let x = r.apply_vector(Vector3::new(1.0, 0.0, -tilt));
    Frame::new(Point3::new(0.5, -1.0, 2.0), z, x).unwrap()
}

/// The self-intersection check passes, but for pairs it can't check
/// because a face is on a surface with no implicit form (a swept or NURBS
/// surface): only analytic pairs are intersected.
fn self_sound(b: &Body) {
    for f in check_intersections(b) {
        let Fault::CannotCheck { a, b: c } = f else {
            panic!("{f:?}")
        };
        let analytic = |x| {
            matches!(
                b.surface(b.face(x).unwrap().surface).unwrap(),
                Surface::Plane(_)
                    | Surface::Cylinder(_)
                    | Surface::Cone(_)
                    | Surface::Sphere(_)
                    | Surface::Torus(_)
            )
        };
        assert!(!analytic(a) || !analytic(c), "{f:?}");
    }
}

fn volume(b: &Body) -> f64 {
    assert_eq!(check(b), []);
    self_sound(b);
    let m = tessellate(b, &MeshParams::new(2e-3, 0.5)).unwrap();
    assert!(m.faults.is_empty(), "{:?}", m.faults);
    check_mesh(b, &m).unwrap();
    volume_and_area(&m).0
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(16))]

    #[test]
    fn extrudes_hold_their_volume(
        lumps in proptest::collection::vec(lump(), 1..3),
        tilt in -0.5f64..0.5,
        turn in 0.0f64..6.0,
        lean in -0.8f64..0.8,
        start in -1.0f64..0.5,
        span in 0.2f64..2.0,
    ) {
        let f = frame(tilt, turn);
        let mut loops = Vec::new();
        let mut area = 0.0;
        for (i, (outer, hole)) in lumps.iter().enumerate() {
            // Lumps side by side along the profile's y.
            let shift = |s: &Shape| match *s {
                Shape::Rect { x, y, w, h } => Shape::Rect { x, y: y + 5.0 * i as f64, w, h },
                Shape::Circle { x, y, r } => Shape::Circle { x, y: y + 5.0 * i as f64, r },
            };
            loops.push(shift(outer).to_loop(10 * i as u128 + 1));
            area += outer.area().0;
            if let Some(h) = hole {
                loops.push(shift(h).to_loop(10 * i as u128 + 2));
                area -= h.area().0;
            }
        }
        let p = ValidProfile::new(Profile { frame: f, loops }).unwrap();
        let dir = f.z().get() + f.x().get() * lean;
        let (b, _) = extrude(OpKey(3), &p, dir, start, start + span).unwrap();
        let along = f.z().get().dot(dir) / dir.length();
        let want = area * span * along;
        let got = volume(&b);
        prop_assert!((got - want).abs() <= 5e-3 * want, "{got} vs {want}");
    }

    #[test]
    fn revolves_hold_pappus(
        (outer, hole) in lump(),
        tilt in -0.5f64..0.5,
        turn in 0.0f64..6.0,
        start in -3.0f64..3.0,
        span in 0.3f64..6.0,
    ) {
        let f = frame(tilt, turn);
        let mut loops = vec![outer.to_loop(1)];
        let (a, cx) = outer.area();
        let mut moment = a * cx;
        if let Some(h) = &hole {
            loops.push(h.to_loop(2));
            let (ha, hx) = h.area();
            moment -= ha * hx;
        }
        let p = ValidProfile::new(Profile { frame: f, loops }).unwrap();
        // The profile's y axis: profile x is the distance from it.
        let axis = Axis::new(f.origin(), f.y().get()).unwrap();
        let (b, _) = revolve(OpKey(3), &p, axis, start, start + span).unwrap();
        let want = moment * span;
        let got = volume(&b);
        prop_assert!((got - want).abs() <= 5e-3 * want, "{got} vs {want}");
    }
}
