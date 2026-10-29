#[cfg(test)]
mod tests {
    use super::*;
    use crate::usd::usda::to_string;
    use ryukei::geom::Vector3;
    use ryukei::mesh::{MeshParams, tessellate};
    use ryukei::name::OpKey;

    fn solid(name: &str, x: f64) -> Solid {
        let b = ryukei::primitive::block(OpKey(1), x, x, x, &Rigid::IDENTITY)
            .unwrap()
            .0;
        let m = tessellate(&b, &MeshParams::new(1e-5, 0.26)).unwrap();
        Solid {
            name: name.into(),
            body: Arc::new(b),
            mesh: Some(Arc::new(m)),
        }
    }

    fn opts() -> Options {
        Options {
            unit: Unit::Millimeter,
            brep: true,
            mesh: true,
            pipe_tolerance: 1e-6,
            layer_data: vec![("version".into(), "0.0.0".into())],
        }
    }

    /// A frame placing a pin twice, at the working files and at release
    /// B, and a subassembly placing the pin once more.
    pub(crate) fn frame() -> Model {
        let at = |x: f64| Rigid::translation(Vector3::new(x, 0.0, 0.01));
        let product = |part: &str, label: Option<&str>, solids, children| Product {
            id: part.to_uppercase(),
            name: part.into(),
            part: part.into(),
            label: label.map(str::to_owned),
            solids,
            children,
        };
        let child = |product, x, name: &str| Child {
            product,
            place: at(x),
            name: name.into(),
        };
        Model {
            products: vec![
                product(
                    "frame",
                    None,
                    vec![solid("plate", 0.1)],
                    vec![
                        child(1, 0.0, "a"),
                        child(2, 0.05, "b-1"),
                        child(3, 0.08, "sub"),
                    ],
                ),
                product("pin", None, vec![solid("shank", 0.01)], vec![]),
                product("pin", Some("B"), vec![solid("shank", 0.01)], vec![]),
                product("hold", None, vec![], vec![child(1, 0.0, "holes:1")]),
            ],
        }
    }

    #[test]
    fn an_assembly_is_instanced_prototypes() {
        let l = layer(&frame(), &opts()).unwrap();
        assert_eq!(l.default_prim.as_deref(), Some("frame"));
        assert_eq!(l.meters_per_unit, 0.001);
        let names = |p: &Prim| {
            p.children
                .iter()
                .map(|c| c.name.clone())
                .collect::<Vec<_>>()
        };
        let [protos, root] = l.prims.as_slice() else {
            panic!()
        };
        assert_eq!(protos.specifier, Specifier::Class);
        assert_eq!(names(protos), ["pin", "pin_B"]);
        assert_eq!(names(root), ["plate", "a", "b_1", "sub"]);
        let a = &root.children[1];
        assert_eq!(a.meta.instanceable, Some(true));
        assert_eq!(a.meta.references, ["/Prototypes/pin"]);
        assert_eq!(root.children[2].meta.references, ["/Prototypes/pin_B"]);
        let sub = &root.children[3];
        assert_eq!(sub.meta.kind.as_deref(), Some("subassembly"));
        assert_eq!(names(sub), ["holes_1"]);
        assert_eq!(sub.children[0].meta.references, ["/Prototypes/pin"]);
        assert_eq!(names(&protos.children[0]), ["shank"]);
        assert_eq!(names(&protos.children[0].children[0]), ["brep", "mesh"]);
        let text = to_string(&l).unwrap();
        assert!(text.contains("matrix4d xformOp:transform = ( (1, 0, 0, 0), (0, 1, 0, 0), (0, 0, 1, 0), (50, 0, 10, 1) )"));
        assert!(text.contains(&format!("string omni_solid = \"{SCHEMA_COMMIT}\"")));
        assert!(text.contains("string reference = \"pin/shank\""));
        assert!(text.contains("string instance = \"b-1\""));
    }

    #[test]
    fn a_part_alone_has_no_prototypes() {
        let mut m = frame();
        m.products.truncate(1);
        m.products[0].children.clear();
        let l = layer(&m, &opts()).unwrap();
        assert_eq!(l.prims.len(), 1);
        assert_eq!(l.prims[0].meta.kind.as_deref(), Some("component"));
        assert_eq!(l.prims[0].children[0].name, "plate");
    }

    #[test]
    fn breps_or_meshes_or_both() {
        let mut o = opts();
        o.mesh = false;
        let l = layer(&frame(), &o).unwrap();
        let shank = &l.prims[0].children[0].children[0];
        assert_eq!(shank.children.len(), 1);
        assert_eq!(shank.children[0].type_name.as_deref(), Some("BrepArray"));
        o.mesh = true;
        o.brep = false;
        let l = layer(&frame(), &o).unwrap();
        let mesh = &l.prims[0].children[0].children[0].children[0];
        assert_eq!(mesh.type_name.as_deref(), Some("Mesh"));
        assert!(mesh.attributes.iter().any(|a| a.name == "ryucad:faceName"));
        assert!(!to_string(&l).unwrap().contains("omni_solid"));
        o.mesh = false;
        assert_eq!(layer(&frame(), &o).unwrap_err(), StageError::Nothing);
    }

    #[test]
    fn transforms_are_rows() {
        let q = Rigid::translation(Vector3::new(0.001, 0.002, 0.003)).then_after(&Rigid::rotation(
            Vector3::new(0.0, 0.0, 1.0).normalize().unwrap(),
            core::f64::consts::FRAC_PI_2,
        ));
        let m = matrix(&q, Unit::Meter);
        // x goes to y.
        assert!((m[0][1] - 1.0).abs() < 1e-15 && m[0][0].abs() < 1e-15);
        assert_eq!(m[3], [0.001, 0.002, 0.003, 1.0]);
    }
}
