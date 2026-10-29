#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn names_are_checked() {
        assert!(is_identifier("_9a"));
        assert!(is_identifier("Prototypes"));
        assert!(!is_identifier("9a"));
        assert!(!is_identifier("a-b"));
        assert!(!is_identifier(""));
        assert!(is_property_name("brep:edge3dLine:curve3d:line:origin"));
        assert!(!is_property_name("brep::x"));
        assert!(!is_property_name("primvars:ryucad:face-name"));
        assert!(Prim::new(Specifier::Def, Some("Xform"), "a-b").is_err());
        assert!(Prim::new(Specifier::Def, Some("Bad Type"), "a").is_err());
        let mut p = Prim::new(Specifier::Def, None, "a").unwrap();
        assert!(p.uniform("x:", Value::Bool(true)).is_err());
        p.uniform("x:y", Value::Bool(true)).unwrap().custom();
        assert!(p.attributes[0].uniform && p.attributes[0].custom);
    }

    #[test]
    fn values_know_their_types() {
        assert_eq!(Value::Int2Array(vec![]).type_name(), "int2[]");
        assert_eq!(Value::Matrix4d([[0.0; 4]; 4]).type_name(), "matrix4d");
        assert!(!Value::DoubleArray(vec![1.0, f64::NAN]).is_finite());
        assert!(!Value::Point3fArray(vec![[0.0, f32::INFINITY, 0.0]]).is_finite());
        assert!(Value::TokenArray(vec!["same".into()]).is_finite());
    }
}
