//! A project folder: finding it, loading it, and writing its files.
//!
//! ```text
//! ryucad.json
//! ryupdm.json                       (optional: ryupdm's settings)
//! parts/<part>/part.json
//! parts/<part>/parameters.json      (optional)
//! parts/<part>/properties.json      (optional: ryupdm's properties)
//! parts/<part>/features/<feature>.json
//! releases/<part>/<label>.json      (ryupdm's release records)
//! ```
//!
//! ryupdm's files are read and checked by ryupdm; here they are only
//! known, so they are tracked and hashed with the files they belong to.
//!
//! Loading never stops at a broken part or feature: the error is kept in
//! its place and the rest of the project stays usable. A file a merge left
//! with conflicts is broken in its place too (`merge_conflict`); a
//! conflicted manifest breaks every part, but the project still opens, so
//! the merge can be reported and resolved.

use super::canonical;
use super::files::{Feature, Manifest, ParametersFile, PartFile};
use super::id::Id;
use super::source::{Folder, Kind, Source, join};
use crate::error::{Code, Error, Result};
use crate::expr::Env;
use serde_json::Value;
use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};

pub const MANIFEST: &str = "ryucad.json";

/// The project's Git attributes file, beside its manifest: it registers
/// ryucad's drivers, and defines nothing ryucad builds.
pub const ATTRIBUTES: &str = ".gitattributes";

/// ryupdm's settings, beside the manifest. Nothing ryucad builds reads it.
pub const PDM_CONFIG: &str = "ryupdm.json";

/// The folder of ryupdm's release records, beside the manifest. Nothing
/// ryucad builds reads it.
pub const RELEASES: &str = "releases";

/// The files at the project's root that a session tracks.
pub const ROOT_FILES: [&str; 3] = [MANIFEST, ATTRIBUTES, PDM_CONFIG];

/// The key under which a file a merge couldn't settle lists its conflicts,
/// by JSON pointer. No file kind uses it otherwise.
pub const CONFLICTS: &str = "conflicts";

#[derive(Clone, Debug)]
pub struct Project {
    pub root: PathBuf,
    pub manifest: Manifest,
    pub parts: BTreeMap<Id, Part>,
    /// Entries under `parts/` that don't belong there, as paths relative to
    /// the root.
    pub unknown: Vec<String>,
    /// Why every part fails when the manifest holds merge conflicts: the
    /// manifest's other values are ours' as the merge left them.
    pub manifest_fault: Option<Error>,
    /// `ryucad.json` as read: every part's revision hashes it.
    manifest_bytes: Vec<u8>,
}

#[derive(Clone, Debug)]
pub struct Part {
    pub id: Id,
    /// The feature list, or why `part.json` can't be used.
    pub file: Result<PartFile>,
    /// `parameters.json`, if the part has one, or why it can't be used.
    pub parameters: Option<Result<ParametersFile>>,
    /// The parameters' values, which the features were evaluated with.
    pub env: Env,
    /// Every feature file in `features/`, listed or not.
    pub features: BTreeMap<Id, Result<Feature>>,
    /// The parameters each feature file names, read from its text, so it
    /// is known even for a feature that doesn't evaluate.
    pub uses: BTreeMap<Id, BTreeSet<String>>,
    /// What each feature file that reads as JSON but doesn't load would do
    /// to the part's solids, read from its text.
    pub acts: BTreeMap<Id, super::files::Acts>,
    /// A hash of the bytes of `ryucad.json`, `part.json`, `parameters.json`
    /// and the listed feature files in order: it changes whenever anything
    /// that defines the part does.
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
pub fn plain(p: PathBuf) -> PathBuf {
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

/// The nearest folder at or above `start` that holds a `ryucad.json`. A
/// folder that has lost its manifest but still has parts is
/// `project_damaged`, not `project_not_found`.
pub fn find_root(start: &Path) -> Result<PathBuf> {
    match find_project(start)? {
        (root, false) => Ok(root),
        (root, true) => Err(Error::new(
            Code::ProjectDamaged,
            format!(
                "{} has parts but no {MANIFEST}; run `ryucad doctor` to repair it",
                root.display()
            ),
        )
        .with("path", &root)
        .with("repair", "ryucad doctor")),
    }
}

/// The project at or above `start`, and whether it is damaged (parts but
/// no manifest). The nearest folder with either wins.
pub fn find_project(start: &Path) -> Result<(PathBuf, bool)> {
    let start = plain(start.canonicalize().map_err(|e| Error::io(start, &e))?);
    for d in start.ancestors() {
        if d.join(MANIFEST).is_file() {
            return Ok((d.to_path_buf(), false));
        }
        if has_parts(d) {
            return Ok((d.to_path_buf(), true));
        }
    }
    Err(Error::new(
        Code::ProjectNotFound,
        format!(
            "no {MANIFEST} in {} or any folder above it; run `ryucad init` or pass -C <dir>",
            start.display()
        ),
    )
    .with("path", &start))
}

/// Whether `dir/parts` holds a ryucad part file.
fn has_parts(dir: &Path) -> bool {
    let Ok(entries) = fs::read_dir(dir.join("parts")) else {
        return false;
    };
    entries.flatten().any(|e| {
        fs::read_to_string(e.path().join("part.json"))
            .ok()
            .and_then(|t| serde_json::from_str::<Value>(&t).ok())
            .is_some_and(|v| v["kind"] == "ryucad.part")
    })
}

pub fn part_dir(root: &Path, part: &Id) -> PathBuf {
    root.join("parts").join(part.as_str())
}

pub fn part_path(root: &Path, part: &Id) -> PathBuf {
    part_dir(root, part).join("part.json")
}

pub const PARAMETERS: &str = "parameters.json";

pub fn parameters_path(root: &Path, part: &Id) -> PathBuf {
    part_dir(root, part).join(PARAMETERS)
}

/// A part's ryupdm properties: one of the files that define the part, so
/// its revision covers them, though nothing ryucad builds reads them.
pub const PROPERTIES: &str = "properties.json";

pub fn properties_path(root: &Path, part: &Id) -> PathBuf {
    part_dir(root, part).join(PROPERTIES)
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

/// `merge_conflict` for the file at `at` if its JSON lists merge
/// conflicts, naming their pointers, and a feature's `type` where no
/// conflict is about it, so a failed feature can still say what it is.
pub fn conflicted(v: &Value, at: &str) -> Option<Error> {
    let listed = v.as_object()?.get(CONFLICTS)?;
    let pointers: Vec<&str> = listed
        .as_object()
        .map(|o| o.keys().map(String::as_str).collect())
        .unwrap_or_default();
    let at_list = if pointers.is_empty() {
        String::new()
    } else {
        format!(" at {}", pointers.join(", "))
    };
    let ty = v
        .get("type")
        .and_then(Value::as_str)
        .filter(|_| v.get("kind").and_then(Value::as_str) == Some("ryucad.feature"))
        .filter(|_| !pointers.iter().any(|p| p.is_empty() || *p == "/type"));
    let e = Error::new(
        Code::MergeConflict,
        format!("{at} has unresolved merge conflicts{at_list}; see `ryucad merge status`"),
    )
    .with("path", at)
    .with("pointers", &pointers);
    Some(match ty {
        Some(t) => e.with("type", t),
        None => e,
    })
}

/// `v` without its merge conflicts: the values the merge left in place.
pub fn without_conflicts(v: &Value) -> Value {
    let mut v = v.clone();
    if let Some(o) = v.as_object_mut() {
        o.remove(CONFLICTS);
    }
    v
}

/// Reads and parses the JSON file at `at` (relative to the root); errors
/// carry that path. A file with merge conflicts is `merge_conflict`.
fn read_json(src: &dyn Source, at: &str) -> Result<(Vec<u8>, Value)> {
    let (bytes, value) = read_raw(src, at)?;
    match conflicted(&value, at) {
        Some(e) => Err(e),
        None => Ok((bytes, value)),
    }
}

/// [`read_json`], conflicts or not.
fn read_raw(src: &dyn Source, at: &str) -> Result<(Vec<u8>, Value)> {
    let bytes = src
        .read(at)
        .map_err(|e| e.with("path", at))?
        .ok_or_else(|| Error::new(Code::NotFound, format!("{at} is missing")).with("path", at))?;
    let text = std::str::from_utf8(&bytes).map_err(|_| {
        Error::new(Code::InvalidJson, format!("{at} is not UTF-8")).with("path", at)
    })?;
    let value = canonical::parse(text).map_err(|e| e.with("path", at))?;
    Ok((bytes, value))
}

impl Project {
    /// Loads the project at `root`. Only a missing or broken manifest is an
    /// error; anything wrong inside a part is kept in that part.
    pub fn load(root: &Path) -> Result<Project> {
        Project::load_from(root, &Folder(root))
    }

    /// Loads the project whose files `src` holds, as if they were at
    /// `root`: the same project, faults and revisions as loading them from
    /// a folder.
    pub fn load_from(root: &Path, src: &dyn Source) -> Result<Project> {
        let (manifest_bytes, v) = read_raw(src, MANIFEST).map_err(|e| {
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
        let manifest_fault = conflicted(&v, MANIFEST);
        let manifest = Manifest::from_json(&without_conflicts(&v)).map_err(|e| {
            manifest_fault
                .clone()
                .unwrap_or_else(|| e.with("path", MANIFEST))
        })?;
        let mut parts = BTreeMap::new();
        let mut unknown = Vec::new();
        for (name, kind) in src.list("parts")? {
            match Id::parse(&name, "part") {
                Ok(id) if kind == Kind::Dir => {
                    let part = load_part(
                        src,
                        &manifest,
                        manifest_fault.as_ref(),
                        &manifest_bytes,
                        id.clone(),
                        &mut unknown,
                    )?;
                    parts.insert(id, part);
                }
                _ => unknown.push(join("parts", &name)),
            }
        }
        Ok(Project {
            root: root.to_path_buf(),
            manifest,
            parts,
            unknown,
            manifest_fault,
            manifest_bytes,
        })
    }

    /// Brings the project up to date after the files at `changed` (paths
    /// relative to the root, with `/`) changed, appeared or went: only the
    /// parts they lie in are read again, unless the manifest or anything
    /// outside a part changed. Leaves the project as [`Project::load`]
    /// would.
    pub fn reload(&mut self, changed: &[String]) -> Result<()> {
        let mut ids = BTreeSet::new();
        for path in changed {
            let mut parts = path.split('/');
            let in_part = match (parts.next(), parts.next(), parts.next()) {
                (Some("parts"), Some(name), Some(_)) => Id::parse(name, "part").ok(),
                _ => None,
            };
            match in_part {
                Some(id) => ids.insert(id),
                // Git's attributes, ryupdm's settings and its release
                // records define nothing ryucad builds.
                None if path == ATTRIBUTES
                    || path == PDM_CONFIG
                    || path.starts_with(&format!("{RELEASES}/")) =>
                {
                    continue;
                }
                None => {
                    *self = Project::load(&self.root)?;
                    return Ok(());
                }
            };
        }
        for id in ids {
            // A part's own entries sit together, where its name sorts.
            let mine = format!("parts/{id}/");
            self.unknown.retain(|u| !u.starts_with(&mine));
            let at = self
                .unknown
                .iter()
                .position(|u| u.split('/').nth(1).is_some_and(|n| n > id.as_str()))
                .unwrap_or(self.unknown.len());
            if part_dir(&self.root, &id).is_dir() {
                let mut unknown = Vec::new();
                let part = load_part(
                    &Folder(&self.root),
                    &self.manifest,
                    self.manifest_fault.as_ref(),
                    &self.manifest_bytes,
                    id.clone(),
                    &mut unknown,
                )?;
                self.parts.insert(id, part);
                self.unknown.splice(at..at, unknown);
            } else {
                self.parts.remove(&id);
            }
        }
        Ok(())
    }

    pub fn part(&self, id: &Id) -> Result<&Part> {
        self.parts
            .get(id)
            .ok_or_else(|| Error::new(Code::NotFound, format!("no part {id}")).with("part", id))
    }
}

fn load_part(
    src: &dyn Source,
    manifest: &Manifest,
    manifest_fault: Option<&Error>,
    manifest_bytes: &[u8],
    id: Id,
    unknown: &mut Vec<String>,
) -> Result<Part> {
    let dir = join("parts", id.as_str());
    // Each piece is length-prefixed, so no two sets of files hash alike.
    let mut hasher = blake3::Hasher::new();
    let piece = |h: &mut blake3::Hasher, bytes: &[u8]| {
        h.update(&(bytes.len() as u64).to_le_bytes());
        h.update(bytes);
    };
    piece(&mut hasher, manifest_bytes);
    let part_at = join(&dir, "part.json");
    let file = read_json(src, &part_at).and_then(|(bytes, v)| {
        piece(&mut hasher, &bytes);
        PartFile::from_json(&v).map_err(|e| e.with("path", &part_at))
    });
    let mut has_parameters = false;
    let mut has_properties = false;
    for (name, kind) in src.list(&dir)? {
        match name.as_str() {
            "part.json" | "features" => {}
            PARAMETERS => has_parameters = kind == Kind::File,
            PROPERTIES => has_properties = kind == Kind::File,
            _ => unknown.push(join(&dir, &name)),
        }
    }
    let params_at = join(&dir, PARAMETERS);
    let parameters = has_parameters.then(|| {
        read_json(src, &params_at).and_then(|(bytes, v)| {
            // Tagged, so a part with an empty parameters file and one
            // with none hash apart; a part with none hashes as before.
            piece(&mut hasher, PARAMETERS.as_bytes());
            piece(&mut hasher, &bytes);
            ParametersFile::from_json(&v).map_err(|e| e.with("path", &params_at))
        })
    });
    let env = match &parameters {
        None => Env::empty(),
        Some(Ok(p)) => p.env(),
        // Features that read a parameter fail with this, naming the file.
        Some(Err(e)) => Env::broken(e.clone().with("parameters", &params_at)),
    };
    let mut features = BTreeMap::new();
    let mut uses = BTreeMap::new();
    let mut acts = BTreeMap::new();
    let mut bytes_of = BTreeMap::new();
    let features_dir = join(&dir, "features");
    for (name, kind) in src.list(&features_dir)? {
        let at = join(&features_dir, &name);
        let fid = name
            .strip_suffix(".json")
            .and_then(|stem| Id::parse(stem, "feature").ok());
        match fid {
            Some(fid) if kind == Kind::File => {
                let feature = read_json(src, &at).and_then(|(bytes, v)| {
                    bytes_of.insert(fid.clone(), bytes);
                    uses.insert(fid.clone(), super::files::feature_names(&v));
                    Feature::from_json(&v, manifest.default_unit, &env).map_err(|e| {
                        acts.insert(fid.clone(), super::files::feature_acts(&v));
                        e.with("path", &at)
                    })
                });
                features.insert(fid, feature);
            }
            _ => unknown.push(at),
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
                    let at = join(&features_dir, &format!("{fid}.json"));
                    features.entry(fid.clone()).or_insert_with(|| {
                        Err(Error::new(Code::NotFound, format!("{at} is missing"))
                            .with("path", &at))
                    });
                }
            }
        }
    }
    // ryupdm's properties: their bytes as they are, tagged like the
    // parameters, so a part without them hashes as before. Reading them is
    // ryupdm's business.
    if has_properties {
        let at = join(&dir, PROPERTIES);
        if let Some(bytes) = src.read(&at).map_err(|e| e.with("path", &at))? {
            piece(&mut hasher, PROPERTIES.as_bytes());
            piece(&mut hasher, &bytes);
        }
    }
    let revision = hasher.finalize().to_hex()[..32].to_owned();
    // A conflicted manifest leaves no part usable.
    let file = match manifest_fault {
        Some(e) => Err(e.clone()),
        None => file,
    };
    Ok(Part {
        id,
        file,
        parameters,
        env,
        features,
        uses,
        acts,
        revision,
    })
}

/// Writes `contents` to `path` so that readers see the old file or the new
/// one, never a mix: a temporary file in the same folder, then a rename.
pub fn write_atomic(path: &Path, contents: impl AsRef<[u8]>) -> Result<()> {
    let dir = path.parent().expect("a file path has a parent");
    fs::create_dir_all(dir).map_err(|e| Error::io(dir, &e))?;
    let name = path.file_name().expect("a file path has a name");
    let tmp = dir.join(format!(".{}.tmp", name.to_string_lossy()));
    let write = || -> std::io::Result<()> {
        let mut f = fs::File::create(&tmp)?;
        f.write_all(contents.as_ref())?;
        f.sync_all()?;
        fs::rename(&tmp, path)
    };
    write().map_err(|e| {
        let _ = fs::remove_file(&tmp);
        Error::io(path, &e)
    })
}

/// Deletes a project file, then the folders above it that are left empty,
/// up to `parts/`.
pub fn remove_file(root: &Path, path: &Path) -> Result<()> {
    fs::remove_file(path).map_err(|e| Error::io(path, &e))?;
    let stop = root.join("parts");
    let mut dir = path.parent();
    while let Some(d) = dir {
        if d == stop || !d.starts_with(&stop) || fs::remove_dir(d).is_err() {
            break;
        }
        dir = d.parent();
    }
    Ok(())
}

/// Writes a JSON document in canonical form.
pub fn write_json(path: &Path, v: &Value) -> Result<()> {
    write_atomic(path, canonical::to_string(v))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::doc::files::{Placement, Primitive, PrimitiveType};
    use crate::units::{Quantity, Unit};

    fn id(s: &str) -> Id {
        Id::parse(s, "feature").unwrap()
    }

    /// A project with part `p` listing features `a` and `b`.
    fn sample() -> tempfile::TempDir {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        write_json(&root.join(MANIFEST), &Manifest::new(Unit::Mm).to_json()).unwrap();
        let p = id("p");
        write_json(
            &part_path(root, &p),
            &PartFile::new(vec![id("a"), id("b")]).to_json(),
        )
        .unwrap();
        for (f, r) in [("a", "1"), ("b", "2")] {
            let feature = Feature::Primitive(Primitive {
                ty: PrimitiveType::Sphere,
                params: [("radius".to_owned(), Quantity::parse(r, Unit::Mm).unwrap())].into(),
                placement: Placement::identity(Unit::Mm),
                combine: crate::doc::solid::Combine::NEW,
            });
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
            part.features[&id("b")]
                .as_ref()
                .unwrap()
                .primitive()
                .unwrap()
                .meters("radius"),
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
            plain(dir.path().canonicalize().unwrap())
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
    fn a_conflicted_file_fails_in_its_place() {
        let dir = sample();
        let root = dir.path();
        let a = feature_path(root, &id("p"), &id("a"));
        let mut v: Value = serde_json::from_str(&fs::read_to_string(&a).unwrap()).unwrap();
        v[CONFLICTS] = serde_json::json!({
            "/params/radius": {"base": "1 mm", "kind": "value", "ours": "1 mm", "theirs": "3 mm"},
            "/placement/x": {"kind": "value", "ours": [1, 0, 0], "theirs": [0, 1, 0]},
        });
        write_json(&a, &v).unwrap();
        let project = Project::load(root).unwrap();
        let part = project.part(&id("p")).unwrap();
        let e = part.features[&id("a")].as_ref().unwrap_err();
        assert_eq!(
            (e.code, e.details["path"].as_str()),
            (Code::MergeConflict, Some("parts/p/features/a.json"))
        );
        assert_eq!(
            e.details["pointers"],
            serde_json::json!(["/params/radius", "/placement/x"])
        );
        // Its type is still known, unless the conflict is about it.
        assert_eq!(e.details["type"], v["type"]);
        assert!(v["type"].is_string());
        let mut w = v.clone();
        w[CONFLICTS]["/type"] = serde_json::json!({"kind": "value"});
        assert!(
            conflicted(&w, "a.json")
                .unwrap()
                .details
                .get("type")
                .is_none()
        );
        assert!(part.features[&id("b")].is_ok());
        // A conflicted manifest fails every part, not the project.
        let mut m = Manifest::new(Unit::Mm).to_json();
        m[CONFLICTS] = serde_json::json!({
            "/default_unit": {"base": "mm", "kind": "value", "ours": "mm", "theirs": "in"},
        });
        write_json(&root.join(MANIFEST), &m).unwrap();
        let project = Project::load(root).unwrap();
        assert_eq!(project.manifest.default_unit, Unit::Mm);
        let e = project.part(&id("p")).unwrap().file.as_ref().unwrap_err();
        assert_eq!(
            (e.code, e.details["path"].as_str()),
            (Code::MergeConflict, Some(MANIFEST))
        );
        assert_eq!(
            format!("{:?}", Project::load_from(root, &all_files(root)).unwrap()),
            format!("{project:?}")
        );
    }

    /// Every file under `root`, by relative path.
    fn all_files(root: &Path) -> BTreeMap<String, Vec<u8>> {
        let mut out = BTreeMap::new();
        let mut dirs = vec![root.to_path_buf()];
        while let Some(d) = dirs.pop() {
            for e in fs::read_dir(&d).unwrap() {
                let p = e.unwrap().path();
                if p.is_dir() {
                    dirs.push(p);
                } else {
                    out.insert(relative(root, &p), fs::read(&p).unwrap());
                }
            }
        }
        out
    }

    #[test]
    fn files_in_memory_load_as_the_folder_does() {
        let dir = sample();
        let root = dir.path();
        let same = || {
            let from = Project::load_from(root, &all_files(root)).unwrap();
            assert_eq!(
                format!("{from:?}"),
                format!("{:?}", Project::load(root).unwrap())
            );
        };
        same();
        // Faults stay in their places, with the same paths.
        fs::write(feature_path(root, &id("p"), &id("a")), "{ nope").unwrap();
        fs::write(root.join("parts/p/features/c.json"), "{}").unwrap();
        fs::write(root.join("parts/p/notes.txt"), "hi").unwrap();
        fs::write(root.join("parts/stray.json"), "{}").unwrap();
        fs::create_dir_all(root.join("parts/p/features/deeper")).unwrap();
        fs::write(root.join("parts/p/features/deeper/x.json"), "{}").unwrap();
        write_json(
            &parameters_path(root, &id("p")),
            &serde_json::json!({"format": 1, "kind": "ryucad.parameters",
                                "parameters": {"r": "3 mm"}}),
        )
        .unwrap();
        same();
        fs::remove_file(root.join(MANIFEST)).unwrap();
        let e = Project::load_from(root, &all_files(root)).unwrap_err();
        assert_eq!(e.code, Code::ProjectNotFound);
    }

    #[test]
    fn reloading_what_changed_matches_a_fresh_load() {
        let dir = sample();
        let root = dir.path();
        let mut project = Project::load(root).unwrap();
        let same = |project: &Project| {
            assert_eq!(
                format!("{project:?}"),
                format!("{:?}", Project::load(root).unwrap())
            );
        };
        let feature = |r: &str| {
            Feature::Primitive(Primitive {
                ty: PrimitiveType::Sphere,
                params: [("radius".to_owned(), Quantity::parse(r, Unit::Mm).unwrap())].into(),
                placement: Placement::identity(Unit::Mm),
                combine: crate::doc::solid::Combine::NEW,
            })
            .to_json()
        };
        // An edit inside a part, a broken file and a stray one.
        write_json(&feature_path(root, &id("p"), &id("a")), &feature("5")).unwrap();
        fs::write(feature_path(root, &id("p"), &id("b")), "{ nope").unwrap();
        fs::write(root.join("parts/p/notes.txt"), "hi").unwrap();
        project
            .reload(&[
                "parts/p/features/a.json".into(),
                "parts/p/features/b.json".into(),
                "parts/p/notes.txt".into(),
            ])
            .unwrap();
        same(&project);
        // New parts either side of it, with strays of their own.
        for q in ["a", "q"] {
            write_json(&part_path(root, &id(q)), &PartFile::new(vec![]).to_json()).unwrap();
            fs::write(root.join(format!("parts/{q}/x.txt")), "x").unwrap();
            let changed = [format!("parts/{q}/part.json"), format!("parts/{q}/x.txt")];
            project.reload(&changed).unwrap();
            same(&project);
        }
        // A part that goes, then the manifest, which every part hashes.
        fs::remove_dir_all(root.join("parts/q")).unwrap();
        project
            .reload(&["parts/q/part.json".into(), "parts/q/x.txt".into()])
            .unwrap();
        same(&project);
        write_json(&root.join(MANIFEST), &Manifest::new(Unit::In).to_json()).unwrap();
        project.reload(&[MANIFEST.into()]).unwrap();
        same(&project);
        // A release record defines nothing a part is: nothing is read.
        fs::create_dir_all(root.join("releases/p")).unwrap();
        fs::write(root.join("releases/p/A.json"), "{}").unwrap();
        fs::remove_file(root.join(MANIFEST)).unwrap();
        let before = format!("{project:?}");
        project.reload(&["releases/p/A.json".into()]).unwrap();
        assert_eq!(format!("{project:?}"), before);
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
            Manifest::new(Unit::In).to_json().to_string(),
        )
        .unwrap();
        let r2 = rev();
        assert_ne!(r2, r1);
        // ryupdm's properties define the part; its settings don't.
        let props = properties_path(root, &id("p"));
        fs::write(&props, "{}").unwrap();
        let r3 = rev();
        assert_ne!(r3, r2);
        fs::write(&props, "{\"number\": \"1\"}").unwrap();
        assert_ne!(rev(), r3);
        fs::remove_file(&props).unwrap();
        assert_eq!(rev(), r2);
        fs::write(root.join(PDM_CONFIG), "{}").unwrap();
        assert_eq!(rev(), r2);
        let project = Project::load(root).unwrap();
        assert!(project.unknown.is_empty(), "{:?}", project.unknown);
    }

    #[test]
    fn ryupdm_files_are_known_and_reload_nothing_they_dont_define() {
        let dir = sample();
        let root = dir.path();
        fs::write(properties_path(root, &id("p")), "{ not even json").unwrap();
        let mut project = Project::load(root).unwrap();
        assert!(project.unknown.is_empty());
        let rev = project.parts[&id("p")].revision.clone();
        fs::write(root.join(PDM_CONFIG), "{}").unwrap();
        project.reload(&[PDM_CONFIG.into()]).unwrap();
        assert_eq!(project.parts[&id("p")].revision, rev);
        fs::write(properties_path(root, &id("p")), "{}").unwrap();
        project.reload(&["parts/p/properties.json".into()]).unwrap();
        assert_ne!(project.parts[&id("p")].revision, rev);
    }

    #[test]
    fn features_read_the_parts_parameters() {
        let dir = sample();
        let root = dir.path();
        let p = id("p");
        let before = Project::load(root).unwrap().parts[&p].revision.clone();
        write_json(
            &parameters_path(root, &p),
            &serde_json::json!({"format": 1, "kind": "ryucad.parameters",
                                "parameters": {"r": "3 mm"}}),
        )
        .unwrap();
        let a = feature_path(root, &p, &id("a"));
        let text = fs::read_to_string(&a).unwrap();
        fs::write(&a, text.replace("\"1 mm\"", "\"r * 2\"")).unwrap();
        let project = Project::load(root).unwrap();
        let part = &project.parts[&p];
        assert!(project.unknown.is_empty());
        assert_ne!(part.revision, before);
        assert_eq!(part.env.get("r").unwrap().x, 0.003);
        let f = part.features[&id("a")].as_ref().unwrap();
        assert_eq!(f.primitive().unwrap().meters("radius"), 0.006);
        // The revision follows the parameters.
        write_json(
            &parameters_path(root, &p),
            &serde_json::json!({"format": 1, "kind": "ryucad.parameters",
                                "parameters": {"r": "4 mm"}}),
        )
        .unwrap();
        let again = Project::load(root).unwrap();
        assert_ne!(again.parts[&p].revision, part.revision);
        assert_eq!(
            again.parts[&p].features[&id("a")]
                .as_ref()
                .unwrap()
                .primitive()
                .unwrap()
                .meters("radius"),
            0.008
        );
        // A broken parameters file fails the features that read it, alone.
        fs::write(parameters_path(root, &p), "{ nope").unwrap();
        let broken = Project::load(root).unwrap();
        let part = &broken.parts[&p];
        assert_eq!(
            part.parameters.as_ref().unwrap().as_ref().unwrap_err().code,
            Code::InvalidJson
        );
        let e = part.features[&id("a")].as_ref().unwrap_err();
        assert_eq!(
            (e.code, e.details["path"].as_str()),
            (Code::InvalidJson, Some("parts/p/features/a.json"))
        );
        assert_eq!(e.details["parameters"], "parts/p/parameters.json");
        assert!(part.features[&id("b")].is_ok());
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

    #[test]
    fn a_folder_with_parts_and_no_manifest_is_damaged() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().canonicalize().unwrap();
        let part = root.join("parts/p");
        fs::create_dir_all(part.join("features")).unwrap();
        fs::write(
            part.join("part.json"),
            r#"{"features":[],"format":1,"kind":"ryucad.part"}"#,
        )
        .unwrap();
        let e = find_root(&part.join("features")).unwrap_err();
        assert_eq!(e.code, Code::ProjectDamaged);
        assert_eq!(find_project(&part).unwrap(), (plain(root.clone()), true));
        // Some other folder called parts isn't a project.
        fs::write(part.join("part.json"), "{}").unwrap();
        assert_ne!(find_root(&part).unwrap_err().code, Code::ProjectDamaged);
    }
}
