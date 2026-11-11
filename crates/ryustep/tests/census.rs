//! The census over the committed files (design 448): every instance the
//! decoder types has a category, `open` keeps it, and the NIST files
//! hold the metadata 24.B to 24.D will read.

use ryustep::census::Category;
use ryustep::schema::{Entities, Entity};
use std::collections::BTreeSet;
use std::path::PathBuf;

fn files() -> Vec<PathBuf> {
    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests");
    let mut out = Vec::new();
    for dir in ["corpus", "data/occt"] {
        for e in std::fs::read_dir(root.join(dir)).unwrap() {
            let p = e.unwrap().path();
            if p.extension().is_some_and(|x| x == "stp" || x == "step") {
                out.push(p);
            }
        }
    }
    out.sort();
    out
}

#[test]
fn every_typed_instance_of_the_committed_files_has_a_category() {
    for path in files() {
        let bytes = std::fs::read(&path).unwrap();
        let file = ryustep::part21::parse(&bytes).unwrap();
        let (e, census) = Entities::decode_counted(&file);
        assert_eq!(census.len(), e.len());
        for &id in e.ids() {
            if let Ok(x) = e.get(id, id)
                && !matches!(x, Entity::Other(_))
            {
                assert_ne!(
                    census.category_at(id),
                    Some(Category::Unknown),
                    "{}: #{id} {}",
                    path.display(),
                    x.kind()
                );
            }
        }
    }
}

#[test]
fn the_nist_files_hold_styles_layers_properties_and_management() {
    let mut seen = BTreeSet::new();
    for path in files()
        .iter()
        .filter(|p| p.to_string_lossy().contains("nist_"))
    {
        let opened = ryustep::open(&std::fs::read(path).unwrap()).unwrap();
        seen.extend(opened.census.counts().into_iter().map(|(c, _, _)| c));
    }
    for c in [
        Category::Geometry,
        Category::Topology,
        Category::ProductStructure,
        Category::Units,
        Category::Appearance,
        Category::Layers,
        Category::Management,
    ] {
        assert!(seen.contains(&c), "{c:?} in {seen:?}");
    }
    assert!(
        seen.contains(&Category::Properties) || seen.contains(&Category::Validation),
        "{seen:?}"
    );
}
