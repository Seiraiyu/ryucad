//! `reps` (design 938): the projects a catalogue reaches, without
//! reading them. Each ryucad or ryueda project its links name, each other
//! catalogue its lines name, and the folder each was found in, or none:
//! what a tool that builds a catalogue elsewhere (the hosted service)
//! must put beside it first.

use crate::catalogue::{MANIFEST, Manifest};
use crate::error::Result;
use crate::id::ProjectId;
use crate::session::Session;
use serde_json::{Value, json};
use std::collections::BTreeMap;
use std::path::PathBuf;

/// `reps.list`: `{schema, rev, projects, catalogues, searched}`.
/// `projects` are `{product, project, items, folder}` by product then
/// project ID, `items` how many link there; `catalogues` are `{project,
/// lines, folder}` for each other catalogue an own line names. `folder`
/// is `null` when no folder searched holds it. Searched: the catalogue's
/// work tree, `within`, then the `pdm.dependents` preference.
pub fn list(session: &mut Session, rev: Option<&str>, within: &[String]) -> Result<Value> {
    let (cat, _) = crate::bom::at_rev(session, rev)?;
    let disk = crate::bom::disk(session, within);
    let own = &cat.manifest.project;
    let mut projects: BTreeMap<(String, ProjectId), usize> = BTreeMap::new();
    let mut catalogues: BTreeMap<ProjectId, usize> = BTreeMap::new();
    for item in cat.items.values() {
        for l in &item.links {
            *projects
                .entry((l.product.clone(), l.project.clone()))
                .or_default() += 1;
        }
        for line in &item.bom {
            if let Some(p) = line.item.catalogue.as_ref().filter(|p| *p != own) {
                *catalogues.entry(p.clone()).or_default() += 1;
            }
        }
    }
    // Other catalogues found, by project ID; the first folder wins.
    let folders = crate::check::elsewhere(within);
    let mut found: BTreeMap<ProjectId, PathBuf> = BTreeMap::new();
    for dir in crate::numbers::others(session.root(), &folders) {
        if let Ok(bytes) = std::fs::read(dir.join(MANIFEST))
            && let Ok(m) = Manifest::read(&bytes)
        {
            found.entry(m.project).or_insert(dir);
        }
    }
    let shown = |p: Option<&std::path::Path>| p.map(|p| p.display().to_string());
    Ok(json!({
        "schema": crate::OUTPUT_SCHEMA,
        "rev": rev.unwrap_or("working"),
        "projects": projects.iter().map(|((product, project), n)| json!({
            "product": product,
            "project": project,
            "items": n,
            "folder": shown(disk.index.dir(product, project)),
        })).collect::<Vec<_>>(),
        "catalogues": catalogues.iter().map(|(project, n)| json!({
            "project": project,
            "lines": n,
            "folder": shown(found.get(project).map(PathBuf::as_path)),
        })).collect::<Vec<_>>(),
        "searched": disk.index.searched.iter().map(|p| p.display().to_string()).collect::<Vec<_>>(),
    }))
}

#[cfg(test)]
mod tests {
    use crate::proto::Request;
    use crate::server::tests::{ask, catalogue};
    use serde_json::json;

    #[test]
    fn a_catalogue_names_the_projects_it_reaches() {
        let (_d, root) = catalogue();
        let mk = |n: &str| {
            ask(
                &root,
                Request::ItemCreate {
                    fields: Default::default(),
                    number: Some(n.into()),
                    next: None,
                    expected_revision: None,
                },
            )
            .unwrap()["item"]["id"]
                .as_str()
                .unwrap()
                .to_owned()
        };
        let (a, b) = (mk("A-1"), mk("B-1"));
        // A ryucad project beside the items, found by its manifest.
        let cad = "33333333333333333333333333333333";
        std::fs::create_dir_all(root.join("mech/parts/box")).unwrap();
        std::fs::write(
            root.join("mech/ryucad.json"),
            crate::canonical::to_string(
                &json!({"default_unit": "mm", "format": 2, "kind": "ryucad.project", "project": cad}),
            ),
        )
        .unwrap();
        std::fs::write(
            root.join("mech/parts/box/part.json"),
            crate::canonical::to_string(
                &json!({"features": [], "format": 1, "kind": "ryucad.part"}),
            ),
        )
        .unwrap();
        let link = |item: &str, product: &str, project: &str, part: &str| {
            ask(
                &root,
                Request::ItemLink {
                    item: item.into(),
                    product: product.into(),
                    project: project.into(),
                    part: part.into(),
                    expected_revision: None,
                },
            )
            .unwrap();
        };
        link(&a, "ryucad", cad, "box");
        // A ryueda project nobody has here.
        let eda = "44444444444444444444444444444444";
        link(&b, "ryueda", eda, "main");
        // And a line naming another catalogue's item.
        let other = "55555555555555555555555555555555";
        let mut item = crate::item::Item::read(
            &crate::id::Id::parse(&a, "item").unwrap(),
            &std::fs::read(root.join(crate::ids::item_path(
                &crate::id::Id::parse(&a, "item").unwrap(),
            )))
            .unwrap(),
        )
        .unwrap();
        item.bom.push(crate::bom::tests::line(
            1,
            &format!("{other}:0000000000009"),
            "1",
        ));
        std::fs::write(
            root.join(crate::ids::item_path(
                &crate::id::Id::parse(&a, "item").unwrap(),
            )),
            item.to_text(),
        )
        .unwrap();
        let v = ask(
            &root,
            Request::RepsList {
                rev: None,
                within: vec![],
            },
        )
        .unwrap();
        assert_eq!(v["schema"], 1);
        assert_eq!(v["rev"], "working");
        let p = v["projects"].as_array().unwrap();
        assert_eq!(p.len(), 2);
        assert_eq!(
            (
                p[0]["product"].clone(),
                p[0]["project"].clone(),
                p[0]["items"].clone()
            ),
            (json!("ryucad"), json!(cad), json!(1))
        );
        assert!(p[0]["folder"].as_str().unwrap().ends_with("mech"), "{v}");
        assert_eq!(
            (p[1]["product"].clone(), p[1]["project"].clone()),
            (json!("ryueda"), json!(eda))
        );
        assert!(p[1]["folder"].is_null());
        assert_eq!(
            v["catalogues"],
            json!([{"project": other, "lines": 1, "folder": null}])
        );
        assert!(!v["searched"].as_array().unwrap().is_empty());
    }
}
