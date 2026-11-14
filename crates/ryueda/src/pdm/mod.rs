//! Product data management for ryueda projects (design 629): a design
//! project's designs and a library project's components are ryupdm
//! parts, with properties in `properties/<part>.json`, settings in
//! `ryupdm.json` and release records under `releases/`, as ryupdm reads
//! them for every product. Not to be confused with [`crate::props`], the
//! named text values designator formats read (design 236).

pub use ryupdm::{PDM_CONFIG, RELEASES};

/// The folder of parts' PDM properties, beside the manifest.
pub const PROPERTIES: &str = "properties";

pub mod content;

use crate::conn::{BLOCKS, CLASSES, DESIGNS, RAILS};
use crate::error::{Code, Error, Result};
use crate::id::Id;
use crate::project::{MANIFEST, Manifest, ProjectKind};
use crate::source::Source;
use ryupdm::product::{Identity, PartFiles, Product};
use serde_json::Value;

/// A library's components, symbols and footprints, below the project.
const LIB_COMPONENTS: &str = "library/components";
const LIB_SYMBOLS: &str = "library/symbols";
const LIB_FOOTPRINTS: &str = "library/footprints";

/// The folders a part's revision reads besides the root files: what
/// the content hashes read (never `library/models/`, whose bytes a
/// footprint names by BLAKE3), and the properties.
const PART_FOLDERS: [&str; 8] = [
    DESIGNS,
    BLOCKS,
    RAILS,
    CLASSES,
    LIB_COMPONENTS,
    LIB_SYMBOLS,
    LIB_FOOTPRINTS,
    PROPERTIES,
];

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

/// What a part of this project is: `design` or `component`.
pub fn kind_name(kind: ProjectKind) -> &'static str {
    match kind {
        ProjectKind::Design => "design",
        ProjectKind::Library => "component",
    }
}

/// The ID a part's file (`<folder>/<id>.json`) names, if `path` is one.
fn file_id(path: &str, folder: &str) -> Option<Id> {
    let name = path.strip_prefix(folder)?.strip_prefix('/')?;
    Id::parse(name.strip_suffix(".json")?, "part").ok()
}

impl Product for crate::Eda {
    const PART_FOLDERS: &'static [&'static str] = &PART_FOLDERS;

    /// A design project's designs, a library project's components.
    fn parts(src: &dyn Source) -> Result<Vec<Id>> {
        let folder = match manifest(src)?.project_kind {
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
        Ok(out)
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
            folders: SHARED_FOLDERS.map(String::from).to_vec(),
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
        assert_eq!(<crate::Eda as Product>::parts(&f).unwrap(), [id("d")]);
        let l = board(ProjectKind::Library);
        assert_eq!(<crate::Eda as Product>::parts(&l).unwrap(), [id("r_100k")]);
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
                folders: [
                    "blocks",
                    "rails",
                    "classes",
                    "library/components",
                    "library/symbols",
                    "library/footprints"
                ]
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
        assert_eq!(d.get("description").is_some(), !c.description.is_empty());
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
