//! The XML reader (design 935): records found by an element path from the
//! root, each flattened to its attributes and its elements' text, named
//! by their paths relative to the record. Elements are matched by local
//! name, so a namespaced export reads without declaring its namespaces,
//! as XLSX's do; roxmltree refuses DTDs, so no entity expands.

/// Whether `p` is an element path: `/A/B` when absolute, `A/B` when not,
/// each step a local name (no `@`, `[`, `]`, `:` or space).
pub fn path_ok(p: &str, absolute: bool) -> bool {
    let rest = if absolute {
        match p.strip_prefix('/') {
            Some(r) => r,
            None => return false,
        }
    } else {
        p
    };
    !rest.is_empty()
        && rest
            .split('/')
            .all(|s| !s.is_empty() && !s.contains(['@', '[', ']', ':', ' ']))
}
