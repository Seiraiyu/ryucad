//! A catalogue held in memory between requests: its tracked files by
//! stamp, and the catalogue read from them, read again only when a file
//! changed.

use crate::catalogue::MANIFEST;
use crate::error::{Code, Error, Result};
use crate::load::Catalogue;
use ryu_session::fresh::{Files, Stamps};
use std::path::{Path, PathBuf};

pub struct Session {
    root: PathBuf,
    catalogue: Catalogue,
    stamps: Stamps,
    /// The release branch as read, by its refs' key (ryueda's `Session::pdm`).
    pub pdm: Option<(String, ryupdm::history::Releases)>,
    /// Set by [`Session::opened_now`]: the next refresh is skipped, since
    /// opening has just read every file (at 100,000 items a second scan
    /// costs as much as the answer).
    fresh: bool,
    /// Counts catalogue loads: what was derived from the catalogue (the
    /// listing) is good while it stands. Hashing every file for
    /// [`Session::revision`] would cost more than a list window.
    generation: u64,
    /// The item list as last built ([`crate::listing`]), and how many
    /// were built.
    listing: Option<crate::listing::Listing>,
    listings: u64,
}

pub fn path_of(root: &Path, rel: &str) -> PathBuf {
    rel.split('/').fold(root.to_path_buf(), |p, s| p.join(s))
}

/// The files a catalogue's session tracks: the root files, every item
/// file one shard below `items/`, every release record. Nothing at all
/// without a manifest.
pub fn tracked(root: &Path) -> Vec<(String, std::fs::Metadata)> {
    let meta = |rel: &str| {
        std::fs::metadata(path_of(root, rel))
            .ok()
            .filter(|m| m.is_file())
    };
    if meta(MANIFEST).is_none() {
        return Vec::new();
    }
    let mut out = Vec::new();
    for f in crate::pdm::ROOT_FILES {
        if let Some(m) = meta(f) {
            out.push((f.to_owned(), m));
        }
    }
    let dirs = |rel: &str| -> Vec<String> {
        let mut v: Vec<String> = std::fs::read_dir(path_of(root, rel))
            .into_iter()
            .flatten()
            .flatten()
            .filter(|e| e.file_type().is_ok_and(|t| t.is_dir()))
            .map(|e| e.file_name().to_string_lossy().into_owned())
            .collect();
        v.sort();
        v
    };
    let files = |rel: &str| -> Vec<(String, std::fs::Metadata)> {
        let mut v: Vec<(String, std::fs::Metadata)> = std::fs::read_dir(path_of(root, rel))
            .into_iter()
            .flatten()
            .flatten()
            .filter_map(|e| {
                let m = std::fs::symlink_metadata(e.path()).ok()?;
                m.is_file()
                    .then(|| (format!("{rel}/{}", e.file_name().to_string_lossy()), m))
            })
            .collect();
        v.sort_by(|a, b| a.0.cmp(&b.0));
        v
    };
    for top in [crate::ids::ITEMS, ryupdm::RELEASES] {
        // Files directly under items/ are tracked too, so a misplaced one
        // is reported, not missed.
        out.extend(files(top));
        // Each shard listed on its own thread, gathered in order: 100,000
        // items' metadata took most of a second one folder at a time.
        let shards: Vec<String> = dirs(top)
            .into_iter()
            .map(|d| format!("{top}/{d}"))
            .collect();
        let per = shards.len().div_ceil(8).max(1);
        let listed: Vec<Vec<(String, std::fs::Metadata)>> = std::thread::scope(|s| {
            let handles: Vec<_> = shards
                .chunks(per)
                .map(|c| {
                    let files = &files;
                    s.spawn(move || c.iter().flat_map(|d| files(d)).collect::<Vec<_>>())
                })
                .collect();
            handles
                .into_iter()
                .map(|h| h.join().expect("a listing thread doesn't panic"))
                .collect()
        });
        out.extend(listed.into_iter().flatten());
    }
    // Mapping files (`imports/<name>.json`): one level, no shards.
    out.extend(files(crate::import::IMPORTS));
    out
}

fn listed(root: &Path) -> Vec<(PathBuf, std::fs::Metadata)> {
    tracked(root)
        .into_iter()
        .map(|(rel, m)| (path_of(root, &rel), m))
        .collect()
}

fn load(root: &Path, stamps: &Stamps) -> Result<Catalogue> {
    let files = stamps.snapshot();
    if !files.contains_key(MANIFEST) {
        return Err(Error::new(
            Code::ProjectNotFound,
            format!("{} has no {MANIFEST}", root.display()),
        )
        .with("path", root));
    }
    Catalogue::load(&*files)
}

impl Session {
    pub fn open(root: &Path) -> Result<Session> {
        let mut stamps = Stamps::default();
        stamps.scan(root, listed(root))?;
        let catalogue = load(root, &stamps)?;
        Ok(Session {
            root: root.to_path_buf(),
            catalogue,
            stamps,
            pdm: None,
            fresh: false,
            generation: 0,
            listing: None,
            listings: 0,
        })
    }

    pub fn root(&self) -> &Path {
        &self.root
    }

    pub fn catalogue(&self) -> &Catalogue {
        &self.catalogue
    }

    /// Marks a session opened for the request about to be answered: its
    /// files were read just now, so that request's refresh is skipped.
    pub fn opened_now(&mut self) {
        self.fresh = true;
    }

    pub fn refresh_changes(&mut self) -> Result<Vec<(String, &'static str)>> {
        if std::mem::take(&mut self.fresh) {
            return Ok(Vec::new());
        }
        let changes = self.stamps.scan(&self.root, listed(&self.root))?;
        if !changes.is_empty() {
            self.catalogue = load(&self.root, &self.stamps)?;
            self.generation += 1;
        }
        Ok(changes)
    }

    /// How many times the catalogue has been read again since opening.
    pub fn generation(&self) -> u64 {
        self.generation
    }

    /// The listing held, taken out, and how many have been built.
    pub fn take_listing(&mut self) -> (Option<crate::listing::Listing>, u64) {
        (self.listing.take(), self.listings)
    }

    pub fn keep_listing(&mut self, listing: crate::listing::Listing) {
        self.listings = self.listings.max(listing.built());
        self.listing = Some(listing);
    }

    pub fn reload(&mut self) -> Result<()> {
        self.refresh_changes().map(drop)
    }

    pub fn snapshot(&self) -> Files {
        Files::clone(&self.stamps.snapshot())
    }

    pub fn files(&self) -> std::sync::Arc<Files> {
        self.stamps.snapshot()
    }

    /// The catalogue's revision: BLAKE3 over every tracked file's path and
    /// bytes, 32 hex digits. What `--expect` guards on catalogue-wide
    /// writes.
    pub fn revision(&self) -> String {
        let mut h = blake3::Hasher::new();
        h.update(b"ryuplm.catalogue.1");
        for (p, b) in self.files().iter() {
            for piece in [p.as_bytes(), b] {
                h.update(&(piece.len() as u64).to_le_bytes());
                h.update(piece);
            }
        }
        h.finalize().to_hex()[..32].to_owned()
    }

    /// Item `id`'s revision: BLAKE3 over its file's bytes, 32 hex digits.
    pub fn item_revision(&self, id: &crate::id::Id) -> Option<String> {
        let b = self.files().get(&crate::ids::item_path(id)).cloned()?;
        let mut h = blake3::Hasher::new();
        h.update(b"ryuplm.item.1");
        h.update(&b);
        Some(h.finalize().to_hex()[..32].to_owned())
    }

    /// Every item's revision, and `catalogue`'s: what `project.changed`
    /// tells subscribers.
    pub fn revisions(&self) -> std::collections::BTreeMap<String, serde_json::Value> {
        let mut out: std::collections::BTreeMap<String, serde_json::Value> = self
            .catalogue
            .items
            .keys()
            .filter_map(|id| Some((format!("item/{id}"), self.item_revision(id)?.into())))
            .collect();
        out.insert("catalogue".into(), self.revision().into());
        out
    }
}

/// Removes `path` and any folders it leaves empty up to `root`, as
/// ryudfm's `session::remove_file`.
pub fn remove_file(root: &Path, path: &Path) -> Result<()> {
    crate::source::remove_file(path)?;
    let mut dir = path.parent();
    while let Some(d) = dir {
        if d == root || !d.starts_with(root) || std::fs::remove_dir(d).is_err() {
            break;
        }
        dir = d.parent();
    }
    Ok(())
}

/// Every tracked file's bytes now, for undo (ryudfm's `session::current`).
pub fn current(root: &Path) -> Result<Files> {
    tracked(root)
        .into_iter()
        .map(|(rel, _)| {
            let b = std::fs::read(path_of(root, &rel))
                .map_err(|e| Error::io(&path_of(root, &rel), &e))?;
            Ok((rel, std::sync::Arc::from(b)))
        })
        .collect()
}
