//! Blend-aware edits (plan 13.C): blends deleted by size, resized, and
//! made again after a tweak that would break them, keeping their names,
//! against exact volumes.

use ryukei::blend::{BlendKind, Chamfer, EdgeBlend, blend};
use ryukei::boolean::{BooleanOp, Tool, boolean};
use ryukei::check::{check, check_intersections};
use ryukei::geom::{Rigid, Surface, Vector3};
use ryukei::mesh::{MeshParams, tessellate};
use ryukei::name::{FaceRole, Name, OpKey, Role, ToolKey};
use ryukei::primitive;
use ryukei::topo::{Body, EdgeId};
use ryukei::tweak::{
    Blends, DeleteBlends, MoveSpec, OffsetFacesSpec, RecogniseOptions, Topology, delete_blends,
    move_faces, offset_faces, recognise_blends, resize_blends,
};
use ryukei::{Error, TweakError};
use std::f64::consts::PI;

const K: OpKey = OpKey(0x13c0);

fn face(r: FaceRole) -> Name {
    Name::derive(K, Role::Face(r), &[])
}

fn faces_of(b: &Body, e: EdgeId) -> Vec<Name> {
    b.edge_fins(e)
        .map(|fin| {
            b.face(b.loop_(b.fin(fin).unwrap().loop_).unwrap().face)
                .unwrap()
                .name
        })
        .collect()
}

fn edge_between(b: &Body, a: Name, c: Name) -> Name {
    let e = b
        .edges()
        .find(|&e| {
            let n = faces_of(b, e);
            n.contains(&a) && n.contains(&c)
        })
        .unwrap();
    b.edge(e).unwrap().name
}

/// The volume a sound body's mesh encloses.
fn volume(b: &Body) -> f64 {
    assert_eq!(check(b), []);
    assert_eq!(check_intersections(b), []);
    let m = tessellate(b, &MeshParams::new(1e-4, 0.1)).unwrap();
    assert!(m.faults.is_empty(), "{:?}", m.faults);
    m.welded_triangles()
        .map(|t| {
            let [a, b, c] = t.map(|i| m.positions[i as usize].to_vector());
            a.dot(b.cross(c)) / 6.0
        })
        .sum()
}

fn near(got: f64, want: f64) {
    assert!((got - want).abs() < 1e-5, "{got} against {want}");
}

/// Every face, edge and vertex name of `b`, sorted.
fn names(b: &Body) -> Vec<Name> {
    let mut v: Vec<Name> = b
        .faces()
        .map(|f| b.face(f).unwrap().name)
        .chain(b.edges().map(|e| b.edge(e).unwrap().name))
        .chain(b.vertices().map(|v| b.vertex(v).unwrap().name))
        .collect();
    v.sort();
    v
}

fn block(h: f64) -> Body {
    primitive::block(K, 1.0, 0.6, h, &Rigid::IDENTITY)
        .unwrap()
        .0
}

/// The block 1 × 0.6 × 0.4 with its top's +x edge filleted at `r`.
fn one_fillet(r: f64) -> Body {
    let b = block(0.4);
    let e = edge_between(&b, face(FaceRole::PlusZ), face(FaceRole::PlusX));
    let edge = EdgeBlend {
        edge: e,
        kind: BlendKind::Fillet { radius: r },
    };
    blend(OpKey(1), &b, &[edge]).unwrap().0
}

/// What a fillet of radius `r` along an edge `l` long takes off a block.
fn fillet_loss(r: f64, l: f64) -> f64 {
    l * r * r * (1.0 - PI / 4.0)
}

fn every_edge(b: &Body, kind: BlendKind) -> Body {
    let all: Vec<EdgeBlend> = b
        .edges()
        .map(|e| EdgeBlend {
            edge: b.edge(e).unwrap().name,
            kind,
        })
        .collect();
    blend(OpKey(7), b, &all).unwrap().0
}

/// A box `a × b × c` with every edge rounded at `r`, ball corners.
fn rounded_volume(a: f64, b: f64, c: f64, r: f64) -> f64 {
    a * b * c - (8.0 - 4.0 * PI / 3.0) * r * r * r - (4.0 - PI) * r * r * (a + b + c - 6.0 * r)
}

#[test]
fn blends_under_a_size_are_deleted_and_the_rest_kept() {
    let r = every_edge(&block(0.4), BlendKind::Fillet { radius: 0.05 });
    let spec = DeleteBlends {
        max_size: Some(0.06),
        ..Default::default()
    };
    let (sharp, j) = delete_blends(OpKey(21), &r, &spec).unwrap();
    near(volume(&sharp), 0.24);
    assert_eq!(sharp.faces().count(), 6);
    assert!(!j.events().is_empty());
    // Nothing that small: the body as it was.
    let small = DeleteBlends {
        max_size: Some(0.01),
        ..Default::default()
    };
    let (same, j) = delete_blends(OpKey(21), &r, &small).unwrap();
    assert_eq!(names(&same), names(&r));
    assert!(j.events().is_empty());
}

#[test]
fn a_named_blend_is_deleted_with_its_chain() {
    let f = one_fillet(0.1);
    let fl = recognise_blends(&f, &RecogniseOptions::default())
        .unwrap()
        .blends[0]
        .faces[0];
    let spec = DeleteBlends {
        faces: vec![fl],
        ..Default::default()
    };
    let (sharp, _) = delete_blends(OpKey(21), &f, &spec).unwrap();
    near(volume(&sharp), 0.24);
    let not = DeleteBlends {
        faces: vec![face(FaceRole::PlusZ)],
        ..Default::default()
    };
    let got = delete_blends(OpKey(21), &f, &not);
    assert!(
        matches!(got, Err(Error::InvalidTweak(TweakError::NotABlend { .. }))),
        "{got:?}"
    );
}

#[test]
fn a_fillet_resized_keeps_every_name() {
    let f = one_fillet(0.1);
    let fl = recognise_blends(&f, &RecogniseOptions::default())
        .unwrap()
        .blends[0]
        .faces[0];
    let (o, _) = resize_blends(OpKey(21), &f, &[(fl, BlendKind::Fillet { radius: 0.15 })]).unwrap();
    near(volume(&o), 0.24 - fillet_loss(0.15, 0.6));
    assert_eq!(names(&o), names(&f));
    // At its own size: the body back, names and all.
    let (same, _) =
        resize_blends(OpKey(21), &f, &[(fl, BlendKind::Fillet { radius: 0.1 })]).unwrap();
    assert_eq!(names(&same), names(&f));
    near(volume(&same), volume(&f));
}

#[test]
fn a_rounded_box_resized_whole_keeps_every_name() {
    let r = every_edge(&block(0.4), BlendKind::Fillet { radius: 0.05 });
    let all: Vec<(Name, BlendKind)> = recognise_blends(&r, &RecogniseOptions::default())
        .unwrap()
        .blends
        .iter()
        .map(|b| (b.faces[0], BlendKind::Fillet { radius: 0.08 }))
        .collect();
    let (o, _) = resize_blends(OpKey(20), &r, &all).unwrap();
    near(volume(&o), rounded_volume(1.0, 0.6, 0.4, 0.08));
    assert_eq!(names(&o), names(&r));
}

#[test]
fn a_chamfer_resized_keeps_every_name() {
    let b = block(0.4);
    let e = edge_between(&b, face(FaceRole::PlusZ), face(FaceRole::PlusX));
    let edge = EdgeBlend {
        edge: e,
        kind: BlendKind::Chamfer(Chamfer::Equal { distance: 0.05 }),
    };
    let (c, _) = blend(OpKey(1), &b, &[edge]).unwrap();
    let cf = recognise_blends(&c, &RecogniseOptions::default())
        .unwrap()
        .blends[0]
        .faces[0];
    let kind = BlendKind::Chamfer(Chamfer::Equal { distance: 0.1 });
    let (o, _) = resize_blends(OpKey(21), &c, &[(cf, kind)]).unwrap();
    near(volume(&o), 0.24 - 0.6 * 0.1 * 0.1 / 2.0);
    assert_eq!(names(&o), names(&c));
}

#[test]
fn a_top_raised_past_its_fillet_rebuilds_it() {
    let f = one_fillet(0.1);
    let mut spec = MoveSpec {
        faces: vec![face(FaceRole::PlusZ)],
        motion: Rigid::translation(Vector3::new(0.0, 0.0, 0.05)),
        tangent: false,
        topology: Topology::Keep,
        blends: Blends::Keep,
    };
    let kept = move_faces(OpKey(3), &f, &spec);
    assert!(
        matches!(
            kept,
            Err(Error::InvalidTweak(TweakError::TangencyBroken { .. }))
        ),
        "{kept:?}"
    );
    spec.blends = Blends::Rebuild;
    let (o, _) = move_faces(OpKey(3), &f, &spec).unwrap();
    near(volume(&o), 1.0 * 0.6 * 0.45 - fillet_loss(0.1, 0.6));
    assert_eq!(names(&o), names(&f));
}

#[test]
fn a_side_offset_under_its_fillet_rebuilds_it() {
    let f = one_fillet(0.1);
    let spec = OffsetFacesSpec {
        faces: vec![(face(FaceRole::PlusX), 0.05)],
        tangent: false,
        topology: Topology::Keep,
        blends: Blends::Rebuild,
    };
    let (o, _) = offset_faces(OpKey(3), &f, &spec).unwrap();
    near(volume(&o), 1.05 * 0.6 * 0.4 - fillet_loss(0.1, 0.6));
    assert_eq!(names(&o), names(&f));
}

/// A plate 1 × 1 × 0.2 drilled through at (0.5, 0.4) with radius 0.05,
/// its top's `-y` edge filleted at 0.1, and the name of the hole's wall.
fn drilled_and_filleted() -> (Body, Name) {
    let (p, _) = primitive::block(K, 1.0, 1.0, 0.2, &Rigid::IDENTITY).unwrap();
    let (pin, _) = primitive::cylinder(OpKey(9), 0.05, 0.6, &Rigid::IDENTITY).unwrap();
    let tool = Tool {
        key: ToolKey(1),
        body: &pin,
        place: Rigid::translation(Vector3::new(0.5, 0.4, -0.2)),
    };
    let (p, _) = boolean(OpKey(10), BooleanOp::Subtract, &p, &[tool]).unwrap();
    let e = edge_between(&p, face(FaceRole::PlusZ), face(FaceRole::MinusY));
    let edge = EdgeBlend {
        edge: e,
        kind: BlendKind::Fillet { radius: 0.1 },
    };
    let (f, _) = blend(OpKey(1), &p, &[edge]).unwrap();
    let wall = f
        .faces()
        .find(|&x| matches!(f.surface(f.face(x).unwrap().surface).unwrap(), Surface::Cylinder(c) if c.radius() < 0.06))
        .map(|x| f.face(x).unwrap().name)
        .unwrap();
    (f, wall)
}

#[test]
fn a_hole_moved_into_a_fillets_strip() {
    let (f, wall) = drilled_and_filleted();
    let mut spec = MoveSpec {
        faces: vec![wall],
        motion: Rigid::translation(Vector3::new(0.0, -0.33, 0.0)),
        tangent: false,
        topology: Topology::Change,
        blends: Blends::Keep,
    };
    let kept = move_faces(OpKey(5), &f, &spec);
    assert!(
        kept.is_err(),
        "the re-trim alone can't move it into the strip"
    );
    spec.blends = Blends::Rebuild;
    let (o, _) = move_faces(OpKey(5), &f, &spec).unwrap();
    // The hole runs through the fillet: every face keeps its name.
    let mut was: Vec<Name> = f.faces().map(|x| f.face(x).unwrap().name).collect();
    let mut now: Vec<Name> = o.faces().map(|x| o.face(x).unwrap().name).collect();
    was.sort();
    now.sort();
    assert_eq!(now, was);
    // The plate with its fillet, less the hole's part inside it.
    let v = volume(&o);
    assert!(
        v < 0.2 - fillet_loss(0.1, 1.0) && v > 0.2 - fillet_loss(0.1, 1.0) - PI * 0.05 * 0.05 * 0.2,
        "{v}"
    );
}

/// #626: the hole moved 0.33, 0.30 and 0.27 towards the fillet. At 0.30
/// and 0.27 its plug's top circle crosses the remade fillet's tangent
/// line, where the fillet's stretch along the plug's wall leaves the
/// circle tangent to it; the split ordered the two by their first chords
/// and walked out of the wall (`Uncertain` naming one face twice). Each
/// position keeps every face name and the volume between the plate less
/// its fillet and that less the whole hole.
#[test]
fn a_hole_moved_into_a_fillets_strip_at_every_position() {
    let (f, wall) = drilled_and_filleted();
    let mut was: Vec<Name> = f.faces().map(|x| f.face(x).unwrap().name).collect();
    was.sort();
    for dy in [-0.33, -0.30, -0.27] {
        let spec = MoveSpec {
            faces: vec![wall],
            motion: Rigid::translation(Vector3::new(0.0, dy, 0.0)),
            tangent: false,
            topology: Topology::Change,
            blends: Blends::Rebuild,
        };
        let (o, _) = move_faces(OpKey(5), &f, &spec).unwrap_or_else(|e| panic!("dy {dy}: {e:?}"));
        let mut now: Vec<Name> = o.faces().map(|x| o.face(x).unwrap().name).collect();
        now.sort();
        assert_eq!(now, was, "dy {dy}");
        let v = volume(&o);
        let full = 0.2 - fillet_loss(0.1, 1.0);
        // At 0.27 the hole barely reaches under the fillet, so the true
        // volume is within about 1e-6 of the lower bound: the mesh's chord
        // error (1e-5, as `near`) is allowed below it.
        assert!(
            v < full && v > full - PI * 0.05 * 0.05 * 0.2 - 1e-5,
            "dy {dy}: {v}"
        );
    }
}

#[test]
fn a_tweak_that_names_a_blend_face_isnt_rebuilt() {
    let f = one_fillet(0.1);
    let fl = recognise_blends(&f, &RecogniseOptions::default())
        .unwrap()
        .blends[0]
        .faces[0];
    let spec = OffsetFacesSpec {
        faces: vec![(fl, 0.01)],
        tangent: false,
        topology: Topology::Keep,
        blends: Blends::Rebuild,
    };
    let plain = OffsetFacesSpec {
        blends: Blends::Keep,
        ..spec.clone()
    };
    assert_eq!(
        offset_faces(OpKey(3), &f, &spec).map(|(b, _)| names(&b)),
        offset_faces(OpKey(3), &f, &plain).map(|(b, _)| names(&b))
    );
}
