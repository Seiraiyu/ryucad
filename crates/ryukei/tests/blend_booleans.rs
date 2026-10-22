//! Booleans on blend faces: holes and cuts through fillets, analytic
//! (a torus) and not (a pipe). Every result passes the checker, meshes
//! without faults, and encloses the volume it should: exactly where the fillet is analytic, half the body
//! where a symmetry plane halves it, and the rod's volume less where a
//! rod drills through. Blending and meshing pipes is slow in a debug
//! build, so the pipe cases run in release only (`cargo test -p ryukei
//! --release --test blend_booleans`).

use ryukei::blend::{BlendKind, EdgeBlend, blend};
use ryukei::boolean::{BooleanOp, Tool, ToolKey, boolean};
use ryukei::check::check;
use ryukei::geom::{Curve, Rigid, Surface, Vector3};
use ryukei::mesh::{MeshParams, check_mesh, tessellate};
use ryukei::name::OpKey;
use ryukei::primitive;
use ryukei::topo::Body;
use std::f64::consts::{FRAC_PI_2, PI};

/// The volume a sound body's mesh encloses.
fn volume(b: &Body, chord: f64) -> f64 {
    assert_eq!(check(b), []);
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

fn near(got: f64, want: f64, tol: f64) {
    assert!((got - want).abs() <= tol, "{got} against {want}");
}

fn at(x: f64, y: f64, z: f64) -> Rigid {
    Rigid::translation(Vector3::new(x, y, z))
}

fn fillet(b: &Body, edge: ryukei::topo::EdgeId, radius: f64) -> EdgeBlend {
    EdgeBlend {
        edge: b.edge(edge).unwrap().name,
        kind: BlendKind::Fillet { radius },
    }
}

/// `body` less a placed tool.
fn minus(body: &Body, tool: &Body, place: Rigid) -> Body {
    let t = Tool {
        key: ToolKey(1),
        body: tool,
        place,
    };
    boolean(OpKey(5), BooleanOp::Subtract, body, &[t])
        .unwrap()
        .0
}

/// How many faces of a body lie on pipes.
fn pipes(b: &Body) -> usize {
    b.faces()
        .filter(|&f| {
            matches!(
                b.surface(b.face(f).unwrap().surface),
                Some(Surface::Pipe(_))
            )
        })
        .count()
}

#[test]
fn a_rounded_rim_cut_in_half() {
    // A cylinder of radius 1 and height 1 with its top rim rounded by
    // 0.2: the torus loses the corner's area at the centroid's radius
    // (Pappus), and a block across the axis takes exactly half.
    let b = primitive::cylinder(OpKey(1), 1.0, 1.0, &Rigid::IDENTITY)
        .unwrap()
        .0;
    let rim = b.edges().next().unwrap();
    let (r, _) = blend(OpKey(9), &b, &[fillet(&b, rim, 0.2)]).unwrap();
    let area = (1.0 - PI / 4.0) * 0.04;
    let d = 0.2 * (10.0 - 3.0 * PI) / (12.0 - 3.0 * PI);
    let whole = PI - 2.0 * PI * (1.0 - d) * area;
    let block = primitive::block(OpKey(2), 3.0, 3.0, 3.0, &Rigid::IDENTITY)
        .unwrap()
        .0;
    let half = minus(&r, &block, at(0.0, -1.5, -1.0));
    near(volume(&half, 1e-5), whole / 2.0, 2e-4);
    // A hole down through the fillet takes the rod's volume less.
    let rod = primitive::cylinder(OpKey(3), 0.05, 3.0, &Rigid::IDENTITY)
        .unwrap()
        .0;
    let drilled = minus(&r, &rod, at(0.9, 0.0, -1.0));
    let v = volume(&drilled, 1e-5);
    assert!(
        v < whole - PI * 0.0025 * 0.8 && v > whole - PI * 0.0025,
        "{v}"
    );
}

/// A shaft of radius 1 along `x`, a boss of radius 0.5 standing on it,
/// and the boss's foot filled by a fillet of 0.1: a pipe, the boss's
/// cylinder meeting the shaft's in a curve that is no circle.
fn filleted_boss() -> Body {
    let y = Vector3::new(0.0, 1.0, 0.0).normalize().unwrap();
    let along_x = Rigid::rotation(y, FRAC_PI_2).then_after(&at(0.0, 0.0, -2.0));
    let shaft = primitive::cylinder(OpKey(1), 1.0, 4.0, &along_x).unwrap().0;
    let boss = primitive::cylinder(OpKey(2), 0.5, 2.0, &Rigid::IDENTITY)
        .unwrap()
        .0;
    let tool = Tool {
        key: ToolKey(1),
        body: &boss,
        place: Rigid::IDENTITY,
    };
    let b = boolean(OpKey(3), BooleanOp::Unite, &shaft, &[tool])
        .unwrap()
        .0;
    let foot = b
        .edges()
        .find(|&e| matches!(b.curve(b.edge(e).unwrap().curve), Some(Curve::Nurbs(_))))
        .unwrap();
    let (r, _) = blend(OpKey(9), &b, &[fillet(&b, foot, 0.1)]).unwrap();
    assert_eq!(pipes(&r), 1);
    r
}

#[test]
#[cfg_attr(
    debug_assertions,
    ignore = "slow in a debug build: CI runs it in release"
)]
fn a_pipe_fillet_cut_in_half() {
    // The planes x = 0 and y = 0 are symmetry planes of the filleted
    // boss: a block on one side of either takes half of it, cutting the
    // pipe face across.
    let r = filleted_boss();
    let whole = volume(&r, 1e-4);
    let block = primitive::block(OpKey(4), 6.0, 3.0, 6.0, &Rigid::IDENTITY)
        .unwrap()
        .0;
    let half = minus(&r, &block, at(-3.0, 0.0, -3.0));
    assert_eq!(pipes(&half), 1);
    near(volume(&half, 1e-4), whole / 2.0, 1e-5);
    let across = Rigid::rotation(Vector3::new(0.0, 0.0, 1.0).normalize().unwrap(), FRAC_PI_2)
        .then_after(&at(-3.0, 0.0, -3.0));
    let other = minus(&r, &block, across);
    near(volume(&other, 1e-4), whole / 2.0, 1e-5);
}

#[test]
#[cfg_attr(
    debug_assertions,
    ignore = "slow in a debug build: CI runs it in release"
)]
fn holes_through_a_pipe_fillet() {
    // Rods of radius 0.02 down through the fillet at three places round
    // the boss, each through the shaft too: each takes at least its
    // length through the shaft (at most 2) and less than that plus the
    // fillet's height (0.2).
    let r = filleted_boss();
    let whole = volume(&r, 1e-4);
    let rod = primitive::cylinder(OpKey(4), 0.02, 4.0, &Rigid::IDENTITY)
        .unwrap()
        .0;
    let disc = PI * 0.02 * 0.02;
    for (x, y) in [(0.56, 0.0), (0.4, 0.4), (0.0, 0.57)] {
        let drilled = minus(&r, &rod, at(x, y, -2.0));
        assert_eq!(pipes(&drilled), 1);
        let chord = 2.0 * (1.0 - y * y).sqrt();
        let taken = whole - volume(&drilled, 1e-4);
        assert!(
            taken > disc * chord * 0.99 && taken < disc * (chord + 0.2),
            "{x}, {y}: {taken}"
        );
    }
}
