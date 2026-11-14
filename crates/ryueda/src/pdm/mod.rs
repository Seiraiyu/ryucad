//! Product data management for ryueda projects (design 629): a design
//! project's designs and a library project's components are ryupdm
//! parts, with properties in `properties/<part>.json`, settings in
//! `ryupdm.json` and release records under `releases/`, as ryupdm reads
//! them for every product. Not to be confused with [`crate::props`], the
//! named text values designator formats read (design 236).

pub use ryupdm::{PDM_CONFIG, RELEASES};

/// The folder of parts' PDM properties, beside the manifest.
pub const PROPERTIES: &str = "properties";

pub mod bom;
pub mod cmd;
pub mod content;
pub mod diff;
pub mod release;

use crate::conn::{BLOCKS, CLASSES, DESIGNS, RAILS};
use crate::error::{Code, Error, Result};
use crate::id::Id;
use crate::project::{MANIFEST, Manifest, ProjectKind};
use crate::session::Session;
use crate::source::Source;
use ryupdm::product::{Identity, PartFiles, Product};
use serde_json::Value;

/// A library's components, symbols and footprints, below the project.
const LIB_COMPONENTS: &str = "library/components";
const LIB_SYMBOLS: &str = "library/symbols";
const LIB_FOOTPRINTS: &str = "library/footprints";

/// The folders a part's revision reads besides the root files: what the
/// content hashes read, what ERC reads to work a design's results out
/// again at a commit (the whole library: a footprint's model must be
/// there, though no hash reads its bytes), and the properties.
const PART_FOLDERS: [&str; 6] = [
    DESIGNS,
    BLOCKS,
    RAILS,
    CLASSES,
    crate::library::DIR,
    PROPERTIES,
];

/// The folders one part's revision reads ([`Product::part_files`]).
const READ_FOLDERS: [&str; 4] = [BLOCKS, RAILS, CLASSES, crate::library::DIR];

/// The folders whose files may belong to any part ([`Product::shared`]).
const SHARED_FOLDERS: [&str; 6] = [
    BLOCKS,
    RAILS,
    CLASSES,
    LIB_COMPONENTS,
    LIB_SYMBOLS,
    LIB_FOOTPRINTS,
];

/// The manifest of the project `src` holds.
pub(crate) fn manifest(src: &dyn Source) -> Result<Manifest> {
    let bytes = src.read(MANIFEST)?.ok_or_else(|| {
        Error::new(Code::ProjectNotFound, format!("no {MANIFEST}")).with("path", MANIFEST)
    })?;
    Manifest::read(&bytes)
}

/// The product's name in ryupdm's messages.
pub(crate) const NAME: &str = <crate::Eda as ryu_git::Layout>::NAME;

/// What guards a PDM write: BLAKE3 over the project's revision and every
/// byte of `ryupdm.json`, `properties/` and `releases/` (the project's
/// revision covers none of them). 32 hex digits.
pub fn revision(session: &Session) -> String {
    let files = session.files();
    let mut h = blake3::Hasher::new();
    h.update(session.project().revision.as_bytes());
    for (path, bytes) in files.iter().filter(|(p, _)| pdm_file(p)) {
        h.update(&(path.len() as u64).to_le_bytes());
        h.update(path.as_bytes());
        h.update(&(bytes.len() as u64).to_le_bytes());
        h.update(bytes);
    }
    h.finalize().to_hex()[..32].to_owned()
}

/// Whether `path` (relative to the project) is one of ryupdm's files:
/// `ryupdm.json`, a properties file or a release record.
pub fn pdm_file(path: &str) -> bool {
    path == PDM_CONFIG
        || path.starts_with(&format!("{PROPERTIES}/"))
        || path.starts_with(&format!("{RELEASES}/"))
}

/// Refuses a PDM write when `expected` isn't the PDM revision now.
pub fn check_revision(session: &Session, expected: Option<&str>) -> Result<()> {
    crate::server::check_revision("the project's PDM files", &revision(session), expected)
}

/// Writes `files` (each path's text, or `None` to delete it) as one
/// change, judged as every ryueda write is.
pub(crate) fn write(
    session: &mut Session,
    what: &str,
    files: Vec<(String, Option<String>)>,
) -> Result<()> {
    crate::write::commit(session, what, crate::write::Writes(files))
}

/// The project's repository and its release branch as read, from the
/// session's copy while the refs haven't moved; `not_a_git_repo` outside
/// a repository.
pub fn history(session: &mut Session) -> Result<(crate::git::Repo, ryupdm::history::Releases)> {
    let repo: crate::git::Repo = ryu_git::repo::repo(session.root())?;
    let config = ryupdm::config::Config::read_in(&session.snapshot(), NAME)?;
    let key = ryupdm::release::refs_key(repo.gix(), &config)?;
    if let Some((k, r)) = &session.pdm
        && *k == key
    {
        return Ok((repo, r.clone()));
    }
    let releases = ryupdm::history::Releases::read(repo.gix(), repo.prefix(), &config)?;
    session.pdm = Some((key, releases.clone()));
    Ok((repo, releases))
}

/// The release branch, or nothing official outside a repository.
pub fn releases_or_none(session: &mut Session) -> ryupdm::history::Releases {
    history(session).map(|(_, r)| r).unwrap_or_default()
}

/// A part's state, for `props.get`: in work outside a repository.
pub fn state_of(session: &mut Session, part: &Id) -> Value {
    let releases = releases_or_none(session);
    ryupdm::state::derive_for::<crate::Eda>(&session.snapshot(), part, &releases).to_json()
}

/// Whether `id` looks like one `name::random` drew (10 base32
/// characters with a digit or at most one vowel): a design that should
/// get a readable ID before its first property or release (design 629).
pub fn random_looking(id: &Id) -> bool {
    let s = id.as_str();
    s.len() == 10
        && s.bytes()
            .all(|c| c.is_ascii_lowercase() || (b'2'..=b'7').contains(&c))
        && (s.bytes().any(|c| c.is_ascii_digit())
            || s.bytes().filter(|c| b"aeiou".contains(c)).count() <= 1)
}

/// What a part of this project is: a design project's designs are
/// `design`, a library project's components `component`.
pub fn part_kind(kind: ProjectKind) -> ryupdm::product::PartKind {
    match kind {
        ProjectKind::Design => ryupdm::product::PartKind::Design,
        ProjectKind::Library => ryupdm::product::PartKind::Component,
    }
}

/// [`part_kind`]'s name: `design` or `component`.
pub fn kind_name(kind: ProjectKind) -> &'static str {
    part_kind(kind).as_str()
}

/// The ID a part's file (`<folder>/<id>.json`) names, if `path` is one.
fn file_id(path: &str, folder: &str) -> Option<Id> {
    let name = path.strip_prefix(folder)?.strip_prefix('/')?;
    Id::parse(name.strip_suffix(".json")?, "part").ok()
}

impl Product for crate::Eda {
    const PART_FOLDERS: &'static [&'static str] = &PART_FOLDERS;

    /// A design project's designs, a library project's components.
    fn parts(src: &dyn Source) -> Result<Vec<(Id, ryupdm::product::PartKind)>> {
        let kind = manifest(src)?.project_kind;
        let folder = match kind {
            ProjectKind::Design => DESIGNS,
            ProjectKind::Library => LIB_COMPONENTS,
        };
        let mut out: Vec<Id> = src
            .list(folder)?
            .into_iter()
            .filter(|(_, k)| *k == crate::source::Kind::File)
            .filter_map(|(n, _)| file_id(&format!("{folder}/{n}"), folder))
            .collect();
        out.sort();
        Ok(out.into_iter().map(|id| (id, part_kind(kind))).collect())
    }

    fn properties_path(part: &Id) -> String {
        format!("{PROPERTIES}/{part}.json")
    }

    fn part_files(part: &Id) -> PartFiles {
        PartFiles {
            files: vec![
                format!("{DESIGNS}/{part}.json"),
                Self::properties_path(part),
            ],
            folders: READ_FOLDERS.map(String::from).to_vec(),
        }
    }

    fn part_of(path: &str) -> Option<Id> {
        file_id(path, DESIGNS)
            .or_else(|| file_id(path, LIB_COMPONENTS))
            .or_else(|| file_id(path, PROPERTIES))
    }

    /// The manifest, `erc.json`, blocks, rails, classes and the library's
    /// components, symbols and footprints: a design reaches blocks through
    /// instances and a footprint serves many components, so no one part
    /// owns them. A library component is a part as well (`part_of`).
    fn shared(path: &str) -> bool {
        path == MANIFEST
            || path == crate::project::ERC
            || SHARED_FOLDERS
                .iter()
                .any(|d| path.strip_prefix(d).is_some_and(|r| r.starts_with('/')))
    }

    fn content(src: &dyn Source, part: &Id) -> Result<String> {
        content::hash(src, part)
    }

    /// A library component's `name` (its `value`), `description` and
    /// `make_buy` (`buy`: every library component is bought), and a
    /// design's `name`, where `properties/` doesn't set them (design 629).
    fn defaults(src: &dyn Source, part: &Id) -> Result<std::collections::BTreeMap<String, Value>> {
        let mut out = std::collections::BTreeMap::new();
        match manifest(src)?.project_kind {
            ProjectKind::Library => {
                let c = content::libcomp(src, part)?;
                if !c.value.is_empty() {
                    out.insert("name".to_owned(), Value::from(c.value));
                }
                if !c.description.is_empty() {
                    out.insert("description".to_owned(), Value::from(c.description));
                }
                out.insert("make_buy".to_owned(), Value::from("buy"));
            }
            ProjectKind::Design => {
                if let Ok((_, d)) = content::design_of(src, part) {
                    out.insert("name".to_owned(), Value::from(d.name));
                }
            }
        }
        Ok(out)
    }

    fn identity(manifest: &Value) -> Option<Identity> {
        let m = Manifest::read(crate::canonical::to_string(manifest).as_bytes()).ok()?;
        Some(Identity {
            project: Some(m.project),
            forked_from: m.forked_from.map(|f| f.project),
            unit: m.default_unit,
        })
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use crate::project::Project;
    use crate::project::tests::{design_in, library_in, manifest};
    use crate::source::write_atomic;
    use ryupdm::product::{PartFiles, Product};
    use std::collections::BTreeMap;

    pub(crate) fn id(s: &str) -> Id {
        Id::parse(s, "part").unwrap()
    }

    /// The test project's files, as a source: the manifest, the test
    /// library as `self` and, for a design project, the breakout's
    /// connectivity.
    pub(crate) fn board(kind: ProjectKind) -> BTreeMap<String, Vec<u8>> {
        let mut f: BTreeMap<String, Vec<u8>> = BTreeMap::new();
        let m = Manifest::new(crate::project::tests::pid(1), crate::units::Unit::Mm, kind);
        f.insert(MANIFEST.into(), m.to_text().into_bytes());
        for (p, b) in crate::library::tests::files(crate::library::tests::STEP) {
            f.insert(p, b);
        }
        if kind == ProjectKind::Design {
            for (p, b) in crate::design::tests::files() {
                f.insert(p, b);
            }
        }
        f
    }

    #[test]
    fn a_design_projects_parts_are_its_designs_a_librarys_its_components() {
        let f = board(ProjectKind::Design);
        let l = board(ProjectKind::Library);
        use ryupdm::product::PartKind;
        assert_eq!(
            <crate::Eda as Product>::parts(&f).unwrap(),
            [(id("d"), PartKind::Design)]
        );
        assert_eq!(
            <crate::Eda as Product>::parts(&l).unwrap(),
            [(id("r_100k"), PartKind::Component)]
        );
        assert_eq!(
            part_kind(ProjectKind::Library).as_str(),
            kind_name(ProjectKind::Library)
        );
        assert_eq!(
            <crate::Eda as Product>::properties_path(&id("d")),
            "properties/d.json"
        );
        assert_eq!(
            <crate::Eda as Product>::part_of("designs/d.json"),
            Some(id("d"))
        );
        assert_eq!(
            <crate::Eda as Product>::part_of("library/components/r_100k.json"),
            Some(id("r_100k"))
        );
        assert_eq!(
            <crate::Eda as Product>::part_of("properties/d.json"),
            Some(id("d"))
        );
        assert_eq!(
            <crate::Eda as Product>::part_of("blocks/top/block.json"),
            None
        );
        for s in [
            "blocks/top/block.json",
            "rails/gnd.json",
            "library/footprints/f.json",
            "erc.json",
            "ryueda.json",
        ] {
            assert!(<crate::Eda as Product>::shared(s), "{s}");
        }
        for s in [
            "style.json",
            "parts.json",
            "lint.json",
            "releases/d/A.json",
            "library/models/x.step",
            "blocksx/y.json",
        ] {
            assert!(!<crate::Eda as Product>::shared(s), "{s}");
        }
        assert_eq!(
            <crate::Eda as Product>::part_files(&id("d")),
            PartFiles {
                files: vec!["designs/d.json".into(), "properties/d.json".into()],
                folders: ["blocks", "rails", "classes", "library"]
                    .map(String::from)
                    .to_vec(),
            }
        );
        let v: Value = serde_json::from_slice(&f[MANIFEST]).unwrap();
        let i = <crate::Eda as Product>::identity(&v).unwrap();
        assert_eq!(i.project, Some(crate::project::tests::pid(1)));
        assert_eq!((i.forked_from, i.unit), (None, crate::units::Unit::Mm));
    }

    #[test]
    fn a_session_holds_the_pdm_files() {
        let dir = tempfile::tempdir().unwrap();
        let board = dir.path().join("board");
        manifest(&board, ProjectKind::Design, &[]);
        library_in(&board);
        design_in(&board);
        std::fs::create_dir_all(board.join("properties")).unwrap();
        std::fs::create_dir_all(board.join("releases/d")).unwrap();
        let canonical = |v: serde_json::Value| crate::canonical::to_string(&v);
        write_atomic(
            &board.join("ryupdm.json"),
            canonical(serde_json::json!({"format": 4, "kind": "ryupdm.config"})),
        )
        .unwrap();
        write_atomic(
            &board.join("properties/d.json"),
            canonical(serde_json::json!({"format": 1, "kind": "ryupdm.properties"})),
        )
        .unwrap();
        write_atomic(&board.join("releases/d/A.json"), "{}\n").unwrap();
        let tracked: Vec<String> = Project::tracked(&board)
            .into_iter()
            .map(|(p, _)| p)
            .collect();
        for f in ["ryupdm.json", "properties/d.json", "releases/d/A.json"] {
            assert!(tracked.contains(&f.to_owned()), "{f}: {tracked:?}");
        }
        // Nothing reads them as a fault of the project, nor doctor.
        let p = Project::load(&board).unwrap();
        assert_eq!(p.faults(), []);
        let d = crate::doctor::diagnose(&board, &p);
        assert!(
            d.iter()
                .all(|i| !i.files.iter().any(|f| f.starts_with("releases")
                    || f.starts_with("properties")
                    || f == "ryupdm.json")),
            "{:?}",
            d.iter().map(|i| &i.files).collect::<Vec<_>>()
        );
    }

    #[test]
    fn components_and_designs_give_their_names_as_defaults() {
        let l = board(ProjectKind::Library);
        let d = <crate::Eda as Product>::defaults(&l, &id("r_100k")).unwrap();
        let c = content::libcomp(&l, &id("r_100k")).unwrap();
        assert_eq!(d["name"], serde_json::json!(c.value));
        assert_eq!(d.contains_key("description"), !c.description.is_empty());
        assert_eq!(d["make_buy"], "buy");
        let f = board(ProjectKind::Design);
        let d = <crate::Eda as Product>::defaults(&f, &id("d")).unwrap();
        assert_eq!(
            d,
            BTreeMap::from([("name".to_owned(), serde_json::json!("Breakout"))])
        );
        // Through ryupdm: the defaults fill what properties/ leaves out.
        let pdm = ryupdm::props::Pdm::of_for::<crate::Eda>(&f, [id("d")], crate::units::Unit::Mm);
        assert!(pdm.parts[&id("d")].from_product("name"));
    }
}
