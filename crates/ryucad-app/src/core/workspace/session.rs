//! The window's place in the session, a sub-state of the window's
//! workspace (design 541, "The app's workspace"): its mode, the
//! participants and what this window's participant shows (visibility,
//! sections, explode, motion), settings, the doctor and the cache.

use super::*;

/// This window's session state.
#[derive(Default)]
pub struct Session {
    /// The window's mode (design 374): View when it opens and whenever it
    /// connects again; Annotate and Direct edit by the person's choice.
    pub mode: crate::core::modes::Mode,
    pub people: People,
    /// The participant whose camera this window follows.
    pub following: Option<String>,
    pub settings: Settings,
    /// The last doctor report, once asked for.
    pub doctor: Option<Value>,
    /// Whether a report is on its way, and whether the project changed
    /// meanwhile, so the badges need another.
    pub(super) doctor_waiting: bool,
    pub(super) doctor_again: bool,
    /// What the disk cache held when last asked (`cache.stats`), for the
    /// Preferences window.
    pub cache: Option<Value>,
    /// The last answer to storing or removing the shared cache's token
    /// (`cache.login`, `cache.logout`), or its error as `{"error": …}`.
    pub cache_token: Option<Value>,
}
