//! ryupdm: product data management for ryu projects, local and
//! offline. A project's parts carry properties checked against the
//! project's settings (this crate's [`props`] and [`config`]), and are
//! released under labels its revision scheme derives ([`label`]), each
//! release naming the hash of what it released ([`content`]) in a record
//! under `releases/` ([`record`]) that is official once the release
//! branch holds it ([`history`]). From those, each part has a state:
//! in work, released, modified or obsolete ([`state`]), and the records
//! are checked against the parts, the branch and the tags ([`check`]).
//! A merge's result is checked against its sides for faults neither had
//! ([`merge`]).
//!
//! ryupdm reads projects through a [`product::Product`]; ryucad's is
//! [`cad::Cad`], behind the default `cad` feature. It never calls the kernel,
//! a session, the daemon or the app: ryucad exposes it through its
//! protocol and CLI, and a service can link it to read any revision.
//!
//! # As a library
//!
//! Every function takes a [`ryu_doc::source::Source`], and where
//! history matters a `gix::Repository` and the project's place in it, so
//! it works on a bare repository at any commit ([`history::files`] reads
//! one): [`content::hash`], [`props::Pdm::read`],
//! [`history::Releases::read`], [`state::derive`], [`check::run`] (whose
//! findings have a versioned shape, [`check::SCHEMA`]) and
//! [`check::staged`], the commit guard, and [`merge::validate`], what a
//! merge broke. This API follows the workspace's
//! version, and changes to these functions are called out in the release
//! notes.

pub mod approvals;
pub mod attest;
pub mod bom;
pub mod check;
pub mod config;
pub mod content;
pub mod deps;
pub mod history;
pub mod items;
pub mod label;
pub mod merge;
pub mod package;
pub mod props;
pub mod record;
pub mod state;
pub mod structure;
pub mod where_used;

/// ryupdm's settings, beside a project's manifest, in every product.
pub const PDM_CONFIG: &str = "ryupdm.json";
/// The folder of release records, beside a project's manifest.
pub const RELEASES: &str = "releases";

/// The product named in the messages of readers that aren't yet told
/// which product reads them: ryupdm's own files (settings, properties,
/// records, attestations, packages). Plan 10.B threads the product
/// through them; until then every message reads as it always has.
pub(crate) const NAMED: &str = "ryucad";

/// [`ryu_doc::files::header_in`] for ryupdm's own files.
pub(crate) fn header<'a>(v: &'a Value, kind: &str, newest: u64) -> Result<ryu_doc::files::Obj<'a>> {
    ryu_doc::files::header_in(v, kind, newest, NAMED)
}

use ryu_doc::canonical;
use ryu_doc::error::{Code, Error, Result};
use ryu_doc::id::ProjectId;
use ryu_doc::source::Source;
use serde_json::Value;

/// A new project ID: 128 bits from the operating system's random source.
pub fn new_project_id() -> Result<ProjectId> {
    let mut bytes = [0u8; 16];
    getrandom::fill(&mut bytes)
        .map_err(|e| Error::new(Code::Internal, format!("no random source: {e}")))?;
    Ok(ProjectId::from_bytes(bytes))
}

/// The JSON of the file at `at` (relative to the project), `None` when it
/// isn't there. A file a merge left with conflicts is `merge_conflict`.
pub(crate) fn read_json(src: &dyn Source, at: &str) -> Result<Option<Value>> {
    let Some(bytes) = src.read(at).map_err(|e| e.with("path", at))? else {
        return Ok(None);
    };
    let text = std::str::from_utf8(&bytes).map_err(|_| {
        Error::new(Code::InvalidJson, format!("{at} is not UTF-8")).with("path", at)
    })?;
    let v = canonical::parse(text).map_err(|e| e.with("path", at))?;
    match ryu_doc::files::conflicted_in(&v, at, NAMED) {
        Some(e) => Err(e),
        None => Ok(Some(v)),
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn the_cores_folders_are_ryucads() {
        assert_eq!(super::PDM_CONFIG, "ryupdm.json");
        assert_eq!(super::RELEASES, "releases");
        #[cfg(feature = "cad")]
        {
            assert_eq!(super::PDM_CONFIG, ryucad_doc::doc::project::PDM_CONFIG);
            assert_eq!(super::RELEASES, ryucad_doc::doc::project::RELEASES);
        }
    }

    use super::*;

    #[test]
    fn project_ids_are_random() {
        let (a, b) = (new_project_id().unwrap(), new_project_id().unwrap());
        assert_ne!(a, b);
        assert_eq!(ProjectId::parse(a.as_str()).unwrap(), a);
    }

    #[test]
    fn files_read_as_json_or_say_why_not() {
        let mut src = std::collections::BTreeMap::new();
        assert_eq!(read_json(&src, "ryupdm.json").unwrap(), None);
        src.insert("ryupdm.json".to_owned(), b"{".to_vec());
        let e = read_json(&src, "ryupdm.json").unwrap_err();
        assert_eq!(
            (e.code, &e.details["path"]),
            (Code::InvalidJson, &serde_json::json!("ryupdm.json"))
        );
        src.insert(
            "ryupdm.json".to_owned(),
            br#"{"conflicts":{"/revisions":{"kind":"value"}}}"#.to_vec(),
        );
        assert_eq!(
            read_json(&src, "ryupdm.json").unwrap_err().code,
            Code::MergeConflict
        );
    }
}
