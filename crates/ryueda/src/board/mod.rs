//! Boards (design 732): `boards/<board>/board.json` and its placements,
//! loaded with the project, their shapes built through `ryu-board`, and
//! the `board.*` commands.

pub mod cmd;
pub mod file;
pub mod graphic;
pub mod load;
pub mod occ;
pub mod opening;
pub mod outline;
pub mod pad;
pub mod preset;
pub mod text;

pub use file::{BOARD_FILE, BOARD_KIND, BOARDS, FORMAT, PLACEMENT_KIND, PLACEMENTS};
