use super::*;
use crate::geom::Rigid;
use crate::primitive;

/// Runs each step of an offset in turn, saying which refuses.
fn steps(b: &Body, d: f64) -> Result<(), String> {
    let spec = OffsetSpec {
        distance: d,
        faces: Vec::new(),
    };
    let plan = classify::Plan::new(b, &spec).map_err(|e| format!("plan: {e:?}"))?;
    let geo = edges::curves(&plan).map_err(|e| format!("edges: {e:?}"))?;
    let vgeo = vertices::vertices(&plan, &geo).map_err(|e| format!("vertices: {e:?}"))?;
    build::build(OpKey(2), &plan, &geo, &vgeo).map_err(|e| format!("build: {e:?}"))?;
    Ok(())
}

#[test]
fn a_box_grown_step_by_step() {
    let (b, _) = primitive::block(OpKey(1), 1.0, 0.6, 0.4, &Rigid::IDENTITY).unwrap();
    assert_eq!(steps(&b, 0.1), Ok(()));
}

#[test]
fn scratch_top() {
    use crate::name::{FaceRole, Name, Role};
    let (b, _) = primitive::block(OpKey(1), 1.0, 0.6, 0.4, &Rigid::IDENTITY).unwrap();
    let spec = OffsetSpec {
        distance: 0.1,
        faces: vec![(Name::derive(OpKey(1), Role::Face(FaceRole::PlusZ), &[]), 0.0)],
    };
    let (o, _) = offset_body(OpKey(2), &b, &spec).unwrap();
    let m = crate::mesh::tessellate(&o, &crate::mesh::MeshParams::new(2e-4, 0.1)).unwrap();
    for f in &m.faults {
        eprintln!("fault {f:?}");
    }
    for f in o.faces() {
        eprintln!("face {f:?} {:?} {:?}", o.surfaces[o.faces[f].surface], o.faces[f].sense);
        for l in o.face_loops(f) {
            for fin in o.loop_fins(l) {
                let e = &o.edges[o.fins[fin].edge];
                let c = &o.curves[e.curve];
                eprintln!(
                    "   edge {:?} {:?} {:?} -> {:?} tol {}",
                    o.fins[fin].edge,
                    e.interval,
                    c.point(e.interval[0]),
                    c.point(e.interval[1]),
                    e.tolerance.get()
                );
            }
        }
    }
}
