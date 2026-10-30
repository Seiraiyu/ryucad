//! USD: a stage of every product as `usda` text or `usdc` crate bytes,
//! each solid as an omniSolid `BrepArray` (its exact B-rep) with a `Mesh`
//! beside it, an assembly's parts defined once and placed as instances.

pub mod brep;
pub mod compress;
pub mod layer;
pub mod mesh;
pub mod names;
pub mod stage;
pub mod usda;
pub mod usdc;
pub mod usdz;
