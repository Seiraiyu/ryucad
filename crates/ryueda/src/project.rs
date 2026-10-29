//! A project: a folder with `ryueda.json`, its own library under
//! `library/`, and library projects beside it that its manifest names by
//! alias. A library project is a project whose manifest says
//! `project_kind: library`; it holds a library and no design.

use crate::doc;
use crate::error::{Code, Error, Result};
use crate::fault::{Fault, Severity};
use crate::footprint::Footprint;
use crate::id::{Id, ProjectId};
use crate::libcomp::LibRef;
use crate::library::{self, Library, Loaded, ObjKind};
use crate::source::{Source, join, plain};
use crate::symbol::Symbol;
use crate::units::Unit;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

pub const MANIFEST: &str = "ryueda.json";
pub const KIND: &str = "ryueda.project";
pub const FORMAT: u64 = 1;
/// The alias of a project's own library, never written.
pub const SELF: &str = "self";

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ProjectKind {
    /// Designs and a library of its own.
    #[default]
    Design,
    /// Only a library, for other projects to name.
    Library,
}

impl ProjectKind {
    fn is_design(&self) -> bool {
        *self == ProjectKind::Design
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Manifest {
    /// 128 random bits, written at `init`: the same form as a ryucad
    /// manifest's, so ryupdm can name ryueda projects.
    pub project: ProjectId,
    /// The unit of lengths an agent types as bare numbers.
    pub default_unit: Unit,
    #[serde(default, skip_serializing_if = "ProjectKind::is_design")]
    pub project_kind: ProjectKind,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub libraries: BTreeMap<Id, Alias>,
}

/// A library project, by its folder relative to this one.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Alias {
    pub path: String,
}

impl Manifest {
    pub fn new(project: ProjectId, default_unit: Unit, kind: ProjectKind) -> Manifest {
        Manifest {
            project,
            default_unit,
            project_kind: kind,
            libraries: BTreeMap::new(),
        }
    }

    /// Reads a manifest. An alias named `self` is refused: it would stand
    /// in for the project's own library, so writes would land in another
    /// project, outside the activity log.
    pub fn read(bytes: &[u8]) -> Result<Manifest> {
        let m: Manifest = doc::read(bytes, MANIFEST, KIND, FORMAT)?;
        if m.libraries.keys().any(|a| a.as_str() == SELF) {
            return Err(Error::new(
                Code::InvalidDocument,
                format!("{MANIFEST}: no library may be named {SELF}, the project's own"),
            )
            .with("path", MANIFEST)
            .with("pointer", format!("/libraries/{SELF}")));
        }
        Ok(m)
    }

    pub fn to_text(&self) -> String {
        doc::to_string(self, KIND, FORMAT)
    }
}

/// Whether `path` is written as an alias's path must be: relative, with
/// `/` between names, none of them empty or `.`, nothing a platform would
/// read as a root or drive (`/x`, `C:`, `\\`).
pub fn path_ok(path: &str) -> bool {
    !path.is_empty()
        && !path.contains('\\')
        && !path.contains(':')
        && path.split('/').all(|s| !s.is_empty() && s != ".")
}

/// Where each alias's library project is, relative to the project (as the
/// manifest writes it), or why it can't be used: a path written wrong, no
/// project there, not a library, one naming libraries of its own, or one
/// outside the repository (outside a repository, outside the project's
/// parent folder), through a symlink or not.
pub fn resolve(root: &Path, m: &Manifest) -> BTreeMap<Id, Result<String>> {
    let canonical = root.canonicalize().map(plain);
    let fence = canonical.as_ref().ok().and_then(|r| boundary(r));
    m.libraries
        .iter()
        .map(|(alias, a)| {
            (
                alias.clone(),
                resolve_one(root, &a.path, canonical.as_deref().ok(), fence.as_deref())
                    .map_err(|e| e.with("alias", alias).with("path", &a.path)),
            )
        })
        .collect()
}

/// The folder library projects must be in: the repository holding `root`,
/// else `root`'s parent.
fn boundary(root: &Path) -> Option<PathBuf> {
    root.ancestors()
        .find(|d| d.join(".git").exists())
        .or(root.parent())
        .map(Path::to_path_buf)
}

fn missing(why: &str) -> Error {
    Error::new(Code::LibraryMissing, why.to_owned())
}

fn resolve_one(
    root: &Path,
    path: &str,
    canonical: Option<&Path>,
    fence: Option<&Path>,
) -> Result<String> {
    if !path_ok(path) {
        return Err(missing(&format!(
            "library path {path:?} isn't a relative path of folder names joined by /"
        )));
    }
    let at = path.split('/').fold(root.to_path_buf(), |p, s| p.join(s));
    let found = at
        .canonicalize()
        .map(plain)
        .map_err(|_| missing(&format!("no folder at {path}")))?;
    let (Some(canonical), Some(fence)) = (canonical, fence) else {
        return Err(missing("the project's own folder can't be found"));
    };
    if found == canonical || !found.starts_with(fence) {
        return Err(missing(&format!(
            "{path} isn't a library project beside this one, inside {}",
            fence.display()
        )));
    }
    let bytes = std::fs::read(found.join(MANIFEST))
        .map_err(|_| missing(&format!("{path} holds no ryueda project ({MANIFEST})")))?;
    let m = Manifest::read(&bytes)
        .map_err(|e| missing(&format!("{path}/{MANIFEST}: {}", e.message)))?;
    if m.project_kind != ProjectKind::Library {
        return Err(missing(&format!(
            "{path} is a design project, not a library project"
        )));
    }
    if !m.libraries.is_empty() {
        return Err(Error::new(
            Code::LibraryNested,
            format!("{path} names libraries of its own; a library project can't"),
        ));
    }
    Ok(path.to_owned())
}

/// A loaded project.
#[derive(Clone, Debug)]
pub struct Project {
    pub root: PathBuf,
    pub manifest: Manifest,
    /// A hash of the manifest: writes to it are guarded by it.
    pub revision: String,
    /// `self` and every alias, each its library or why it can't be used.
    pub libraries: BTreeMap<String, Result<Library>>,
}

/// The nearest folder at or above `start` holding a `ryueda.json`.
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
                    "no {MANIFEST} in {} or any folder above it; run `ryueda init` or pass -C <dir>",
                    start.display()
                ),
            )
            .with("path", &start)
        })
}

/// A reference's text: `lib/<alias>/<kind>/<id>`.
pub fn ref_text(alias: &str, kind: ObjKind, id: &Id) -> String {
    format!("lib/{alias}/{}/{id}", kind.as_str())
}

impl Project {
    /// Loads the project at `root` from `src`, with its aliases resolved
    /// as [`resolve`] found them.
    pub fn load_from(
        root: &Path,
        src: &dyn Source,
        aliases: &BTreeMap<Id, Result<String>>,
    ) -> Result<Project> {
        let bytes = src.read(MANIFEST)?.ok_or_else(|| {
            Error::new(
                Code::ProjectNotFound,
                format!("{} has no {MANIFEST}", root.display()),
            )
            .with("path", root)
        })?;
        let manifest = Manifest::read(&bytes)?;
        let revision = blake3::hash(&bytes).to_hex()[..32].to_owned();
        let mut libraries = BTreeMap::new();
        libraries.insert(SELF.to_owned(), library::load(src, SELF, ""));
        for alias in manifest.libraries.keys() {
            let lib = match aliases.get(alias) {
                Some(Ok(base)) => library::load(src, alias.as_str(), base),
                Some(Err(e)) => Err(e.clone()),
                None => {
                    Err(missing(&format!("library {alias} isn't resolved")).with("alias", alias))
                }
            };
            libraries.insert(alias.to_string(), lib);
        }
        let mut p = Project {
            root: root.to_path_buf(),
            manifest,
            revision,
            libraries,
        };
        p.judge_components();
        Ok(p)
    }

    /// Loads the project in its folder.
    pub fn load(root: &Path) -> Result<Project> {
        let bytes =
            std::fs::read(root.join(MANIFEST)).map_err(|e| Error::io(&root.join(MANIFEST), &e))?;
        let aliases = resolve(root, &Manifest::read(&bytes)?);
        Project::load_from(root, &crate::source::Folder(root), &aliases)
    }

    /// The library `alias` names, or why it can't be used.
    pub fn library(&self, alias: &str) -> Result<&Library> {
        match self.libraries.get(alias) {
            Some(Ok(l)) => Ok(l),
            Some(Err(e)) => Err(e.clone()),
            None => Err(Error::new(
                Code::LibraryMissing,
                format!("the project names no library {alias}"),
            )
            .with("alias", alias)),
        }
    }

    /// The library a reference from a file in library `from` reaches: its
    /// own when it names none or `self` (as its own project reads it). A
    /// library project's files name only their own library.
    fn reached(&self, from: &str, r: &LibRef) -> Result<&Library> {
        let named = r.library.as_ref().map(Id::as_str).filter(|l| *l != SELF);
        let to = named.unwrap_or(from);
        let nested = from != SELF || self.manifest.project_kind == ProjectKind::Library;
        if nested && named.is_some() {
            return Err(Error::new(
                Code::LibraryNested,
                format!("a library project's files name only their own library, not {to}"),
            )
            .with("library", to));
        }
        self.library(to)
    }

    fn object<'a, T>(
        &self,
        lib: &'a Library,
        map: &'a BTreeMap<Id, Loaded<T>>,
        kind: ObjKind,
        id: &Id,
    ) -> Result<&'a T> {
        let at = ref_text(&lib.alias, kind, id);
        match map.get(id) {
            Some(Loaded { item: Ok(t), .. }) => Ok(t),
            Some(Loaded { item: Err(e), .. }) => Err(e.clone().with("reference", &at)),
            None => Err(
                Error::new(Code::NotFound, format!("{at} isn't in the library"))
                    .with("reference", &at),
            ),
        }
    }

    /// The symbol a file in library `from` names.
    pub fn symbol(&self, from: &str, r: &LibRef) -> Result<&Symbol> {
        let lib = self.reached(from, r)?;
        self.object(lib, &lib.symbols, ObjKind::Symbol, &r.id)
    }

    /// The footprint a file in library `from` names.
    pub fn footprint(&self, from: &str, r: &LibRef) -> Result<&Footprint> {
        let lib = self.reached(from, r)?;
        self.object(lib, &lib.footprints, ObjKind::Footprint, &r.id)
    }

    /// Gives every library component the faults its checks find against
    /// the symbol and footprints it names.
    fn judge_components(&mut self) {
        let mut found: Vec<(String, Id, Vec<Fault>)> = Vec::new();
        for (alias, lib) in &self.libraries {
            let Ok(lib) = lib else { continue };
            for (id, c) in &lib.components {
                let Ok(c) = &c.item else { continue };
                let symbol = self.symbol(alias, &c.symbol);
                let footprints: Vec<Result<&Footprint>> = c
                    .footprints
                    .iter()
                    .map(|u| self.footprint(alias, &u.footprint))
                    .collect();
                let path = lib.path(ObjKind::Libcomp, id);
                let faults = c
                    .check(symbol, &footprints)
                    .into_iter()
                    .map(|f| f.with("path", &path))
                    .collect();
                found.push((alias.clone(), id.clone(), faults));
            }
        }
        for (alias, id, faults) in found {
            if let Some(Ok(lib)) = self.libraries.get_mut(&alias) {
                lib.components.get_mut(&id).expect("judged").faults = faults;
            }
        }
    }

    /// Every fault of every library: unusable aliases, files that don't
    /// read, and what checks find, in library and path order.
    pub fn faults(&self) -> Vec<Fault> {
        let mut out = Vec::new();
        for (alias, lib) in &self.libraries {
            let lib = match lib {
                Ok(l) => l,
                Err(e) => {
                    out.push(Fault {
                        severity: Severity::Error,
                        error: e.clone().with("alias", alias),
                    });
                    continue;
                }
            };
            out.extend(lib.faults.iter().cloned());
            for kind in [ObjKind::Symbol, ObjKind::Footprint, ObjKind::Libcomp] {
                for id in lib.ids(kind) {
                    let (item, faults) = match kind {
                        ObjKind::Symbol => {
                            let l = &lib.symbols[id];
                            (l.item.as_ref().err(), &l.faults)
                        }
                        ObjKind::Footprint => {
                            let l = &lib.footprints[id];
                            (l.item.as_ref().err(), &l.faults)
                        }
                        ObjKind::Libcomp => {
                            let l = &lib.components[id];
                            (l.item.as_ref().err(), &l.faults)
                        }
                    };
                    if let Some(e) = item {
                        out.push(Fault {
                            severity: Severity::Error,
                            error: e.clone(),
                        });
                    }
                    out.extend(faults.iter().cloned());
                }
            }
        }
        out
    }

    /// The files a session tracks, relative to the project: the manifest,
    /// and the library folders of this project and each alias it can use.
    pub fn tracked(root: &Path) -> Vec<(String, std::fs::Metadata)> {
        let mut out = Vec::new();
        let manifest = root.join(MANIFEST);
        let Ok(meta) = std::fs::metadata(&manifest) else {
            return out;
        };
        out.push((MANIFEST.to_owned(), meta));
        let mut bases = vec![String::new()];
        if let Ok(m) = std::fs::read(&manifest)
            .map_err(drop)
            .and_then(|b| Manifest::read(&b).map_err(drop))
        {
            bases.extend(resolve(root, &m).into_values().flatten());
        }
        for base in bases {
            if !base.is_empty() {
                let at = join(&base, MANIFEST);
                if let Ok(meta) = std::fs::metadata(root.join(&at)) {
                    out.push((at, meta));
                }
            }
            let dir = join(&base, library::DIR);
            for (name, meta) in listed(root, &dir) {
                if meta.is_dir() {
                    let sub = join(&dir, &name);
                    out.extend(
                        listed(root, &sub)
                            .into_iter()
                            .filter(|(_, m)| m.is_file())
                            .map(|(f, m)| (join(&sub, &f), m)),
                    );
                } else if meta.is_file() {
                    out.push((join(&dir, &name), meta));
                }
            }
        }
        out
    }
}

/// The entries of the folder at `rel` (relative to `root`), with their
/// metadata, sorted; none when it isn't there.
fn listed(root: &Path, rel: &str) -> Vec<(String, std::fs::Metadata)> {
    let at = rel.split('/').fold(root.to_path_buf(), |p, s| p.join(s));
    let Ok(entries) = std::fs::read_dir(at) else {
        return Vec::new();
    };
    let mut out: Vec<(String, std::fs::Metadata)> = entries
        .flatten()
        .filter_map(|e| {
            let meta = std::fs::metadata(e.path()).ok()?;
            Some((e.file_name().to_string_lossy().into_owned(), meta))
        })
        .collect();
    out.sort_by(|a, b| a.0.cmp(&b.0));
    out
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use crate::source::write_atomic;

    pub fn pid(n: u8) -> ProjectId {
        ProjectId::from_bytes([n; 16])
    }

    /// Writes a project's manifest in `dir`.
    pub fn manifest(dir: &Path, kind: ProjectKind, libraries: &[(&str, &str)]) {
        let mut m = Manifest::new(pid(1), Unit::Mm, kind);
        for (alias, path) in libraries {
            m.libraries.insert(
                Id::parse(alias, "alias").unwrap(),
                Alias {
                    path: (*path).into(),
                },
            );
        }
        write_atomic(&dir.join(MANIFEST), m.to_text()).unwrap();
    }

    /// The test library's files written under `dir`.
    pub fn library_in(dir: &Path) {
        use crate::library::tests::{STEP, files};
        for (p, b) in files(STEP) {
            write_atomic(&dir.join(p), b).unwrap();
        }
    }

    #[test]
    fn manifests_read_and_write_canonically() {
        let m = Manifest::new(pid(7), Unit::Mm, ProjectKind::Design);
        let text = m.to_text();
        assert_eq!(
            text,
            format!(
                "{{\n  \"default_unit\": \"mm\",\n  \"format\": 1,\n  \"kind\": \"ryueda.project\",\n  \"project\": \"{}\"\n}}\n",
                "07".repeat(16)
            )
        );
        assert_eq!(Manifest::read(text.as_bytes()).unwrap(), m);
        let lib = Manifest::new(pid(7), Unit::In, ProjectKind::Library).to_text();
        assert!(lib.contains("\"project_kind\": \"library\""));
        let bad = text.replace("\"project\": \"07", "\"project\": \"zz");
        assert_eq!(
            Manifest::read(bad.as_bytes()).unwrap_err().code,
            Code::InvalidDocument
        );
    }

    #[test]
    fn alias_paths_are_relative_folder_names() {
        for ok in ["../passives", "libs/passives", "../../a/b"] {
            assert!(path_ok(ok), "{ok}");
        }
        for bad in ["", "/abs", "a//b", "./a", "a/", "C:/x", "..\\x", "a/./b"] {
            assert!(!path_ok(bad), "{bad}");
        }
    }

    /// A repository holding a design project `board`, a library project
    /// `passives` beside it, and what else a test adds.
    fn repo() -> tempfile::TempDir {
        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir(dir.path().join(".git")).unwrap();
        manifest(&dir.path().join("passives"), ProjectKind::Library, &[]);
        library_in(&dir.path().join("passives"));
        dir
    }

    #[test]
    fn aliases_reach_library_projects_beside_the_project() {
        let dir = repo();
        let board = dir.path().join("board");
        let outside = tempfile::tempdir().unwrap();
        manifest(&outside.path().join("lib"), ProjectKind::Library, &[]);
        manifest(&dir.path().join("design"), ProjectKind::Design, &[]);
        manifest(
            &dir.path().join("nested"),
            ProjectKind::Library,
            &[("x", "../passives")],
        );
        let far = format!("../{}", "../".repeat(board.components().count() + 2));
        let far = format!(
            "{far}{}",
            outside
                .path()
                .join("lib")
                .to_string_lossy()
                .trim_start_matches('/')
        );
        manifest(
            &board,
            ProjectKind::Design,
            &[
                ("passives", "../passives"),
                ("none", "../none"),
                ("design", "../design"),
                ("nested", "../nested"),
                ("bad", "/abs"),
                ("far", &far),
            ],
        );
        let p = Project::load(&board).unwrap();
        let code = |a: &str| p.library(a).map(|_| ()).map_err(|e| e.code);
        assert_eq!(code("passives"), Ok(()));
        assert_eq!(code("self"), Ok(()));
        for a in ["none", "design", "bad", "far"] {
            assert_eq!(code(a), Err(Code::LibraryMissing), "{a}");
        }
        assert_eq!(code("nested"), Err(Code::LibraryNested));
        assert_eq!(code("other"), Err(Code::LibraryMissing));
        let lib = p.library("passives").unwrap();
        assert_eq!(lib.base, "../passives");
        assert!(lib.components.values().all(Loaded::is_sound));
        // Every fault, an alias's included, comes out once.
        assert_eq!(p.faults().len(), 5);
        let tracked: Vec<String> = Project::tracked(&board)
            .into_iter()
            .map(|(p, _)| p)
            .collect();
        assert!(
            tracked.contains(&"../passives/library/symbols/r.json".to_owned()),
            "{tracked:?}"
        );
        assert!(tracked.contains(&"../passives/ryueda.json".to_owned()));
        assert_eq!(tracked[0], MANIFEST);
    }

    #[test]
    fn components_resolve_across_libraries_one_step_only() {
        let dir = repo();
        let board = dir.path().join("board");
        manifest(&board, ProjectKind::Design, &[("passives", "../passives")]);
        // The board's own component, on the alias's symbol and footprint.
        let c = r#"{"format": 1, "kind": "ryueda.libcomp", "prefix": "R", "symbol": {"library": "passives", "id": "r"},
            "footprints": [{"footprint": {"library": "passives", "id": "r_0402_1005metric"}}, {"footprint": {"id": "nothing"}}]}"#;
        write_atomic(&board.join("library/components/mine.json"), c).unwrap();
        // The library project's component naming a library of its own.
        let n = r#"{"format": 1, "kind": "ryueda.libcomp", "prefix": "R", "symbol": {"library": "other", "id": "r"}}"#;
        write_atomic(
            &dir.path().join("passives/library/components/nested.json"),
            n,
        )
        .unwrap();
        let p = Project::load(&board).unwrap();
        let mine = &p.library("self").unwrap().components[&Id::parse("mine", "c").unwrap()];
        let codes: Vec<(Code, &str)> = mine
            .faults
            .iter()
            .map(|f| (f.error.code, f.error.details["pointer"].as_str().unwrap()))
            .collect();
        assert_eq!(codes, [(Code::NotFound, "/footprints/1/footprint")]);
        assert_eq!(
            mine.faults[0].error.details["path"],
            "library/components/mine.json"
        );
        let nested = &p.library("passives").unwrap().components[&Id::parse("nested", "c").unwrap()];
        assert_eq!(nested.faults[0].error.code, Code::LibraryNested);
        assert_eq!(
            nested.faults[0].error.details["path"],
            "../passives/library/components/nested.json"
        );
    }

    /// `self` in a library project's file is that file's own library, as
    /// the library project reads it, never the library of a project that
    /// names it by an alias.
    #[test]
    fn self_in_an_aliased_library_is_its_own() {
        let dir = repo();
        let board = dir.path().join("board");
        manifest(&board, ProjectKind::Design, &[("passives", "../passives")]);
        // The board's own library has a symbol `r` but no footprint, so
        // reaching it instead of the library project's would be `not_found`.
        let drawing = r#"{"format": 1, "kind": "ryueda.symbol", "name": "R"}"#;
        write_atomic(&board.join("library/symbols/r.json"), drawing).unwrap();
        let c = r#"{"format": 1, "kind": "ryueda.libcomp", "prefix": "R", "symbol": {"library": "self", "id": "r"},
            "footprints": [{"footprint": {"library": "self", "id": "r_0402_1005metric"}}]}"#;
        let path = dir.path().join("passives/library/components/explicit.json");
        write_atomic(&path, c).unwrap();
        let alone = Project::load(&dir.path().join("passives")).unwrap();
        let p = Project::load(&board).unwrap();
        let id = Id::parse("explicit", "c").unwrap();
        let faults = |p: &Project, alias: &str| {
            p.library(alias).unwrap().components[&id]
                .faults
                .iter()
                .map(|f| f.error.code)
                .collect::<Vec<_>>()
        };
        assert_eq!(faults(&alone, SELF), faults(&p, "passives"));
        assert!(
            !faults(&p, "passives").contains(&Code::NotFound),
            "{:?}",
            faults(&p, "passives")
        );
    }

    #[test]
    fn manifests_name_no_alias_self() {
        let mut m = Manifest::new(pid(7), Unit::Mm, ProjectKind::Design);
        m.libraries.insert(
            Id::parse(SELF, "alias").unwrap(),
            Alias {
                path: "../passives".into(),
            },
        );
        let e = Manifest::read(m.to_text().as_bytes()).unwrap_err();
        assert_eq!(
            (e.code, &e.details["pointer"]),
            (Code::InvalidDocument, &serde_json::json!("/libraries/self"))
        );
    }

    #[test]
    fn roots_are_found_above() {
        let dir = repo();
        let lib = dir.path().join("passives");
        assert_eq!(
            find_root(&lib.join("library/symbols")).unwrap(),
            plain(lib.canonicalize().unwrap())
        );
        assert_eq!(
            find_root(dir.path()).unwrap_err().code,
            Code::ProjectNotFound
        );
    }
}
