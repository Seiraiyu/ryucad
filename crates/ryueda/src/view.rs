//! What a participant's window shows besides its selection (ryu-session's
//! `Product::View` for ryueda): a compare against a Git revision (plan
//! 4.B). Stage 3 adds the sheet, layer and camera it shows.

/// What a participant shows.
#[derive(Clone, Debug, Default)]
pub struct View {
    /// The Git revision the participant compares the working files
    /// against, as given (`HEAD`, a branch, a commit); `None` when it
    /// isn't comparing.
    pub compare: Option<String>,
}
