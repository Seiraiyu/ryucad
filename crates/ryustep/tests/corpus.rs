//! The committed corpus: real files, read as `manifest.txt` says. Each
//! body ryustep reads is healed or refused as listed; a healed one passes
//! the checker, meshes without faults and encloses the volume OCCT gives
//! its solid (its exact B-rep volume, from `ryukei-bench`'s `step` test)
//! to within two thousandths, measured on a mesh at a five-thousandth of its
//! size. The NIST files are public domain (US Government works, from
//! NIST's MBE PMI validation and conformance testing models).

use ryukei::check::check;
use ryukei::mesh::{MeshParams, tessellate};
use ryukei::name::OpKey;
use ryukei::topo::Body;
use ryukei_testkit::mesh::volume_and_area;
use ryustep::{ImportError, ReadOptions, read};
use std::collections::BTreeMap;

fn dir() -> std::path::PathBuf {
    std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/corpus")
}

/// What a body's result is, as the manifest writes it: `ok`, or its
/// refusal in snake case (`open_shell` for `Heal(OpenShell { .. })`).
fn outcome<T>(r: &Result<T, ImportError>) -> String {
    let Err(e) = r else {
        return "ok".into();
    };
    let text = match e {
        ImportError::Heal(h) => format!("{h:?}"),
        e => format!("{e:?}"),
    };
    let name: String = text
        .chars()
        .take_while(|c| c.is_ascii_alphanumeric())
        .collect();
    let mut out = String::new();
    for (i, c) in name.chars().enumerate() {
        if c.is_ascii_uppercase() {
            if i > 0 {
                out.push('_');
            }
            out.push(c.to_ascii_lowercase());
        } else {
            out.push(c);
        }
    }
    out
}

/// A body's volume in mm³, from a mesh at a five-thousandth of its size.
fn volume(b: &Body) -> f64 {
    let size = b
        .faces()
        .map(|f| ryukei::intersect::face_box(b, f))
        .reduce(|x, y| x.union(&y))
        .map(|x| (x.max() - x.min()).length())
        .unwrap();
    let m = tessellate(b, &MeshParams::new(size * 2e-4, 0.3)).unwrap();
    assert_eq!(m.faults, []);
    volume_and_area(&m).0 * 1e9
}

#[test]
fn the_corpus_reads_as_its_manifest_says() {
    let text = std::fs::read_to_string(dir().join("manifest.txt")).unwrap();
    // file -> representation -> (outcome, OCCT's volume)
    let mut want: BTreeMap<String, BTreeMap<u64, (String, Option<f64>)>> = BTreeMap::new();
    for line in text.lines() {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        let f: Vec<&str> = line.split_whitespace().collect();
        want.entry(f[0].into()).or_default().insert(
            f[1].parse().unwrap(),
            (f[2].into(), f.get(3).map(|v| v.parse().unwrap())),
        );
    }
    let opts = ReadOptions {
        key: OpKey(7),
        heal: Default::default(),
    };
    let mut files: Vec<String> = std::fs::read_dir(dir())
        .unwrap()
        .map(|e| e.unwrap().file_name().into_string().unwrap())
        .filter(|n| n.ends_with(".stp"))
        .collect();
    files.sort();
    assert_eq!(files, want.keys().cloned().collect::<Vec<_>>());
    let mut wrong = Vec::new();
    for name in &files {
        let model = read(&std::fs::read(dir().join(name)).unwrap(), &opts).unwrap();
        let listed = &want[name];
        let got: Vec<u64> = model.bodies.iter().map(|b| b.representation).collect();
        if got != listed.keys().copied().collect::<Vec<_>>() {
            wrong.push(format!("{name}: bodies {got:?}"));
            continue;
        }
        for b in &model.bodies {
            let (outcome_wanted, occt) = &listed[&b.representation];
            let o = outcome(&b.result);
            if &o != outcome_wanted {
                wrong.push(format!(
                    "{name} #{}: {o}, not {outcome_wanted}",
                    b.representation
                ));
                continue;
            }
            if let (Ok((body, _, _)), Some(w)) = (&b.result, occt) {
                assert_eq!(check(body), [], "{name} #{}", b.representation);
                let v = volume(body);
                if (v - w).abs() > 2e-3 * w {
                    wrong.push(format!("{name} #{}: {v} mm³, OCCT {w}", b.representation));
                }
            }
        }
    }
    assert!(wrong.is_empty(), "{wrong:#?}");
}
