//! Representations (design 734's Links): the ryucad and ryueda projects
//! items link to, found on disk by their manifests, and (Tasks 9 and 10)
//! the lines their BOMs give. ryuplm reads them through their files,
//! never their crates: ryucad's through ryupdm's structure reader
//! (feature `cad`), ryueda's through the `ryueda` executable.

use crate::error::{Code, Error, Result};
use crate::id::{Id, ProjectId};
use crate::source::Source;
use serde_json::Value;
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

/// The products an item may link to, and their manifests.
pub const PRODUCTS: [(&str, &str); 2] = [("ryucad", "ryucad.json"), ("ryueda", "ryueda.json")];

/// The manifest of `product`, `None` for a product links can't name.
pub fn manifest_of(product: &str) -> Option<&'static str> {
    PRODUCTS
        .iter()
        .find(|(p, _)| *p == product)
        .map(|(_, m)| *m)
}

/// The project ID the manifest `manifest` in `src` gives (its `project`),
/// `None` when it has none or doesn't read.
pub fn project_id(src: &dyn Source, manifest: &str) -> Option<ProjectId> {
    let bytes = src.read(manifest).ok()??;
    let v: Value = serde_json::from_slice(&bytes).ok()?;
    ProjectId::parse(v.get("project")?.as_str()?).ok()
}

/// Whether `part` is a part of the product's project `src` holds: a
/// ryucad part or assembly (`parts/<id>/part.json`), a ryueda design
/// (`designs/<id>.json`) or library component
/// (`library/components/<id>.json`).
pub fn part_exists(src: &dyn Source, product: &str, part: &Id) -> bool {
    let paths = match product {
        "ryucad" => vec![format!("parts/{part}/part.json")],
        "ryueda" => vec![
            format!("designs/{part}.json"),
            format!("library/components/{part}.json"),
        ],
        _ => Vec::new(),
    };
    paths.iter().any(|p| matches!(src.read(p), Ok(Some(_))))
}

/// The `number` property the representation's properties file gives
/// (ryupdm's: `parts/<id>/properties.json` in ryucad,
/// `properties/<id>.json` in ryueda), `None` when it gives none.
pub fn number_of(src: &dyn Source, product: &str, part: &Id) -> Option<String> {
    let path = match product {
        "ryucad" => format!("parts/{part}/properties.json"),
        "ryueda" => format!("properties/{part}.json"),
        _ => return None,
    };
    let bytes = src.read(&path).ok()??;
    let v: Value = serde_json::from_slice(&bytes).ok()?;
    let props = ryupdm::props::Properties::from_json_in(&v, product).ok()?;
    props.values.get("number")?.as_str().map(str::to_owned)
}

/// The representation projects found under some folders, by product and
/// project ID.
#[derive(Clone, Debug, Default)]
pub struct Index {
    found: BTreeMap<(String, ProjectId), PathBuf>,
    /// The folders scanned, in order.
    pub searched: Vec<PathBuf>,
}

impl Index {
    /// Every ryucad and ryueda project at or under `tops`, in order; a
    /// project ID found twice (two checkouts) keeps its first folder.
    pub fn scan(tops: &[PathBuf]) -> Index {
        let mut ix = Index::default();
        for t in tops {
            ix.searched.push(t.clone());
            for (product, manifest) in PRODUCTS {
                for dir in ryupdm::dependents::projects_under(t, manifest).unwrap_or_default() {
                    let dir = dir.canonicalize().unwrap_or(dir);
                    if let Some(pid) = project_id(&crate::source::Folder(&dir), manifest) {
                        ix.found.entry((product.to_owned(), pid)).or_insert(dir);
                    }
                }
            }
        }
        ix
    }

    /// Records `dir` as `product`'s project `project` (tests).
    pub fn insert(&mut self, product: &str, project: ProjectId, dir: PathBuf) {
        self.found.insert((product.to_owned(), project), dir);
    }

    /// The folder holding `product`'s project `project`.
    pub fn dir(&self, product: &str, project: &ProjectId) -> Option<&Path> {
        self.found
            .get(&(product.to_owned(), project.clone()))
            .map(PathBuf::as_path)
    }
}

/// Where representations are looked for: the catalogue's Git work tree
/// (its own folder outside one), then `folders`.
pub fn tops(here: &Path, folders: &[PathBuf]) -> Vec<PathBuf> {
    let mut out = Vec::new();
    match gix::discover(here).ok().and_then(|r| r.workdir().map(Path::to_path_buf)) {
        Some(w) => out.push(w.canonicalize().unwrap_or(w)),
        None => out.push(here.to_path_buf()),
    }
    for f in folders {
        if !out.contains(f) {
            out.push(f.clone());
        }
    }
    out
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use std::fs;

    pub(crate) fn pid(n: u8) -> ProjectId {
        ProjectId::from_bytes([n; 16])
    }

    /// A ryucad project at `dir` with ID `project`: `parts` as plain
    /// boxes, `assemblies` placing `(part, count)` instances.
    pub(crate) fn cad_project(
        dir: &Path,
        project: &ProjectId,
        parts: &[&str],
        assemblies: &[(&str, &[(&str, usize)])],
    ) {
        let put = |rel: &str, v: serde_json::Value| {
            let p = dir.join(rel);
            fs::create_dir_all(p.parent().unwrap()).unwrap();
            fs::write(p, crate::canonical::to_string(&v)).unwrap();
        };
        put(
            "ryucad.json",
            serde_json::json!({"default_unit": "mm", "format": 2, "kind": "ryucad.project", "project": project}),
        );
        for part in parts {
            put(
                &format!("parts/{part}/part.json"),
                serde_json::json!({"features": ["b"], "format": 1, "kind": "ryucad.part"}),
            );
            put(
                &format!("parts/{part}/features/b.json"),
                serde_json::json!({"format": 1, "kind": "ryucad.feature", "type": "box", "x": "1", "y": "1", "z": "1"}),
            );
        }
        for (asm, children) in assemblies {
            let mut ids = Vec::new();
            for (child, count) in *children {
                for k in 0..*count {
                    let f = format!("{child}{k}");
                    put(
                        &format!("parts/{asm}/features/{f}.json"),
                        serde_json::json!({"format": 4, "kind": "ryucad.feature", "type": "instance",
                            "part": child, "rev": "working",
                            "pose": {"origin_m": [0, 0, 0], "z": [0, 0, 1], "x": [1, 0, 0]}}),
                    );
                    ids.push(f);
                }
            }
            put(
                &format!("parts/{asm}/part.json"),
                serde_json::json!({"features": ids, "format": 2, "kind": "ryucad.part", "part_kind": "assembly"}),
            );
        }
    }

    #[test]
    fn projects_are_found_by_manifest_and_id() {
        let d = tempfile::tempdir().unwrap();
        let top = d.path().canonicalize().unwrap();
        cad_project(&top.join("mech"), &pid(3), &["bolt"], &[]);
        cad_project(&top.join("copy"), &pid(3), &["bolt"], &[]);
        fs::create_dir_all(top.join("eda")).unwrap();
        fs::write(
            top.join("eda/ryueda.json"),
            format!(r#"{{"kind":"ryueda.project","project":"{}"}}"#, pid(4)),
        )
        .unwrap();
        fs::create_dir_all(top.join("eda/designs")).unwrap();
        fs::write(top.join("eda/designs/main.json"), "{}").unwrap();
        fs::create_dir_all(top.join("eda/properties")).unwrap();
        fs::write(
            top.join("eda/properties/main.json"),
            r#"{"format":1,"kind":"ryupdm.properties","number":"PCB-1"}"#,
        )
        .unwrap();
        let ix = Index::scan(&[top.clone()]);
        // The first folder in sorted order wins: copy before mech.
        assert_eq!(ix.dir("ryucad", &pid(3)), Some(top.join("copy").as_path()));
        assert_eq!(ix.dir("ryueda", &pid(4)), Some(top.join("eda").as_path()));
        assert_eq!(ix.dir("ryueda", &pid(3)), None);
        assert_eq!(ix.searched, [top.clone()]);
        let id = |s: &str| Id::parse(s, "part").unwrap();
        let mech = crate::source::Folder(&top.join("mech"));
        assert!(part_exists(&mech, "ryucad", &id("bolt")));
        assert!(!part_exists(&mech, "ryucad", &id("nut")));
        let eda = crate::source::Folder(&top.join("eda"));
        assert!(part_exists(&eda, "ryueda", &id("main")));
        assert_eq!(number_of(&eda, "ryueda", &id("main")).as_deref(), Some("PCB-1"));
        assert_eq!(number_of(&mech, "ryucad", &id("bolt")), None);
        assert_eq!(manifest_of("ryudfm"), None);
    }
}
