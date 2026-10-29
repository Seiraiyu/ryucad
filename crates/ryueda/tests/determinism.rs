//! Bit-exact results on every platform and CPU level: `tests/golden/ryueda.txt`
//! holds hashes of the sensor breakout's netlist and ERC (entered by its
//! script through the CLI), of a generated design's, and of the library
//! our KiCad-format test files import to, each the same on one thread and
//! eight. After a deliberate change, regenerate with
//! `UPDATE_GOLDEN=1 cargo test -p ryueda --test determinism` and review
//! the diff like any other change.

#[path = "support/breakout.rs"]
mod breakout;
#[path = "support/draw.rs"]
mod draw;
#[path = "support/generated.rs"]
mod generated;

use assert_cmd::Command;
use serde_json::Value;
use std::collections::BTreeMap;
use std::path::Path;

/// The golden file: a hash per name, compared or (with `UPDATE_GOLDEN=1`)
/// written.
struct Golden {
    path: &'static str,
    got: BTreeMap<String, String>,
}

impl Golden {
    fn new(path: &'static str) -> Golden {
        Golden {
            path,
            got: BTreeMap::new(),
        }
    }

    /// Hashes `v`'s canonical text under `name`.
    fn json(&mut self, name: &str, v: &Value) {
        let text = ryueda::canonical::to_string(v);
        self.got.insert(
            name.to_owned(),
            blake3::hash(text.as_bytes()).to_hex()[..32].to_owned(),
        );
    }

    fn finish(self) {
        let text: String = std::iter::once(
            "# ryueda's golden hashes: UPDATE_GOLDEN=1 cargo test -p ryueda --test determinism\n"
                .to_owned(),
        )
        .chain(self.got.iter().map(|(k, v)| format!("{k} {v}\n")))
        .collect();
        if std::env::var("UPDATE_GOLDEN").is_ok_and(|v| v == "1") {
            std::fs::write(self.path, text).unwrap();
            return;
        }
        let want = std::fs::read_to_string(self.path).unwrap_or_default();
        let want: BTreeMap<&str, &str> = want
            .lines()
            .filter(|l| !l.starts_with('#'))
            .filter_map(|l| l.split_once(' '))
            .collect();
        let differ: Vec<&String> = self
            .got
            .iter()
            .filter(|(k, v)| want.get(k.as_str()) != Some(&v.as_str()))
            .map(|(k, _)| k)
            .collect();
        assert!(
            differ.is_empty() && want.len() == self.got.len(),
            "golden hashes differ: {differ:?}; regenerate with UPDATE_GOLDEN=1 if that was meant"
        );
    }
}

/// `ryueda` in `dir`, in process, its IDs the golden seed's.
fn ryueda(dir: &Path, threads: usize, args: &[&str]) -> Value {
    let out = Command::cargo_bin("ryueda")
        .unwrap()
        .current_dir(dir)
        .env("RYUEDA_NO_DAEMON", "1")
        .env("RYUEDA_ID_SEED", "golden")
        .env("RAYON_NUM_THREADS", threads.to_string())
        .env("RYUEDA_CONFIG_DIR", dir.join("..").join("config"))
        .args(args)
        .output()
        .unwrap();
    let v: Value = serde_json::from_slice(&out.stdout).unwrap();
    assert_eq!(v["ok"], true, "{args:?}: {v}");
    v["result"].clone()
}

#[test]
fn netlists_and_erc_are_bit_exact() {
    let mut golden = Golden::new(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/tests/golden/ryueda.txt"
    ));
    let dir = tempfile::tempdir().unwrap();
    let board = dir.path().join("board");
    std::fs::create_dir_all(&board).unwrap();
    ryueda(&board, 1, &["init"]);
    for (path, text) in breakout::footprints() {
        let at = board.join(path);
        std::fs::create_dir_all(at.parent().unwrap()).unwrap();
        std::fs::write(at, text).unwrap();
    }
    for args in breakout::script() {
        let args: Vec<&str> = args.iter().map(String::as_str).collect();
        ryueda(&board, 1, &args);
    }
    for what in ["netlist", "erc"] {
        let one = ryueda(&board, 1, &[what]);
        assert_eq!(ryueda(&board, 8, &[what]), one, "{what}");
        golden.json(&format!("breakout.{what}"), &one);
    }
    let made = dir.path().join("generated");
    generated::write(&made, 8, 16);
    let p = ryueda::project::Project::load(&made).unwrap();
    let design = ryueda::id::Id::parse("d", "design").unwrap();
    let run = |n: usize| {
        rayon::ThreadPoolBuilder::new()
            .num_threads(n)
            .build()
            .unwrap()
            .install(|| {
                (
                    ryueda::netlist::flatten(&p, &design).unwrap().to_json(),
                    ryueda::erc::erc(&p, None).unwrap(),
                )
            })
    };
    let (netlist, erc) = run(1);
    assert_eq!(run(8), (netlist.clone(), erc.clone()));
    golden.json("generated.netlist", &netlist);
    golden.json("generated.erc", &erc);
    let one = kicad_library(1);
    assert_eq!(kicad_library(8), one);
    golden.json("kicad.library", &one);
    golden.finish();
}

/// Every file our KiCad-format test files import to, by path, each its
/// BLAKE3, imported on `threads` threads.
fn kicad_library(threads: usize) -> Value {
    let data = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/data/kicad");
    let paths = vec![
        data.join("symbols/Ryu_Test.kicad_sym"),
        data.join("footprints/Ryu_Test.pretty"),
    ];
    let opts = ryueda::kicad::import::Options {
        paths,
        models: vec![data.join("3dmodels")],
        ..Default::default()
    };
    let dir = tempfile::tempdir().unwrap();
    ryueda::server::init(dir.path(), None, false).unwrap();
    let mut s = ryueda::session::Session::open(dir.path()).unwrap();
    rayon::ThreadPoolBuilder::new()
        .num_threads(threads)
        .build()
        .unwrap()
        .install(|| ryueda::kicad::import::import(&mut s, &opts, None))
        .unwrap();
    let mut files = BTreeMap::new();
    let mut stack = vec![dir.path().join("library")];
    while let Some(d) = stack.pop() {
        for e in std::fs::read_dir(&d).unwrap() {
            let path = e.unwrap().path();
            if path.is_dir() {
                stack.push(path);
            } else {
                let rel = ryueda::source::relative(dir.path(), &path);
                let hash = blake3::hash(&std::fs::read(&path).unwrap())
                    .to_hex()
                    .to_string();
                files.insert(rel, Value::from(hash));
            }
        }
    }
    serde_json::to_value(files).unwrap()
}
