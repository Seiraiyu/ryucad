//! A project folder: finding it, loading it, and writing its files.
//!
//! ```text
//! ryucad.json
//! parts/<part>/part.json
//! parts/<part>/features/<feature>.json
//! ```
//!
//! Loading never stops at a broken part or feature: the error is kept in
//! its place and the rest of the project stays usable.

use super::canonical;
use super::files::{Feature, Manifest, PartFile};
use super::id::Id;
use crate::error::{Code, Error, Result};
use serde_json::Value;
use std::collections::BTreeMap;
use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};

pub const MANIFEST: &str = "ryucad.json";

#[derive(Clone, Debug)]
pub struct Project {
    pub root: PathBuf,
    pub manifest: Manifest,
    pub parts: BTreeMap<Id, Part>,
    /// Entries under `parts/` that don't belong there, as paths relative to
    /// the root.
    pub unknown: Vec<String>,
}

#[derive(Clone, Debug)]
pub struct Part {
    pub id: Id,
    /// The feature list, or why `part.json` can't be used.
    pub file: Result<PartFile>,
    /// Every feature file in `features/`, listed or not.
    pub features: BTreeMap<Id, Result<Feature>>,
    /// A hash of the bytes of `ryucad.json`, `part.json` and the listed
    /// feature files in order: it changes whenever anything that defines the
    /// part does.
    pub revision: String,
}

impl Part {
    /// The ordered feature IDs, empty when `part.json` is broken.
    pub fn order(&self) -> &[Id] {
        self.file.as_ref().map_or(&[], |f| &f.features)
    }

    /// Feature files that `part.json` doesn't list.
    pub fn orphans(&self) -> Vec<&Id> {
        let order = self.order();
        self.features
            .keys()
            .filter(|id| !order.contains(id))
            .collect()
    }
}

/// On Windows `canonicalize` gives a verbatim path (`\\?\C:\…`). Agents see
/// the ordinary `C:\…` form, which is what `init` reports too. UNC paths and
/// paths too long for the ordinary form stay verbatim.
fn plain(p: PathBuf) -> PathBuf {
    #[cfg(windows)]
    {
        if let Some(rest) = p.to_str().and_then(|s| s.strip_prefix(r"\\?\"))
            && rest.as_bytes().get(1) == Some(&b':')
            && rest.len() < 260
        {
            return PathBuf::from(rest);
        }
    }
    p
}

/// The nearest folder at or above `start` that holds a `ryucad.json`.
pub fn find_root(start: &Path) -> Result<PathBuf> {
    let start = plain(start.canonicalize().map_err(|e| Error::io(start, &e))?);
    start
        .ancestors()
        .find(|d| d.join(MANIFEST).is_file())
        .map(Path::to_path_buf)
        .ok_or_else(|| {
            Error::new(
                Code::ProjectNotFound,
                format!(
                    "no {MANIFEST} in {} or any folder above it; run `ryucad init` or pass -C <dir>",
                    start.display()
                ),
            )
            .with("path", &start)
        })
}

pub fn part_dir(root: &Path, part: &Id) -> PathBuf {
    root.join("parts").join(part.as_str())
}

pub fn part_path(root: &Path, part: &Id) -> PathBuf {
    part_dir(root, part).join("part.json")
}

pub fn feature_path(root: &Path, part: &Id, feature: &Id) -> PathBuf {
    part_dir(root, part)
        .join("features")
        .join(format!("{feature}.json"))
}

/// `path` relative to `root`, with `/` on every platform.
pub fn relative(root: &Path, path: &Path) -> String {
    let rel = path.strip_prefix(root).unwrap_or(path);
    rel.components()
        .map(|c| c.as_os_str().to_string_lossy())
        .collect::<Vec<_>>()
        .join("/")
}

/// Reads and parses a JSON file; errors carry its path relative to `root`.
fn read_json(root: &Path, path: &Path) -> Result<(Vec<u8>, Value)> {
    let at = relative(root, path);
    let bytes = fs::read(path).map_err(|e| {
        if e.kind() == std::io::ErrorKind::NotFound {
            Error::new(Code::NotFound, format!("{at} is missing")).with("path", &at)
        } else {
            Error::io(path, &e).with("path", &at)
        }
    })?;
    let text = std::str::from_utf8(&bytes).map_err(|_| {
        Error::new(Code::InvalidJson, format!("{at} is not UTF-8")).with("path", &at)
    })?;
    let value = canonical::parse(text).map_err(|e| e.with("path", &at))?;
    Ok((bytes, value))
}

impl Project {
    /// Loads the project at `root`. Only a missing or broken manifest is an
    /// error; anything wrong inside a part is kept in that part.
    pub fn load(root: &Path) -> Result<Project> {
        let manifest_path = root.join(MANIFEST);
        let (manifest_bytes, v) = read_json(root, &manifest_path).map_err(|e| {
            if e.code == Code::NotFound {
                Error::new(
                    Code::ProjectNotFound,
                    format!("no {MANIFEST} in {}", root.display()),
                )
                .with("path", root)
            } else {
                e
            }
        })?;
        let manifest = Manifest::from_json(&v).map_err(|e| e.with("path", MANIFEST))?;
        let mut parts = BTreeMap::new();
        let mut unknown = Vec::new();
        for (name, path) in list_dir(&root.join("parts"))? {
            match Id::parse(&name, "part") {
                Ok(id) if path.is_dir() => {
                    let part =
                        load_part(root, &manifest, &manifest_bytes, id.clone(), &mut unknown)?;
                    parts.insert(id, part);
                }
                _ => unknown.push(relative(root, &path)),
            }
        }
        Ok(Project {
            root: root.to_path_buf(),
            manifest,
            parts,
            unknown,
        })
    }

    pub fn part(&self, id: &Id) -> Result<&Part> {
        self.parts
            .get(id)
            .ok_or_else(|| Error::new(Code::NotFound, format!("no part {id}")).with("part", id))
    }
}

fn load_part(
    root: &Path,
    manifest: &Manifest,
    manifest_bytes: &[u8],
    id: Id,
    unknown: &mut Vec<String>,
) -> Result<Part> {
    let dir = part_dir(root, &id);
    // Each piece is length-prefixed, so no two sets of files hash alike.
    let mut hasher = blake3::Hasher::new();
    let piece = |h: &mut blake3::Hasher, bytes: &[u8]| {
        h.update(&(bytes.len() as u64).to_le_bytes());
        h.update(bytes);
    };
    piece(&mut hasher, manifest_bytes);
    let file = read_json(root, &part_path(root, &id)).and_then(|(bytes, v)| {
        piece(&mut hasher, &bytes);
        PartFile::from_json(&v).map_err(|e| e.with("path", relative(root, &part_path(root, &id))))
    });
    for (name, path) in list_dir(&dir)? {
        if name != "part.json" && name != "features" {
            unknown.push(relative(root, &path));
        }
    }
    let mut features = BTreeMap::new();
    let mut bytes_of = BTreeMap::new();
    for (name, path) in list_dir(&dir.join("features"))? {
        let fid = name
            .strip_suffix(".json")
            .and_then(|stem| Id::parse(stem, "feature").ok());
        match fid {
            Some(fid) if path.is_file() => {
                let at = relative(root, &path);
                let feature = read_json(root, &path).and_then(|(bytes, v)| {
                    bytes_of.insert(fid.clone(), bytes);
                    Feature::from_json(&v, manifest.default_unit).map_err(|e| e.with("path", &at))
                });
                features.insert(fid, feature);
            }
            _ => unknown.push(relative(root, &path)),
        }
    }
    if let Ok(f) = &file {
        for fid in &f.features {
            // A listed feature with no file is an error in its place.
            match bytes_of.get(fid) {
                Some(bytes) => {
                    piece(&mut hasher, fid.as_str().as_bytes());
                    piece(&mut hasher, bytes);
                }
                None => {
                    let at = relative(root, &feature_path(root, &id, fid));
                    features.entry(fid.clone()).or_insert_with(|| {
                        Err(Error::new(Code::NotFound, format!("{at} is missing"))
                            .with("path", &at))
                    });
                }
            }
        }
    }
    let revision = hasher.finalize().to_hex()[..32].to_owned();
    Ok(Part {
        id,
        file,
        features,
        revision,
    })
}

/// `(name, path)` for each entry in `dir`, sorted by name; empty when `dir`
/// doesn't exist.
fn list_dir(dir: &Path) -> Result<Vec<(String, PathBuf)>> {
    let entries = match fs::read_dir(dir) {
        Ok(entries) => entries,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(e) => return Err(Error::io(dir, &e)),
    };
    let mut out = Vec::new();
    for entry in entries {
        let entry = entry.map_err(|e| Error::io(dir, &e))?;
        out.push((
            entry.file_name().to_string_lossy().into_owned(),
            entry.path(),
        ));
    }
    out.sort();
    Ok(out)
}

/// Writes `contents` to `path` so that readers see the old file or the new
/// one, never a mix: a temporary file in the same folder, then a rename.
pub fn write_atomic(path: &Path, contents: &str) -> Result<()> {
    let dir = path.parent().expect("a file path has a parent");
    fs::create_dir_all(dir).map_err(|e| Error::io(dir, &e))?;
    let name = path.file_name().expect("a file path has a name");
    let tmp = dir.join(format!(".{}.tmp", name.to_string_lossy()));
    let write = || -> std::io::Result<()> {
        let mut f = fs::File::create(&tmp)?;
        f.write_all(contents.as_bytes())?;
        f.sync_all()?;
        fs::rename(&tmp, path)
    };
    write().map_err(|e| {
        let _ = fs::remove_file(&tmp);
        Error::io(path, &e)
    })
}

/// Writes a JSON document in canonical form.
pub fn write_json(path: &Path, v: &Value) -> Result<()> {
    write_atomic(path, &canonical::to_string(v))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::doc::files::{FeatureType, Placement};
    use crate::units::{Quantity, Unit};

    fn id(s: &str) -> Id {
        Id::parse(s, "feature").unwrap()
    }

    /// A project with part `p` listing features `a` and `b`.
    fn sample() -> tempfile::TempDir {
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
        let p = id("p");
        write_json(
            &part_path(root, &p),
            &PartFile {
                features: vec![id("a"), id("b")],
            }
            .to_json(),
        )
        .unwrap();
        for (f, r) in [("a", "1"), ("b", "2")] {
            let feature = Feature {
                ty: FeatureType::Sphere,
                params: [("radius".to_owned(), Quantity::parse(r, Unit::Mm).unwrap())].into(),
                placement: Placement::identity(Unit::Mm),
            };
            write_json(&feature_path(root, &p, &id(f)), &feature.to_json()).unwrap();
        }
        dir
    }

    #[test]
    fn loads_a_project() {
        let dir = sample();
        let project = Project::load(dir.path()).unwrap();
        assert_eq!(project.manifest.default_unit, Unit::Mm);
        let part = project.part(&id("p")).unwrap();
        assert_eq!(part.order(), [id("a"), id("b")]);
        assert_eq!(
            part.features[&id("b")].as_ref().unwrap().meters("radius"),
            0.002
        );
        assert!(part.orphans().is_empty());
        assert!(project.unknown.is_empty());
        assert_eq!(part.revision.len(), 32);
    }

    #[test]
    fn finds_the_root_from_below() {
        let dir = sample();
        let deep = dir.path().join("parts/p/features");
        assert_eq!(
            find_root(&deep).unwrap(),
            dir.path().canonicalize().unwrap()
        );
        let elsewhere = tempfile::tempdir().unwrap();
        assert_eq!(
            find_root(elsewhere.path()).unwrap_err().code,
            Code::ProjectNotFound
        );
    }

    #[test]
    fn broken_files_stay_in_their_place() {
        let dir = sample();
        let root = dir.path();
        fs::write(feature_path(root, &id("p"), &id("a")), "{ nope").unwrap();
        fs::remove_file(feature_path(root, &id("p"), &id("b"))).unwrap();
        fs::write(root.join("parts/p/features/c.json"), "{}").unwrap();
        fs::write(root.join("parts/p/notes.txt"), "hi").unwrap();
        fs::create_dir_all(root.join("parts/Bad Name")).unwrap();
        let project = Project::load(root).unwrap();
        let part = project.part(&id("p")).unwrap();
        let a = part.features[&id("a")].as_ref().unwrap_err();
        assert_eq!(
            (a.code, a.details["path"].as_str()),
            (Code::InvalidJson, Some("parts/p/features/a.json"))
        );
        let b = part.features[&id("b")].as_ref().unwrap_err();
        assert_eq!(b.code, Code::NotFound);
        let c = part.features[&id("c")].as_ref().unwrap_err();
        assert_eq!(c.code, Code::InvalidDocument);
        assert_eq!(part.orphans(), [&id("c")]);
        assert_eq!(project.unknown, ["parts/Bad Name", "parts/p/notes.txt"]);
    }

    #[test]
    fn a_missing_or_newer_manifest_is_fatal() {
        let dir = sample();
        let root = dir.path();
        fs::write(
            root.join(MANIFEST),
            "{\"default_unit\": \"mm\", \"format\": 9, \"kind\": \"ryucad.project\"}",
        )
        .unwrap();
        let e = Project::load(root).unwrap_err();
        assert_eq!(
            (e.code, e.details["path"].as_str()),
            (Code::FormatTooNew, Some(MANIFEST))
        );
        fs::remove_file(root.join(MANIFEST)).unwrap();
        assert_eq!(Project::load(root).unwrap_err().code, Code::ProjectNotFound);
    }

    #[test]
    fn the_revision_follows_every_defining_byte() {
        let dir = sample();
        let root = dir.path();
        let rev = || {
            Project::load(root).unwrap().parts[&id("p")]
                .revision
                .clone()
        };
        let r0 = rev();
        assert_eq!(rev(), r0);
        let a = feature_path(root, &id("p"), &id("a"));
        let text = fs::read_to_string(&a).unwrap();
        fs::write(&a, text.replace("\"1 mm\"", "\"1.0 mm\"")).unwrap();
        let r1 = rev();
        assert_ne!(r1, r0);
        // An unlisted file doesn't define the part.
        fs::write(root.join("parts/p/features/z.json"), "{}").unwrap();
        assert_eq!(rev(), r1);
        fs::write(
            root.join(MANIFEST),
            Manifest {
                default_unit: Unit::In,
            }
            .to_json()
            .to_string(),
        )
        .unwrap();
        assert_ne!(rev(), r1);
    }

    #[test]
    fn atomic_writes_leave_no_temporary_file() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("a/b/c.json");
        write_atomic(&path, "one").unwrap();
        write_atomic(&path, "two").unwrap();
        assert_eq!(fs::read_to_string(&path).unwrap(), "two");
        let names: Vec<_> = fs::read_dir(path.parent().unwrap())
            .unwrap()
            .map(|e| e.unwrap().file_name())
            .collect();
        assert_eq!(names, ["c.json"]);
    }
}
