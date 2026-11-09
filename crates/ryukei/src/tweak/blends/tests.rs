//! Unit tests of blend recognition and editing.

use super::recognise::{across, chamfer, edges_of, fillet};
use super::*;
use crate::blend::{Chamfer, EdgeBlend, blend};
use crate::boolean::{BooleanOp, Tool, boolean};
use crate::geom::{Rigid, Surface, Vector3};
use crate::name::{FaceRole, Role, ToolKey};
use crate::primitive;
use crate::topo::FaceId;

const K: OpKey = OpKey(0x13c0);

fn all_edges(b: &Body, kind: BlendKind) -> Body {
    let all: Vec<EdgeBlend> = b.edges().map(|e| EdgeBlend { edge: b.edges[e].name, kind }).collect();
    blend(OpKey(7), b, &all).unwrap().0
}

fn kind_of(b: &Body, f: FaceId) -> &'static str {
    match &b.surfaces[b.faces[f].surface] {
        Surface::Plane(_) => "plane",
        Surface::Cylinder(_) => "cyl",
        Surface::Cone(_) => "cone",
        Surface::Sphere(_) => "sphere",
        Surface::Torus(_) => "torus",
        Surface::Nurbs(_) => "nurbs",
        Surface::Pipe(_) => "pipe",
        _ => "other",
    }
}

#[test]
fn debug_probe() {
    let tol = DEFAULT_TOLERANCE;
    let (b, _) = primitive::block(K, 1.0, 0.6, 0.4, &Rigid::IDENTITY).unwrap();
    let r = all_edges(&b, BlendKind::Fillet { radius: 0.05 });
    for f in r.faces() {
        if kind_of(&r, f) == "sphere" {
            for e in edges_of(&r, f) {
                let g = across(&r, e, f);
                let ta = crate::blend::convex::tangent_along(&r, e, 1e-6);
                eprintln!(
                    "sphere edge: across {:?} fillet {:?} tangent {:?}",
                    g.map(|g| kind_of(&r, g)),
                    g.map(|g| fillet(&r, g, tol).is_some()),
                    ta
                );
            }
            break;
        }
    }
    let c = all_edges(&b, BlendKind::Chamfer(Chamfer::Equal { distance: 0.05 }));
    for f in c.faces() {
        let n = edges_of(&c, f).len();
        eprintln!("chamfered face {} edges {} chamfer {:?}", kind_of(&c, f), n, chamfer(&c, f, tol).map(|x| x.kind));
    }
    let p = primitive::block(K, 1.0, 1.0, 0.2, &Rigid::IDENTITY).unwrap().0;
    let pin = primitive::cylinder(OpKey(9), 0.1, 0.2, &Rigid::IDENTITY).unwrap().0;
    let boss = boolean(
        OpKey(10),
        BooleanOp::Unite,
        &p,
        &[Tool { key: ToolKey(1), body: &pin, place: Rigid::translation(Vector3::new(0.5, 0.5, 0.2)) }],
    )
    .unwrap()
    .0;
    let top = Name::derive(K, Role::Face(FaceRole::PlusZ), &[]);
    let side = boss.faces().find(|&x| kind_of(&boss, x) == "cyl").unwrap();
    let e = boss
        .edges()
        .find(|&e| {
            let fs: Vec<Name> = boss.edge_fins(e).map(|fin| boss.faces[boss.loops[boss.fins[fin].loop_].face].name).collect();
            fs.contains(&top) && fs.contains(&boss.faces[side].name)
        })
        .unwrap();
    let (bf, _) = blend(OpKey(11), &boss, &[EdgeBlend { edge: boss.edges[e].name, kind: BlendKind::Fillet { radius: 0.02 } }]).unwrap();
    for f in bf.faces() {
        if kind_of(&bf, f) == "torus" {
            eprintln!("torus sense {:?}", bf.faces[f].sense);
            for e in edges_of(&bf, f) {
                let g = across(&bf, e, f);
                eprintln!(
                    "  edge to {:?} tangent {:?}",
                    g.map(|g| kind_of(&bf, g)),
                    crate::blend::convex::tangent_along(&bf, e, 1e-6)
                );
            }
            eprintln!("  fillet {:?}", fillet(&bf, f, tol).map(|c| c.kind));
        }
    }
}
