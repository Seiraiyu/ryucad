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
