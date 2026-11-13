//! What a session keeps only to answer faster (design 541, ryucad 27.E).

use serde_json::Value;
use std::collections::BTreeMap;
use std::sync::Arc;

/// What a session keeps only to answer faster: derived from the project,
/// its refs and the kernel's results, never committed.
#[derive(Default)]
pub struct Caches {
    /// Display meshes of rebuilt bodies.
    pub meshes: crate::mesh::Cache,
    /// The last flat patterns asked for (`flat.get`, DXF exports,
    /// `render --flat`), by the shown body and the options.
    pub flats: crate::sheet_metal::flat::Cache,
    /// What intersecting pairs of placed solids found, for `interference`.
    pub interference: crate::interference::Cache,
    /// Drawing views' projections, by their placed bodies and the view.
    pub projections: crate::drawing::Cache,
    /// Section views' cut bodies and caps, by body, pose and cut.
    pub sections: crate::drawing::section::Cache,
    /// Mass properties worked out, for `mass`.
    pub mass: crate::mass::Cache,
    /// Blends recognised, for `blends`, by body hash and tolerance.
    pub blends: crate::blends::Cache,
    /// Distances between placed solids, for `clearance`.
    pub clearance: crate::clearance::Cache,
    /// The last sweeps' answers (`assembly.sweep`), by what they swept.
    pub sweeps: crate::motion::Cache,
    /// The last merge status, by the key of what it was computed from.
    pub(crate) merge_status: Option<(String, serde_json::Value)>,
    /// The release branch as last read, by the key of the refs it was
    /// read from (see [`crate::release::history`]).
    pub(crate) releases: Option<(String, ryupdm::history::Releases)>,
    /// Release states as last published, by part revision.
    pub(crate) states: Option<crate::release::States>,
    /// Bodies of released content as this build rebuilds them, by content
    /// hash, for `release_result_changed`.
    pub(crate) release_results: BTreeMap<String, BTreeMap<String, String>>,
    /// Parts read at commits for BOMs and where-used, by commit and part.
    pub(crate) structure: ryupdm::structure::Cache,
    /// How many project files each worktree's working files change from
    /// its head, by the head and a digest of the files.
    pub(crate) worktree_changes: BTreeMap<String, usize>,
    /// The last merge previews, by the key of what each was made from,
    /// most recently made first.
    pub(crate) previews: Vec<(String, serde_json::Value)>,
    /// Warnings the revision a request named gives its answer (a stale
    /// `annotation:<id>`), taken by `server::answer_live`.
    pub(crate) rev_warnings: Vec<Value>,
}

impl Caches {
    /// The caches a revision's session starts with (see
    /// [`super::Session::at`]): the verdicts, projections, sections,
    /// sweeps and structure as this session has them, meshes afresh over
    /// `disk`, and nothing else.
    pub(crate) fn for_revision(&self, disk: Option<Arc<crate::cache::Disk>>) -> Caches {
        Caches {
            meshes: crate::mesh::Cache::with_disk(disk),
            flats: Default::default(),
            interference: self.interference.clone(),
            projections: self.projections.clone(),
            sections: self.sections.clone(),
            mass: self.mass.clone(),
            blends: self.blends.clone(),
            clearance: self.clearance.clone(),
            sweeps: self.sweeps.clone(),
            merge_status: None,
            releases: None,
            states: None,
            release_results: BTreeMap::new(),
            structure: self.structure.clone(),
            worktree_changes: BTreeMap::new(),
            previews: Vec::new(),
            rev_warnings: Vec::new(),
        }
    }
}
