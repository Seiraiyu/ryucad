//! The schematic corpus's references (`tests/schematic_corpus/manifest.txt`):
//! every circuit declared once with a source and a licence; every committed
//! file declared, under a licence that allows it, its bytes its SHA-256;
//! every fetched one pinned by URL and SHA-256; and the circuits the
//! corpus builds. `scripts/fetch-schematic-refs.sh` fetches the rest.

use sha2::{Digest, Sha256};
use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};

/// Licences a reference may be committed under.
const PERMISSIVE: &[&str] = &["Apache-2.0", "MIT", "BSD-3-Clause", "CC-BY-4.0"];

#[derive(Default, Debug)]
struct Circuit {
    sources: usize,
    licence: Option<String>,
    files: Vec<(String, String)>,
    fetches: Vec<(String, String)>,
}

fn dir() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/schematic_corpus")
}

fn manifest() -> BTreeMap<String, Circuit> {
    let text = std::fs::read_to_string(dir().join("manifest.txt")).unwrap();
    let mut out: BTreeMap<String, Circuit> = BTreeMap::new();
    let mut at: Option<String> = None;
    for (n, line) in text.lines().enumerate() {
        if line.starts_with('#') || line.trim().is_empty() {
            continue;
        }
        if let Some(name) = line.strip_prefix("circuit ") {
            assert!(!out.contains_key(name), "line {}: {name} twice", n + 1);
            out.insert(name.to_owned(), Circuit::default());
            at = Some(name.to_owned());
            continue;
        }
        let c = out
            .get_mut(at.as_deref().expect("a circuit first"))
            .unwrap();
        let line = line.trim_start();
        let (key, rest) = line.split_once(' ').unwrap_or((line, ""));
        let pair = || {
            let (a, b) = rest.rsplit_once(' ').expect("two fields");
            (a.to_owned(), b.to_owned())
        };
        match key {
            "source" => c.sources += 1,
            "licence" => c.licence = Some(rest.to_owned()),
            "file" => c.files.push(pair()),
            "fetch" => c.fetches.push(pair()),
            "crossings" => {
                rest.parse::<u32>().expect("a count");
            }
            k => panic!("line {}: unknown key {k}", n + 1),
        }
    }
    out
}

fn sha256(bytes: &[u8]) -> String {
    Sha256::digest(bytes)
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect()
}

#[test]
fn every_reference_is_declared_licensed_and_pinned() {
    let m = manifest();
    assert_eq!(
        m.keys().map(String::as_str).collect::<Vec<_>>(),
        ["breakout", "buck", "ethernet", "frontend", "rp2040"]
    );
    let mut declared = BTreeSet::new();
    for (name, c) in &m {
        let licence = c
            .licence
            .as_deref()
            .unwrap_or_else(|| panic!("{name}: no licence"));
        assert!(c.sources > 0, "{name}: no source");
        assert!(
            !c.files.is_empty() || !c.fetches.is_empty(),
            "{name}: nothing to compare with"
        );
        if !c.files.is_empty() {
            assert!(
                PERMISSIVE.contains(&licence),
                "{name}: {licence} may only be fetched"
            );
        }
        for (path, hash) in &c.files {
            let bytes = std::fs::read(dir().join("references").join(path))
                .unwrap_or_else(|e| panic!("{name}: {path}: {e}"));
            assert_eq!(&sha256(&bytes), hash, "{name}: {path}");
            declared.insert(path.clone());
        }
        for (url, hash) in &c.fetches {
            assert!(url.starts_with("https://"), "{name}: {url}");
            assert_eq!(hash.len(), 64, "{name}: {url}");
        }
    }
    // Nothing committed that isn't declared.
    let mut stack = vec![dir().join("references")];
    while let Some(d) = stack.pop() {
        for e in std::fs::read_dir(&d).unwrap() {
            let p = e.unwrap().path();
            if p.is_dir() {
                stack.push(p);
            } else {
                let rel = p
                    .strip_prefix(dir().join("references"))
                    .unwrap()
                    .to_string_lossy()
                    .replace('\\', "/");
                assert!(declared.contains(&rel), "{rel} isn't in the manifest");
            }
        }
    }
}
