//! step-hunt: finds STEP import, healing and round-trip bugs.
//!
//! It splits the pinned corpora's files into bodies (`split`), runs each
//! through a `ryucad` binary given by path, as an agent would (`stages`,
//! each command a process of its own: `call`), signs every failure and
//! slow stage (`classify`), and judges the findings against the ledger
//! of what was filed (`ledger`, `report`). It links no ryucad: the same
//! finder runs against a release branch's build, `main`'s, or a release.
//! Design 277 in the Coxswain planning project `ryucad-ryukei`.

pub mod body;
pub mod call;
pub mod classify;
pub mod ledger;
pub mod sources;
pub mod split;
pub mod stages;
