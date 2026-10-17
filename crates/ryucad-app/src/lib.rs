//! The ryucad desktop app. `core` is everything but the window: the
//! daemon connection and the model it shows, tested headless. `ui` is the
//! window, a thin view over `core`.

pub mod core;
pub mod ui;
