//! Edge flanges: bends and flanges added on straight sheet edges.
//!
//! Each [`EdgeFlange`](super::EdgeFlange) is planned in the frame of its
//! edge ([`plan`]): `s` along the edge from its start vertex, `x` into the
//! base from the wall the edge bounds, `h` along the side's outward normal,
//! the way the flange folds.

pub(crate) mod plan;

#[cfg(test)]
mod tests;
