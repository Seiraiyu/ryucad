//! Tessellation: a body's faces as triangles that are certified to lie
//! within a chord tolerance of the exact geometry.

// The stages land one task at a time; `tessellate` uses them all.
#![allow(dead_code)]

mod bound;
mod cdt;
mod uv;
