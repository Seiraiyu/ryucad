//! ryucad's product: its projects as ryupdm reads them. A part is a
//! folder `parts/<part>/` (`part.json`, its features, parameters,
//! drawings and `properties.json`); its content hash is
//! [`crate::content::hash`]; the manifest is `ryucad.json`.

use crate::product::{Identity, PartFiles, Product};
use ryu_doc::error::Result;
use ryu_doc::id::Id;
use ryu_doc::source::{Kind, Source, join};
use ryucad_doc::doc::files::Manifest;
use ryucad_doc::doc::project;
use serde_json::Value;

/// ryucad, as ryupdm reads its projects.
pub struct Cad;

impl ryu_git::Layout for Cad {
    const NAME: &'static str = "ryucad";
    const MANIFEST: &'static str = project::MANIFEST;
    const ROOT_FILES: &'static [&'static str] = &project::ROOT_FILES;
    const FOLDERS: &'static [&'static str] = &project::FOLDERS;
    fn project_file(rel: &str) -> bool {
        project::project_file(rel)
    }
}

impl Product for Cad {
    const PART_FOLDERS: &'static [&'static str] = &["parts"];

    fn parts(src: &dyn Source) -> Result<Vec<Id>> {
        Ok(src
            .list("parts")?
            .into_iter()
            .filter(|(_, k)| *k == Kind::Dir)
            .filter_map(|(n, _)| Id::parse(&n, "part").ok())
            .collect())
    }

    fn properties_path(part: &Id) -> String {
        join(&join("parts", part.as_str()), project::PROPERTIES)
    }

    fn part_files(part: &Id) -> PartFiles {
        PartFiles {
            files: Vec::new(),
            folders: vec![join("parts", part.as_str())],
        }
    }

    fn part_of(path: &str) -> Option<Id> {
        let mut seg = path.split('/');
        match (seg.next(), seg.next(), seg.next()) {
            (Some("parts"), Some(part), Some(_)) => Id::parse(part, "part").ok(),
            _ => None,
        }
    }

    fn content(src: &dyn Source, part: &Id) -> Result<String> {
        crate::content::hash(src, part)
    }

    fn identity(manifest: &Value) -> Option<Identity> {
        let m = Manifest::from_json(manifest).ok()?;
        Some(Identity {
            project: m.project,
            forked_from: m.forked_from.map(|f| f.project),
            unit: m.default_unit,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use ryu_doc::canonical;
    use ryu_git::Layout;
    use serde_json::json;
    use std::collections::BTreeMap;

    fn id(s: &str) -> Id {
        Id::parse(s, "part").unwrap()
    }

    #[test]
    fn cads_layout_is_ryucads() {
        assert_eq!(Cad::NAME, "ryucad");
        assert_eq!(Cad::MANIFEST, "ryucad.json");
        assert_eq!(Cad::ROOT_FILES, project::ROOT_FILES);
        assert_eq!(Cad::FOLDERS, project::FOLDERS);
        assert!(Cad::ROOT_FILES.contains(&crate::PDM_CONFIG));
        assert!(Cad::project_file("parts/p/part.json"));
    }

    #[test]
    fn cads_parts_are_folders_under_parts() {
        let mut f: BTreeMap<String, Vec<u8>> = BTreeMap::new();
        for p in ["b", "a", "Not An ID"] {
            f.insert(format!("parts/{p}/part.json"), b"{}".to_vec());
        }
        f.insert("parts/loose.json".into(), b"{}".to_vec());
        assert_eq!(Cad::parts(&f).unwrap(), [id("a"), id("b")]);
        assert_eq!(Cad::properties_path(&id("p")), "parts/p/properties.json");
        assert_eq!(
            Cad::part_files(&id("p")),
            PartFiles {
                files: vec![],
                folders: vec!["parts/p".into()]
            }
        );
        assert_eq!(Cad::part_of("parts/p/features/a.json"), Some(id("p")));
        assert_eq!(Cad::part_of("parts/p"), None);
        assert_eq!(Cad::part_of("releases/p/A.json"), None);
    }

    #[test]
    fn cads_identity_is_its_manifests() {
        let p = "0123456789abcdef0123456789abcdef";
        let v = json!({"default_unit": "in", "format": 2, "kind": "ryucad.project", "project": p});
        let i = Cad::identity(&v).unwrap();
        assert_eq!(i.project.clone().unwrap().as_str(), p);
        assert_eq!(
            (i.forked_from.clone(), i.unit),
            (None, ryu_doc::units::Unit::In)
        );
        let mut f: BTreeMap<String, Vec<u8>> = BTreeMap::new();
        f.insert("ryucad.json".into(), canonical::to_string(&v).into_bytes());
        assert_eq!(crate::product::identity::<Cad>(&f), Some(i));
        assert_eq!(Cad::identity(&json!({"kind": "ryueda.project"})), None);
    }
}
