//! The content hash: what a release record says was released. It covers
//! exactly the bytes that define a part's model, so a part whose files
//! are unchanged hashes alike on every platform and from any checkout
//! path, and nothing else (the project's ID, the part's properties,
//! ryupdm's settings) makes a released part look modified.
//!
//! The hash is the full BLAKE3 hash, in hex, of these pieces in order,
//! each its length as a little-endian `u64` and then its bytes:
//!
//! 1. the tag `ryupdm.content.1`;
//! 2. the project's `default_unit` as the manifest writes it;
//! 3. the bytes of `part.json`;
//! 4. for each listed feature in order, its ID and then its file's bytes;
//! 5. if the part has `parameters.json`, the tag `parameters.json` and then
//!    its bytes;
//! 6. if the part reads any of the project's parameters (the root
//!    `parameters.json`), the tag `ryucad.project_parameters` and then,
//!    for each one it reads in name order, its name and its expression's
//!    canonical text. It reads those its listed features and its own
//!    parameters name and doesn't have itself, and what those read in
//!    turn ([`ryucad_doc::doc::project::project_reads`]), so a part
//!    reading none hashes as it always has.

use ryucad_doc::doc::files::{ParametersFile, PartFile, feature_names};
use ryucad_doc::doc::id::Id;
use ryucad_doc::doc::project::{MANIFEST, PARAMETERS, PROJECT_PARAMETERS_TAG, project_reads};
use ryucad_doc::doc::source::{Source, join};
use ryucad_doc::error::{Code, Error, Result};
use std::collections::BTreeSet;

/// The first piece of every content hash: a new definition gets a new tag.
pub const TAG: &str = "ryupdm.content.1";

/// The content hash of `part` in the project `src` holds. A part whose
/// files can't be read (missing, not JSON, or with merge conflicts) has
/// none: the error says which file.
pub fn hash(src: &dyn Source, part: &Id) -> Result<String> {
    let manifest = crate::read_json(src, MANIFEST)?.ok_or_else(|| missing(MANIFEST))?;
    let unit = manifest["default_unit"].as_str().ok_or_else(|| {
        Error::new(
            Code::InvalidDocument,
            format!("{MANIFEST} has no default_unit"),
        )
        .with("path", MANIFEST)
        .with("pointer", "/default_unit")
    })?;
    let dir = join("parts", part.as_str());
    let part_at = join(&dir, "part.json");
    let bytes = |at: &str| -> Result<Vec<u8>> {
        src.read(at)
            .map_err(|e| e.with("path", at))?
            .ok_or_else(|| missing(at))
    };
    let part_bytes = bytes(&part_at)?;
    let list = crate::read_json(src, &part_at)?.ok_or_else(|| missing(&part_at))?;
    let list = PartFile::from_json(&list).map_err(|e| e.with("path", &part_at))?;
    let mut h = blake3::Hasher::new();
    let mut piece = |b: &[u8]| {
        h.update(&(b.len() as u64).to_le_bytes());
        h.update(b);
    };
    piece(TAG.as_bytes());
    piece(unit.as_bytes());
    piece(&part_bytes);
    // What the part's files name, for the project parameters it reads.
    let mut names = BTreeSet::new();
    for f in &list.features {
        let at = join(&join(&dir, "features"), &format!("{f}.json"));
        piece(f.as_str().as_bytes());
        let b = bytes(&at)?;
        if let Ok(Some(v)) = crate::read_json(src, &at) {
            names.extend(feature_names(&v));
        }
        piece(&b);
    }
    let params_at = join(&dir, PARAMETERS);
    let mut own = BTreeSet::new();
    if let Some(p) = src
        .read(&params_at)
        .map_err(|e| e.with("path", &params_at))?
    {
        piece(PARAMETERS.as_bytes());
        piece(&p);
        if let Ok(Some(v)) = crate::read_json(src, &params_at)
            && let Ok(f) = ParametersFile::from_json(&v)
        {
            names.extend(f.parameters.values().flat_map(|e| e.names()));
            own.extend(f.parameters.into_keys());
        }
    }
    let foreign = names.iter().any(|n| !own.contains(n));
    if foreign
        && src
            .read(PARAMETERS)
            .map_err(|e| e.with("path", PARAMETERS))?
            .is_some()
    {
        // The project's parameters must be readable to say what it reads.
        let v = crate::read_json(src, PARAMETERS)?.ok_or_else(|| missing(PARAMETERS))?;
        let project = ParametersFile::from_json(&v).map_err(|e| e.with("path", PARAMETERS))?;
        let reads = project_reads(&project, |n| own.contains(n), &names);
        if !reads.is_empty() {
            piece(PROJECT_PARAMETERS_TAG.as_bytes());
            for n in &reads {
                piece(n.as_bytes());
                piece(project.parameters[n].text().as_bytes());
            }
        }
    }
    Ok(h.finalize().to_hex().to_string())
}

fn missing(at: &str) -> Error {
    Error::new(Code::NotFound, format!("{at} is missing")).with("path", at)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeMap;

    fn id(s: &str) -> Id {
        Id::parse(s, "part").unwrap()
    }

    /// A project with part `p` listing features `a` and `b`, and an
    /// unlisted `c`.
    fn files() -> BTreeMap<String, Vec<u8>> {
        [
            (
                "ryucad.json",
                "{\n  \"default_unit\": \"mm\",\n  \"format\": 1,\n  \"kind\": \"ryucad.project\"\n}\n",
            ),
            (
                "parts/p/part.json",
                "{\n  \"features\": [\n    \"a\",\n    \"b\"\n  ],\n  \"format\": 1,\n  \"kind\": \"ryucad.part\"\n}\n",
            ),
            ("parts/p/features/a.json", "a\n"),
            ("parts/p/features/b.json", "b\n"),
            ("parts/p/features/c.json", "c\n"),
        ]
        .into_iter()
        .map(|(k, v)| (k.to_owned(), v.as_bytes().to_vec()))
        .collect()
    }

    /// The hash written out piece by piece, as the definition says.
    fn by_hand(unit: &str, pieces: &[&[u8]]) -> String {
        let mut h = blake3::Hasher::new();
        for p in [TAG.as_bytes(), unit.as_bytes()].iter().chain(pieces) {
            h.update(&(p.len() as u64).to_le_bytes());
            h.update(p);
        }
        h.finalize().to_hex().to_string()
    }

    #[test]
    fn the_hash_is_its_definition() {
        let f = files();
        let part = &f["parts/p/part.json"];
        let want = by_hand("mm", &[part, b"a", b"a\n", b"b", b"b\n"]);
        assert_eq!(hash(&f, &id("p")).unwrap(), want);
        assert_eq!(want.len(), 64);
        let mut with = f.clone();
        with.insert("parts/p/parameters.json".into(), b"x\n".to_vec());
        assert_eq!(
            hash(&with, &id("p")).unwrap(),
            by_hand(
                "mm",
                &[part, b"a", b"a\n", b"b", b"b\n", b"parameters.json", b"x\n"]
            )
        );
    }

    #[test]
    fn the_project_parameters_a_part_reads_count() {
        let mut f = files();
        let part = f["parts/p/part.json"].clone();
        f.insert(
            "parts/p/features/a.json".into(),
            b"{\"params\": {\"radius\": \"r * 2\"}}\n".to_vec(),
        );
        f.insert(
            "parts/p/parameters.json".into(),
            b"{\"format\": 1, \"kind\": \"ryucad.parameters\", \"parameters\": {\"r\": \"gap\"}}\n"
                .to_vec(),
        );
        // No project parameters: as before.
        let a = f["parts/p/features/a.json"].clone();
        let own = f["parts/p/parameters.json"].clone();
        let base: Vec<&[u8]> = vec![&part, b"a", &a, b"b", b"b\n", b"parameters.json", &own];
        assert_eq!(hash(&f, &id("p")).unwrap(), by_hand("mm", &base));
        // Some, not read: as before.
        let project = |v: &str| {
            format!("{{\"format\": 1, \"kind\": \"ryucad.parameters\", \"parameters\": {v}}}\n")
                .into_bytes()
        };
        f.insert(
            "parameters.json".into(),
            project("{\"wall\": \"3 mm\", \"other\": \"1 mm\"}"),
        );
        assert_eq!(hash(&f, &id("p")).unwrap(), by_hand("mm", &base));
        // Read through the part's own parameter, and what it reads in turn.
        f.insert(
            "parameters.json".into(),
            project("{\"wall\": \"3 mm\", \"gap\": \"wall/2\", \"other\": \"1 mm\"}"),
        );
        let mut with = base.clone();
        with.extend([
            PROJECT_PARAMETERS_TAG.as_bytes(),
            b"gap",
            b"wall / 2",
            b"wall",
            b"3 mm",
        ]);
        let read = hash(&f, &id("p")).unwrap();
        assert_eq!(read, by_hand("mm", &with));
        // Another parameter changing leaves it; one it reads moves it.
        f.insert(
            "parameters.json".into(),
            project("{\"wall\": \"3 mm\", \"gap\": \"wall/2\", \"other\": \"9 mm\"}"),
        );
        assert_eq!(hash(&f, &id("p")).unwrap(), read);
        f.insert(
            "parameters.json".into(),
            project("{\"wall\": \"4 mm\", \"gap\": \"wall/2\"}"),
        );
        assert_ne!(hash(&f, &id("p")).unwrap(), read);
        // One that can't be read gives no hash, naming it.
        f.insert("parameters.json".into(), b"{ nope".to_vec());
        let e = hash(&f, &id("p")).unwrap_err();
        assert_eq!(e.details["path"], "parameters.json");
    }

    #[test]
    fn only_what_defines_the_model_counts() {
        let f = files();
        let h = hash(&f, &id("p")).unwrap();
        let mut other = f.clone();
        // The project's ID, an unlisted feature, the part's properties and
        // ryupdm's settings don't count.
        other.insert(
            "ryucad.json".into(),
            b"{\n  \"default_unit\": \"mm\",\n  \"format\": 2,\n  \"kind\": \"ryucad.project\",\n  \"project\": \"0123456789abcdef0123456789abcdef\"\n}\n".to_vec(),
        );
        other.insert("parts/p/features/c.json".into(), b"changed\n".to_vec());
        other.insert("parts/p/properties.json".into(), b"{}".to_vec());
        other.insert("ryupdm.json".into(), b"{}".to_vec());
        assert_eq!(hash(&other, &id("p")).unwrap(), h);
        // A listed feature's bytes, and the default unit, do.
        let mut edited = f.clone();
        edited.insert("parts/p/features/b.json".into(), b"B\n".to_vec());
        assert_ne!(hash(&edited, &id("p")).unwrap(), h);
        let mut inch = f.clone();
        inch.insert(
            "ryucad.json".into(),
            b"{\n  \"default_unit\": \"in\",\n  \"format\": 1,\n  \"kind\": \"ryucad.project\"\n}\n"
                .to_vec(),
        );
        assert_ne!(hash(&inch, &id("p")).unwrap(), h);
    }

    #[test]
    fn the_same_files_hash_alike_from_any_folder() {
        use ryucad_doc::doc::source::Folder;
        let (one, two) = (tempfile::tempdir().unwrap(), tempfile::tempdir().unwrap());
        for dir in [one.path(), two.path().join("deeper").as_path()] {
            for (at, bytes) in files() {
                let path = at.split('/').fold(dir.to_path_buf(), |p, s| p.join(s));
                std::fs::create_dir_all(path.parent().unwrap()).unwrap();
                std::fs::write(path, bytes).unwrap();
            }
        }
        let want = hash(&files(), &id("p")).unwrap();
        assert_eq!(hash(&Folder(one.path()), &id("p")).unwrap(), want);
        let deeper = two.path().join("deeper");
        assert_eq!(hash(&Folder(&deeper), &id("p")).unwrap(), want);
    }

    #[test]
    fn pieces_are_length_prefixed() {
        // Moving a byte from one feature's ID to its bytes changes the hash.
        let one = by_hand("mm", &[b"x", b"ab", b"c"]);
        let two = by_hand("mm", &[b"x", b"a", b"bc"]);
        assert_ne!(one, two);
    }

    #[test]
    fn a_part_that_cant_be_read_has_no_hash() {
        let mut f = files();
        f.remove("parts/p/features/b.json");
        let e = hash(&f, &id("p")).unwrap_err();
        assert_eq!(
            (e.code, e.details["path"].as_str()),
            (Code::NotFound, Some("parts/p/features/b.json"))
        );
        let e = hash(&files(), &id("q")).unwrap_err();
        assert_eq!(e.code, Code::NotFound);
        let mut conflicted = files();
        conflicted.insert(
            "parts/p/part.json".into(),
            br#"{"conflicts": {"/features": {"kind": "order"}}, "features": [], "format": 1, "kind": "ryucad.part"}"#.to_vec(),
        );
        assert_eq!(
            hash(&conflicted, &id("p")).unwrap_err().code,
            Code::MergeConflict
        );
    }
}
