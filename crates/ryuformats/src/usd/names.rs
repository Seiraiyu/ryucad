//! Prim names from ryucad's IDs. An ID (`bolt-2`, a copy's `holes:3`) is
//! made an identifier by turning every character that can't be in one
//! into `_` and putting `_` before a leading digit; names that then
//! collide among siblings take `_2`, `_3`, … in the order they are met.
//! The ID itself goes in the prim's `customData`.

use super::layer::is_identifier;
use std::collections::BTreeSet;

/// `id` as an identifier.
pub fn identifier(id: &str) -> String {
    let mut s: String = id
        .chars()
        .map(|c| {
            if c == '_' || c.is_ascii_alphanumeric() {
                c
            } else {
                '_'
            }
        })
        .collect();
    if !s.starts_with(|c: char| c == '_' || c.is_ascii_alphabetic()) {
        s.insert(0, '_');
    }
    debug_assert!(is_identifier(&s));
    s
}

/// The names given to one prim's children, so none collide.
#[derive(Clone, Debug, Default)]
pub struct Siblings {
    taken: BTreeSet<String>,
}

impl Siblings {
    /// Siblings that may not take any of `reserved` (a prim's own
    /// children's fixed names, such as `brep` and `mesh`).
    pub fn reserving(reserved: &[&str]) -> Siblings {
        Siblings {
            taken: reserved.iter().map(|&s| s.to_owned()).collect(),
        }
    }

    /// A free name for `id`: its identifier, numbered if that is taken.
    pub fn name(&mut self, id: &str) -> String {
        let base = identifier(id);
        let mut name = base.clone();
        let mut n = 2;
        while self.taken.contains(&name) {
            name = format!("{base}_{n}");
            n += 1;
        }
        self.taken.insert(name.clone());
        name
    }
}

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
