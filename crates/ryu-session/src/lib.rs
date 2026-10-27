//! What every ryu product's session daemon shares. A product (ryucad,
//! ryueda) is a marker type implementing [`Product`]; the modules here are
//! generic over it, and the product names their types for itself
//! (`ryucad::endpoint::Endpoint` is `Endpoint<ryucad::Cad>`), so its own
//! callers never see the parameter.
//!
//! Nothing here reads a product's files or knows its requests: those stay
//! the product's, behind the trait.

pub mod endpoint;
pub mod notify;
pub mod rpc;

/// A product built on these sessions: its name and the variables that
/// move its folders.
pub trait Product: Sized + Send + Sync + 'static {
    /// The product's name: its binary, and the folder and socket names its
    /// daemons use (`ryucad`). Two products never share a daemon.
    const NAME: &'static str;
    /// The variable that overrides the runtime folder (tests keep their
    /// daemons apart with it): `RYUCAD_RUNTIME_DIR`.
    const RUNTIME_DIR_VAR: &'static str;
}
