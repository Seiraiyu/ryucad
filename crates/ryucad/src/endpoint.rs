//! Where a project's daemon listens: `ryu-session`'s endpoints, for
//! ryucad. One per user and project, outside the project: a Unix domain
//! socket in a private folder, or a named pipe that only its owner may
//! open.

use crate::Cad;
use crate::error::Result;
use ryu_session::Product;
use std::path::PathBuf;

/// Where ryucad's daemon for one project listens.
pub type Endpoint = ryu_session::endpoint::Endpoint<Cad>;

/// Overrides the runtime folder; tests use it to keep their daemons apart.
pub const RUNTIME_DIR_VAR: &str = Cad::RUNTIME_DIR_VAR;

/// The per-user folder for sockets and logs, created if needed. On Unix it
/// must belong to this user and be closed to everyone else.
pub fn runtime_dir() -> Result<PathBuf> {
    ryu_session::endpoint::runtime_dir::<Cad>()
}
