//! Boards (design 732): `boards/<board>/board.json` and its placements,
//! loaded with the project, their shapes built through `ryu-board`, and
//! the `board.*` commands.

pub mod cmd;
pub mod connect;
pub mod courtyard;
pub mod drc;
pub mod fab;
pub mod file;
pub mod graphic;
pub mod layout;
pub mod load;
pub mod occ;
pub mod opening;
pub mod outline;
pub mod pad;
pub mod preset;
pub mod render;
pub mod route;
pub mod shapes;
#[cfg(test)]
pub(crate) mod testing;
pub mod text;

pub use file::{
    BOARD_FILE, BOARD_KIND, BOARDS, FORMAT, HOLE_KIND, HOLES, PLACEMENT_KIND, PLACEMENTS,
    TRACK_KIND, TRACKS, VIA_KIND, VIAS,
};
