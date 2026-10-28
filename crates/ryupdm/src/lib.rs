//! ryupdm: product data management for ryucad projects, local and
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
//! ryupdm reads projects through `ryucad-doc` and never calls the kernel,
//! a session, the daemon or the app: ryucad exposes it through its
//! protocol and CLI, and a service can link it to read any revision.
//!
//! # As a library
//!
//! Every function takes a [`ryucad_doc::doc::source::Source`], and where
//! history matters a `gix::Repository` and the project's place in it, so
//! it works on a bare repository at any commit ([`history::files`] reads
//! one): [`content::hash`], [`props::Pdm::read`],
//! [`history::Releases::read`], [`state::derive`], [`check::run`] (whose
//! findings have a versioned shape, [`check::SCHEMA`]) and
//! [`check::staged`], the commit guard, and [`merge::validate`], what a
//! merge broke. This API follows the workspace's
//! version, and changes to these functions are called out in the release
//! notes.

pub mod check;
pub mod config;
pub mod content;
pub mod history;
pub mod label;
pub mod merge;
pub mod props;
pub mod record;
pub mod state;
pub mod structure;

use ryucad_doc::doc::canonical;
use ryucad_doc::doc::id::ProjectId;
use ryucad_doc::doc::project::conflicted;
use ryucad_doc::doc::source::Source;
use ryucad_doc::error::{Code, Error, Result};
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
    match conflicted(&v, at) {
        Some(e) => Err(e),
        None => Ok(Some(v)),
    }
}

#[cfg(test)]
mod tests {
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
