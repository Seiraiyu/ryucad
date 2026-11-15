//! ryukei phase 20's twin of #652's and #658's carried pairs (design 669,
//! plan 20.B): a bar 1 × 0.2 × 0.2 with a hole drilled into each end, a
//! big one along x from x = 0 and a small one from x = 1, tilted 2°
//! about z, their axes passing [`BIG`] + [`SMALL`] less [`LAP`] apart at
//! the bar's middle, each hole's floor rim filleted, and a thin channel
//! from one floor to the other through both fillets (never a wall).
//! Deleting the fillets and the channel is one connected set
//! (`delete_faces` grows each edge-connected set in its own re-trim), so
//! rung 0's box grows by that set's reach, about the bar's length.
//! Carried over it, the two walls cross in a thin loop at the bar's
//! middle, far from both inputs, with no edge between them (walls only
//! touching there trace clean; overlapping by [`LAP`] they refuse). Each
//! wall touches only its own fillet, so over its near box it is carried
//! only past its own floor.

use core::f64::consts::{FRAC_PI_2, PI};
use ryukei::blend::{BlendKind, EdgeBlend, blend};
use ryukei::boolean::{BooleanOp, Tool, boolean};
use ryukei::geom::{Point3, Rigid, Surface, UnitVector3, Vector3};
use ryukei::name::{OpKey, ToolKey};
use ryukei::primitive;
use ryukei::topo::{Body, EdgeId, FaceId};
use ryukei::tweak::{DeleteSpec, Heal};

/// The holes' radii, their depth, the small hole's tilt in degrees, the
/// floor fillets' radii, and the channel's radius.
pub const BIG: f64 = 0.03;
pub const SMALL: f64 = 0.01;
pub const DEPTH: f64 = 0.15;
pub const TILT_DEGREES: f64 = 2.0;
pub const BIG_FILLET: f64 = 0.005;
pub const SMALL_FILLET: f64 = 0.003;
pub const CHANNEL: f64 = 0.003;
/// How far the carried walls overlap at the bar's middle.
pub const LAP: f64 = 1e-4;

fn unit(x: f64, y: f64, z: f64) -> UnitVector3 {
    UnitVector3::exact(Vector3::new(x, y, z)).unwrap()
}

/// The faces of `b` using edge `e`.
fn faces_of(b: &Body, e: EdgeId) -> Vec<FaceId> {
    b.edge_fins(e)
        .map(|fin| b.loop_(b.fin(fin).unwrap().loop_).unwrap().face)
        .collect()
}

/// The small hole's tilt, its entry and its direction into the bar.
fn small_axis() -> (f64, Point3, Vector3) {
    let t = TILT_DEGREES * PI / 180.0;
    let (st, ct) = ryukei::math::sin_cos(t);
    // From x = 1.01 back along (-cos t, sin t, 0), its axis through
    // (0.5, 0.1, 0.11 - LAP): there it passes BIG + SMALL - LAP over the
    // big hole's axis, square to it, so the walls carried that far
    // overlap by LAP about (0.5, 0.1, 0.10).
    let back = 0.51 / ct;
    let entry = Point3::new(0.5 + back * ct, 0.1 - back * st, 0.07 + BIG + SMALL - LAP);
    (t, entry, Vector3::new(-ct, st, 0.0))
}

/// The bar with both holes, before the fillets.
pub fn drilled_bare() -> Body {
    let (bar, _) = primitive::block(OpKey(0x20b0), 1.0, 0.2, 0.2, &Rigid::IDENTITY).unwrap();
    let (big, _) = primitive::cylinder(OpKey(0x20b1), BIG, DEPTH, &Rigid::IDENTITY).unwrap();
    let (small, _) = primitive::cylinder(OpKey(0x20b2), SMALL, DEPTH, &Rigid::IDENTITY).unwrap();
    // The big hole along +x from x = -0.01, its axis at y = 0.1, z = 0.07:
    // its top runs along z = 0.10.
    let big_place = Rigid::translation(Vector3::new(-0.01, 0.1, 0.07))
        .then_after(&Rigid::rotation(unit(0.0, 1.0, 0.0), FRAC_PI_2));
    let (t, entry, _) = small_axis();
    let turn = Rigid::rotation(unit(0.0, 0.0, 1.0), -t)
        .then_after(&Rigid::rotation(unit(0.0, 1.0, 0.0), -FRAC_PI_2));
    let small_place = Rigid::translation(entry - Point3::ORIGIN).then_after(&turn);
    boolean(
        OpKey(0x20b3),
        BooleanOp::Subtract,
        &bar,
        &[
            Tool {
                key: ToolKey(1),
                body: &big,
                place: big_place,
            },
            Tool {
                key: ToolKey(2),
                body: &small,
                place: small_place,
            },
        ],
    )
    .unwrap()
    .0
}

/// The bar with both holes' floor rims filleted: [`BIG_FILLET`] in the
/// big hole, [`SMALL_FILLET`] in the small one.
pub fn filleted() -> Body {
    let b = drilled_bare();
    let is = |f: FaceId, pick: fn(&Surface) -> bool| {
        pick(b.surface(b.face(f).unwrap().surface).unwrap())
    };
    // A floor: a plane inside the bar, away from both ends.
    let floor = |f: FaceId| {
        let bx = ryukei::intersect::face_box(&b, f);
        is(f, |s| matches!(s, Surface::Plane(_))) && bx.min().x > 0.05 && bx.max().x < 0.95
    };
    let mut edges = Vec::new();
    for e in b.edges() {
        let fs = faces_of(&b, e);
        let Some(&fl) = fs.iter().find(|&&f| floor(f)) else {
            continue;
        };
        if !fs
            .iter()
            .any(|&f| is(f, |s| matches!(s, Surface::Cylinder(_))))
        {
            continue;
        }
        let near_start = ryukei::intersect::face_box(&b, fl).max().x < 0.5;
        edges.push(EdgeBlend {
            edge: b.edge(e).unwrap().name,
            kind: BlendKind::Fillet {
                radius: if near_start { BIG_FILLET } else { SMALL_FILLET },
            },
        });
    }
    assert_eq!(edges.len(), 2, "one floor rim a hole");
    blend(OpKey(0x20b4), &b, &edges).unwrap().0
}

/// The filleted bar with the channel: radius [`CHANNEL`], from a point
/// of the big floor 0.025 above its centre (inside the fillet's foot,
/// 0.025 to 0.03 from the axis) to one of the small floor 0.0065 below
/// its centre (its fillet's foot is 0.007 to 0.01), carried 0.007 past
/// each into the holes, so it cuts each floor and fillet and neither
/// wall.
pub fn drilled() -> Body {
    let b = filleted();
    let (_, entry, dir) = small_axis();
    let q1 = Point3::new(-0.01 + DEPTH, 0.1, 0.07 + 0.025);
    let q2 = entry + dir * DEPTH + Vector3::new(0.0, 0.0, -0.0065);
    let along = q2 - q1;
    let d = along.normalize().unwrap().get();
    let over = 0.007;
    let start = q1 - d * over;
    let length = along.length() + 2.0 * over;
    let (channel, _) =
        primitive::cylinder(OpKey(0x20b6), CHANNEL, length, &Rigid::IDENTITY).unwrap();
    // z turned onto d about z × d.
    let axis = Vector3::new(-d.y, d.x, 0.0).normalize().unwrap();
    let place = Rigid::translation(start - Point3::ORIGIN)
        .then_after(&Rigid::rotation(axis, ryukei::math::acos(d.z)));
    let end = place.apply_point(Point3::new(0.0, 0.0, length));
    assert!(end.distance(q2 + d * over) < 1e-12, "{end:?}");
    boolean(
        OpKey(0x20b7),
        BooleanOp::Subtract,
        &b,
        &[Tool {
            key: ToolKey(3),
            body: &channel,
            place,
        }],
    )
    .unwrap()
    .0
}

/// The faces the delete takes: both floor fillets (tori) and the
/// channel's wall, one set joined across edges.
pub fn deleted(b: &Body) -> Vec<FaceId> {
    b.faces()
        .filter(|&f| match b.surface(b.face(f).unwrap().surface).unwrap() {
            Surface::Torus(_) => true,
            Surface::Cylinder(c) => (c.radius() - CHANNEL).abs() < 1e-12,
            _ => false,
        })
        .collect()
}

/// The delete of [`deleted`]'s faces, healed by growth.
pub fn spec(b: &Body) -> DeleteSpec {
    DeleteSpec {
        faces: deleted(b)
            .iter()
            .map(|&f| b.face(f).unwrap().name)
            .collect(),
        heal: Heal::Grow,
    }
}
