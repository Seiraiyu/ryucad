//! The window's connection to a project's daemon: `ryu_app::link`'s, as
//! ryueda's client ([`crate::core::product::Eda`]).

use crate::core::product::Eda;
use ryu_doc::error::Result;

pub use ryu_app::link::{Event, Wake};

pub type Link = ryu_app::link::Link<Eda>;
pub type Daemon = ryu_session::client::Daemon<ryueda::Eda>;

/// The `ryueda` binary the app starts daemons from: beside the app, else
/// on `PATH`, and of the app's own build.
pub fn find_ryueda() -> Result<Daemon> {
    ryu_app::link::find::<Eda>()
}
