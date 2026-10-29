//! The committed corpus, every board `manifest.txt` declares but the
//! pinned ones: its archive has the SHA-256 recorded, its licence text is
//! here, it reads to the counts recorded, as a folder and as its archive
//! alike, and nothing in this folder is undeclared. `UPDATE_MANIFEST=1`
//! writes the counts read now; review the diff.

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

use ryu_board::model::Board;
use ryudfm::odb::{ReadOptions, read_path};
use ryudfm::testing::{Declared, check_manifest, manifest, summary};
use sha2::{Digest, Sha256};

fn corpus() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/corpus")
}

fn committed() -> Vec<Declared> {
    let text = std::fs::read_to_string(corpus().join("manifest.txt")).unwrap();
    manifest(&text).into_iter().filter(|d| !d.pinned).collect()
}

fn options(d: &Declared) -> ReadOptions {
    ReadOptions {
        step: d.step.clone(),
        ..ReadOptions::default()
    }
}

/// A committed board, from its archive or else its folder.
pub fn board(d: &Declared) -> Board {
    let path = match (&d.archive, &d.folder) {
        (Some((a, _)), _) => corpus().join(a),
        (None, Some(f)) => corpus().join(f),
        (None, None) => panic!("{}: neither an archive nor a folder", d.name),
    };
    read_path(&path, &options(d)).unwrap()
}

#[test]
fn every_board_is_declared_and_licensed() {
    let boards = committed();
    assert!(boards.len() >= 3);
    let mut owned =
        BTreeSet::from(["manifest.txt", "pinned.txt", "licenses", "reference"].map(String::from));
    for d in &boards {
        assert!(!d.source.is_empty(), "{}: no source", d.name);
        let licence = d
            .licence
            .as_deref()
            .unwrap_or_else(|| panic!("{}: no licence", d.name));
        if licence != "owner" {
            assert!(
                corpus()
                    .join("licenses")
                    .join(format!("{licence}.txt"))
                    .is_file(),
                "{}: no licenses/{licence}.txt",
                d.name
            );
        }
        if let Some((file, hash)) = &d.archive {
            let bytes = std::fs::read(corpus().join(file)).unwrap();
            let got: String = Sha256::digest(&bytes)
                .iter()
                .map(|b| format!("{b:02x}"))
                .collect();
            assert_eq!(&got, hash, "{}: {file} isn't the archive recorded", d.name);
            owned.insert(file.clone());
        }
        if let Some(f) = &d.folder {
            owned.insert(f.clone());
        }
        assert!(
            d.archive.is_some() || d.folder.is_some(),
            "{}: no job",
            d.name
        );
    }
    for e in std::fs::read_dir(corpus()).unwrap() {
        let name = e.unwrap().file_name().to_string_lossy().into_owned();
        assert!(
            owned.contains(&name),
            "{name} is in tests/corpus but no board in manifest.txt declares it"
        );
    }
}

#[test]
fn boards_read_to_their_manifest() {
    let mut got = Vec::new();
    for d in committed() {
        let a = board(&d);
        if let (Some(_), Some(f)) = (&d.archive, &d.folder) {
            let mut b = read_path(&corpus().join(f), &options(&d)).unwrap();
            // Exports packed apart may differ in their dates alone.
            b.source.created.clone_from(&a.source.created);
            b.source.saved.clone_from(&a.source.saved);
            assert!(
                a == b,
                "{}: the folder and the archive read differently",
                d.name
            );
        }
        got.push((d.name.clone(), summary(&a)));
    }
    let differing = check_manifest(&corpus().join("manifest.txt"), &got);
    assert!(
        differing.is_empty(),
        "counts changed for {differing:?}: UPDATE_MANIFEST=1 and review the diff"
    );
}

#[test]
fn one_thread_and_eight_read_alike() {
    for d in committed() {
        let read = |threads: usize| {
            rayon::ThreadPoolBuilder::new()
                .num_threads(threads)
                .build()
                .unwrap()
                .install(|| board(&d))
        };
        assert!(read(1) == read(8), "{}", d.name);
    }
}
