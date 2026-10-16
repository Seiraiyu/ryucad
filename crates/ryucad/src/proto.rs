//! The request and result types shared by the CLI, the daemon and every
//! later adapter. One definition, so they never diverge.

use serde::Serialize;

/// The protocol version. A CLI and a daemon talk only when theirs match.
pub const PROTOCOL_VERSION: u32 = 1;

/// What `ryucad version` reports. Future cache keys are built from it.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct VersionInfo {
    pub ryucad: String,
    pub ryukei: String,
    pub commit: String,
    pub target: String,
    pub protocol: u32,
}

impl VersionInfo {
    /// This binary's version information.
    pub fn current() -> Self {
        Self {
            ryucad: env!("CARGO_PKG_VERSION").to_owned(),
            ryukei: ryukei::VERSION.to_owned(),
            commit: env!("RYUCAD_COMMIT").to_owned(),
            target: env!("RYUCAD_TARGET").to_owned(),
            protocol: PROTOCOL_VERSION,
        }
    }
}
