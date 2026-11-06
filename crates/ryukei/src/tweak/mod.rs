//! Face tweaks (design 384): chosen faces take new surfaces (moved,
//! offset or replaced), and their neighbours' edges and vertices are
//! found again on them, keeping the body's topology and every name.
//! Whatever would change the topology is refused, naming where.

// The engine is wired up a task at a time; the public operations
// (plan 9.A's Task 10) use everything here.
#![allow(dead_code, unused_imports)]

pub(crate) mod motion;

pub(crate) use motion::Motion;
