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

/// Faults of the self-intersection check other than blend faces meeting
/// tangentially along a section, which plan 6.D certifies apart.
fn crossings_but_sections(b: &Body) -> Vec<ryukei::check::Fault> {
    check_intersections(b)
        .into_iter()
        .filter(|f| !matches!(f, ryukei::check::Fault::CannotCheck { .. }))
        .collect()
}

/// The area of the corner a fillet of radius `r` takes from a square
/// edge, and how far its centroid is from the edge along each face.
fn wedge(r: f64) -> (f64, f64) {
    (
        (1.0 - PI / 4.0) * r * r,
        r * (10.0 - 3.0 * PI) / (12.0 - 3.0 * PI),
    )
}

/// How deep a blend of `size` cuts into a unit cube's edge at `u` from
/// one face, along the other: a fillet's arc or a chamfer's line.
fn depth(fillet: bool, size: f64, u: f64) -> f64 {
    let u = u - (1.0 - size);
    if u <= 0.0 {
        0.0
    } else if fillet {
        size - (size * size - u * u).max(0.0).sqrt()
    } else {
        u
    }
}

/// The volume blends of sizes `a` and `b` on edges of a unit cube meeting
/// at a corner (sharing a face) take away together: each one's wedge,
/// less what both take, by Simpson's rule over the shared face's depth.
fn two_wedges(a: (bool, f64), b: (bool, f64)) -> f64 {
    let area = |(f, r): (bool, f64)| if f { wedge(r).0 } else { r * r / 2.0 };
    let lo = 1.0 - a.1.min(b.1);
    let n = 200_000;
    let h = (1.0 - lo) / n as f64;
    let g = |y: f64| depth(a.0, a.1, y) * depth(b.0, b.1, y);
    let mut sum = g(lo) + g(1.0);
    for i in 1..n {
        let y = lo + h * i as f64;
        sum += g(y) * if i % 2 == 1 { 4.0 } else { 2.0 };
    }
    area(a) + area(b) - sum * h / 3.0
}

#[test]
#[cfg_attr(
    debug_assertions,
    ignore = "slow in a debug build: CI runs it in release"
)]
fn two_fillets_at_a_corner_trim_each_other() {
    let k = OpKey(1);
    let bx = primitive::block(k, 1.0, 1.0, 1.0, &Rigid::IDENTITY)
        .unwrap()
        .0;
    for (a, b) in [(0.2, 0.2), (0.15, 0.3), (0.3, 0.15)] {
        let (out, _) = blend(
            K,
            &bx,
            &[
                fillet(edge(k, FaceRole::PlusY, FaceRole::PlusZ), a),
                fillet(edge(k, FaceRole::PlusX, FaceRole::PlusY), b),
            ],
        )
        .unwrap();
        assert_eq!(check_intersections(&out), []);
        if a == b {
            near(
                two_wedges((true, a), (true, b)),
                2.0 * (1.0 - PI / 4.0) * a * a - (5.0 / 3.0 - PI / 2.0) * (a * a * a),
                1e-9,
            );
        }
        near(
            volume(&out, 1e-5),
            1.0 - two_wedges((true, a), (true, b)),
            2e-5,
        );
    }
}

fn chamfer(edge: Name, distance: f64) -> EdgeBlend {
    EdgeBlend {
        edge,
        kind: BlendKind::Chamfer(ryukei::blend::Chamfer::Equal { distance }),
    }
}

#[test]
fn a_fillet_and_a_chamfer_trim_each_other() {
    use FaceRole::*;
    let k = OpKey(1);
    let bx = primitive::block(k, 1.0, 1.0, 1.0, &Rigid::IDENTITY)
        .unwrap()
        .0;
    for (a, b) in [(0.2, 0.2), (0.1, 0.25), (0.25, 0.1)] {
        let (out, _) = blend(
            K,
            &bx,
            &[
                fillet(edge(k, PlusY, PlusZ), a),
                chamfer(edge(k, PlusX, PlusY), b),
            ],
        )
        .unwrap();
        assert_eq!(check_intersections(&out), []);
        for f in out.faces() {
            assert_eq!(out.face_loops(f).count(), 1);
        }
        near(
            volume(&out, 1e-5),
            1.0 - two_wedges((true, a), (false, b)),
            2e-5,
        );
    }
}

#[test]
fn chamfers_at_a_corner() {
    use FaceRole::*;
    let k = OpKey(1);
    let bx = primitive::block(k, 1.0, 1.0, 1.0, &Rigid::IDENTITY)
        .unwrap()
        .0;
    let d = 0.2;
    // Two trim each other.
    let (out, _) = blend(
        K,
        &bx,
        &[
            chamfer(edge(k, PlusY, PlusZ), d),
            chamfer(edge(k, PlusX, PlusY), d),
        ],
    )
    .unwrap();
    assert_eq!(check_intersections(&out), []);
    near(volume(&out, 1e-5), 1.0 - d * d + (d * d * d) / 3.0, 1e-9);
    // Three are mitred, at every corner of the box.
    for x in [PlusX, MinusX] {
        for y in [PlusY, MinusY] {
            for z in [PlusZ, MinusZ] {
                let blends = [
                    chamfer(edge(k, x, y), d),
                    chamfer(edge(k, y, z), d),
                    chamfer(edge(k, z, x), d),
                ];
                let (out, _) = blend(K, &bx, &blends).unwrap();
                assert_eq!(check_intersections(&out), []);
                near(
                    volume(&out, 1e-5),
                    1.0 - 1.5 * d * d + 0.75 * (d * d * d),
                    1e-9,
                );
            }
        }
    }
    let blends: Vec<EdgeBlend> = all_edges(k).into_iter().map(|e| chamfer(e, d)).collect();
    let (out, _) = blend(K, &bx, &blends).unwrap();
    assert_eq!(check_intersections(&out), []);
    near(
        volume(&out, 1e-5),
        1.0 - 6.0 * d * d + 6.0 * (d * d * d),
        1e-9,
    );
}

#[test]
#[cfg_attr(
    debug_assertions,
    ignore = "slow in a debug build: CI runs it in release"
)]
fn a_smaller_pair_rolls_over_a_larger_fillet() {
    let k = OpKey(1);
    let b = primitive::block(k, 1.0, 1.0, 1.0, &Rigid::IDENTITY)
        .unwrap()
        .0;
    let (big, small) = (0.3, 0.1);
    let (out, _) = blend(
        K,
        &b,
        &[
            fillet(edge(k, FaceRole::PlusX, FaceRole::PlusZ), big),
            fillet(edge(k, FaceRole::PlusY, FaceRole::PlusZ), small),
            fillet(edge(k, FaceRole::PlusX, FaceRole::PlusY), small),
        ],
    )
    .unwrap();
    let (a_big, _) = wedge(big);
    let (a, d) = wedge(small);
    // The big fillet all along; the small ones along the box's edges up
    // to the big one, and round its end on +Y.
    let want = 1.0 - a_big - 2.0 * (1.0 - big) * a - PI / 2.0 * (big - d) * a;
    assert_eq!(crossings_but_sections(&out), []);
    near(volume(&out, 1e-5), want, 2e-5);
}

/// A block 1 × 1 × 1 standing on a plate 3 × 3 × 1, united.
fn block_on_plate() -> Body {
    use ryukei::boolean::{BooleanOp, Tool, ToolKey, boolean};
    use ryukei::geom::Vector3;
    let plate = primitive::block(OpKey(1), 3.0, 3.0, 1.0, &Rigid::IDENTITY)
        .unwrap()
        .0;
    let block = primitive::block(OpKey(2), 1.0, 1.0, 1.0, &Rigid::IDENTITY)
        .unwrap()
        .0;
    let tool = Tool {
        key: ToolKey(1),
        body: &block,
        place: Rigid::translation(Vector3::new(1.0, 1.0, 1.0)),
    };
    boolean(OpKey(3), BooleanOp::Unite, &plate, &[tool])
        .unwrap()
        .0
}

#[test]
#[cfg_attr(
    debug_assertions,
    ignore = "slow in a debug build: CI runs it in release"
)]
fn a_blocks_foot_rolls_round_its_rounded_edges() {
    let b = block_on_plate();
    let (big, small) = (0.25, 0.1);
    let vertical = lines_where(&b, |p, q| {
        (p.x - q.x).abs() < 1e-9 && (p.y - q.y).abs() < 1e-9 && p.z.min(q.z) > 0.5
    });
    let foot = lines_where(&b, |p, q| {
        (p.z - 1.0).abs() < 1e-9
            && (q.z - 1.0).abs() < 1e-9
            && [p, q]
                .iter()
                .all(|r| (1.0..=2.0).contains(&r.x) && (1.0..=2.0).contains(&r.y))
    });
    assert_eq!((vertical.len(), foot.len()), (4, 4));
    let blends: Vec<EdgeBlend> = vertical
        .iter()
        .map(|&e| fillet(e, big))
        .chain(foot.iter().map(|&e| fillet(e, small)))
        .collect();
    let (out, _) = blend(K, &b, &blends).unwrap();
    let (a_big, _) = wedge(big);
    let (a, d) = wedge(small);
    let want = 9.0 + 1.0 - 4.0 * a_big + 4.0 * (1.0 - 2.0 * big) * a + 2.0 * PI * (big + d) * a;
    assert_eq!(crossings_but_sections(&out), []);
    near(volume(&out, 1e-5), want, 4e-5);
}

#[test]
#[cfg_attr(
    debug_assertions,
    ignore = "slow in a debug build: CI runs it in release"
)]
fn rolled_corners_are_their_stages() {
    use FaceRole::*;
    let k = OpKey(1);
    let bx = primitive::block(k, 1.0, 1.0, 1.0, &Rigid::IDENTITY)
        .unwrap()
        .0;
    // Each edge rolls over the one before at the corner they share.
    let (a, b, c) = (0.3, 0.12, 0.05);
    let blends = [
        fillet(edge(k, PlusX, PlusZ), a),
        fillet(edge(k, PlusY, PlusZ), b),
        fillet(edge(k, PlusX, PlusY), b),
        fillet(edge(k, MinusX, PlusZ), c),
        fillet(edge(k, MinusX, PlusY), c),
    ];
    let (out, _) = blend(K, &bx, &blends).unwrap();
    assert_eq!(crossings_but_sections(&out), []);
    // The same by hand, stage by stage: each end curve at a corner joins
    // the edges rolling over it.
    let (s1, _) = blend(OpKey(1), &bx, &blends[..1]).unwrap();
    let chain1 = ryukei::blend::tangent_chain(
        &s1,
        s1.edges()
            .find(|&e| s1.edge(e).unwrap().name == blends[1].edge)
            .unwrap(),
    )
    .unwrap();
    let step: Vec<EdgeBlend> = chain1
        .iter()
        .map(|&e| fillet(s1.edge(e).unwrap().name, b))
        .collect();
    let (s2, _) = blend(OpKey(2), &s1, &step).unwrap();
    let chain2 = ryukei::blend::tangent_chain(
        &s2,
        s2.edges()
            .find(|&e| s2.edge(e).unwrap().name == blends[3].edge)
            .unwrap(),
    )
    .unwrap();
    let step: Vec<EdgeBlend> = chain2
        .iter()
        .map(|&e| fillet(s2.edge(e).unwrap().name, c))
        .collect();
    let (s3, _) = blend(OpKey(3), &s2, &step).unwrap();
    near(volume(&out, 1e-5), volume(&s3, 1e-5), 1e-9);
    assert_eq!(out.counts().faces, s3.counts().faces);
}
