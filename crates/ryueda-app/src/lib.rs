//! The ryueda desktop app (design 519): a client of the project's daemon
//! that draws its `scene`, picks by stable reference, and shows findings,
//! instructions and activity. `core` is everything but the window, tested
//! headless; `ui` is the window, a thin view over `core`.

pub mod core;
pub mod ui;
