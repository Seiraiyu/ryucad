//! The one write path for a part's files: each feature file, then the
//! part's list, each through `Session::write_json` (atomic, synced).
//! Moved out of `server` (ryucad 27.B).

use crate::doc::files::{Feature, PartFile};
use crate::doc::id::Id;
use crate::doc::project::{feature_path, part_path};
use crate::error::Result;
use crate::part::part_kind;
use crate::session::Session;

/// Writes a feature file, then the part's list: a crash in between leaves
/// at worst an unlisted feature file, which `check_format` reports.
pub(crate) fn save(
    session: &mut Session,
    part: &Id,
    writes: &[(&Id, &Feature)],
    order: Vec<Id>,
) -> Result<String> {
    for (id, f) in writes {
        session.write_json(&feature_path(session.root(), part, id), &f.to_json())?;
    }
    // A part the write makes the assembly place takes the next item number;
    // one it no longer places loses its own.
    let p = session.project().part(part)?;
    let items = p.file.as_ref().map(|f| f.items.clone()).unwrap_or_default();
    let feature = |fid: &Id| match writes.iter().find(|(w, _)| *w == fid) {
        Some((_, f)) => Some(*f),
        None => p.features.get(fid)?.as_ref().ok(),
    };
    let after = ryupdm::items::placed_by(&order, feature);
    let whole = order.iter().all(|f| feature(f).is_some());
    let items = ryupdm::items::assigned(&items, &ryupdm::items::placed(p), &after, whole);
    let file = PartFile {
        kind: part_kind(p),
        features: order,
        items,
    }
    .to_json();
    session.write_json(&part_path(session.root(), part), &file)?;
    session.reload()?;
    // What the working files place from other projects may have changed.
    if crate::deps::keep_lock(session)? {
        session.reload()?;
    }
    Ok(session.project().part(part)?.revision.clone())
}
