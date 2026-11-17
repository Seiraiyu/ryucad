//! ryuplm: product lifecycle management (design 734). A catalogue is a
//! folder of items in Git (`ryuplm.json`, `items/<ab>/<id>.json`), each
//! with a part number, a description, an AML, documents and a BOM,
//! whether or not any authoring tool ever touches it. Items are
//! ryupdm parts of kind `item` ([`pdm`]), so revisions, releases, states
//! and the commit guard are ryupdm's.

pub use ryu_doc::{canonical, error, id, source, units};

pub mod ids;

/// ryuplm, as ryupdm, ryu-git and ryu-session know it.
pub struct Plm;
