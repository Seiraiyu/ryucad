//! Healing that depended on the import key (#234's class): a file's part
//! heals under the part ID its product gives, as under any other.

use std::path::Path;
use std::process::Command;

fn ryucad() -> &'static str {
    env!("CARGO_BIN_EXE_ryucad")
}

fn run(dir: &Path, args: &[&str]) -> serde_json::Value {
    let out = Command::new(ryucad())
        .arg("-C")
        .arg(dir.join("project"))
        .args(args)
        .env("RYUCAD_NO_DAEMON", "1")
        .env("RYUCAD_RUNTIME_DIR", dir.join("run"))
        .env("RYUCAD_CACHE_DIR", dir.join("cache"))
        .env("RYUCAD_CONFIG_DIR", dir.join("config"))
        .output()
        .unwrap();
    serde_json::from_slice(&out.stdout)
        .unwrap_or_else(|e| panic!("{e}: {}", String::from_utf8_lossy(&out.stdout)))
}

/// A project folder in a fresh temporary folder, its runtime, cache and
/// config folders beside it.
fn project() -> tempfile::TempDir {
    let d = tempfile::tempdir().unwrap();
    std::fs::create_dir(d.path().join("project")).unwrap();
    let init = run(d.path(), &["init", "--unit", "mm"]);
    assert_eq!(init["ok"], true, "{init:#}");
    d
}

/// The features of every part the import made that failed, as `part/feature code`.
fn failed(import: &serde_json::Value) -> Vec<String> {
    let mut out = vec![];
    for p in import["result"]["parts"].as_array().into_iter().flatten() {
        for f in p["features"].as_array().into_iter().flatten() {
            if f["ok"] == false {
                out.push(format!("{}/{} {}", p["part"], f["id"], f["error"]["code"]));
            }
        }
    }
    out
}

fn data(name: &str) -> String {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests/data")
        .join(name)
        .to_string_lossy()
        .into_owned()
}

/// Issue #299: NIST CTC 03 (AP203 with PMI) failed the topology check
/// under its product's part ID, `nist_ctc_03_asme1`, and healed under any
/// other: 8.F's loops gathered whatever the build order.
#[test]
#[cfg_attr(
    debug_assertions,
    ignore = "healing in a debug build: run it in release"
)]
fn nist_ctc_03_heals_under_its_own_part_id() {
    let d = project();
    let import = run(d.path(), &["import", &data("nist_ctc_03_asme1_ap203.stp")]);
    assert_eq!(import["ok"], true, "{import:#}");
    assert!(
        !import["result"]["parts"].as_array().unwrap().is_empty(),
        "{import:#}"
    );
    assert_eq!(failed(&import), Vec::<String>::new(), "{import:#}");
}

/// Issue #285: NIST FTC 10, exported by ryucad, didn't heal again under
/// its product's part ID `nist_ftc_10_asme1` (the topology check), and
/// did under `body` or `x1`: two faces the build left each holding only
/// one face's loops, neither able to give one up to the keeper the build
/// order chose. The keeper is now any holder that can take them. Every
/// key heals it, to the same faces, edges and vertices.
#[test]
#[cfg_attr(
    debug_assertions,
    ignore = "healing in a debug build: run it in release"
)]
fn nist_ftc_10_s_own_export_heals_under_every_key() {
    let file = data("nist_ftc_10_export.step");
    let keys = [None, Some("body".to_string()), Some("x1".to_string())]
        .into_iter()
        .chain((0..32).map(|i| Some(format!("k{i}"))));
    let mut counts = std::collections::BTreeSet::new();
    for part in keys {
        let d = project();
        let mut args = vec!["import", file.as_str()];
        if let Some(p) = &part {
            args.extend(["--part", p.as_str()]);
        }
        let import = run(d.path(), &args);
        assert_eq!(import["ok"], true, "--part {part:?}: {import:#}");
        assert_eq!(
            failed(&import),
            Vec::<String>::new(),
            "--part {part:?}: {import:#}"
        );
        counts.insert(import["result"]["parts"][0]["features"][0]["counts"].to_string());
    }
    assert_eq!(counts.len(), 1, "{counts:?}");
}
