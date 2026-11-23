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

/// Binary files to write, by path: documents' bytes (design 937).
pub type Bytes = Vec<(String, std::sync::Arc<[u8]>)>;

/// The session's files with `writes` and `bytes` applied, in memory.
pub fn proposed_with(session: &Session, writes: &Writes, bytes: &Bytes) -> Files {
    let mut f = proposed(session, writes);
    for (p, b) in bytes {
        f.insert(p.clone(), b.clone());
    }
    f
}

/// Judges the catalogue's own files only: numbers elsewhere are warnings
/// and never refuse, so a write never reads other folders.
fn faults(files: &Files, cat: &Catalogue) -> Vec<Fault> {
    judge::all(files, cat, &Context::default())
}

/// The error faults `writes` would add: what `commit` refuses.
pub fn would_add(session: &Session, writes: &Writes) -> Result<Vec<Fault>> {
    would_add_with(session, writes, &Vec::new())
}

/// [`would_add`], with binary files written too.
pub fn would_add_with(session: &Session, writes: &Writes, bytes: &Bytes) -> Result<Vec<Fault>> {
    let after_files = proposed_with(session, writes, bytes);
    let after = Catalogue::load(&after_files)?;
    let before = faults(&session.snapshot(), session.catalogue());
    Ok(added_errors(&before, &faults(&after_files, &after)))
}

pub fn commit(session: &mut Session, what: &str, writes: Writes) -> Result<()> {
    commit_with(session, what, writes, Vec::new())
}

/// [`commit`], with binary files written too.
pub fn commit_with(session: &mut Session, what: &str, writes: Writes, bytes: Bytes) -> Result<()> {
    let added = would_add_with(session, &writes, &bytes)?;
    if !added.is_empty() {
        return Err(refused(what, &added));
    }
    write_all(session.root(), &writes, &bytes)?;
    session.reload()
}

/// Writes at or above this many files go out on threads, synced once.
const PARALLEL_FROM: usize = 64;

/// Writes `writes` under `root`: deletes one by one (each may remove the
/// folders it empties), files on up to eight threads once there are many,
/// with syncs deferred and all waited for before returning (#270), so a
/// 100,000-item import waits on the journal once.
fn write_all(root: &std::path::Path, writes: &Writes, bytes: &Bytes) -> Result<()> {
    let (files, deletes): (Vec<_>, Vec<_>) = writes.0.iter().partition(|(_, t)| t.is_some());
    if files.len() < PARALLEL_FROM {
        for (p, t) in &files {
            crate::source::write_atomic(&path_of(root, p), t.as_deref().expect("a file"))?;
        }
    } else {
        crate::source::defer_syncs();
        let threads = std::thread::available_parallelism()
            .map_or(4, |n| n.get())
            .min(8);
        let per = files.len().div_ceil(threads);
        let results: Vec<Result<()>> = std::thread::scope(|s| {
            let handles: Vec<_> = files
                .chunks(per)
                .map(|chunk| {
                    s.spawn(move || -> Result<()> {
                        for (p, t) in chunk {
                            crate::source::write_atomic(
                                &path_of(root, p),
                                t.as_deref().expect("a file"),
                            )?;
                        }
                        Ok(())
                    })
                })
                .collect();
            handles
                .into_iter()
                .map(|h| h.join().expect("a writer thread doesn't panic"))
                .collect()
        });
        // One sync of the whole file system, then the queued syncs find
        // nothing to write: on a network disk 100,000 fsyncs took half a
        // minute even on sixteen threads.
        sync_filesystem(root);
        crate::source::flush_syncs();
        results.into_iter().collect::<Result<()>>()?;
    }
    for (p, b) in bytes {
        crate::source::write_atomic(&path_of(root, p), &**b)?;
    }
    for (p, _) in &deletes {
        remove_file(root, &path_of(root, p))?;
    }
    Ok(())
}

/// Syncs the file system `root` is on, once (Linux's `syncfs`); elsewhere
/// nothing, and the deferred syncs do it file by file.
#[cfg(target_os = "linux")]
fn sync_filesystem(root: &std::path::Path) {
    use std::os::fd::AsRawFd;
    if let Ok(dir) = std::fs::File::open(root) {
        // SAFETY: `syncfs` takes any open descriptor, and `dir` holds it
        // open for the call.
        unsafe { libc::syncfs(dir.as_raw_fd()) };
    }
}

#[cfg(not(target_os = "linux"))]
fn sync_filesystem(_: &std::path::Path) {}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::item::Item;

    #[test]
    fn bytes_are_written_beside_their_record_and_tracked() {
        let (_d, root) = crate::server::tests::catalogue();
        let mut session = Session::open(&root).unwrap();
        let id = crate::id::Id::parse("k3v9q2m8x4c1p", "document").unwrap();
        let bytes: &[u8] = b"%PDF-1.7 bytes";
        let rec = crate::document::Document::new(
            "DOC-1",
            "Spec",
            crate::document::File::of("Spec.pdf", bytes, None).unwrap(),
        );
        commit_with(
            &mut session,
            "adding",
            Writes(vec![(crate::ids::document_path(&id), Some(rec.to_text()))]),
            vec![(
                crate::ids::bytes_path(&id, "Spec.pdf"),
                std::sync::Arc::from(bytes),
            )],
        )
        .unwrap();
        assert_eq!(
            std::fs::read(root.join("documents/k3/k3v9q2m8x4c1p/Spec.pdf")).unwrap(),
            bytes
        );
        assert!(
            session
                .files()
                .contains_key("documents/k3/k3v9q2m8x4c1p/Spec.pdf")
        );
        assert_eq!(session.catalogue().documents.len(), 1);
        assert!(session.document_revision(&id).is_some());
        assert!(session.revisions().contains_key("document/k3v9q2m8x4c1p"));
    }

    #[test]
    fn a_large_write_lands_whole_and_synced() {
        let (_d, root) = crate::server::tests::catalogue();
        let mut session = Session::open(&root).unwrap();
        let writes: Vec<(String, Option<String>)> = (0..300u64)
            .map(|k| {
                let id = crate::id::Id::parse(&crate::ids::text(k << 8), "item").unwrap();
                (
                    crate::ids::item_path(&id),
                    Some(Item::new(&format!("BULK-{k:04}")).to_text()),
                )
            })
            .collect();
        commit(&mut session, "bulk", Writes(writes)).unwrap();
        assert_eq!(session.catalogue().items.len(), 300);
        assert_eq!(
            ryu_doc::source::pending_syncs(),
            0,
            "every file synced before the answer"
        );
    }
}
