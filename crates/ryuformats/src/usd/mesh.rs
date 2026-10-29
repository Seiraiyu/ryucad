//! A solid's ryukei mesh as a `UsdGeomMesh` prim, which every USD viewer
//! draws without the omniSolid plugin: its welded positions as `points`
//! (in the stage's unit, rounded to `f32` once), all triangles, each
//! corner's outward normal (`faceVarying`), `rightHanded`, single-sided,
//! no subdivision, and `primvars:ryucad:face`, each triangle's face index
//! into the BrepArray's face arrays ([`super::brep::face_order`]). Written
//! without a BrepArray, it carries `ryucad:faceName` itself.

use super::brep::face_order;
use super::layer::{BadName, Prim, Specifier, Value};
use crate::scene::Unit;
use ryukei::mesh::BodyMesh;
use ryukei::topo::{Body, FaceId};
use std::collections::BTreeMap;

/// `mesh` (of `body`) as a `Mesh` prim named `name`, in `unit`; with
/// `names`, the faces' names too.
pub fn mesh_prim(
    name: &str,
    body: &Body,
    mesh: &BodyMesh,
    unit: Unit,
    names: bool,
) -> Result<Prim, BadName> {
    let order = face_order(body);
    let index: BTreeMap<FaceId, i32> = order
        .iter()
        .enumerate()
        .map(|(i, &f)| (f, i as i32))
        .collect();
    let k = unit.per_meter();
    let points: Vec<[f32; 3]> = mesh
        .positions
        .iter()
        .map(|p| [(p.x * k) as f32, (p.y * k) as f32, (p.z * k) as f32])
        .collect();
    let mut lo = [f32::INFINITY; 3];
    let mut hi = [f32::NEG_INFINITY; 3];
    for p in &points {
        for j in 0..3 {
            lo[j] = lo[j].min(p[j]);
            hi[j] = hi[j].max(p[j]);
        }
    }
    if points.is_empty() {
        (lo, hi) = ([0.0; 3], [0.0; 3]);
    }
    let corners: Vec<u32> = mesh.triangles.iter().flatten().copied().collect();
    let mut faces = vec![0; mesh.triangles.len()];
    for f in &mesh.faces {
        for t in f.triangles.clone() {
            faces[t as usize] = index[&f.face];
        }
    }
    let mut p = Prim::new(Specifier::Def, Some("Mesh"), name)?;
    p.varying("points", Value::Point3fArray(points))?;
    p.varying(
        "faceVertexCounts",
        Value::IntArray(vec![3; mesh.triangles.len()]),
    )?;
    p.varying(
        "faceVertexIndices",
        Value::IntArray(
            corners
                .iter()
                .map(|&c| mesh.corners[c as usize].position as i32)
                .collect(),
        ),
    )?;
    p.varying(
        "normals",
        Value::Normal3fArray(
            corners
                .iter()
                .map(|&c| {
                    let n = mesh.corners[c as usize].normal.get();
                    [n.x as f32, n.y as f32, n.z as f32]
                })
                .collect(),
        ),
    )?
    .interpolation("faceVarying");
    p.varying("primvars:ryucad:face", Value::IntArray(faces))?
        .interpolation("uniform");
    p.varying("extent", Value::Float3Array(vec![lo, hi]))?;
    p.uniform("orientation", Value::Token("rightHanded".into()))?;
    p.uniform("doubleSided", Value::Bool(false))?;
    p.uniform("subdivisionScheme", Value::Token("none".into()))?;
    if names {
        let names = order
            .iter()
            .map(|&f| body.face(f).expect("a live face").name.to_string())
            .collect();
        p.uniform("ryucad:faceName", Value::StringArray(names))?
            .custom();
    }
    Ok(p)
}

#[cfg(test)]
mod tests {
    use super::*;
    use ryukei::geom::Rigid;
    use ryukei::mesh::{MeshParams, tessellate};
    use ryukei::name::OpKey;

    #[test]
    fn a_blocks_mesh() {
        let b = ryukei::primitive::block(OpKey(1), 0.01, 0.02, 0.03, &Rigid::IDENTITY)
            .unwrap()
            .0;
        let m = tessellate(&b, &MeshParams::new(1e-5, 0.26)).unwrap();
        let p = mesh_prim("mesh", &b, &m, Unit::Millimeter, true).unwrap();
        let get = |n: &str| &p.attributes.iter().find(|a| a.name == n).unwrap().value;
        let Value::Point3fArray(pts) = get("points") else {
            panic!()
        };
        assert_eq!(pts.len(), 8);
        let Value::IntArray(counts) = get("faceVertexCounts") else {
            panic!()
        };
        assert_eq!(counts, &vec![3; 12]);
        let Value::IntArray(faces) = get("primvars:ryucad:face") else {
            panic!()
        };
        // Two triangles a face, each face index once a pair.
        let mut f = faces.clone();
        f.sort();
        assert_eq!(f, [0, 0, 1, 1, 2, 2, 3, 3, 4, 4, 5, 5]);
        assert_eq!(
            get("extent"),
            &Value::Float3Array(vec![[0.0, 0.0, 0.0], [10.0, 20.0, 30.0]])
        );
        let Value::StringArray(names) = get("ryucad:faceName") else {
            panic!()
        };
        let mut sorted = names.clone();
        sorted.sort();
        assert_eq!(&sorted, names, "faces in name order");
        assert_eq!(names.len(), 6);
        let without = mesh_prim("mesh", &b, &m, Unit::Meter, false).unwrap();
        assert!(
            without
                .attributes
                .iter()
                .all(|a| a.name != "ryucad:faceName")
        );
    }
}
