//! Boards (design 732): `boards/<board>/board.json` and its placements,
//! loaded with the project, their shapes built through `ryu-board`, and
//! the `board.*` commands.

pub mod assembly;
pub mod cmd;
pub mod connect;
pub mod courtyard;
pub mod drawcmd;
pub mod drawing;
pub mod drc;
pub mod export;
pub mod fab;
pub mod file;
pub mod graphic;
pub mod group;
pub mod keepcmd;
pub mod keepout;
pub mod layout;
pub mod load;
pub mod measure;
pub mod occ;
pub mod opening;
pub mod outline;
pub mod pad;
pub mod place;
pub mod pour;
pub mod preset;
pub mod presetcmd;
pub mod render;
pub mod route;
pub mod rulecmd;
pub mod rules;
pub mod shapes;
pub mod stackup;
#[cfg(test)]
pub(crate) mod testing;
pub mod text;

pub use file::{
    BOARD_FILE, BOARD_KIND, BOARDS, FORMAT, HOLE_KIND, HOLES, PLACEMENT_KIND, PLACEMENTS,
    RULES_FILE, RULES_KIND, TRACK_KIND, TRACKS, VIA_KIND, VIAS,
};
