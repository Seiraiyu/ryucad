//! What a session keeps only to answer faster (design 541, ryucad 27.E):
//! derived from the project, its refs and the kernel's results, never
//! committed. Each entry's lifetime is what it was before 27.E.
//!
//! **By content** (meshes, flats, verdicts, projections, sections,
//! sweeps): keyed by the hashes of what they were made from, so never
//! stale; each cache bounds itself when it is filled (meshes by the mesh
//! budget, least recently used first; projections and sections at their
//! `MAX`; flats and sweeps by count). **By state** (the merge status, the
//! release branch, release states and results, ryupdm's structure,
//! worktree counts, merge previews): keyed by what they were read from,
//! and given only while that key is what the caller reads now, here, in
//! the getters. [`Caches::changed`] is where a move lets entries go.

use crate::doc::id::Id;
use serde_json::Value;
use std::collections::{BTreeMap, BTreeSet};
use std::sync::Arc;

/// How many merge previews a session keeps, most recently made first.
pub(crate) const PREVIEWS: usize = 4;

/// What moved under a session, for [`Caches::changed`].
#[derive(Clone, Copy, Debug)]
pub(crate) enum Moved<'a> {
    /// The project's files: a reload of what the session or anyone wrote.
    Files,
    /// The heal or edit budget.
    Budget,
    /// A fetch into the dependency cache.
    Fetched,
    /// The worktrees as `worktree.list` read them: the keys (head and
    /// digest) of the changes it counted.
    Worktrees(&'a BTreeSet<String>),
}

/// What a session keeps only to answer faster: derived from the project,
/// its refs and the kernel's results, never committed.
#[derive(Default)]
pub struct Caches {
    /// Display meshes of rebuilt bodies.
    meshes: crate::mesh::Cache,
    /// The last flat patterns asked for (`flat.get`, DXF exports,
    /// `render --flat`), by the shown body and the options.
    flats: crate::sheet_metal::flat::Cache,
    /// What intersecting pairs of placed solids found, for `interference`.
    interference: crate::interference::Cache,
    /// Drawing views' projections, by their placed bodies and the view.
    projections: crate::drawing::Cache,
    /// Section views' cut bodies and caps, by body, pose and cut.
    sections: crate::drawing::section::Cache,
    /// Mass properties worked out, for `mass`.
    mass: crate::mass::Cache,
    /// Blends recognised, for `blends`, by body hash and tolerance.
    blends: crate::blends::Cache,
    /// Distances between placed solids, for `clearance`.
    clearance: crate::clearance::Cache,
    /// The last sweeps' answers (`assembly.sweep`), by what they swept.
    sweeps: crate::motion::Cache,
    /// The last merge status, by the key of what it was computed from.
    merge_status: Option<(String, serde_json::Value)>,
    /// The release branch as last read, by the key of the refs it was
    /// read from (see [`crate::release::history`]).
    releases: Option<(String, ryupdm::history::Releases)>,
    /// Release states as last published, by part revision.
    states: Option<crate::release::States>,
    /// Bodies of released content as this build rebuilds them, by content
    /// hash, for `release_result_changed`.
    release_results: BTreeMap<String, BTreeMap<String, String>>,
    /// Parts read at commits for BOMs and where-used, by commit and part.
    structure: ryupdm::structure::Cache,
    /// How many project files each worktree's working files change from
    /// its head, by the head and a digest of the files.
    worktree_changes: BTreeMap<String, usize>,
    /// The last merge previews, by the key of what each was made from,
    /// most recently made first.
    previews: Vec<(String, serde_json::Value)>,
    /// Warnings the revision a request named gives its answer (a stale
    /// `annotation:<id>`), taken by `server::answer_live`.
    rev_warnings: Vec<Value>,
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

    /// The disk cache beneath the meshes.
    pub(crate) fn set_disk(&mut self, disk: Option<Arc<crate::cache::Disk>>) {
        self.meshes.disk = disk;
    }

    /// Lets go of what `moved` leaves no one to ask for. A change of files
    /// or budgets lets nothing here go: every entry by content is keyed by
    /// what it was made from, and every entry by state is checked against
    /// its key when read (the store's rebuilds, which a change does make
    /// stale, are the session's). A fetch lets go of what was read of
    /// other projects through the cache's repositories, keeping the
    /// project's own structure; a worktree listing, of the counts of
    /// worktrees no longer there.
    pub(crate) fn changed(&mut self, moved: Moved<'_>) {
        match moved {
            Moved::Files | Moved::Budget => {}
            Moved::Fetched => self
                .structure
                .retain(|(project, _, _), _| project.is_none()),
            Moved::Worktrees(seen) => self.worktree_changes.retain(|k, _| seen.contains(k)),
        }
    }

    /// The display meshes' bytes, which the session's budget counts.
    pub(crate) fn mesh_bytes(&self) -> u64 {
        self.meshes.bytes() as u64
    }

    /// How many verdicts are kept (interference, clearance, mass,
    /// blends), which the budget's `Fixed` counts.
    pub(crate) fn verdicts(&self) -> u64 {
        (self.interference.len() + self.clearance.len() + self.mass.len() + self.blends.len())
            as u64
    }

    pub fn meshes(&self) -> &crate::mesh::Cache {
        &self.meshes
    }

    pub fn meshes_mut(&mut self) -> &mut crate::mesh::Cache {
        &mut self.meshes
    }

    pub fn flats_mut(&mut self) -> &mut crate::sheet_metal::flat::Cache {
        &mut self.flats
    }

    pub fn interference(&self) -> &crate::interference::Cache {
        &self.interference
    }

    pub fn interference_mut(&mut self) -> &mut crate::interference::Cache {
        &mut self.interference
    }

    pub fn projections_mut(&mut self) -> &mut crate::drawing::Cache {
        &mut self.projections
    }

    pub fn sections_mut(&mut self) -> &mut crate::drawing::section::Cache {
        &mut self.sections
    }

    pub fn mass(&self) -> &crate::mass::Cache {
        &self.mass
    }

    pub fn mass_mut(&mut self) -> &mut crate::mass::Cache {
        &mut self.mass
    }

    pub fn blends(&self) -> &crate::blends::Cache {
        &self.blends
    }

    pub fn blends_mut(&mut self) -> &mut crate::blends::Cache {
        &mut self.blends
    }

    pub fn clearance(&self) -> &crate::clearance::Cache {
        &self.clearance
    }

    pub fn clearance_mut(&mut self) -> &mut crate::clearance::Cache {
        &mut self.clearance
    }

    pub fn sweeps(&self) -> &crate::motion::Cache {
        &self.sweeps
    }

    pub fn sweeps_mut(&mut self) -> &mut crate::motion::Cache {
        &mut self.sweeps
    }

    /// Both verdict caches at once, as a sweep fills them.
    pub fn verdicts_mut(&mut self) -> crate::motion::Caches<'_> {
        crate::motion::Caches {
            interference: &mut self.interference,
            clearance: &mut self.clearance,
        }
    }

    /// The merge status kept for `key`, if that is what was kept.
    pub(crate) fn merge_status(&self, key: &str) -> Option<&Value> {
        self.merge_status
            .as_ref()
            .filter(|(k, _)| k == key)
            .map(|(_, v)| v)
    }

    pub(crate) fn put_merge_status(&mut self, key: String, v: Value) {
        self.merge_status = Some((key, v));
    }

    /// The release branch as read from the refs `key` names, if kept.
    pub(crate) fn releases(&self, key: &str) -> Option<&ryupdm::history::Releases> {
        self.releases
            .as_ref()
            .filter(|(k, _)| k == key)
            .map(|(_, r)| r)
    }

    pub(crate) fn put_releases(&mut self, key: String, r: ryupdm::history::Releases) {
        self.releases = Some((key, r));
    }

    /// The release states last published, by part, taken: those kept
    /// for `key`, else none (each part's own revision is checked by
    /// [`crate::release::summary`]).
    pub(crate) fn kept_states(&mut self, key: &str) -> BTreeMap<Id, (Option<String>, Value)> {
        match self.states.take() {
            Some(s) if s.key == key => s.parts,
            _ => Default::default(),
        }
    }

    pub(crate) fn put_states(&mut self, s: crate::release::States) {
        self.states = Some(s);
    }

    /// Released content's bodies as this build rebuilds them, by its
    /// content hash.
    pub(crate) fn release_result(&self, content: &str) -> Option<&BTreeMap<String, String>> {
        self.release_results.get(content)
    }

    pub(crate) fn put_release_result(&mut self, content: String, bodies: BTreeMap<String, String>) {
        self.release_results.insert(content, bodies);
    }

    /// ryupdm's structure, taken for a reader to fill;
    /// [`Caches::put_structure`] gives it back.
    pub(crate) fn take_structure(&mut self) -> ryupdm::structure::Cache {
        std::mem::take(&mut self.structure)
    }

    pub(crate) fn put_structure(&mut self, c: ryupdm::structure::Cache) {
        self.structure = c;
    }

    /// How many files a worktree's working files change, by its head and
    /// digest.
    pub(crate) fn worktree_changes(&self, key: &str) -> Option<usize> {
        self.worktree_changes.get(key).copied()
    }

    pub(crate) fn put_worktree_changes(&mut self, key: String, n: usize) {
        self.worktree_changes.insert(key, n);
    }

    /// The merge preview kept for `key`.
    pub(crate) fn preview(&self, key: &str) -> Option<&Value> {
        self.previews.iter().find(|(k, _)| k == key).map(|(_, v)| v)
    }

    /// Keeps a merge preview, first; the oldest past [`PREVIEWS`] goes.
    pub(crate) fn put_preview(&mut self, key: String, v: Value) {
        self.previews.insert(0, (key, v));
        self.previews.truncate(PREVIEWS);
    }

    /// A warning the revision a request named gives its answer.
    pub(crate) fn note_rev_warning(&mut self, w: Value) {
        self.rev_warnings.push(w);
    }

    /// The warnings noted for the request being answered, taken.
    pub(crate) fn take_rev_warnings(&mut self) -> Vec<Value> {
        std::mem::take(&mut self.rev_warnings)
    }

    #[cfg(test)]
    pub(crate) fn merge_status_key(&self) -> Option<&str> {
        self.merge_status.as_ref().map(|(k, _)| k.as_str())
    }

    #[cfg(test)]
    pub(crate) fn releases_key(&self) -> Option<&str> {
        self.releases.as_ref().map(|(k, _)| k.as_str())
    }

    #[cfg(test)]
    pub(crate) fn release_results_len(&self) -> usize {
        self.release_results.len()
    }

    #[cfg(test)]
    pub(crate) fn structure(&self) -> &ryupdm::structure::Cache {
        &self.structure
    }

    #[cfg(test)]
    pub(crate) fn has_state(&self) -> bool {
        self.merge_status.is_some() || !self.previews.is_empty()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn states(key: &str) -> crate::release::States {
        crate::release::States {
            key: key.into(),
            parts: [(
                crate::part::part_id("p").unwrap(),
                (Some("1".to_owned()), json!({})),
            )]
            .into(),
        }
    }

    #[test]
    fn what_is_kept_by_state_is_given_only_for_its_key() {
        let mut c = Caches::default();
        c.put_merge_status("a".into(), json!(1));
        assert_eq!(c.merge_status("a"), Some(&json!(1)));
        assert_eq!(c.merge_status("b"), None);
        c.put_releases("r".into(), Default::default());
        assert!(c.releases("r").is_some() && c.releases("s").is_none());
        c.put_worktree_changes("w".into(), 3);
        assert_eq!(
            (c.worktree_changes("w"), c.worktree_changes("x")),
            (Some(3), None)
        );
        c.put_release_result("h".into(), BTreeMap::new());
        assert!(c.release_result("h").is_some() && c.release_result("g").is_none());
        c.put_states(states("k"));
        assert!(c.kept_states("other").is_empty(), "another key kept states");
        c.put_states(states("k"));
        assert_eq!(c.kept_states("k").len(), 1);
        assert!(c.kept_states("k").is_empty(), "states are taken");
    }

    #[test]
    fn previews_keep_the_most_recent_first() {
        let mut c = Caches::default();
        for i in 0..PREVIEWS + 2 {
            c.put_preview(format!("k{i}"), json!(i));
        }
        assert_eq!(c.previews.len(), PREVIEWS);
        let last = PREVIEWS + 1;
        assert_eq!(c.preview(&format!("k{last}")), Some(&json!(last)));
        assert_eq!(c.preview("k0"), None, "the oldest stayed");
    }

    #[test]
    fn listing_worktrees_lets_go_of_counts_not_seen() {
        let mut c = Caches::default();
        c.put_worktree_changes("seen".into(), 1);
        c.put_worktree_changes("gone".into(), 2);
        c.changed(Moved::Worktrees(&BTreeSet::from(["seen".to_owned()])));
        assert_eq!(
            (c.worktree_changes("seen"), c.worktree_changes("gone")),
            (Some(1), None)
        );
    }

    #[test]
    fn files_budgets_and_fetches_keep_what_they_dont_name() {
        let mut c = Caches::default();
        c.put_merge_status("a".into(), json!(1));
        c.put_worktree_changes("w".into(), 1);
        c.put_preview("p".into(), json!(0));
        c.put_release_result("x".into(), BTreeMap::new());
        for m in [Moved::Files, Moved::Budget, Moved::Fetched] {
            c.changed(m);
        }
        assert!(c.merge_status("a").is_some());
        assert!(c.worktree_changes("w").is_some());
        assert!(c.preview("p").is_some());
        assert!(c.release_result("x").is_some());
    }

    #[test]
    fn revision_warnings_are_taken_once() {
        let mut c = Caches::default();
        c.note_rev_warning(json!({"code": "w"}));
        assert_eq!(c.take_rev_warnings(), [json!({"code": "w"})]);
        assert!(c.take_rev_warnings().is_empty());
    }
}
