//! Boards (design 732): `boards/<board>/board.json` and its placements,
//! loaded with the project, their shapes built through `ryu-board`, and
//! the `board.*` commands.

pub mod file;
pub mod load;
pub mod outline;

pub use file::{BOARD_FILE, BOARD_KIND, BOARDS, FORMAT, PLACEMENT_KIND, PLACEMENTS};

/// Whether a board's `preset` is one this ryueda knows.
pub fn preset_known(name: &str) -> bool {
    name == file::JLC_2L
}
