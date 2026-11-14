//! The property panel's forms, a sub-state of the window's workspace
//! (design 541, "The app's workspace"): the chosen feature's form, a
//! dimension's, an insert's preview, and each one's save and conflict.

/// The forms: what they hold, the requests they send (the `impl
/// Workspace` below) and their answers (`Forms::answer`).
#[derive(Default)]
pub struct Forms {}
