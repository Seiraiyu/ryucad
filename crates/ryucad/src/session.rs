//! A loaded project and its rebuilt parts, kept fresh against the files.
//!
//! The files are the truth. Before every request the session checks each
//! file it depends on: an unchanged size and modification time is trusted
//! unless the time is too recent to be reliable; otherwise the content is
//! hashed. Any real change reloads the project. Rebuilds are cached by part
//! revision, so only parts whose files changed are rebuilt.

use crate::doc::id::Id;
use crate::doc::project::{MANIFEST, Project};
use crate::error::{Error, Result};
use crate::rebuild::{PartResult, rebuild_part};
use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime};

/// A modification time this close to when it was read may be followed by
/// another write in the same timestamp tick, so it isn't trusted.
const RACY: Duration = Duration::from_secs(2);

#[derive(Clone, Debug, PartialEq)]
struct Stamp {
    modified: Option<SystemTime>,
    len: u64,
    hash: blake3::Hash,
}

pub struct Session {
    project: Project,
    stamps: BTreeMap<PathBuf, Stamp>,
    stamped_at: SystemTime,
    results: BTreeMap<Id, (String, PartResult)>,
}

impl Session {
    /// Loads the project at `root`.
    pub fn open(root: &Path) -> Result<Session> {
        let mut s = Session {
            project: Project::load(root)?,
            stamps: BTreeMap::new(),
            stamped_at: SystemTime::now(),
            results: BTreeMap::new(),
        };
        s.stamps = s.stamp_all()?;
        Ok(s)
    }

    pub fn root(&self) -> &Path {
        &self.project.root
    }

    pub fn project(&self) -> &Project {
        &self.project
    }

    /// Brings the session up to date with the files. Returns whether the
    /// project was reloaded.
    pub fn refresh(&mut self) -> Result<bool> {
        let files = files(self.root())?;
        let trusted_before = self.stamped_at.checked_sub(RACY);
        let mut changed = files.len() != self.stamps.len();
        let mut stamps = BTreeMap::new();
        for path in files {
            let meta = fs::metadata(&path).map_err(|e| Error::io(&path, &e))?;
            let (modified, len) = (meta.modified().ok(), meta.len());
            let old = self.stamps.get(&path);
            let trusted = match (old, modified, trusted_before) {
                (Some(o), Some(m), Some(t)) => o.modified == Some(m) && o.len == len && m < t,
                _ => false,
            };
            let stamp = if trusted {
                old.cloned().expect("trusted stamps exist")
            } else {
                let hash = hash_file(&path)?;
                Stamp {
                    modified,
                    len,
                    hash,
                }
            };
            changed |= old.map(|o| o.hash) != Some(stamp.hash);
            stamps.insert(path, stamp);
        }
        self.stamped_at = SystemTime::now();
        self.stamps = stamps;
        if changed {
            self.project = Project::load(&self.project.root)?;
            let live = &self.project.parts;
            self.results
                .retain(|id, (rev, _)| live.get(id).is_some_and(|p| &p.revision == rev));
        }
        Ok(changed)
    }

    /// Re-reads everything after this session wrote files.
    pub fn reload(&mut self) -> Result<()> {
        self.project = Project::load(self.root())?;
        self.stamps = self.stamp_all()?;
        let live = &self.project.parts;
        self.results
            .retain(|id, (rev, _)| live.get(id).is_some_and(|p| &p.revision == rev));
        Ok(())
    }

    /// The rebuild of `part`, from the cache when its files haven't changed.
    pub fn rebuilt(&mut self, part: &Id) -> Result<&PartResult> {
        let p = self.project.part(part)?;
        let fresh = self
            .results
            .get(part)
            .is_some_and(|(rev, _)| *rev == p.revision);
        if !fresh {
            let result = rebuild_part(p);
            self.results
                .insert(part.clone(), (p.revision.clone(), result));
        }
        Ok(&self.results[part].1)
    }

    fn stamp_all(&mut self) -> Result<BTreeMap<PathBuf, Stamp>> {
        self.stamped_at = SystemTime::now();
        files(self.root())?
            .into_iter()
            .map(|path| {
                let meta = fs::metadata(&path).map_err(|e| Error::io(&path, &e))?;
                let hash = hash_file(&path)?;
                Ok((
                    path,
                    Stamp {
                        modified: meta.modified().ok(),
                        len: meta.len(),
                        hash,
                    },
                ))
            })
            .collect()
    }
}

fn hash_file(path: &Path) -> Result<blake3::Hash> {
    fs::read(path)
        .map(|b| blake3::hash(&b))
        .map_err(|e| Error::io(path, &e))
}

/// The files a project's state depends on: the manifest, and everything
/// under `parts/` two and three levels down.
fn files(root: &Path) -> Result<Vec<PathBuf>> {
    let mut out = vec![root.join(MANIFEST)];
    for part in read_dir(&root.join("parts"))? {
        if part.is_dir() {
            for entry in read_dir(&part)? {
                if entry.is_dir() {
                    out.extend(read_dir(&entry)?.into_iter().filter(|p| p.is_file()));
                } else {
                    out.push(entry);
                }
            }
        }
    }
    out.retain(|p| p.is_file());
    Ok(out)
}

fn read_dir(dir: &Path) -> Result<Vec<PathBuf>> {
    match fs::read_dir(dir) {
        Ok(entries) => entries
            .map(|e| e.map(|e| e.path()).map_err(|e| Error::io(dir, &e)))
            .collect(),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(Vec::new()),
        Err(e) => Err(Error::io(dir, &e)),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::doc::files::{Feature, FeatureType, Manifest, PartFile, Placement};
    use crate::doc::project::{feature_path, part_path, write_json};
    use crate::units::{Quantity, Unit};

    fn id(s: &str) -> Id {
        Id::parse(s, "feature").unwrap()
    }

    fn sphere(r: &str) -> serde_json::Value {
        Feature {
            ty: FeatureType::Sphere,
            params: [("radius".to_owned(), Quantity::parse(r, Unit::Mm).unwrap())].into(),
            placement: Placement::identity(Unit::Mm),
        }
        .to_json()
    }

    fn project() -> tempfile::TempDir {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        write_json(
            &root.join(MANIFEST),
            &Manifest {
                default_unit: Unit::Mm,
            }
            .to_json(),
        )
        .unwrap();
        write_json(
            &part_path(root, &id("p")),
            &PartFile {
                features: vec![id("a")],
            }
            .to_json(),
        )
        .unwrap();
        write_json(&feature_path(root, &id("p"), &id("a")), &sphere("1 mm")).unwrap();
        dir
    }

    fn radius(s: &mut Session) -> f64 {
        let p = s.project().part(&id("p")).unwrap();
        p.features[&id("a")].as_ref().unwrap().meters("radius")
    }

    #[test]
    fn sees_external_edits_even_within_the_same_tick() {
        let dir = project();
        let mut s = Session::open(dir.path()).unwrap();
        assert!(!s.refresh().unwrap());
        // Same size, written immediately: only the content hash can tell.
        write_json(
            &feature_path(dir.path(), &id("p"), &id("a")),
            &sphere("2 mm"),
        )
        .unwrap();
        assert!(s.refresh().unwrap());
        assert_eq!(radius(&mut s), 0.002);
        assert!(!s.refresh().unwrap());
    }

    #[test]
    fn touching_without_changing_reloads_nothing() {
        let dir = project();
        let mut s = Session::open(dir.path()).unwrap();
        let path = feature_path(dir.path(), &id("p"), &id("a"));
        let text = fs::read(&path).unwrap();
        fs::write(&path, &text).unwrap();
        assert!(!s.refresh().unwrap());
    }

    #[test]
    fn sees_files_added_and_removed() {
        let dir = project();
        let mut s = Session::open(dir.path()).unwrap();
        write_json(
            &part_path(dir.path(), &id("q")),
            &PartFile { features: vec![] }.to_json(),
        )
        .unwrap();
        assert!(s.refresh().unwrap());
        assert!(s.project().parts.contains_key(&id("q")));
        fs::remove_dir_all(dir.path().join("parts/q")).unwrap();
        assert!(s.refresh().unwrap());
        assert!(!s.project().parts.contains_key(&id("q")));
    }

    #[test]
    fn rebuilds_are_cached_by_revision() {
        let dir = project();
        let mut s = Session::open(dir.path()).unwrap();
        let h1 = s.rebuilt(&id("p")).unwrap().hash.clone();
        let ptr1 = s.rebuilt(&id("p")).unwrap() as *const PartResult;
        let ptr2 = s.rebuilt(&id("p")).unwrap() as *const PartResult;
        assert_eq!(ptr1, ptr2);
        write_json(
            &feature_path(dir.path(), &id("p"), &id("a")),
            &sphere("3 mm"),
        )
        .unwrap();
        s.refresh().unwrap();
        assert_ne!(s.rebuilt(&id("p")).unwrap().hash, h1);
    }

    #[test]
    fn a_vanished_manifest_is_an_error() {
        let dir = project();
        let mut s = Session::open(dir.path()).unwrap();
        fs::remove_file(dir.path().join(MANIFEST)).unwrap();
        assert!(s.refresh().is_err());
    }
}
