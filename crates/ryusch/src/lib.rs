//! ryusch: ryueda's schematic engine. It takes a block as data (its
//! symbols, nets, ports and child blocks, the sheet's paper, the previous
//! layout and an agent's instructions) and gives a layout: each part's
//! place, each net's wires or labels, each text's place, with the lint
//! that judges it and SVG that draws it. It reads no files, projects or
//! sessions; ryueda feeds it and writes what it gives.
//!
//! An agent makes the choices a schematic designer makes (what sits
//! beside what, wire or label); the engine does the drafting and keeps
//! the hard rules of readability.

pub mod font;
pub mod geom;
pub mod input;
pub mod layout;
pub mod shape;
mod widths;
