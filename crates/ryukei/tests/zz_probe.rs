use ryukei::blend::{BlendKind, EdgeBlend, blend};
use ryukei::boolean::{BooleanOp, Tool, boolean};
use ryukei::check::{check, check_intersections};
use ryukei::geom::{Rigid, Surface, Vector3};
use ryukei::mesh::{MeshParams, tessellate};
use ryukei::name::{FaceRole, Name, OpKey, Role, ToolKey};
use ryukei::primitive;
use ryukei::topo::{Body, EdgeId};
use ryukei::tweak::{DeleteSpec, Heal, MoveSpec, Topology, delete_faces, move_faces};

const K: OpKey = OpKey(0x13d0);
fn face(r: FaceRole) -> Name { Name::derive(K, Role::Face(r), &[]) }
fn vol(b: &Body) -> f64 {
    assert_eq!(check(b), []);
    assert_eq!(check_intersections(b), []);
    let m = tessellate(b, &MeshParams::new(1e-4, 0.1)).unwrap();
    assert!(m.faults.is_empty());
    m.welded_triangles().map(|t| { let [a,b,c]=t.map(|i| m.positions[i as usize].to_vector()); a.dot(b.cross(c))/6.0 }).sum()
}
fn faces_of(b: &Body, e: EdgeId) -> Vec<Name> {
    b.edge_fins(e).map(|fin| b.face(b.loop_(b.fin(fin).unwrap().loop_).unwrap().face).unwrap().name).collect()
}
fn edge_between(b: &Body, a: Name, c: Name) -> Name {
    let e = b.edges().find(|&e| { let n = faces_of(b, e); n.contains(&a) && n.contains(&c) }).unwrap();
    b.edge(e).unwrap().name
}
fn blendface(b: &Body) -> Name {
    b.faces().find(|&f| matches!(b.surface(b.face(f).unwrap().surface).unwrap(), Surface::Cylinder(_)) && b.face(f).unwrap().name != Name::from_u128(0)).map(|f| b.face(f).unwrap().name).unwrap()
}

#[test]
fn probe_rebuild_after_raise() {
    let (b, _) = primitive::block(K, 1.0, 0.6, 0.4, &Rigid::IDENTITY).unwrap();
    let e = edge_between(&b, face(FaceRole::PlusZ), face(FaceRole::PlusX));
    let (f, _) = blend(OpKey(1), &b, &[EdgeBlend { edge: e, kind: BlendKind::Fillet { radius: 0.1 } }]).unwrap();
    let fl = blendface(&f);
    let t = std::time::Instant::now();
    let (sharp, j) = delete_faces(OpKey(2), &f, &DeleteSpec { faces: vec![fl], heal: Heal::Grow }).unwrap();
    eprintln!("grow {:?} {:?}", t.elapsed(), j.events());
    let (raised, _) = move_faces(OpKey(3), &sharp, &MoveSpec { faces: vec![face(FaceRole::PlusZ)], motion: Rigid::translation(Vector3::new(0.0,0.0,0.05)), tangent: false, topology: Topology::Keep }).unwrap();
    let e2 = edge_between(&raised, face(FaceRole::PlusZ), face(FaceRole::PlusX));
    let (re, j) = blend(OpKey(4), &raised, &[EdgeBlend { edge: e2, kind: BlendKind::Fillet { radius: 0.1 } }]).unwrap();
    eprintln!("total {:?} journal {:?}", t.elapsed(), j.events().len());
    let want = 1.0*0.6*0.45 - 0.6*0.01*(1.0 - std::f64::consts::PI/4.0);
    eprintln!("vol {} want {}", vol(&re), want);
}

#[test]
fn probe_hole_into_strip() {
    let (p, _) = primitive::block(K, 1.0, 1.0, 0.2, &Rigid::IDENTITY).unwrap();
    let pin = primitive::cylinder(OpKey(9), 0.05, 0.6, &Rigid::IDENTITY).unwrap().0;
    let p = boolean(OpKey(10), BooleanOp::Subtract, &p, &[Tool { key: ToolKey(1), body: &pin, place: Rigid::translation(Vector3::new(0.5,0.4,-0.2)) }]).unwrap().0;
    let e = edge_between(&p, face(FaceRole::PlusZ), face(FaceRole::MinusY));
    let (f, _) = blend(OpKey(1), &p, &[EdgeBlend { edge: e, kind: BlendKind::Fillet { radius: 0.1 } }]).unwrap();
    let wall = f.faces().find(|&x| matches!(f.surface(f.face(x).unwrap().surface).unwrap(), Surface::Cylinder(c) if c.radius() < 0.06)).map(|x| f.face(x).unwrap().name).unwrap();
    for dy in [-0.2, -0.25, -0.3, -0.35] {
        let spec = MoveSpec { faces: vec![wall], motion: Rigid::translation(Vector3::new(0.0, dy, 0.0)), tangent: false, topology: Topology::Change };
        let t = std::time::Instant::now();
        match move_faces(OpKey(5), &f, &spec) {
            Ok((o, _)) => eprintln!("dy {dy}: ok {:?} faces {} vol {}", t.elapsed(), o.faces().count(), vol(&o)),
            Err(e) => eprintln!("dy {dy}: {e:?} {:?}", t.elapsed()),
        }
    }
}

#[test]
fn probe_hole_rebuild_manual() {
    let (p, _) = primitive::block(K, 1.0, 1.0, 0.2, &Rigid::IDENTITY).unwrap();
    let pin = primitive::cylinder(OpKey(9), 0.05, 0.6, &Rigid::IDENTITY).unwrap().0;
    let p = boolean(OpKey(10), BooleanOp::Subtract, &p, &[Tool { key: ToolKey(1), body: &pin, place: Rigid::translation(Vector3::new(0.5,0.4,-0.2)) }]).unwrap().0;
    let e = edge_between(&p, face(FaceRole::PlusZ), face(FaceRole::MinusY));
    let (f, _) = blend(OpKey(1), &p, &[EdgeBlend { edge: e, kind: BlendKind::Fillet { radius: 0.1 } }]).unwrap();
    let fl = f.faces().find(|&x| matches!(f.surface(f.face(x).unwrap().surface).unwrap(), Surface::Cylinder(c) if c.radius() > 0.06)).map(|x| f.face(x).unwrap().name).unwrap();
    let (sharp, _) = delete_faces(OpKey(2), &f, &DeleteSpec { faces: vec![fl], heal: Heal::Grow }).unwrap();
    let wall = sharp.faces().find(|&x| matches!(sharp.surface(sharp.face(x).unwrap().surface).unwrap(), Surface::Cylinder(c) if c.radius() < 0.06)).map(|x| sharp.face(x).unwrap().name).unwrap();
    for dy in [-0.25, -0.3] {
        let spec = MoveSpec { faces: vec![wall], motion: Rigid::translation(Vector3::new(0.0, dy, 0.0)), tangent: false, topology: Topology::Change };
        let (m, _) = move_faces(OpKey(5), &sharp, &spec).unwrap();
        let e2 = edge_between(&m, face(FaceRole::PlusZ), face(FaceRole::MinusY));
        eprintln!("rebuild dy {dy}: {:?}", blend(OpKey(4), &m, &[EdgeBlend { edge: e2, kind: BlendKind::Fillet { radius: 0.1 } }]).map(|(b,_)| b.faces().count()));
    }
}

use ryukei::tweak::{recognise_blends, RecogniseOptions, resize_blends, delete_blends, DeleteBlends};
use ryukei::blend::Chamfer;

fn all_edges(b: &Body, kind: BlendKind) -> Body {
    let all: Vec<EdgeBlend> = b.edges().map(|e| EdgeBlend { edge: b.edge(e).unwrap().name, kind }).collect();
    blend(OpKey(7), b, &all).unwrap().0
}

#[test]
fn probe_recognise() {
    let (b, _) = primitive::block(K, 1.0, 0.6, 0.4, &Rigid::IDENTITY).unwrap();
    let r = all_edges(&b, BlendKind::Fillet { radius: 0.05 });
    let t = std::time::Instant::now();
    let rec = recognise_blends(&r, &RecogniseOptions::default()).unwrap();
    eprintln!("rounded box: {} blends {} corners chains {:?} in {:?}", rec.blends.len(), rec.corners.len(), rec.blends.iter().map(|b| b.chain).collect::<Vec<_>>(), t.elapsed());
    let c = all_edges(&b, BlendKind::Chamfer(Chamfer::Equal { distance: 0.05 }));
    let rec = recognise_blends(&c, &RecogniseOptions::default()).unwrap();
    eprintln!("chamfered box: {} blends {:?}", rec.blends.len(), rec.blends.iter().map(|b| b.kind).collect::<Vec<_>>().first());
    // A boss with its foot filleted, a hole with its rim chamfered.
    let p = primitive::block(K, 1.0, 1.0, 0.2, &Rigid::IDENTITY).unwrap().0;
    let pin = primitive::cylinder(OpKey(9), 0.1, 0.2, &Rigid::IDENTITY).unwrap().0;
    let boss = boolean(OpKey(10), BooleanOp::Unite, &p, &[Tool { key: ToolKey(1), body: &pin, place: Rigid::translation(Vector3::new(0.5,0.5,0.2)) }]).unwrap().0;
    let side = boss.faces().find(|&x| matches!(boss.surface(boss.face(x).unwrap().surface).unwrap(), Surface::Cylinder(_))).map(|x| boss.face(x).unwrap().name).unwrap();
    let e = edge_between(&boss, face(FaceRole::PlusZ), side);
    let (bf, _) = blend(OpKey(11), &boss, &[EdgeBlend { edge: e, kind: BlendKind::Fillet { radius: 0.02 } }]).unwrap();
    let rec = recognise_blends(&bf, &RecogniseOptions::default()).unwrap();
    eprintln!("boss foot: {:?}", rec.blends.iter().map(|b| b.kind).collect::<Vec<_>>());
    let hole = boolean(OpKey(12), BooleanOp::Subtract, &p, &[Tool { key: ToolKey(1), body: &pin, place: Rigid::translation(Vector3::new(0.5,0.5,-0.1)) }]).unwrap().0;
    let wall = hole.faces().find(|&x| matches!(hole.surface(hole.face(x).unwrap().surface).unwrap(), Surface::Cylinder(_))).map(|x| hole.face(x).unwrap().name).unwrap();
    let e = edge_between(&hole, face(FaceRole::PlusZ), wall);
    let (hc, _) = blend(OpKey(13), &hole, &[EdgeBlend { edge: e, kind: BlendKind::Chamfer(Chamfer::Distances { first: 0.02, second: 0.03, first_face: wall }) }]).unwrap();
    let rec = recognise_blends(&hc, &RecogniseOptions::default()).unwrap();
    eprintln!("hole rim chamfer: {:?}", rec.blends.iter().map(|b| b.kind).collect::<Vec<_>>());
    // A hexagonal prism: no chamfers.
    eprintln!("plain block: {}", recognise_blends(&b, &RecogniseOptions::default()).unwrap().blends.len());
    // Resize the rounded box's one edge chain.
    let n = recognise_blends(&r, &RecogniseOptions::default()).unwrap();
    let f0 = n.blends[0].faces[0];
    let t = std::time::Instant::now();
    match resize_blends(OpKey(20), &r, &[(f0, BlendKind::Fillet { radius: 0.08 })]) {
        Ok((o, j)) => { 
            let kept = r.faces().filter(|&f| o.by_name(r.face(f).unwrap().name).is_some()).count();
            let keptv = r.vertices().filter(|&v| o.by_name(r.vertex(v).unwrap().name).is_some()).count();
            eprintln!("resize ok {:?}: faces {} of {} kept names, vertices {} of {}, journal {}", t.elapsed(), kept, r.faces().count(), keptv, r.vertices().count(), j.events().len());
            eprintln!("resized vol {}", vol(&o));
        }
        Err(e) => eprintln!("resize err {e:?}"),
    }
    // Same-size resize: the body back, names and all.
    let (same, _) = resize_blends(OpKey(20), &r, &[(f0, BlendKind::Fillet { radius: 0.05 })]).unwrap();
    let names = |b: &Body| { let mut v: Vec<Name> = b.faces().map(|f| b.face(f).unwrap().name).chain(b.edges().map(|e| b.edge(e).unwrap().name)).chain(b.vertices().map(|v| b.vertex(v).unwrap().name)).collect(); v.sort(); v };
    eprintln!("same-size resize names equal: {}", names(&same) == names(&r));
    let (d, _) = delete_blends(OpKey(21), &bf, &DeleteBlends { max_size: Some(0.03), ..Default::default() }).unwrap();
    eprintln!("defeatured boss: {} faces vol {}", d.faces().count(), vol(&d));
}
