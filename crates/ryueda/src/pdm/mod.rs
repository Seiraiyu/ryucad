//! Product data management for ryueda projects (design 629): a design
//! project's designs and a library project's components are ryupdm
//! parts, with properties in `properties/<part>.json`, settings in
//! `ryupdm.json` and release records under `releases/`, as ryupdm reads
//! them for every product. Not to be confused with [`crate::props`], the
//! named text values designator formats read (design 236).

pub use ryupdm::{PDM_CONFIG, RELEASES};

/// The folder of parts' PDM properties, beside the manifest.
pub const PROPERTIES: &str = "properties";

#[cfg(test)]
pub(crate) mod tests {
    use crate::project::tests::{design_in, library_in, manifest};
    use crate::project::{Project, ProjectKind};
    use crate::source::write_atomic;

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
}
