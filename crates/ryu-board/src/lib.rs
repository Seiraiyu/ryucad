//! The board model every ryu product measures boards in.
//!
//! Integer geometry (`geom`), the symbol vocabulary (`symbol`), exact shapes
//! of everything on a board (`shape`), the board model (`model`) and
//! pictures of shapes (`render`).
//! Product-neutral: nothing here knows ODB++, projects or sessions, and the
//! crate depends on no kernel and no product.

pub mod geom;
pub mod model;
pub mod render;
pub mod shape;
pub mod symbol;
#[cfg(feature = "testing")]
pub mod testing;
