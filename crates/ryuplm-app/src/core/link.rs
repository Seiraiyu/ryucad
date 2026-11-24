//! The window's connection to a catalogue's daemon: `ryu_app::link`'s,
//! as ryuplm's client ([`crate::core::product::Plm`]).

use crate::core::product::Plm;
use ryu_doc::error::Result;

pub use ryu_app::link::{Event, Wake};

pub type Link = ryu_app::link::Link<Plm>;
pub type Daemon = ryu_session::client::Daemon<ryuplm::Plm>;

/// The `ryuplm` binary the app starts daemons from: beside the app, else
/// on `PATH`, and of the app's own build.
pub fn find_ryuplm() -> Result<Daemon> {
    ryu_app::link::find::<Plm>()
}
