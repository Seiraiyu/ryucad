//! Other projects' parts: what a foreign instance places, read from the
//! machine's dependency cache at the commit the lock names.

use crate::assembly::Pinned;
use crate::doc::id::{Id, ProjectId};
use crate::error::{Code, Error, Result};
use crate::session::Session;

/// `part` of the other project `project` as its release `label` has it.
/// A project neither the manifest nor the lock names is
/// `dependency_missing`; otherwise its files must be fetched
/// (`dependency_not_fetched`).
pub(crate) fn pinned(
    session: &mut Session,
    project: &ProjectId,
    part: &Id,
    label: &str,
) -> Result<Pinned> {
    let p = session.project();
    let known = p.manifest.dependencies.contains_key(project)
        || p.lock
            .as_ref()
            .and_then(|l| l.as_ref().ok())
            .is_some_and(|l| l.projects.contains_key(project));
    if !known {
        return Err(Error::new(
            Code::DependencyMissing,
            format!("{part}@{label} is of project {project}, which isn't a dependency"),
        )
        .with("project", project)
        .with("part", part)
        .with("label", label));
    }
    Err(Error::new(
        Code::DependencyNotFetched,
        format!("{part}@{label} of {project} isn't fetched: run `ryucad deps fetch`"),
    )
    .with("project", project)
    .with("part", part)
    .with("label", label))
}

#[cfg(test)]
pub(crate) mod tests {
    use crate::assembly::tests::{id, project};
    use crate::doc::files::{Manifest, PartFile};
    use crate::doc::id::ProjectId;
    use crate::doc::project::{MANIFEST, feature_path, part_path, write_json};
    use crate::error::Code;
    use serde_json::json;
    use std::path::Path;

    /// Writes instance `inst` of `project`'s `part` at `label` into
    /// assembly `asm`, listed last, as a file would hold it.
    pub(crate) fn place_foreign(
        root: &Path,
        asm: &str,
        inst: &str,
        project: &ProjectId,
        part: &str,
        label: &str,
    ) {
        let v = json!({"format": 6, "kind": "ryucad.feature", "type": "instance",
                       "project": project, "part": part, "rev": {"label": label},
                       "pose": {"origin_m": [0, 0, 0], "z": [0, 0, 1], "x": [1, 0, 0]}});
        write_json(&feature_path(root, &id(asm), &id(inst)), &v).unwrap();
        let p = part_path(root, &id(asm));
        let text = std::fs::read_to_string(&p).unwrap();
        let mut list = PartFile::from_json(&crate::doc::canonical::parse(&text).unwrap()).unwrap();
        list.features.push(id(inst));
        write_json(&p, &list.to_json()).unwrap();
    }

    #[test]
    fn a_foreign_instance_needs_a_dependency_then_its_files() {
        let (dir, mut s) = project();
        let lib = ProjectId::from_bytes([7; 16]);
        place_foreign(dir.path(), "frame", "b", &lib, "m6-bolt", "B");
        s.refresh().unwrap();
        let r = s.rebuilt(&id("frame")).unwrap().clone();
        // An instance fails through what it places, as for a local label.
        let e = r.features[0].1.as_ref().unwrap_err();
        assert_eq!(
            (e.code, &e.details["cause"]),
            (Code::DependencyFailed, &json!("dependency_missing"))
        );
        // Listed as a dependency, it must be fetched.
        let text = std::fs::read_to_string(dir.path().join(MANIFEST)).unwrap();
        let mut m = Manifest::from_json(&crate::doc::canonical::parse(&text).unwrap()).unwrap();
        m.dependencies.insert(
            lib.clone(),
            crate::doc::deps::Dependency {
                name: "fasteners".into(),
                url: "file:///nowhere".into(),
                path: "".into(),
            },
        );
        write_json(&dir.path().join(MANIFEST), &m.to_json()).unwrap();
        s.refresh().unwrap();
        let r = s.rebuilt(&id("frame")).unwrap().clone();
        let e = r.features[0].1.as_ref().unwrap_err();
        assert_eq!(e.details["cause"], "dependency_not_fetched");
    }
}
