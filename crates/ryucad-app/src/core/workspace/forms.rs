//! The property panel's forms, a sub-state of the window's workspace
//! (design 541, "The app's workspace"): the chosen feature's form, a
//! dimension's, an insert's preview, and each one's save and conflict.

use super::*;

/// The forms: what they hold, the requests they send (the `impl
/// Workspace` below) and their answers (`Forms::answer`).
#[derive(Default)]
pub struct Forms {
    /// The property panel's form, for the feature chosen in the tree.
    pub form: Option<Form>,
    /// The property panel's form for a dimension chosen by its label;
    /// shown instead of `form`.
    pub dimension: Option<DimForm>,
    /// What the property panel was last asked to show, a feature or a
    /// dimension: only its answer opens a form.
    pub(super) form_for: Option<(String, String)>,
    pub(super) dimension_for: Option<(String, String, String)>,
    /// What moving the insert form's chosen pin changes, once asked.
    pub insert_preview: Option<crate::core::inserts::Preview>,
}
