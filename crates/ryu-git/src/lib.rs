//! Git for every ryu product: a project's repository read without
//! writing to it (revisions, the merge in progress, the index), and what
//! registers a product with Git (attribute lines, driver entries and the
//! shared hook). Nothing here knows a product's files: a product names
//! them through [`Layout`] and [`drivers::Drivers`], as ryucad's
//! `ryucad::Cad` and ryueda's `ryueda::Eda` do.

pub mod drivers;
pub mod repo;
#[cfg(test)]
pub(crate) mod testing;

pub use repo::{MergeState, Repo, Revision, Step, Unmerged, Worktree, merging_projects};

/// Where a product's project files are, as Git sees them.
pub trait Layout: 'static {
    /// The product (`ryucad`, `ryueda`): its messages name it.
    const NAME: &'static str;
    /// The file that marks a project folder (`ryucad.json`).
    const MANIFEST: &'static str;
    /// Files at the project's root a revision holds.
    const ROOT_FILES: &'static [&'static str];
    /// Folders below the project a revision holds whole.
    const FOLDERS: &'static [&'static str];
    /// Whether `rel` (relative to the project, `/`-separated) is a file
    /// the product's session tracks: what a commit of the index must hold
    /// as the work tree has it.
    fn project_file(rel: &str) -> bool;
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_layout_names_its_files() {
        use testing::Toy;
        assert_eq!(Toy::MANIFEST, "toy.json");
        assert!(Toy::project_file("toy.json"));
        assert!(Toy::project_file("things/a.json"));
        assert!(!Toy::project_file("things/deep/a.json"));
        assert!(!Toy::project_file("README"));
    }
}
