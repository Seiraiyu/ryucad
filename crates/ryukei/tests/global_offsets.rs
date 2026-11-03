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
