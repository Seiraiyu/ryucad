//! Releases, PDM settings, part properties, where-used and the BOM, a
//! sub-state of the window's workspace (design 541, "The app's
//! workspace", its `releases`).

use super::*;

/// What the Releases and BOM tabs and the Part section show of ryupdm.
#[derive(Default)]
pub struct Pdm {
    /// The Releases tab, and whether it was opened (it follows changes
    /// from then on).
    pub releases: ReleaseTab,
    pub(super) releases_open: bool,
    /// Every part's release state (ryupdm), for the tree's badges.
    pub states: States,
    /// ryupdm's settings (`pdm.config`): `project` is null until PDM is
    /// set up.
    pub pdm_config: Option<Value>,
    /// The Part section's form, for the part it shows (see `show_props`).
    pub props: Option<PartProps>,
    pub(super) props_part: Option<String>,
    /// The BOM tab, whether it was opened (it follows changes from then
    /// on), and whether a reading is on its way and another wanted after.
    pub bom: BomTab,
    pub(super) bom_open: bool,
    pub(super) bom_waiting: bool,
    pub(super) bom_again: bool,
    /// The BOM tab's Export CSV…, while open.
    pub bom_export: Option<crate::core::bom::BomExport>,
    /// Where the Part section's part is used, once read, and its part.
    pub where_used: Option<crate::core::where_used::WhereUsed>,
    pub(super) where_used_part: Option<String>,
    /// The review service's organization-wide where-used on its way, for
    /// this part; and one that came before `bom.where_used` answered.
    pub(super) org: Option<(String, std::sync::mpsc::Receiver<Result<Value>>)>,
    pub(super) org_early: Option<(String, Result<Vec<crate::core::where_used::Use>>)>,
}
