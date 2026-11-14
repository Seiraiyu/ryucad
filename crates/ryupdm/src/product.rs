//! What ryupdm reads of a product's projects. Every core function that
//! reads a project is generic over a [`Product`] (`…_for::<P>`); ryucad's
//! is `crate::cad::Cad` (feature `cad`), ryueda's `ryueda::pdm::Eda`.
//! ryupdm's own files (`ryupdm.json`, `releases/`) sit beside any
//! product's manifest; where a part's properties live is the product's.

use ryu_doc::error::Result;
use ryu_doc::id::{Id, ProjectId};
use ryu_doc::source::Source;
use ryu_doc::units::Unit;
use serde_json::Value;

/// What a project's manifest says that the core reads.
#[derive(Clone, Debug, PartialEq)]
pub struct Identity {
    /// The project's ID, from its first PDM write on.
    pub project: Option<ProjectId>,
    /// The ID the project had before `pdm new-id` gave it a new one:
    /// records naming it are its upstream's history.
    pub forked_from: Option<ProjectId>,
    /// The unit a part's quantities default to.
    pub unit: Unit,
}

/// The files one part's revision reads at a commit, besides the files at
/// the project's root and `ryupdm.json` (`history::part_files_for`):
/// single files, and folders read whole. Paths are relative to the
/// project, `/`-separated.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct PartFiles {
    pub files: Vec<String>,
    pub folders: Vec<String>,
}

/// A product whose projects ryupdm manages.
pub trait Product: ryu_git::Layout {
    /// The folders below the project holding its parts' files, which a
    /// revision's files (`history::files_for`) and the commit guard read
    /// besides the root files and `releases/`.
    const PART_FOLDERS: &'static [&'static str];

    /// The project's releasable parts, in ID order.
    fn parts(src: &dyn Source) -> Result<Vec<Id>>;

    /// Where `part`'s properties live, relative to the project.
    fn properties_path(part: &Id) -> String;

    /// The files `part`'s revision reads at a commit.
    fn part_files(part: &Id) -> PartFiles;

    /// The part a project file (relative to the project) belongs to, if
    /// any: what a commit touching it may make stale.
    fn part_of(path: &str) -> Option<Id>;

    /// `part`'s content hash: what a release record says was released.
    /// A part that can't be read has none, the error saying which file.
    fn content(src: &dyn Source, part: &Id) -> Result<String>;

    /// What the manifest (as `read_json` read it) says; `None` when it
    /// doesn't read as the product's manifest.
    fn identity(manifest: &Value) -> Option<Identity>;
}

/// The identity of the project `src` holds, `None` when it has no
/// manifest or the manifest doesn't read (a conflicted one included:
/// `read_json` refuses it).
pub fn identity<P: Product>(src: &dyn Source) -> Option<Identity> {
    crate::read_json(src, P::MANIFEST)
        .ok()
        .flatten()
        .and_then(|v| P::identity(&v))
}
