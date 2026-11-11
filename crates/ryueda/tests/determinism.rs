//! Bit-exact results on every platform and CPU level: `tests/golden/ryueda.txt`
//! holds hashes of the sensor breakout's netlist and ERC (entered by its
//! script through the CLI), of a generated design's, and of the library
//! our KiCad-format test files import to, each the same on one thread and
//! eight. After a deliberate change, regenerate with
//! `UPDATE_GOLDEN=1 cargo test -p ryueda --test determinism` and review
//! the diff like any other change.

#[path = "support/breakout.rs"]
mod breakout;
#[path = "support/buck.rs"]
mod buck;
#[path = "support/draw.rs"]
mod draw;
#[path = "support/ethernet.rs"]
mod ethernet;
#[path = "support/frontend.rs"]
mod frontend;
#[path = "support/generated.rs"]
mod generated;
#[path = "support/parts.rs"]
mod parts;
#[path = "support/rp2040.rs"]
mod rp2040;

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
    ryueda_env(dir, threads, args, &[])
}

/// [`ryueda`] with `env` set too.
fn ryueda_env(dir: &Path, threads: usize, args: &[&str], env: &[(&str, &str)]) -> Value {
    let v = ryueda_out(dir, threads, args, env);
    assert_eq!(v["ok"], true, "{args:?}: {v}");
    v["result"].clone()
}

/// The envelope `ryueda` prints, whether it succeeded or not.
fn ryueda_out(dir: &Path, threads: usize, args: &[&str], env: &[(&str, &str)]) -> Value {
    let mut cmd = Command::cargo_bin("ryueda").unwrap();
    cmd.current_dir(dir)
        .env("RYUEDA_NO_DAEMON", "1")
        .env("RYUEDA_ID_SEED", "golden")
        .env("RAYON_NUM_THREADS", threads.to_string())
        .env("RYUEDA_CONFIG_DIR", dir.join("..").join("config"))
        // No catalogue this machine holds ever shows in a golden.
        .env("RYUEDA_CACHE_DIR", dir.join("..").join("cache"));
    // No person's distributor keys or API ever reach a golden.
    for v in ryueda_parts::distributor::env_vars() {
        cmd.env_remove(v);
    }
    for (k, v) in env {
        cmd.env(k, v);
    }
    let out = cmd.args(args).output().unwrap();
    serde_json::from_slice(&out.stdout).unwrap()
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
    // Schematics: the breakout laid out, then the RP2040 board and the
    // rest of the corpus, each one thread and eight; their layout files,
    // SVG, descriptions, stacked layers and PDFs.
    schematic(&mut golden, &board, "breakout");
    styled(&mut golden, &board);
    let pico = dir.path().join("pico");
    std::fs::create_dir_all(&pico).unwrap();
    ryueda(&pico, 1, &["init"]);
    for args in rp2040::script() {
        let args: Vec<&str> = args.iter().map(String::as_str).collect();
        ryueda(&pico, 1, &args);
    }
    schematic(&mut golden, &pico, "rp2040");
    // The buck supply, the front end (two stacks) and the Ethernet PHY
    // (a stack of status LEDs).
    for (name, script) in [
        ("buck", buck::script()),
        ("frontend", frontend::script()),
        ("ethernet", ethernet::script()),
    ] {
        let at = dir.path().join(name);
        std::fs::create_dir_all(&at).unwrap();
        ryueda(&at, 1, &["init"]);
        for args in script {
            let args: Vec<&str> = args.iter().map(String::as_str).collect();
            ryueda(&at, 1, &args);
        }
        schematic(&mut golden, &at, name);
    }
    // Part data (design 347): library searches over a seeded library, and
    // the identification findings of a design placing some of it, one
    // thread and eight.
    let lib = dir.path().join("parts");
    parts::write(&lib, 200, true);
    for (name, args) in [
        (
            "parts.search.capacitor",
            vec![
                "parts",
                "search",
                "capacitor",
                "capacitance=100nF",
                "voltage_rating>=16V",
                "package=0402",
            ],
        ),
        (
            "parts.search.resistor",
            vec![
                "parts",
                "search",
                "resistor",
                "resistance<=10 kOhm",
                "tolerance<=5%",
                "--limit",
                "50",
            ],
        ),
        (
            "parts.search.like",
            vec!["parts", "search", "--like", "p00001"],
        ),
    ] {
        let one = ryueda(&lib, 1, &args);
        assert_eq!(ryueda(&lib, 8, &args), one, "{name}");
        assert!(
            !one["results"].as_array().unwrap().is_empty(),
            "{name}: {one}"
        );
        golden.json(name, &one);
    }
    let one = ryueda(&lib, 1, &["erc"]);
    assert_eq!(ryueda(&lib, 8, &["erc"]), one);
    let codes: Vec<&str> = one["designs"][0]["findings"]
        .as_array()
        .unwrap()
        .iter()
        .filter_map(|f| f["code"].as_str())
        .filter(|c| c.starts_with("part_"))
        .collect();
    assert!(
        codes.contains(&"part_unidentified") && codes.contains(&"part_unsourced"),
        "{one}"
    );
    golden.json("parts.erc", &one);
    // The catalogue (design 347): a catalogue of the seeded library's
    // offers, written by us as a synced index, searched, shown and judged
    // for supply, one thread and eight.
    let cache = dir.path().join("catalogue");
    let parts_list = parts::lcsc_parts(200);
    let refs: Vec<(&str, &str, u64, bool)> = parts_list
        .iter()
        .map(|(k, m, s, b)| (k.as_str(), m.as_str(), *s, *b))
        .collect();
    ryueda_parts::jlc::testing::catalogue_of(
        &cache.join("parts").join("jlc"),
        &refs,
        1_793_964_810,
    );
    let cache_text = cache.to_str().unwrap().to_owned();
    let env = [
        ("RYUEDA_CACHE_DIR", cache_text.as_str()),
        ("RYUEDA_NOW", "1793964810"),
    ];
    for (name, args) in [
        (
            "parts.jlc.search",
            vec![
                "parts", "search", "resistor", "--in", "jlc", "--stock", "100", "--limit", "30",
            ],
        ),
        ("parts.jlc.show", vec!["parts", "show", "lcsc:C10007"]),
        ("parts.jlc.sources", vec!["parts", "sources"]),
    ] {
        let mut one = ryueda_env(&lib, 1, &args, &env);
        assert_eq!(ryueda_env(&lib, 8, &args, &env), one, "{name}");
        if one.get("cache").is_some() {
            one["cache"] = Value::from("<cache>");
        }
        golden.json(name, &one);
    }
    // DigiKey (6.C): our fixtures' products as records, at a fixed time.
    let records: Vec<Value> = [
        ryueda_parts::digikey::testing::CAP,
        ryueda_parts::digikey::testing::LDO,
    ]
    .into_iter()
    .map(|f| {
        let p = ryueda_parts::digikey::testing::product(f);
        serde_json::to_value(ryueda_parts::digikey::record::record(
            &p,
            None,
            1_793_964_810,
        ))
        .unwrap()
    })
    .collect();
    golden.json("parts.digikey.records", &Value::from(records));
    std::fs::write(
        lib.join("parts.json"),
        "{\"assembly\": \"jlcpcb\", \"build_quantity\": 3, \"format\": 1, \"kind\": \"ryueda.parts\"}\n",
    )
    .unwrap();
    let report = |threads| {
        let v = ryueda_out(&lib, threads, &["check"], &env);
        if v["ok"] == true {
            v["result"]["supply"].clone()
        } else {
            v["error"]["details"]["report"]["supply"].clone()
        }
    };
    let supply = report(1);
    assert_eq!(report(8), supply);
    let codes: Vec<&str> = supply
        .as_array()
        .unwrap()
        .iter()
        .filter_map(|f| f["code"].as_str())
        .collect();
    for c in ["part_out_of_stock", "part_extended", "parts_mismatch"] {
        assert!(codes.contains(&c), "{c}: {supply}");
    }
    golden.json("parts.supply", &supply);
    let one = kicad_library(1);
    assert_eq!(kicad_library(8), one);
    golden.json("kicad.library", &one);
    // Every template's file under the built-in standards, both shapes (plan
    // 2.D): the standards' own drawings, byte for byte.
    let mut templates = serde_json::Map::new();
    for shapes in ["ansi", "iec"] {
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path().join("t");
        std::fs::create_dir_all(&p).unwrap();
        ryueda(&p, 1, &["init"]);
        ryueda(
            &p,
            1,
            &["standards", "set", &format!("{{\"shapes\": \"{shapes}\"}}")],
        );
        for class in ["resistor", "capacitor", "inductor", "diode", "led"] {
            ryueda(&p, 1, &["symbol", "create", class, "--template", class]);
        }
        ryueda(
            &p,
            1,
            &[
                "symbol",
                "create",
                "mcu",
                "--template",
                "ic",
                "--pins",
                MCU_PINS,
            ],
        );
        let mut files: Vec<_> = std::fs::read_dir(p.join("library/symbols"))
            .unwrap()
            .map(|f| f.unwrap().path())
            .collect();
        files.sort();
        for f in files {
            templates.insert(
                format!("{shapes}/{}", f.file_name().unwrap().to_string_lossy()),
                std::fs::read_to_string(&f).unwrap().into(),
            );
        }
    }
    golden.json("symbols.templates", &Value::Object(templates));
    // The committed KiCad test library, imported and judged.
    let one = kicad_lint(1);
    assert_eq!(kicad_lint(8), one);
    golden.json("symbols.lint", &one);
    golden.finish();
}

/// A circuit's schematic under `name`: laid out on one thread and eight
/// alike, then each block's layout file, its SVG and its description.
fn schematic(golden: &mut Golden, dir: &Path, name: &str) {
    let one = ryueda(dir, 1, &["layout", "--dry-run"]);
    assert_eq!(ryueda(dir, 8, &["layout", "--dry-run"]), one, "{name}");
    ryueda(dir, 1, &["layout"]);
    let blocks: Vec<String> = one["blocks"]
        .as_array()
        .unwrap()
        .iter()
        .map(|b| b["block"].as_str().unwrap().to_owned())
        .collect();
    for b in blocks {
        let file = std::fs::read(dir.join(format!("blocks/{b}/layout.json"))).unwrap();
        golden.got.insert(
            format!("schematic.{name}.{b}.layout"),
            blake3::hash(&file).to_hex()[..32].to_owned(),
        );
        ryueda(dir, 1, &["render", "--block", &b, "--out", "x.svg"]);
        let svg = std::fs::read(dir.join("x.svg")).unwrap();
        golden.got.insert(
            format!("schematic.{name}.{b}.svg"),
            blake3::hash(&svg).to_hex()[..32].to_owned(),
        );
        // The dark theme (plan 2.E).
        ryueda(
            dir,
            1,
            &["render", "--block", &b, "--theme", "dark", "--out", "x.svg"],
        );
        let svg = std::fs::read(dir.join("x.svg")).unwrap();
        golden.got.insert(
            format!("schematic.{name}.{b}.dark.svg"),
            blake3::hash(&svg).to_hex()[..32].to_owned(),
        );
        let d = ryueda(dir, 1, &["describe", "--block", &b]);
        golden.json(&format!("schematic.{name}.{b}.describe"), &d);
        // A stacked block: each layer drawn with its own designators.
        let layers = d["blocks"][0]["layers"]
            .as_array()
            .cloned()
            .unwrap_or_default();
        for l in layers {
            let path = l["path"].as_str().unwrap();
            ryueda(
                dir,
                1,
                &["render", "--block", &b, "--layer", path, "--out", "x.svg"],
            );
            let svg = std::fs::read(dir.join("x.svg")).unwrap();
            golden.got.insert(
                format!("schematic.{name}.{b}.{}.svg", path.replace('/', ".")),
                blake3::hash(&svg).to_hex()[..32].to_owned(),
            );
        }
    }
    // The output job's PDF, alike on one thread and eight.
    ryueda(dir, 1, &["output", "--out", "x.pdf"]);
    let pdf = std::fs::read(dir.join("x.pdf")).unwrap();
    ryueda(dir, 8, &["output", "--out", "x.pdf"]);
    assert_eq!(std::fs::read(dir.join("x.pdf")).unwrap(), pdf, "{name}");
    golden.got.insert(
        format!("schematic.{name}.pdf"),
        blake3::hash(&pdf).to_hex()[..32].to_owned(),
    );
    // Each block's first component as the style draws it (plan 2.E).
    for b in &blocks_of(dir) {
        let shown = ryueda(dir, 1, &["block", "show", b]);
        let first = shown["components"]
            .as_array()
            .and_then(|c| c.iter().find_map(|c| c["refdes"].as_str()))
            .map(str::to_owned);
        if let Some(r) = first {
            let e = ryueda(dir, 1, &["style", "explain", &r, "--block", b]);
            golden.json(&format!("schematic.{name}.{b}.explain"), &e);
        }
    }
    // What annotate would number, each order, alike on one thread and
    // eight (plan 2.F); a dry run, so nothing above moves.
    for order in ["down", "across"] {
        let args = ["annotate", "--reset", "all", "--order", order, "--dry-run"];
        let mut a = ryueda(dir, 1, &args);
        assert_eq!(ryueda(dir, 8, &args), a, "{name} {order}");
        // Annotate's revision hashes the project's, which hashes the
        // manifest, whose project ID `init` draws at random (the golden
        // seed is only for objects' IDs): every run has its own.
        a.as_object_mut().unwrap().remove("revision");
        golden.json(&format!("schematic.{name}.annotate.{order}"), &a);
    }
}

/// The blocks a project lays out, by ID.
fn blocks_of(dir: &Path) -> Vec<String> {
    ryueda(dir, 1, &["layout", "--dry-run"])["blocks"]
        .as_array()
        .unwrap()
        .iter()
        .map(|b| b["block"].as_str().unwrap().to_owned())
        .collect()
}

/// The breakout styled (plan 2.E): a palette entry, a rule by tag, an
/// override and a tagged part, its layout file, SVG light and dark and
/// PDF under `schematic.breakout.styled.*`.
fn styled(golden: &mut Golden, dir: &Path) {
    let b = blocks_of(dir)[0].clone();
    let shown = ryueda(dir, 1, &["block", "show", &b]);
    let refs: Vec<String> = shown["components"]
        .as_array()
        .unwrap()
        .iter()
        .filter_map(|c| c["refdes"].as_str().map(str::to_owned))
        .collect();
    ryueda(
        dir,
        1,
        &["style", "palette", "set", "brand", "#0055aa/#66aaff"],
    );
    ryueda(
        dir,
        1,
        &[
            "style",
            "rule",
            "add",
            "tag=critical",
            "colour=brand",
            "emphasis=true",
        ],
    );
    ryueda(
        dir,
        1,
        &["tag", &refs[0], "--block", &b, "--add", "critical"],
    );
    ryueda(
        dir,
        1,
        &[
            "style",
            "set",
            &refs[1],
            "line_weight=heavy",
            "dash=true",
            "--block",
            &b,
        ],
    );
    ryueda(dir, 1, &["layout"]);
    let file = std::fs::read(dir.join(format!("blocks/{b}/layout.json"))).unwrap();
    golden.got.insert(
        format!("schematic.breakout.styled.{b}.layout"),
        blake3::hash(&file).to_hex()[..32].to_owned(),
    );
    for theme in ["light", "dark"] {
        ryueda(
            dir,
            1,
            &["render", "--block", &b, "--theme", theme, "--out", "x.svg"],
        );
        let svg = std::fs::read(dir.join("x.svg")).unwrap();
        golden.got.insert(
            format!("schematic.breakout.styled.{b}.{theme}.svg"),
            blake3::hash(&svg).to_hex()[..32].to_owned(),
        );
    }
    ryueda(dir, 1, &["output", "--out", "x.pdf"]);
    let pdf = std::fs::read(dir.join("x.pdf")).unwrap();
    golden.got.insert(
        "schematic.breakout.styled.pdf".to_owned(),
        blake3::hash(&pdf).to_hex()[..32].to_owned(),
    );
}

/// A project our KiCad-format test files are imported into, on `threads`
/// threads: the temporary folder that holds it, and its root.
fn kicad_imported(threads: usize) -> (tempfile::TempDir, std::path::PathBuf) {
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
    let root = dir.path().join("k");
    std::fs::create_dir_all(&root).unwrap();
    ryueda::server::init(&root, None, false).unwrap();
    let mut s = ryueda::session::Session::open(&root).unwrap();
    rayon::ThreadPoolBuilder::new()
        .num_threads(threads)
        .build()
        .unwrap()
        .install(|| ryueda::kicad::import::import(&mut s, &opts, None))
        .unwrap();
    (dir, root)
}

/// Every file our KiCad-format test files import to, by path, each its
/// BLAKE3, imported on `threads` threads.
fn kicad_library(threads: usize) -> Value {
    let (_dir, root) = kicad_imported(threads);
    let mut files = BTreeMap::new();
    let mut stack = vec![root.join("library")];
    while let Some(d) = stack.pop() {
        for e in std::fs::read_dir(&d).unwrap() {
            let path = e.unwrap().path();
            if path.is_dir() {
                stack.push(path);
            } else {
                let rel = ryueda::source::relative(&root, &path);
                let hash = blake3::hash(&std::fs::read(&path).unwrap())
                    .to_hex()
                    .to_string();
                files.insert(rel, Value::from(hash));
            }
        }
    }
    serde_json::to_value(files).unwrap()
}

/// The KiCad test library imported, then judged by `lib check` on
/// `threads` threads (plan 2.D).
fn kicad_lint(threads: usize) -> Value {
    let (_dir, root) = kicad_imported(threads);
    ryueda(&root, threads, &["lib", "check"])
}

/// Task 20's MCU's fourteen named pins, for the `ic` template's golden.
const MCU_PINS: &str = r#"[
    {"number": "1", "name": "VDD", "type": "power_in"},
    {"number": "2", "name": "IOVDD", "type": "power_in"},
    {"number": "3", "name": "GND", "type": "power_in"},
    {"number": "4", "name": "AGND", "type": "power_in"},
    {"number": "5", "name": "RUN", "type": "input"},
    {"number": "6", "name": "TX", "type": "output"},
    {"number": "7", "name": "SDA", "type": "bidirectional"},
    {"number": "8", "name": "SCL", "type": "bidirectional"},
    {"number": "9", "name": "MOSI", "type": "bidirectional"},
    {"number": "10", "name": "MISO", "type": "bidirectional"},
    {"number": "11", "name": "SCK", "type": "bidirectional"},
    {"number": "12", "name": "CS", "type": "bidirectional"},
    {"number": "13", "name": "USB_D+", "type": "bidirectional"},
    {"number": "14", "name": "USB_D-", "type": "bidirectional"}
]"#;
