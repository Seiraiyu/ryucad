#[cfg(test)]
mod tests {
    use super::*;
    use crate::usd::layer::{Specifier, UpAxis};

    fn layer() -> Layer {
        let mut data = Dict::default();
        let mut ryucad = Dict::default();
        ryucad.string("version", "0.0.0").string("chord", "0.01 mm");
        data.dict("ryucad", ryucad);
        let mut root = Prim::new(Specifier::Def, Some("Xform"), "frame").unwrap();
        root.meta.kind = Some("assembly".into());
        let mut proto = Prim::new(Specifier::Class, None, "Prototypes").unwrap();
        let mut pin = Prim::new(Specifier::Class, Some("Xform"), "pin").unwrap();
        pin.meta
            .custom_data
            .string("part", "pin")
            .string("odd key", "a \"b\"\n");
        proto.children.push(pin);
        let mut a = Prim::new(Specifier::Def, Some("Xform"), "a").unwrap();
        a.meta.instanceable = Some(true);
        a.meta.references = vec!["/Prototypes/pin".into()];
        let mut m = [[0.0; 4]; 4];
        for (i, row) in m.iter_mut().enumerate() {
            row[i] = 1.0;
        }
        m[3] = [10.0, -0.0, 0.5, 1.0];
        a.varying("xformOp:transform", Value::Matrix4d(m)).unwrap();
        a.uniform(
            "xformOpOrder",
            Value::TokenArray(vec!["xformOp:transform".into()]),
        )
        .unwrap();
        root.children.push(a);
        let mut mesh = Prim::new(Specifier::Def, Some("Mesh"), "mesh").unwrap();
        mesh.meta.api_schemas = vec!["BrepPointAPI:vertexPoint".into()];
        mesh.varying("points", Value::Point3fArray(vec![[0.1, 2.0, -3.5]]))
            .unwrap();
        mesh.varying("normals", Value::Normal3fArray(vec![[0.0, 0.0, 1.0]]))
            .unwrap()
            .interpolation("faceVarying");
        mesh.uniform("ryucad:faceName", Value::StringArray(vec!["ab".into()]))
            .unwrap()
            .custom();
        mesh.uniform("edge:vertexIndices", Value::Int2Array(vec![[0, 1]]))
            .unwrap();
        mesh.uniform("doubleSided", Value::Bool(false)).unwrap();
        root.children.push(mesh);
        Layer {
            default_prim: Some("frame".into()),
            meters_per_unit: 0.001,
            up_axis: UpAxis::Z,
            custom_layer_data: data,
            prims: vec![proto, root],
        }
    }

    #[test]
    fn a_layer_as_text() {
        let text = to_string(&layer()).unwrap();
        let want = r#"#usda 1.0
(
    customLayerData = {
        dictionary ryucad = {
            string version = "0.0.0"
            string chord = "0.01 mm"
        }
    }
    defaultPrim = "frame"
    metersPerUnit = 0.001
    upAxis = "Z"
)

class "Prototypes"
{
    class Xform "pin" (
        customData = {
            string part = "pin"
            string "odd key" = "a \"b\"\n"
        }
    )
    {
    }
}

def Xform "frame" (
    kind = "assembly"
)
{
    def Xform "a" (
        instanceable = true
        prepend references = </Prototypes/pin>
    )
    {
        matrix4d xformOp:transform = ( (1, 0, 0, 0), (0, 1, 0, 0), (0, 0, 1, 0), (10, 0, 0.5, 1) )
        uniform token[] xformOpOrder = ["xformOp:transform"]
    }

    def Mesh "mesh" (
        prepend apiSchemas = ["BrepPointAPI:vertexPoint"]
    )
    {
        point3f[] points = [(0.1, 2, -3.5)]
        normal3f[] normals = [(0, 0, 1)] (
            interpolation = "faceVarying"
        )
        custom uniform string[] ryucad:faceName = ["ab"]
        uniform int2[] edge:vertexIndices = [(0, 1)]
        uniform bool doubleSided = false
    }
}
"#;
        assert_eq!(text, want);
    }

    #[test]
    fn numbers_that_arent_finite_are_refused() {
        let mut l = layer();
        l.prims[1].children[1]
            .uniform("extent", Value::Float3Array(vec![[f32::NAN, 0.0, 0.0]]))
            .unwrap();
        let e = to_string(&l).unwrap_err();
        assert_eq!(e.kind(), io::ErrorKind::InvalidData);
        l = layer();
        l.meters_per_unit = f64::INFINITY;
        assert!(to_string(&l).is_err());
    }
}
