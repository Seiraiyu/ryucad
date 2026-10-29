#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ids_become_identifiers() {
        assert_eq!(identifier("bolt-2"), "bolt_2");
        assert_eq!(identifier("holes:3"), "holes_3");
        assert_eq!(identifier("grid:1:2"), "grid_1_2");
        assert_eq!(identifier("9x"), "_9x");
        assert_eq!(identifier("plate"), "plate");
    }

    #[test]
    fn colliding_names_are_numbered_in_order() {
        let mut s = Siblings::reserving(&["mesh"]);
        assert_eq!(s.name("a-b"), "a_b");
        assert_eq!(s.name("a_b"), "a_b_2");
        assert_eq!(s.name("a:b"), "a_b_3");
        assert_eq!(s.name("mesh"), "mesh_2");
        assert_eq!(s.name("c"), "c");
    }
}
