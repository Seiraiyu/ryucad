//! Global offsets and shells (design 299, plan 14.D): where the local
//! build refuses, the envelope split against itself and kept by distance.
//! Every result passes the checker and the self-intersection check,
//! meshes without faults, and encloses the volume its shape gives.

use core::f64::consts::PI;
use ryukei::check::{check, check_intersections};
use ryukei::geom::{Line2, Point2, Rigid, Vector3};
use ryukei::mesh::{MeshParams, check_mesh, tessellate};
use ryukei::name::{FaceRole, Name, OpKey, Role};
use ryukei::offset::{OffsetSpec, offset_body};
use ryukei::primitive;
use ryukei::sweep::{Loop, LoopKey, Profile, Segment, SegmentKey, ValidProfile, extrude};
use ryukei::topo::Body;
use ryukei::{Error, OffsetError};

const K: OpKey = OpKey(0xc0);
const OFF: OpKey = OpKey(0xc1);

/// The volume a sound body's mesh encloses, after both checks.
fn volume(b: &Body, chord: f64) -> f64 {
    assert_eq!(check(b), []);
    assert_eq!(check_intersections(b), []);
    let m = tessellate(b, &MeshParams::new(chord, 0.1)).unwrap();
    assert!(m.faults.is_empty(), "{:?}", m.faults);
    check_mesh(b, &m).unwrap();
    m.welded_triangles()
        .map(|t| {
            let [a, b, c] = t.map(|i| m.positions[i as usize].to_vector());
            a.dot(b.cross(c)) / 6.0
        })
        .sum()
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

fn face(key: OpKey, role: FaceRole) -> Name {
    Name::derive(key, Role::Face(role), &[])
}

/// A primitive's edge between two faces.
fn edge(key: OpKey, a: FaceRole, b: FaceRole) -> Name {
    let mut f = [face(key, a), face(key, b)];
    f.sort();
    Name::derive(key, Role::Edge, &f)
}

const SIDES: [FaceRole; 6] = [
    FaceRole::PlusX,
    FaceRole::MinusX,
    FaceRole::PlusY,
    FaceRole::MinusY,
    FaceRole::PlusZ,
    FaceRole::MinusZ,
];

/// A box with every edge rounded to `r`.
fn rounded(a: f64, b: f64, c: f64, r: f64) -> Body {
    let (bx, _) = primitive::block(K, a, b, c, &Rigid::IDENTITY).unwrap();
    let mut blends = Vec::new();
    for (i, &x) in SIDES.iter().enumerate() {
        for &y in &SIDES[i + 1..] {
            if i % 2 == 0 && y == SIDES[i + 1] {
                continue;
            }
            blends.push(ryukei::blend::EdgeBlend {
                edge: edge(K, x, y),
                kind: ryukei::blend::BlendKind::Fillet { radius: r },
            });
        }
    }
    ryukei::blend::blend(OpKey(0xc5), &bx, &blends).unwrap().0
}

/// A rounded box shrunk past its radius: the fillets and corner balls
/// vanish, and what is left is the core shrunk, sharp.
#[test]
fn a_rounded_box_shrunk_past_its_radius_is_sharp() {
    let b = rounded(1.0, 0.6, 0.4, 0.1);
    let (o, _) = offset_body(OFF, &b, &by(-0.15)).unwrap();
    assert_eq!(o.counts().faces, 6);
    near(volume(&o, 2e-4), 0.7 * 0.3 * 0.1, 1e-9);
}
/// A tray whose upright edges are rounded smaller than its wall, shelled
/// open at its top: the fillets' inner offsets vanish, the fills about
/// their rims are spindles, and the inner corners are sharp.
#[test]
#[cfg_attr(
    debug_assertions,
    ignore = "minutes in a debug build (75 s on cox-worker-1); full.yml runs it in release"
)]
fn a_tray_shelled_past_its_fillets() {
    use ryukei::offset::{ShellSpec, shell};
    let (a, bb, c, r, t) = (1.0, 0.6, 0.4, 0.05, 0.08);
    let (bx, _) = primitive::block(K, a, bb, c, &Rigid::IDENTITY).unwrap();
    let upright = [
        (FaceRole::PlusX, FaceRole::PlusY),
        (FaceRole::PlusX, FaceRole::MinusY),
        (FaceRole::MinusX, FaceRole::PlusY),
        (FaceRole::MinusX, FaceRole::MinusY),
    ];
    let blends: Vec<ryukei::blend::EdgeBlend> = upright
        .iter()
        .map(|&(x, y)| ryukei::blend::EdgeBlend {
            edge: edge(K, x, y),
            kind: ryukei::blend::BlendKind::Fillet { radius: r },
        })
        .collect();
    let (b, _) = ryukei::blend::blend(OpKey(0xc8), &bx, &blends).unwrap();
    let spec = ShellSpec {
        thickness: t,
        outward: false,
        open: vec![face(K, FaceRole::PlusZ)],
        faces: Vec::new(),
    };
    let (o, _) = shell(OpKey(0xc9), &b, &spec).unwrap();
    let plan = (a * bb - (4.0 - PI) * r * r) * c;
    let want = plan - (a - 2.0 * t) * (bb - 2.0 * t) * (c - t);
    near(volume(&o, 2e-4), want, 1e-4);
}

fn refusal(r: Result<(Body, ryukei::name::Journal), Error>) -> OffsetError {
    match r {
        Err(Error::InvalidOffset(e)) => e,
        other => panic!("not an offset refusal: {:?}", other.map(|_| ())),
    }
}

/// Steiner's formula for a box `a × b × c` grown by `d`.
fn steiner_box(a: f64, b: f64, c: f64, d: f64) -> f64 {
    a * b * c
        + 2.0 * (a * b + b * c + c * a) * d
        + PI * (a + b + c) * d * d
        + 4.0 / 3.0 * PI * d * d * d
}

/// A prism of a polygon `h` tall.
fn prism(pts: &[(f64, f64)], h: f64) -> Body {
    let n = pts.len();
    let segments = (0..n)
        .map(|i| {
            let (p, q) = (pts[i], pts[(i + 1) % n]);
            let (p, q) = (Point2::new(p.0, p.1), Point2::new(q.0, q.1));
            Segment {
                key: SegmentKey(10 + i as u128),
                curve: ryukei::geom::Curve2::Line2(Line2::through(p, q).unwrap()),
                t0: 0.0,
                t1: p.distance(q),
            }
        })
        .collect();
    let profile = ValidProfile::new(Profile {
        frame: ryukei::geom::Frame::WORLD,
        loops: vec![Loop {
            key: LoopKey(1),
            segments,
        }],
    })
    .unwrap();
    extrude(K, &profile, Vector3::new(0.0, 0.0, 1.0), 0.0, h)
        .unwrap()
        .0
}

/// Shrunk past half its thickness, or past its radius, nothing is left.
#[test]
#[cfg_attr(
    debug_assertions,
    ignore = "minutes in a debug build (68 s on cox-worker-1); full.yml runs it in release"
)]
fn bodies_shrunk_away_are_empty() {
    let (b, _) = primitive::block(K, 1.0, 0.6, 0.4, &Rigid::IDENTITY).unwrap();
    assert_eq!(
        refusal(offset_body(OFF, &b, &by(-0.25))),
        OffsetError::Empty
    );
    let (c, _) = primitive::cylinder(K, 0.3, 2.0, &Rigid::IDENTITY).unwrap();
    assert_eq!(
        refusal(offset_body(OFF, &c, &by(-0.35))),
        OffsetError::Empty
    );
    let (s, _) = primitive::sphere(K, 0.3, &Rigid::IDENTITY).unwrap();
    assert_eq!(refusal(offset_body(OFF, &s, &by(-0.4))), OffsetError::Empty);
}

/// A U whose slot is narrower than twice the distance, grown: the slot's
/// walls' offsets cross, and it closes.
#[test]
#[cfg_attr(
    debug_assertions,
    ignore = "minutes in a debug build (260 s on cox-worker-1); full.yml runs it in release"
)]
fn a_slot_grown_shut() {
    let (h, d) = (0.5, 0.6);
    let u = prism(
        &[
            (0.0, 0.0),
            (3.0, 0.0),
            (3.0, 2.0),
            (2.0, 2.0),
            (2.0, 1.0),
            (1.0, 1.0),
            (1.0, 2.0),
            (0.0, 2.0),
        ],
        h,
    );
    let (o, _) = offset_body(OFF, &u, &by(d)).unwrap();
    let v = volume(&o, 2e-4);
    // Within the 3 × 2 box grown, past the 3 × 1 base grown.
    assert!(v < steiner_box(3.0, 2.0, h, d), "{v}");
    assert!(v > steiner_box(3.0, 1.0, h, d), "{v}");
    // The slot's middle is inside now.
    assert_eq!(
        ryukei::intersect::point_in_body(&o, ryukei::geom::Point3::new(1.5, 1.5, 0.25)),
        Ok(ryukei::intersect::Inclusion::Inside)
    );
}

/// A box overhanging another: a vertex of four edges, convex and concave
/// in turn. Grown, it is the union of the two boxes grown.
#[test]
#[cfg_attr(
    debug_assertions,
    ignore = "minutes in a debug build (135 s on cox-worker-1); full.yml runs it in release"
)]
fn a_saddle_grown() {
    use ryukei::boolean::{BooleanOp, Tool, ToolKey, boolean};
    let a = primitive::block(OpKey(1), 2.0, 2.0, 1.0, &Rigid::IDENTITY)
        .unwrap()
        .0;
    let bb = primitive::block(OpKey(2), 2.0, 2.0, 1.0, &Rigid::IDENTITY)
        .unwrap()
        .0;
    let place = Rigid::translation(Vector3::new(1.0, 1.0, 1.0));
    let tool = Tool {
        key: ToolKey(1),
        body: &bb,
        place,
    };
    let u = boolean(OpKey(3), BooleanOp::Unite, &a, &[tool]).unwrap().0;
    let d = 0.1;
    let (o, _) = offset_body(OFF, &u, &by(d)).unwrap();
    let ga = offset_body(OpKey(7), &a, &by(d)).unwrap().0;
    let gb = offset_body(OpKey(8), &bb, &by(d)).unwrap().0;
    let tool = Tool {
        key: ToolKey(1),
        body: &gb,
        place,
    };
    let want = boolean(OpKey(9), BooleanOp::Unite, &ga, &[tool]).unwrap().0;
    near(volume(&o, 2e-4), volume(&want, 2e-4), 1e-4);
}

/// A step thinner than the distance, shrunk: the step's face vanishes
/// and the inset rounds the reflex corner (built locally or globally,
/// the same answer).
#[test]
fn a_thin_step_shrunk_past_it() {
    let (h, s, d) = (0.6, 0.05, 0.1);
    let b = prism(
        &[
            (0.0, 0.0),
            (2.0, 0.0),
            (2.0, 1.0),
            (1.0, 1.0),
            (1.0, 1.0 + s),
            (0.0, 1.0 + s),
        ],
        h,
    );
    let (o, _) = offset_body(OFF, &b, &by(-d)).unwrap();
    // The inset profile: `1 + s - d` high left of the reflex corner's
    // circle (radius `d` about (1, 1)), `1 - d` right of it, from `d`.
    let a = (d * d - (d - s) * (d - s)).sqrt();
    let seg = |u: f64| 0.5 * u * (d * d - u * u).sqrt() + 0.5 * d * d * ryukei::math::asin(u / d);
    let top = (1.0 - a - d) * (1.0 + s - d) + (a - seg(a)) + (1.0 - d) * (1.0 - d);
    let area = top - d * (2.0 - 2.0 * d);
    near(volume(&o, 2e-4), area * (h - 2.0 * d), 1e-4);
}

/// A plate with filleted holes, shelled inwards past the fillets' radius
/// with its bottom open: the rims' inner offsets vanish and the inner
/// corners are sharp.
#[test]
#[cfg_attr(
    debug_assertions,
    ignore = "minutes in a debug build (226 s on cox-worker-1); full.yml runs it in release"
)]
fn filleted_holes_shelled_past_their_radius() {
    use ryukei::boolean::{BooleanOp, Tool, ToolKey, boolean};
    use ryukei::offset::{ShellSpec, shell};
    let n = 3;
    let side = 2.0 * n as f64;
    let plate = primitive::block(OpKey(1), side, side, 1.0, &Rigid::IDENTITY)
        .unwrap()
        .0;
    let rod = primitive::cylinder(OpKey(2), 0.4, 3.0, &Rigid::IDENTITY)
        .unwrap()
        .0;
    let places: Vec<Rigid> = (0..n * n)
        .map(|k| {
            let (i, j) = (k / n, k % n);
            Rigid::translation(Vector3::new(
                1.0 + 2.0 * i as f64,
                1.0 + 2.0 * j as f64,
                -1.0,
            ))
        })
        .collect();
    let tools: Vec<Tool> = places
        .iter()
        .enumerate()
        .map(|(k, &place)| Tool {
            key: ToolKey(1 + k as u128),
            body: &rod,
            place,
        })
        .collect();
    let p = boolean(OpKey(3), BooleanOp::Subtract, &plate, &tools)
        .unwrap()
        .0;
    let rims: Vec<ryukei::blend::EdgeBlend> = p
        .edges()
        .filter(|&e| {
            let ed = p.edge(e).unwrap();
            matches!(p.curve(ed.curve), Some(ryukei::geom::Curve::Circle(c))
                if (c.frame().origin().z - 1.0).abs() < 1e-9)
        })
        .map(|e| ryukei::blend::EdgeBlend {
            edge: p.edge(e).unwrap().name,
            kind: ryukei::blend::BlendKind::Fillet { radius: 0.1 },
        })
        .collect();
    let (b, _) = ryukei::blend::blend(OpKey(4), &p, &rims).unwrap();
    let t = 0.2;
    let spec = ShellSpec {
        thickness: t,
        outward: false,
        open: vec![face(OpKey(1), FaceRole::MinusZ)],
        faces: Vec::new(),
    };
    let (o, _) = shell(OpKey(5), &b, &spec).unwrap();
    let k = (n * n) as f64;
    let inner = (side - 2.0 * t) * (side - 2.0 * t) * (1.0 - t)
        - k * PI * (0.4 + t) * (0.4 + t) * (1.0 - t);
    near(volume(&o, 2e-5), volume(&b, 2e-5) - inner, 1e-4);
}
/// A rounded box shrunk past its radius at two distances: its six
/// planar faces keep their names both times, its fillets and balls are
/// deleted, and nothing else is named differently.
#[test]
fn global_offsets_keep_names_across_distances() {
    use ryukei::name::Event;
    let b = rounded(1.0, 0.6, 0.4, 0.1);
    let names = |o: &Body| {
        let mut n: Vec<Name> = o
            .faces()
            .map(|f| o.face(f).unwrap().name)
            .chain(o.edges().map(|e| o.edge(e).unwrap().name))
            .chain(o.vertices().map(|v| o.vertex(v).unwrap().name))
            .collect();
        n.sort();
        n
    };
    let (a, ja) = offset_body(OFF, &b, &by(-0.15)).unwrap();
    let (c, _) = offset_body(OFF, &b, &by(-0.12)).unwrap();
    assert_eq!(names(&a), names(&c));
    for role in SIDES {
        assert!(a.by_name(face(K, role)).is_some(), "{role:?}");
    }
    // The 12 fillets and 8 balls are gone.
    let deleted = ja
        .events()
        .iter()
        .filter(|e| matches!(e, Event::Deleted(_)))
        .count();
    assert!(deleted >= 20, "{deleted}");
}
