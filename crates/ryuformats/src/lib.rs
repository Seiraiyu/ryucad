//! The file formats ryucad writes besides STEP: STL, 3MF and USD. Every
//! writer takes a tree of products (a [`scene::Scene`] of solids' meshes,
//! or [`usd::stage::Model`]'s bodies and meshes), placed in each other,
//! and streams its bytes into an [`std::io::Write`]; the same model and
//! options give the same bytes on every platform.
//!
//! This crate is ryukei's neighbour, as `ryustep` is: it depends on the
//! kernel for meshes and motions, never on ryucad, `ryustep` or C++, and
//! the kernel never depends on it. Nothing here reads a file back: these
//! formats are written, never read (STEP is how B-rep comes in).

pub mod scene;
pub mod stl;
pub mod text;
pub mod threemf;
pub mod usd;
pub mod zip;

pub use scene::{Child, Placed, Product, Scene, Solid, Unit};
