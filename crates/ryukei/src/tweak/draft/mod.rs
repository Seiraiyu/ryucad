//! Draft (design 384): faces tilted to an angle from a pull direction
//! about a neutral plane or a parting line, so a part releases from a
//! mould. A drafted face's surface is ruled from its hinge: a plane
//! through a line, a cone through a circle about an axis along the pull,
//! else a sweep of a straight profile along the hinge in the binormal
//! frame of the pull.

pub(crate) mod surface;
