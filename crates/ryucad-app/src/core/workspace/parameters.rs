//! The parameter panel and the Equations tile, a sub-state of the
//! window's workspace (design 541, "The app's workspace").

use super::*;

/// The parameters: the panel of the part shown and the Equations table.
#[derive(Default)]
pub struct Parameters {
    /// The parameter panel, for the part it shows (see `show_params`).
    pub params: Option<Params>,
    pub(super) params_part: Option<String>,
    /// The Equations tile's table, once it is open (see `show_equations`).
    pub equations: Option<Equations>,
    pub(super) equations_open: bool,
}
