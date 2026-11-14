//! What the view shows beyond the model, a sub-state of the window's
//! workspace: named views, a face's normal, appearance, section planes,
//! Measure and Mass properties.

use super::*;

/// The view's tools.
#[derive(Default)]
pub struct View {
    /// The named views of the part the window shows, for the cube's menu
    /// and the view selector.
    pub views: crate::core::views::NamedViews,
    /// A camera normal to a face, come from the daemon, for the view to
    /// turn to.
    pub looked: Option<Camera>,
    /// What the window shows changed (View > Show, or an agent's
    /// `ryucad focus`): the view frames it again, as it frames the first
    /// view (issue #211).
    pub refocused: bool,
    /// The Appearance window.
    pub appearance: crate::core::appearance::Editor,
    /// How the shown bodies look, as the daemon last said.
    pub(super) looks_read: BTreeMap<BodyKey, Arc<crate::core::appearance::Look>>,
    /// The last appearance reading asked for: an older answer is dropped.
    pub(super) looks_asked: u64,
    /// The planes this window cuts with, and the gizmo's drag.
    pub sections: crate::core::section::Sections,
    /// Tools > Measure: on or off, its picks and what they measured.
    pub measure: Tool,
    /// Tools > Mass properties, once asked for.
    pub mass: Option<Mass>,
}
