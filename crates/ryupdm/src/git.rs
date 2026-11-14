//! What Git's merge and diff do with ryupdm's files, for every product's
//! driver: which kinds merge whole, which pointers are one value, and
//! (below) their textconv lines. Settings (`ryupdm.json`) and properties
//! merge by the generic per-key walk: `approvals.rules` is an array, so
//! one value; `approvals.keys` per key; each property per key.

use crate::record::{OBSOLETE_KIND, RELEASE_KIND};

/// Whether a file of `kind` merges as one value: release and obsolete
/// records. Two different records at one path are one conflict.
pub fn whole(kind: &str) -> bool {
    matches!(kind, RELEASE_KIND | OBSOLETE_KIND)
}

/// A lock's label is one value: two branches locking one label at
/// different commits conflict there (`/projects/<ID>/labels/<label>`),
/// not in its commit and content apart.
pub fn lock_atom(at: &str) -> bool {
    let seg: Vec<&str> = at.split('/').collect();
    matches!(seg.as_slice(), ["", "projects", _, "labels", _])
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn records_merge_whole_and_a_locks_labels_are_one_value() {
        assert!(whole("ryupdm.release") && whole("ryupdm.obsolete"));
        assert!(!whole("ryupdm.config") && !whole("ryupdm.properties"));
        assert!(lock_atom("/projects/0123/labels/bolt~1A"));
        assert!(!lock_atom("/projects/0123/labels"));
        assert!(!lock_atom("/projects/0123/labels/bolt~1A/commit"));
        assert!(!lock_atom("/dependencies/lib"));
    }
}
