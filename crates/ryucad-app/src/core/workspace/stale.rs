//! What notifications made stale since the window last asked (#849,
//! design 698): `model.changed` and `project.changed` mark it, and
//! [`super::Workspace::pump`] asks each reader once, however many
//! notifications a poll took.

use std::collections::BTreeSet;

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(super) struct Stale {
    /// Parts whose reports to ask for again.
    pub(super) parts: BTreeSet<String>,
    pub(super) part_list: bool,
    pub(super) pdm_config: bool,
    pub(super) bom: bool,
    pub(super) where_used: bool,
    /// The Drawings tile's list.
    pub(super) drawing_list: bool,
    pub(super) diff: bool,
    pub(super) release_check: bool,
    pub(super) doctor: bool,
    pub(super) history: bool,
    pub(super) appearance: bool,
    pub(super) branches: bool,
    pub(super) deps: bool,
    pub(super) equations: bool,
    /// Every part's drawings, for the tree.
    pub(super) drawn: bool,
}
