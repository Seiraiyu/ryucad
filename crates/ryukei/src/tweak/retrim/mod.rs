//! The local re-trim (design 399): the tweak engine for changes of
//! topology. The faces a tweak changes and their neighbours are the
//! region; every face of it becomes a working face, its surface carried
//! past its domain over the region's box; the faces outside the region
//! stay as they are. The working faces and the outside faces go through
//! the self-union's one-body boolean pipeline ([`crate::repair::resolve`]),
//! and the pieces kept are those the frontier walk reaches from the
//! outside faces (see `walk`). Nothing about which side of a surface is
//! material is guessed: the walk only counts.

pub(crate) mod region;
pub(crate) mod working;

#[cfg(test)]
pub(crate) mod tests;
