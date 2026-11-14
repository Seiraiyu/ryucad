//! Import and export, a sub-state of the window's workspace: File > Import
//! STEP…, Update import… and Export….

/// The import and export dialogs.
#[derive(Default)]
pub struct Exchange {
    /// File > Import STEP… or Update import…, while open.
    pub import: Option<crate::core::step::Import>,
    /// File > Export…, while open.
    pub export: Option<crate::core::export::Export>,
    /// What the last export chose, which the next one starts from.
    pub export_choices: crate::core::export::Choices,
}
