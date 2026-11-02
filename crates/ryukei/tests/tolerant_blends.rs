//! Blends on tolerant bodies (design 299, plan 14.B): a box, a cylinder,
//! a box with a boss and a drilled box written as a file describes them,
//! every edge moved off its faces by 1e-7 to 3e-5 m and every vertex by
//! half that, healed with tightening off, then filleted and chamfered as
//! their exact twins are. Each must succeed as the exact one does, check
//! clean, cross nothing, mesh, and enclose the exact result's volume
//! within both bounds and its tolerances' strip; and the seeded cases
//! `tolerant_blend_props` found.

use ryukei::blend::{BlendKind, Chamfer, EdgeBlend, blend};
use ryukei::boolean::{BooleanOp, Tool, ToolKey, boolean};
use ryukei::check::{check, check_intersections};
use ryukei::geom::{Point3, Rigid, Surface, Vector3};
use ryukei::mesh::{MeshParams, tessellate};
use ryukei::name::OpKey;
use ryukei::primitive;
use ryukei::topo::{Body, EdgeId, FaceId};
use ryukei_testkit::tolerant_bytes::{MOVES, tolerant_with, twin_blends};

/// The side of the box, and the cylinder's height and diameter.
const S: f64 = 0.01;

/// Fixed directions for the moves, as `tolerant_booleans` has them.
fn dir(k: usize) -> Vector3 {
    let v = [
        Vector3::new(1.0, 2.0, -1.0),
        Vector3::new(-2.0, 1.0, 1.0),
        Vector3::new(1.0, -1.0, 2.0),
        Vector3::new(2.0, 1.0, 1.0),
        Vector3::new(-1.0, -2.0, 1.0),
    ][k % 5];
    v * (1.0 / v.length())
}

#[derive(Clone, Copy, Debug)]
enum Kind {
    Box,
    Cylinder,
    /// A box with a cylindrical boss on its top.
    Boss,
    /// A box drilled through its middle.
    Drilled,
}

fn exact(kind: Kind) -> Body {
    let bx = || {
        primitive::block(OpKey(1), S, S, S, &Rigid::IDENTITY)
            .unwrap()
            .0
    };
    let at = |x: f64, y: f64, z: f64| Rigid::translation(Vector3::new(x, y, z));
    let with = |op: BooleanOp, tool: Body| {
        let tools = [Tool {
            key: ToolKey(1),
            body: &tool,
            place: Rigid::IDENTITY,
        }];
        boolean(OpKey(3), op, &bx(), &tools).unwrap().0
    };
    match kind {
        Kind::Box => bx(),
        Kind::Cylinder => {
            primitive::cylinder(OpKey(1), S / 2.0, S, &at(S / 2.0, S / 2.0, 0.0))
                .unwrap()
                .0
        }
        Kind::Boss => with(
            BooleanOp::Unite,
            primitive::cylinder(OpKey(2), S / 4.0, S / 2.0, &at(S / 2.0, S / 2.0, S))
                .unwrap()
                .0,
        ),
        Kind::Drilled => with(
            BooleanOp::Subtract,
            primitive::cylinder(OpKey(2), S / 5.0, 3.0 * S, &at(S / 2.0, S / 2.0, -S))
                .unwrap()
                .0,
        ),
    }
}

/// The body with its edges moved by `by` (none for the exact one).
fn body(kind: Kind, by: Option<f64>) -> Body {
    let b = exact(kind);
    match by {
        None => b,
        Some(by) => tolerant_with(OpKey(9), &b, by, dir, |v| dir(v + 2)).unwrap(),
    }
}

/// Which edges a case blends, by where their middles lie.
type Pick = fn(Point3) -> bool;

fn near(a: f64, b: f64) -> bool {
    (a - b).abs() < 1e-6
}

/// A box edge along x on the front of the top.
fn front_top(p: Point3) -> bool {
    near(p.y, 0.0) && near(p.z, S)
}

/// The box's three edges at its corner at `(S, S, S)`.
fn corner(p: Point3) -> bool {
    [p.x, p.y, p.z].iter().filter(|c| near(**c, S)).count() == 2
}

/// The four edges round the top.
fn top(p: Point3) -> bool {
    near(p.z, S)
}

/// A rim round the middle of the top.
fn inner_top(p: Point3) -> bool {
    near(p.z, S) && (0.2 * S..0.8 * S).contains(&p.x) && (0.2 * S..0.8 * S).contains(&p.y)
}

/// The boss's top rim.
fn boss_top(p: Point3) -> bool {
    near(p.z, 1.5 * S)
}

fn mid(b: &Body, e: EdgeId) -> Point3 {
    let x = b.edge(e).unwrap();
    b.curve(x.curve)
        .unwrap()
        .point(0.5 * (x.interval[0] + x.interval[1]))
}

fn face_of(b: &Body, e: EdgeId, planar: bool) -> FaceId {
    let faces: Vec<FaceId> = b
        .edge_fins(e)
        .map(|f| b.loop_(b.fin(f).unwrap().loop_).unwrap().face)
        .collect();
    *faces
        .iter()
        .find(|&&f| {
            matches!(
                b.surface(b.face(f).unwrap().surface),
                Some(Surface::Plane(_))
            ) == planar
        })
        .unwrap_or(&faces[0])
}

/// What a case does to each edge it blends: a fillet, an equal chamfer,
/// or a chamfer at an angle to the edge's planar face.
#[derive(Clone, Copy, Debug)]
enum Make {
    Fillet(f64),
    Chamfer(f64),
    Angle(f64, f64),
}

/// The blends of the edges `pick` takes, in `b`.
fn blends(b: &Body, pick: Pick, make: Make) -> Vec<EdgeBlend> {
    b.edges()
        .filter(|&e| pick(mid(b, e)))
        .map(|e| EdgeBlend {
            edge: b.edge(e).unwrap().name,
            kind: match make {
                Make::Fillet(radius) => BlendKind::Fillet { radius },
                Make::Chamfer(distance) => BlendKind::Chamfer(Chamfer::Equal { distance }),
                Make::Angle(distance, angle) => BlendKind::Chamfer(Chamfer::DistanceAngle {
                    distance,
                    angle,
                    face: b.face(face_of(b, e, true)).unwrap().name,
                }),
            },
        })
        .collect()
}

/// The volume a body's mesh encloses at a chord of a five-thousandth of
/// its size, the bound the chord gives it, and its area: the measured
/// bounds (`ryukei::measure`) refuse some blended boxes' finer meshes.
fn volume(b: &Body) -> (f64, f64, f64) {
    let bx = b
        .faces()
        .map(|f| ryukei::intersect::face_box(b, f))
        .reduce(|x, y| x.union(&y))
        .unwrap();
    let chord = (bx.max() - bx.min()).length() * 2e-4;
    let m = tessellate(b, &MeshParams::new(chord, 0.1)).unwrap();
    assert_eq!(m.faults, []);
    let (v, a) = ryukei_testkit::mesh::volume_and_area(&m);
    (v, 2.0 * chord * a, a)
}

fn worst(b: &Body) -> f64 {
    b.edges()
        .map(|e| b.edge(e).unwrap().tolerance.get())
        .chain(b.vertices().map(|v| b.vertex(v).unwrap().tolerance.get()))
        .fold(0.0, f64::max)
}

/// Blends `b`'s edges and asserts the result is sound: it builds, checks
/// clean, crosses nothing and meshes without faults.
fn sound(what: &str, b: &Body, blends: &[EdgeBlend]) -> Body {
    let (out, _) = blend(OpKey(41), b, blends).unwrap_or_else(|e| panic!("{what}: {e:?}"));
    assert_eq!(check(&out), [], "{what}");
    assert_eq!(check_intersections(&out), [], "{what}");
    let m = tessellate(&out, &MeshParams::new(1e-5, 0.3)).unwrap();
    assert_eq!(m.faults, [], "{what}");
    out
}

/// The case on the exact body and on its twins at every move: each
/// sound, and the twin's result enclosing the exact one's volume within
/// both bounds and its tolerances' strip. Returns the twins' results.
fn case(kind: Kind, pick: Pick, make: Make) -> Vec<Body> {
    let e = exact(kind);
    let ours = blends(&e, pick, make);
    let want = sound(&format!("{make:?} on exact {kind:?}"), &e, &ours);
    let (ve, be, ae) = volume(&want);
    MOVES
        .iter()
        .map(|&by| {
            let what = format!("{make:?} on {kind:?} moved by {by:e}");
            let t = body(kind, Some(by));
            let bl = twin_blends(&e, &ours, &t).expect("the same edges in the twin");
            let got = sound(&what, &t, &bl);
            let (vt, bt, _) = volume(&got);
            let strip = 2.0 * worst(&got) * ae;
            assert!(
                (vt - ve).abs() <= be + bt + strip,
                "{what}: volume {vt} against {ve} (bounds {be}, {bt}, strip {strip})"
            );
            got
        })
        .collect()
}

#[test]
fn box_edges_filleted_and_chamfered() {
    case(Kind::Box, front_top, Make::Fillet(S / 5.0));
    case(Kind::Box, front_top, Make::Chamfer(S / 5.0));
    case(Kind::Box, front_top, Make::Angle(S / 5.0, 0.6));
}

#[test]
fn box_corners_balled_trimmed_and_mitred() {
    case(Kind::Box, corner, Make::Fillet(S / 5.0));
    case(Kind::Box, top, Make::Fillet(S / 5.0));
    case(Kind::Box, corner, Make::Chamfer(S / 5.0));
}

#[test]
fn every_edge_of_a_box() {
    case(Kind::Box, |_| true, Make::Fillet(S / 5.0));
}

#[test]
fn rims_filleted() {
    case(Kind::Cylinder, top, Make::Fillet(S / 5.0));
    case(Kind::Boss, inner_top, Make::Fillet(S / 10.0));
    case(Kind::Boss, boss_top, Make::Fillet(S / 10.0));
    case(Kind::Drilled, inner_top, Make::Fillet(S / 20.0));
}

#[test]
fn rims_chamfered() {
    case(Kind::Cylinder, top, Make::Chamfer(S / 5.0));
    case(Kind::Drilled, inner_top, Make::Chamfer(S / 20.0));
}
