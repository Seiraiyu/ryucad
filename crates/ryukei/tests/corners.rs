//! Corners against exact volumes: every result passes the checker and
//! the self-intersection check, meshes without faults, and encloses the
//! volume it should.

use ryukei::blend::{BlendKind, EdgeBlend, blend};
use ryukei::check::{check, check_intersections};
use ryukei::geom::Rigid;
use ryukei::mesh::{MeshParams, check_mesh, tessellate};
use ryukei::name::{FaceRole, Name, OpKey, Role};
use ryukei::primitive;
use ryukei::topo::Body;
use std::f64::consts::PI;

const K: OpKey = OpKey(0xc0);

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

fn fillet(edge: Name, radius: f64) -> EdgeBlend {
    EdgeBlend {
        edge,
        kind: BlendKind::Fillet { radius },
    }
}

/// A primitive's edge between two faces.
fn edge(key: OpKey, a: FaceRole, b: FaceRole) -> Name {
    let mut f = [
        Name::derive(key, Role::Face(a), &[]),
        Name::derive(key, Role::Face(b), &[]),
    ];
    f.sort();
    Name::derive(key, Role::Edge, &f)
}

#[test]
fn a_box_corner_of_three_fillets() {
    let k = OpKey(1);
    let b = primitive::block(k, 1.0, 1.0, 1.0, &Rigid::IDENTITY)
        .unwrap()
        .0;
    let r = 0.2;
    let (out, _) = blend(
        K,
        &b,
        &[
            fillet(edge(k, FaceRole::PlusX, FaceRole::PlusZ), r),
            fillet(edge(k, FaceRole::PlusY, FaceRole::PlusZ), r),
            fillet(edge(k, FaceRole::PlusX, FaceRole::PlusY), r),
        ],
    )
    .unwrap();
    let c = out.counts();
    assert_eq!((c.faces, c.edges, c.vertices), (10, 21, 13));
    assert_eq!(check_intersections(&out), []);
    let want = 1.0 - 3.0 * (1.0 - r) * (1.0 - PI / 4.0) * r * r - (1.0 - PI / 6.0) * (r * r * r);
    near(volume(&out, 1e-5), want, 1e-5);
}

/// Every edge of a box.
fn all_edges(k: OpKey) -> Vec<Name> {
    use FaceRole::*;
    let mut out = Vec::new();
    for (a, b) in [
        (PlusX, PlusY),
        (PlusX, MinusY),
        (MinusX, PlusY),
        (MinusX, MinusY),
        (PlusX, PlusZ),
        (PlusX, MinusZ),
        (MinusX, PlusZ),
        (MinusX, MinusZ),
        (PlusY, PlusZ),
        (PlusY, MinusZ),
        (MinusY, PlusZ),
        (MinusY, MinusZ),
    ] {
        out.push(edge(k, a, b));
    }
    out
}

#[test]
#[cfg_attr(
    debug_assertions,
    ignore = "slow in a debug build: CI runs it in release"
)]
fn every_edge_of_a_box() {
    let k = OpKey(1);
    let (a, b, c) = (1.0, 2.0, 3.0);
    let bx = primitive::block(k, a, b, c, &Rigid::IDENTITY).unwrap().0;
    let r = 0.25;
    let blends: Vec<EdgeBlend> = all_edges(k).into_iter().map(|e| fillet(e, r)).collect();
    let (out, _) = blend(K, &bx, &blends).unwrap();
    let n = out.counts();
    assert_eq!((n.faces, n.edges, n.vertices), (26, 48, 24));
    assert_eq!(check_intersections(&out), []);
    let (x, y, z) = (a - 2.0 * r, b - 2.0 * r, c - 2.0 * r);
    let want = x * y * z
        + 2.0 * r * (x * y + y * z + z * x)
        + PI * r * r * (x + y + z)
        + 4.0 / 3.0 * PI * (r * r * r);
    near(volume(&out, 1e-5), want, 1e-4);
}

#[test]
#[cfg_attr(
    debug_assertions,
    ignore = "slow in a debug build: CI runs it in release"
)]
fn each_corner_of_a_box() {
    use FaceRole::*;
    let k = OpKey(1);
    let bx = primitive::block(k, 1.0, 1.0, 1.0, &Rigid::IDENTITY)
        .unwrap()
        .0;
    let r = 0.2;
    let want = 1.0 - 3.0 * (1.0 - r) * (1.0 - PI / 4.0) * r * r - (1.0 - PI / 6.0) * (r * r * r);
    for x in [PlusX, MinusX] {
        for y in [PlusY, MinusY] {
            for z in [PlusZ, MinusZ] {
                let blends = [
                    fillet(edge(k, x, y), r),
                    fillet(edge(k, y, z), r),
                    fillet(edge(k, z, x), r),
                ];
                let (out, _) = blend(K, &bx, &blends).unwrap();
                assert_eq!(check_intersections(&out), []);
                near(volume(&out, 1e-5), want, 1e-5);
            }
        }
    }
}

/// The volume of a box with its edges rounded by `r`.
fn rounded_box(a: f64, b: f64, c: f64, r: f64) -> f64 {
    let (x, y, z) = (a - 2.0 * r, b - 2.0 * r, c - 2.0 * r);
    x * y * z
        + 2.0 * r * (x * y + y * z + z * x)
        + PI * r * r * (x + y + z)
        + 4.0 / 3.0 * PI * (r * r * r)
}

/// The straight edges of a body whose ends satisfy `f`.
fn lines_where(
    b: &Body,
    f: impl Fn(ryukei::geom::Point3, ryukei::geom::Point3) -> bool,
) -> Vec<Name> {
    b.edges()
        .filter(|&e| {
            let ed = b.edge(e).unwrap();
            f(
                b.vertex(ed.start).unwrap().point,
                b.vertex(ed.end).unwrap().point,
            )
        })
        .map(|e| b.edge(e).unwrap().name)
        .collect()
}

#[test]
#[cfg_attr(
    debug_assertions,
    ignore = "slow in a debug build: CI runs it in release"
)]
fn a_cavitys_edges_meet_in_concave_balls() {
    use ryukei::boolean::{BooleanOp, Tool, ToolKey, boolean};
    use ryukei::geom::Vector3;
    let outer = primitive::block(OpKey(1), 3.0, 3.0, 3.0, &Rigid::IDENTITY)
        .unwrap()
        .0;
    let inner = primitive::block(OpKey(2), 1.0, 1.0, 1.0, &Rigid::IDENTITY)
        .unwrap()
        .0;
    let tool = Tool {
        key: ToolKey(1),
        body: &inner,
        place: Rigid::translation(Vector3::new(1.0, 1.0, 1.0)),
    };
    let b = boolean(OpKey(3), BooleanOp::Subtract, &outer, &[tool])
        .unwrap()
        .0;
    let inside = lines_where(&b, |p, q| {
        [p, q].iter().all(|r| {
            (1.0..=2.0).contains(&r.x) && (1.0..=2.0).contains(&r.y) && (1.0..=2.0).contains(&r.z)
        })
    });
    assert_eq!(inside.len(), 12);
    let r = 0.2;
    let blends: Vec<EdgeBlend> = inside.iter().map(|&e| fillet(e, r)).collect();
    let (out, _) = blend(K, &b, &blends).unwrap();
    assert_eq!(check_intersections(&out), []);
    near(
        volume(&out, 1e-5),
        27.0 - rounded_box(1.0, 1.0, 1.0, r),
        2e-5,
    );
}
