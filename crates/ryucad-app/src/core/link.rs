//! The app's connection to a project's daemon: `ryu_app::link`'s
//! (ryueda 3.C), as ryucad's client ([`crate::core::product::Ryucad`]).

use crate::core::product::Ryucad;
use ryucad::client::Daemon;
use ryucad::error::Result;

pub use ryu_app::link::{Event, Wake};

/// A live connection to a ryucad daemon.
pub type Link = ryu_app::link::Link<Ryucad>;

/// The `ryucad` binary the app starts daemons from: beside the app, else
/// on `PATH`, and of the app's own build.
pub fn find_ryucad() -> Result<Daemon> {
    ryu_app::link::find::<Ryucad>()
}

/// Refuses a `ryucad` of another build than the app.
pub fn check_build(daemon: &Daemon) -> Result<()> {
    ryu_app::link::check_build::<Ryucad>(daemon)
}
