//! Boards (design 732): `boards/<board>/board.json` and its placements,
//! loaded with the project, their shapes built through `ryu-board`, and
//! the `board.*` commands.

pub mod cmd;
pub mod file;
pub mod graphic;
pub mod load;
pub mod occ;
pub mod outline;
pub mod preset;

pub use file::{BOARD_FILE, BOARD_KIND, BOARDS, FORMAT, PLACEMENT_KIND, PLACEMENTS};
