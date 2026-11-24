//! The ryuplm desktop app (design 938, The window): a client of the
//! catalogue's daemon that lists its items in windows of rows, shows the
//! open item, its BOM, where it is used, the findings, changes, releases,
//! imports, documents and activity, and points with people and agents by
//! stable reference. `core` is everything but the window, tested
//! headless; `ui` is the window, a thin view over `core`.

pub mod core;
pub mod ui;
