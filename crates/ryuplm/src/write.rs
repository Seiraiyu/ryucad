//! Writing a catalogue's files: every change is judged in memory first
//! and refused when it would add an error-severity finding (ryueda's
//! `write::commit`), then written atomically and the session read again.

use crate::error::{Error, Result};
use crate::fault::{self, Fault};
use crate::judge::{self, Context};
use crate::load::Catalogue;
use crate::session::{Session, path_of, remove_file};
use ryu_session::fresh::Files;
use std::sync::Arc;

/// Files to write (`Some` text) or delete (`None`), by path.
pub struct Writes(pub Vec<(String, Option<String>)>);

/// The faults' identity: code, item, pointer and path.
fn key(f: &Fault) -> (String, String, String, String) {
    let d = &f.error.details;
    let s = |k: &str| d[k].as_str().unwrap_or("").to_owned();
    (
        format!("{:?}", f.error.code),
        s("item"),
        s("pointer"),
        s("path"),
    )
}

/// The error faults in `after` that `before` hasn't.
pub fn added_errors(before: &[Fault], after: &[Fault]) -> Vec<Fault> {
    let old: std::collections::BTreeSet<_> =
        before.iter().filter(|f| f.is_error()).map(key).collect();
    after
        .iter()
        .filter(|f| f.is_error() && !old.contains(&key(f)))
        .cloned()
        .collect()
}

/// The refusal: the first new fault's own code, every new fault listed.
pub fn refused(what: &str, errors: &[Fault]) -> Error {
    let first = &errors[0].error;
    Error::new(
        first.code,
        format!(
            "{what} would leave {} fault(s): {}",
            errors.len(),
            first.message
        ),
    )
    .with("faults", fault::list(errors))
}

/// The session's files with `writes` applied, in memory.
pub fn proposed(session: &Session, writes: &Writes) -> Files {
    let mut f = session.snapshot();
    for (p, t) in &writes.0 {
        match t {
            Some(t) => {
                f.insert(p.clone(), Arc::from(t.as_bytes()));
            }
            None => {
                f.remove(p);
            }
        }
    }
    f
}

/// Judges the catalogue's own files only: numbers elsewhere are warnings
/// and never refuse, so a write never reads other folders.
fn faults(files: &Files, cat: &Catalogue) -> Vec<Fault> {
    judge::all(files, cat, &Context::default())
}

pub fn commit(session: &mut Session, what: &str, writes: Writes) -> Result<()> {
    let after_files = proposed(session, &writes);
    let after = Catalogue::load(&after_files)?;
    let before = faults(&session.snapshot(), session.catalogue());
    let added = added_errors(&before, &faults(&after_files, &after));
    if !added.is_empty() {
        return Err(refused(what, &added));
    }
    for (p, t) in &writes.0 {
        let at = path_of(session.root(), p);
        match t {
            Some(t) => crate::source::write_atomic(&at, t)?,
            None => remove_file(session.root(), &at)?,
        }
    }
    session.reload()
}
