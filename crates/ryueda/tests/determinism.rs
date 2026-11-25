//! Bit-exact results on every platform and CPU level: `tests/golden/ryueda.txt`
//! holds hashes of the sensor breakout's netlist and ERC (entered by its
//! script through the CLI), of a generated design's, and of the library
//! our KiCad-format test files import to, each the same on one thread and
//! eight. After a deliberate change, regenerate with
//! `UPDATE_GOLDEN=1 cargo test -p ryueda --test determinism` and review
//! the diff like any other change.

#[path = "support/breakout.rs"]
mod breakout;
#[path = "support/breakout_board.rs"]
mod breakout_board;
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
    // No repository above the test's folder is found (#1004).
    cmd.env("GIT_CEILING_DIRECTORIES", std::env::temp_dir());
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

/// An exported file's hash with ryueda's version (which releases stamp
/// in) written as `VERSION`; the manifest's without its revisions and
/// version, nor each file's `blake3` and `bytes`: they hash the files as
/// written, version and all, so a release's stamped version moved them
/// (ryueda-release.yml's golden step, 5.D). Each file's own hash is a
/// key of its own.
fn export_hash(name: &str, bytes: &[u8]) -> String {
    let v = env!("CARGO_PKG_VERSION");
    let text = String::from_utf8_lossy(bytes).replace(&format!("ryueda,{v}"), "ryueda,VERSION");
    let text = if name == "manifest.json" {
        let mut m: Value = serde_json::from_str(&text).unwrap();
        for k in ["board_revision", "project_revision", "ryueda"] {
            m.as_object_mut().unwrap().remove(k);
        }
        for f in m["files"].as_array_mut().into_iter().flatten() {
            if let Some(f) = f.as_object_mut() {
                f.remove("blake3");
                f.remove("bytes");
            }
        }
        ryueda::canonical::to_string(&m)
    } else {
        text
    };
    blake3::hash(text.as_bytes()).to_hex()[..32].to_owned()
}

/// ryueda's PDM over the breakout (design 629), in a folder of its own
/// so no other key's files change: the design's content hash (which
/// covers the `self` components placed) and defaults through ryupdm's
/// trait, and `release propose --dry-run` in a repository, this
/// machine's supply findings left out. Neither reads the random project
/// ID `init` draws.
fn pdm(golden: &mut Golden, board: &Path) {
    use ryupdm::product::Product;
    std::fs::create_dir_all(board).unwrap();
    ryueda(board, 1, &["init"]);
    breakout_at(board);
    let files = ryueda::session::current(board).unwrap();
    let id = ryueda::id::Id::parse("board", "part").unwrap();
    let content = <ryueda::Eda as Product>::content(&files, &id).unwrap();
    golden.json("pdm.breakout.content", &Value::String(content));
    pins(golden, &files, &id);
    let defaults = <ryueda::Eda as Product>::defaults(&files, &id).unwrap();
    golden.json(
        "pdm.breakout.defaults",
        &serde_json::to_value(defaults).unwrap(),
    );
    git(board, &["init", "-q"]);
    let dry = |threads: usize| {
        let mut v = ryueda(
            board,
            threads,
            &["release", "propose", "board", "--dry-run"],
        );
        v.as_object_mut().unwrap().remove("supply");
        v
    };
    let one = dry(1);
    assert_eq!(dry(8), one, "one thread and eight");
    golden.json("pdm.breakout.dry_run", &one);
    // The BOM, its CSV and a diff after one value changes (design 629),
    // at one thread and at eight.
    let bom = |threads: usize, args: &[&str]| ryueda(board, threads, args);
    let one = bom(1, &["bom"]);
    assert_eq!(bom(8, &["bom"]), one, "one thread and eight");
    golden.json("pdm.breakout.bom", &one);
    let csv = bom(1, &["bom", "--format", "csv"]);
    golden.json("pdm.breakout.bom.csv", &csv["csv"]);
    golden.json("pdm.breakout.bom.by_block", &bom(1, &["bom", "--by-block"]));
    git(board, &["add", "-A"]);
    git(
        board,
        &[
            "-c",
            "user.name=g",
            "-c",
            "user.email=g@example.com",
            "-c",
            "commit.gpgsign=false",
            "commit",
            "-q",
            "-m",
            "golden",
        ],
    );
    let first = std::fs::read_dir(board.join("blocks"))
        .unwrap()
        .flat_map(|b| {
            std::fs::read_dir(b.unwrap().path().join("components"))
                .into_iter()
                .flatten()
        })
        .map(|f| f.unwrap().path())
        .min()
        .unwrap();
    let mut v: Value = serde_json::from_slice(&std::fs::read(&first).unwrap()).unwrap();
    v["value"] = Value::from("47k");
    std::fs::write(&first, ryueda::canonical::to_string(&v)).unwrap();
    let mut diff = bom(1, &["bom", "diff"]);
    diff.as_object_mut().unwrap().remove("from_commit");
    assert_eq!(diff["summary"]["added"], 1, "{diff}");
    golden.json("pdm.breakout.bom.diff", &diff);
}

/// Library labels (design 629): the breakout's content hash with one
/// component placed from `passives` pinned at B (project 2…2 in this
/// repository, so its piece is the project, component and label), at one
/// thread and at eight; and a `ryueda.lock` of one label as written.
fn pins(golden: &mut Golden, files: &ryu_session::fresh::Files, id: &ryueda::id::Id) {
    use ryueda::id::{Id, ProjectId};
    use ryueda::project::{Alias, MANIFEST, Manifest};
    use ryupdm::product::Product;
    let mut f = files.clone();
    let (path, comp) = f
        .iter()
        .filter(|(p, _)| p.starts_with("blocks/") && p.contains("/components/"))
        .find_map(|(p, b)| {
            let v: Value = serde_json::from_slice(b).ok()?;
            v["of"]
                .get("library")
                .is_none()
                .then(|| (p.clone(), v["of"]["id"].as_str().unwrap().to_owned()))
        })
        .expect("a component placed from the board's own library");
    let mut v: Value = serde_json::from_slice(&f[&path]).unwrap();
    v["of"]["library"] = Value::from("passives");
    f.insert(path, ryueda::canonical::to_string(&v).into_bytes().into());
    let mut m = Manifest::read(&f[MANIFEST]).unwrap();
    let mut a = Alias::local("../passives");
    a.project = Some(ProjectId::parse(&"2".repeat(32)).unwrap());
    a.labels.insert(Id::parse(&comp, "c").unwrap(), "B".into());
    m.libraries.insert(Id::parse("passives", "a").unwrap(), a);
    f.insert(MANIFEST.to_owned(), m.to_text().into_bytes().into());
    let hash = |threads: usize| {
        rayon::ThreadPoolBuilder::new()
            .num_threads(threads)
            .build()
            .unwrap()
            .install(|| <ryueda::Eda as Product>::content(&f, id).unwrap())
    };
    let one = hash(1);
    assert_eq!(hash(8), one, "one thread and eight");
    golden.json("pdm.pins", &Value::String(one));
    let mut lock = ryu_doc::lock::Lock::default();
    let mut locked = ryu_doc::lock::Locked {
        url: "file:///libs".into(),
        path: "lib".into(),
        release_branch: "main".into(),
        ..Default::default()
    };
    locked.labels.insert(
        (Id::parse(&comp, "c").unwrap(), "B".into()),
        ryu_doc::lock::LockedLabel {
            commit: "1".repeat(40),
            content: "2".repeat(64),
        },
    );
    lock.projects
        .insert(ProjectId::parse(&"2".repeat(32)).unwrap(), locked);
    golden.json(
        "pdm.pins.lock",
        &Value::String(ryueda::libdeps::lock_text(&lock).unwrap()),
    );
}

/// The sensor breakout entered by its script in `board` (made by `init`).
fn breakout_at(board: &Path) {
    for (path, text) in breakout::footprints() {
        let at = board.join(path);
        std::fs::create_dir_all(at.parent().unwrap()).unwrap();
        std::fs::write(at, text).unwrap();
    }
    for args in breakout::script() {
        let args: Vec<&str> = args.iter().map(String::as_str).collect();
        ryueda(board, 1, &args);
    }
}

/// Boards (5.A to 5.D): the breakout as agent round 5.D laid it out
/// (`support/breakout_board.rs`): its pads, renders, DRC (clean), copper
/// and fabrication package, alike on one thread and eight.
fn boards(golden: &mut Golden, at: &Path) {
    type Got = (
        Value,
        String,
        String,
        Value,
        Value,
        Value,
        BTreeMap<String, String>,
    );
    let id = breakout_board::BOARD;
    let mut first: Option<Got> = None;
    for threads in [1, 8] {
        let board = at.join(format!("t{threads}")).join("board");
        std::fs::create_dir_all(&board).unwrap();
        ryueda(&board, 1, &["init"]);
        breakout_at(&board);
        for args in breakout_board::script() {
            let args: Vec<&str> = args.iter().map(String::as_str).collect();
            ryueda(&board, threads, &args);
        }
        let mut show = ryueda(&board, threads, &["board", "show", id, "--pads"]);
        assert_eq!(show["unplaced"], 0, "{show}");
        assert_eq!(show["notices"], serde_json::json!([]), "{show}");
        // The golden is of what is drawn: revisions hash file bytes.
        if let Some(o) = show.as_object_mut() {
            o.remove("revision");
            o.remove("schema");
        }
        ryueda(
            &board,
            threads,
            &["board", "render", id, "--out", "top.png"],
        );
        ryueda(
            &board,
            threads,
            &[
                "board",
                "render",
                id,
                "--out",
                "bottom.svg",
                "--side",
                "bottom",
            ],
        );
        let hash = |name: &str| {
            blake3::hash(&std::fs::read(board.join(name)).unwrap()).to_hex()[..32].to_owned()
        };
        // Clean, so `board drc` answers rather than refusing.
        let mut drc = ryueda(&board, threads, &["board", "drc", id]);
        assert_eq!(drc["findings"], serde_json::json!([]), "{drc}");
        if let Some(o) = drc.as_object_mut() {
            o.remove("revision");
        }
        ryueda(&board, threads, &["board", "export", id, "--out", "fab"]);
        let mut exported = BTreeMap::new();
        for e in std::fs::read_dir(board.join("fab")).unwrap().flatten() {
            let name = e.file_name().to_string_lossy().into_owned();
            exported.insert(
                name.clone(),
                export_hash(&name, &std::fs::read(e.path()).unwrap()),
            );
        }
        assert!(exported.len() >= 14, "{exported:?}");
        let p = ryueda::project::Project::load(&board).unwrap();
        let bid = ryueda::id::Id::parse(id, "board").unwrap();
        let c = rayon::ThreadPoolBuilder::new()
            .num_threads(threads)
            .build()
            .unwrap()
            .install(|| ryueda::board::drc::check(&p, &p.boards[&bid]).unwrap());
        let layout = serde_json::json!({
            "items": c.conn.built.layout.items.iter().map(|(k, it)| serde_json::json!([c.conn.built.reference(k), it.layer, it.net])).collect::<Vec<_>>(),
            "holes": c.conn.built.layout.holes.iter().map(|(k, h)| serde_json::json!([c.conn.built.reference(k), h.plated])).collect::<Vec<_>>(),
            "points": c.conn.built.layout.points.iter().map(|pt| serde_json::json!([c.conn.built.reference(&pt.key), pt.net, [pt.at.x, pt.at.y]])).collect::<Vec<_>>(),
        });
        let islands: Vec<Vec<String>> = c
            .conn
            .copper
            .islands()
            .iter()
            .map(|i| i.items.iter().map(|k| c.conn.built.reference(k)).collect())
            .collect();
        let got = (
            show,
            hash("top.png"),
            hash("bottom.svg"),
            drc,
            layout,
            serde_json::json!(islands),
            exported,
        );
        match &first {
            None => first = Some(got),
            Some(one) => assert_eq!(&got, one, "one thread and eight"),
        }
    }
    let (show, png, svg, drc, layout, islands, exported) = first.unwrap();
    golden.json("board.breakout.show", &show);
    golden.got.insert("board.breakout.top.png".into(), png);
    golden.got.insert("board.breakout.bottom.svg".into(), svg);
    golden.json("board.breakout.drc", &drc);
    golden.json("board.breakout.layout", &layout);
    golden.json("board.breakout.islands", &islands);
    for (name, hash) in exported {
        golden
            .got
            .insert(format!("board.breakout.export.{name}"), hash);
    }
}

/// The breakout's board on four copper layers (ryueda 7.A): the same
/// round replayed, its stackup made four layers, a track on `in1`
/// between two of its through vias; its pads, DRC, copper, an `in1`
/// render and package, alike on one thread and eight.
fn four_layers(golden: &mut Golden, at: &Path) {
    type Got = (Value, String, Value, Value, Value, BTreeMap<String, String>);
    let id = breakout_board::BOARD;
    let mut first: Option<Got> = None;
    for threads in [1, 8] {
        let board = at.join(format!("t{threads}")).join("board");
        std::fs::create_dir_all(&board).unwrap();
        ryueda(&board, 1, &["init"]);
        breakout_at(&board);
        for args in breakout_board::script() {
            let args: Vec<&str> = args.iter().map(String::as_str).collect();
            ryueda(&board, threads, &args);
        }
        for args in FOUR {
            ryueda(&board, threads, args);
        }
        let mut show = ryueda(&board, threads, &["board", "show", id, "--pads"]);
        if let Some(o) = show.as_object_mut() {
            o.remove("revision");
            o.remove("schema");
        }
        ryueda(
            &board,
            threads,
            &[
                "board",
                "render",
                id,
                "--out",
                "in1.svg",
                "--layer",
                "copper.in1",
                "--layer",
                "drill",
                "--layer",
                "outline",
            ],
        );
        let hash = |name: &str| {
            blake3::hash(&std::fs::read(board.join(name)).unwrap()).to_hex()[..32].to_owned()
        };
        let mut drc = ryueda(&board, threads, &["board", "drc", id]);
        assert_eq!(drc["findings"], serde_json::json!([]), "{drc}");
        if let Some(o) = drc.as_object_mut() {
            o.remove("revision");
        }
        ryueda(&board, threads, &["board", "export", id, "--out", "fab"]);
        let mut exported = BTreeMap::new();
        for e in std::fs::read_dir(board.join("fab")).unwrap().flatten() {
            let name = e.file_name().to_string_lossy().into_owned();
            exported.insert(
                name.clone(),
                export_hash(&name, &std::fs::read(e.path()).unwrap()),
            );
        }
        assert!(
            exported.keys().any(|k| k.ends_with("-copper-in1.g2")),
            "{exported:?}"
        );
        assert!(
            exported.keys().any(|k| k.ends_with("-copper-in2.g3")),
            "{exported:?}"
        );
        let p = ryueda::project::Project::load(&board).unwrap();
        let bid = ryueda::id::Id::parse(id, "board").unwrap();
        let c = rayon::ThreadPoolBuilder::new()
            .num_threads(threads)
            .build()
            .unwrap()
            .install(|| ryueda::board::drc::check(&p, &p.boards[&bid]).unwrap());
        let layout = serde_json::json!({
            "items": c.conn.built.layout.items.iter().map(|(k, it)| serde_json::json!([c.conn.built.reference(k), it.layer, it.net])).collect::<Vec<_>>(),
            "holes": c.conn.built.layout.holes.iter().map(|(k, h)| serde_json::json!([c.conn.built.reference(k), h.plated, [h.span.0, h.span.1]])).collect::<Vec<_>>(),
        });
        let islands: Vec<Vec<String>> = c
            .conn
            .copper
            .islands()
            .iter()
            .map(|i| i.items.iter().map(|k| c.conn.built.reference(k)).collect())
            .collect();
        let got = (
            show,
            hash("in1.svg"),
            drc,
            layout,
            serde_json::json!(islands),
            exported,
        );
        match &first {
            None => first = Some(got),
            Some(one) => assert_eq!(&got, one, "one thread and eight"),
        }
    }
    let (show, svg, drc, layout, islands, exported) = first.unwrap();
    golden.json("board.four.show", &show);
    golden.got.insert("board.four.in1.svg".into(), svg);
    golden.json("board.four.drc", &drc);
    golden.json("board.four.layout", &layout);
    golden.json("board.four.islands", &islands);
    for (name, hash) in exported {
        golden.got.insert(format!("board.four.export.{name}"), hash);
    }
}

/// Board rules (ryueda 7.B): the round's board with a power class, a
/// board-wide clearance of 0.3 mm (which some of its gaps don't meet) and
/// a wider track for the class, each error then waived with a fixed
/// reason: the rules shown, DRC, a measure and the export's manifest,
/// alike on one thread and eight. Every ID is pinned.
fn rules(golden: &mut Golden, at: &Path) {
    let id = breakout_board::BOARD;
    let mut first: Option<(Value, Value, Value, String)> = None;
    for threads in [1, 8] {
        let board = at.join(format!("t{threads}")).join("board");
        std::fs::create_dir_all(&board).unwrap();
        ryueda(&board, 1, &["init"]);
        breakout_at(&board);
        for args in breakout_board::script() {
            let args: Vec<&str> = args.iter().map(String::as_str).collect();
            ryueda(&board, threads, &args);
        }
        for args in [
            &["class", "add", "Power", "--id", "power"][..],
            &["rail", "edit", "v3v3", "--class", "power"],
            &["board", "rules", "set", id, "clearance=0.3mm"],
            &[
                "board",
                "rules",
                "set",
                id,
                "--class",
                "power",
                "track_width=0.3mm",
            ],
        ] {
            ryueda(&board, threads, args);
        }
        let report = ryueda_out(&board, threads, &["board", "drc", id], &[]);
        let errors: Vec<String> = report["error"]["details"]["report"]["findings"]
            .as_array()
            .unwrap_or_else(|| panic!("errors under the rules: {report}"))
            .iter()
            .filter(|f| f["severity"] == "error")
            .map(|f| f["id"].as_str().unwrap().to_owned())
            .collect();
        assert!(!errors.is_empty());
        for f in &errors {
            ryueda(
                &board,
                threads,
                &["board", "waive", id, f, "--reason", "checked by hand"],
            );
        }
        let strip = |mut v: Value| {
            if let Some(o) = v.as_object_mut() {
                o.remove("revision");
            }
            v
        };
        let show = strip(ryueda(&board, threads, &["board", "rules", "show", id]));
        let drc = strip(ryueda(&board, threads, &["board", "drc", id]));
        assert_eq!(drc["errors"], 0, "{drc}");
        let measure = strip(ryueda(
            &board,
            threads,
            &["board", "measure", id, "R1/1", "R1/2"],
        ));
        ryueda(&board, threads, &["board", "export", id, "--out", "fab"]);
        let manifest = export_hash(
            "manifest.json",
            &std::fs::read(board.join("fab").join("manifest.json")).unwrap(),
        );
        let got = (show, drc, measure, manifest);
        match &first {
            None => first = Some(got),
            Some(one) => assert_eq!(&got, one, "one thread and eight"),
        }
    }
    let (show, drc, measure, manifest) = first.unwrap();
    golden.json("board.rules.show", &show);
    golden.json("board.rules.drc", &drc);
    golden.json("board.rules.measure", &measure);
    golden
        .got
        .insert("board.rules.export.manifest.json".into(), manifest);
}

/// Placement, keepouts and drawings (ryueda 7.C): the round's board with
/// R2 placed by the legaliser near R1/2 (grouped, its designator moved),
/// a keepout in a corner, text on the bottom silk and a fab line: the
/// place answer, DRC, the pads, a render of the bottom silk and every
/// exported file, alike on one thread and eight. Every ID is pinned.
fn placed(golden: &mut Golden, at: &Path) {
    type Got = (Value, Value, Value, String, BTreeMap<String, String>);
    let id = breakout_board::BOARD;
    let mut first: Option<Got> = None;
    for threads in [1, 8] {
        let board = at.join(format!("t{threads}")).join("board");
        std::fs::create_dir_all(&board).unwrap();
        ryueda(&board, 1, &["init"]);
        breakout_at(&board);
        for args in breakout_board::script() {
            let args: Vec<&str> = args.iter().map(String::as_str).collect();
            ryueda(&board, threads, &args);
        }
        let strip = |mut v: Value| {
            if let Some(o) = v.as_object_mut() {
                o.remove("revision");
                o.remove("schema");
            }
            v
        };
        let legalised = strip(ryueda(
            &board,
            threads,
            &[
                "board", "place", id, "R2", "--near", "R1/2", "--within", "3mm", "--group", "pull",
                "--ref-at", "0,1.2",
            ],
        ));
        ryueda(
            &board,
            threads,
            &[
                "board",
                "keepout",
                "add",
                id,
                "--rect",
                "15.5,0.2,16.3,1",
                "--keep",
                "vias",
                "--id",
                "k1",
            ],
        );
        ryueda(
            &board,
            threads,
            &[
                "board",
                "text",
                "add",
                id,
                "--layer",
                "silk.bottom",
                "--at",
                "8,1",
                "--text",
                "BACK",
                "--id",
                "back",
            ],
        );
        let line = serde_json::json!([{"layer": "fab.top", "type": "line", "start": ["1", "10"],
            "end": ["3", "10"], "width_nm": 100_000, "id": "f1"}])
        .to_string();
        ryueda(
            &board,
            threads,
            &["board", "drawing", "add", id, "--ops", &line],
        );
        let mut drc = ryueda_out(&board, threads, &["board", "drc", id], &[]);
        if let Some(o) = drc["result"].as_object_mut() {
            o.remove("revision");
        }
        if let Some(o) = drc["error"]["details"]["report"].as_object_mut() {
            o.remove("revision");
        }
        let show = strip(ryueda(&board, threads, &["board", "show", id, "--pads"]));
        ryueda(
            &board,
            threads,
            &[
                "board",
                "render",
                id,
                "--out",
                "back.svg",
                "--layer",
                "silk.bottom",
                "--layer",
                "outline",
                "--side",
                "bottom",
            ],
        );
        let svg =
            blake3::hash(&std::fs::read(board.join("back.svg")).unwrap()).to_hex()[..32].to_owned();
        ryueda(
            &board,
            threads,
            &["board", "export", id, "--out", "fab", "--allow-errors"],
        );
        let mut exported = BTreeMap::new();
        for e in std::fs::read_dir(board.join("fab")).unwrap().flatten() {
            let name = e.file_name().to_string_lossy().into_owned();
            exported.insert(
                name.clone(),
                export_hash(&name, &std::fs::read(e.path()).unwrap()),
            );
        }
        let got = (legalised, drc, show, svg, exported);
        match &first {
            None => first = Some(got),
            Some(one) => assert_eq!(&got, one, "one thread and eight"),
        }
    }
    let (legalised, drc, show, svg, exported) = first.unwrap();
    golden.json("board.place.legalised", &legalised);
    golden.json("board.place.drc", &drc);
    golden.json("board.place.show", &show);
    golden.got.insert("board.place.silk.bottom.svg".into(), svg);
    for (name, hash) in exported {
        golden
            .got
            .insert(format!("board.place.export.{name}"), hash);
    }
}

/// Pours (ryueda 7.D): the round's board with a whole `bottom` ground
/// plane and a `top` pour of `rail:v3v3` round U1 over it in priority:
/// `board show --pours`, DRC, a render of the bottom copper and every
/// exported file, alike on one thread and eight. Every ID is pinned.
fn pours(golden: &mut Golden, at: &Path) {
    type Got = (Value, Value, String, BTreeMap<String, String>, Value);
    let id = breakout_board::BOARD;
    let mut first: Option<Got> = None;
    for threads in [1, 8] {
        let board = at.join(format!("t{threads}")).join("board");
        std::fs::create_dir_all(&board).unwrap();
        ryueda(&board, 1, &["init"]);
        breakout_at(&board);
        for args in breakout_board::script() {
            let args: Vec<&str> = args.iter().map(String::as_str).collect();
            ryueda(&board, threads, &args);
        }
        let strip = |mut v: Value| {
            if let Some(o) = v.as_object_mut() {
                o.remove("revision");
                o.remove("schema");
            }
            v
        };
        ryueda(
            &board,
            threads,
            &[
                "board", "pour", "add", id, "--net", "rail:gnd", "--layer", "bottom", "--whole",
                "--id", "gnd",
            ],
        );
        ryueda(
            &board,
            threads,
            &[
                "board",
                "pour",
                "add",
                id,
                "--net",
                "rail:v3v3",
                "--layer",
                "top",
                "--rect",
                "10,5.5,14,9.5",
                "--priority",
                "1",
                "--id",
                "v33",
            ],
        );
        let show = strip(ryueda(&board, threads, &["board", "show", id, "--pours"]));
        let mut drc = ryueda_out(&board, threads, &["board", "drc", id], &[]);
        if let Some(o) = drc["result"].as_object_mut() {
            o.remove("revision");
        }
        if let Some(o) = drc["error"]["details"]["report"].as_object_mut() {
            o.remove("revision");
        }
        ryueda(
            &board,
            threads,
            &[
                "board",
                "render",
                id,
                "--out",
                "bottom.svg",
                "--layer",
                "copper.bottom",
                "--layer",
                "outline",
                "--side",
                "bottom",
            ],
        );
        let svg = blake3::hash(&std::fs::read(board.join("bottom.svg")).unwrap()).to_hex()[..32]
            .to_owned();
        ryueda(
            &board,
            threads,
            &["board", "export", id, "--out", "fab", "--allow-errors"],
        );
        let mut exported = BTreeMap::new();
        for e in std::fs::read_dir(board.join("fab")).unwrap().flatten() {
            let name = e.file_name().to_string_lossy().into_owned();
            exported.insert(
                name.clone(),
                export_hash(&name, &std::fs::read(e.path()).unwrap()),
            );
        }
        // The ground net opened (ryueda 7.E): its next connections.
        ryueda(&board, threads, &["board", "track", "delete", id, "gnd_c1"]);
        let ratsnest = strip(ryueda(&board, threads, &["board", "ratsnest", id]));
        let got = (show, drc, svg, exported, ratsnest);
        match &first {
            None => first = Some(got),
            Some(one) => assert_eq!(&got, one, "one thread and eight"),
        }
    }
    let (show, drc, svg, exported, ratsnest) = first.unwrap();
    golden.json("board.pour.ratsnest", &ratsnest);
    golden.json("board.pour.show", &show);
    golden.json("board.pour.drc", &drc);
    golden.got.insert("board.pour.bottom.svg".into(), svg);
    for (name, hash) in exported {
        golden.got.insert(format!("board.pour.export.{name}"), hash);
    }
}

/// What makes the breakout's board four layers: the stackup, and a track
/// on `in1` between two of its ground vias.
const FOUR: &[&[&str]] = &[
    &["board", "edit", breakout_board::BOARD, "--layers", "4"],
    &[
        "board",
        "track",
        "add",
        breakout_board::BOARD,
        "--net",
        "rail:gnd",
        "--layer",
        "in1",
        "--path",
        "via/g_u2",
        "via/g_c3",
        "--id",
        "gnd_in1",
    ],
];

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
    breakout_at(&board);
    for what in ["netlist", "erc"] {
        let one = ryueda(&board, 1, &[what]);
        assert_eq!(ryueda(&board, 8, &[what]), one, "{what}");
        golden.json(&format!("breakout.{what}"), &one);
    }
    pdm(&mut golden, &dir.path().join("pdm").join("board"));
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
    compared(&mut golden, &board);
    let repo = dir.path().join("merge");
    std::fs::create_dir_all(&repo).unwrap();
    merged(&mut golden, &repo);
    let repo = dir.path().join("clashes").join("repo");
    std::fs::create_dir_all(&repo).unwrap();
    clashes(&mut golden, &repo);
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
        if name == "buck" {
            golden.json("pdm.buck.bom", &ryueda(&at, 1, &["bom"]));
        }
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
        ryueda(
            &p,
            1,
            &[
                "symbol",
                "create",
                "opamp",
                "--template",
                "opamp",
                "--units",
                "2",
                "--pins",
                OPAMP_PINS,
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
    boards(&mut golden, &dir.path().join("boards"));
    four_layers(&mut golden, &dir.path().join("four"));
    rules(&mut golden, &dir.path().join("rules"));
    placed(&mut golden, &dir.path().join("place"));
    pours(&mut golden, &dir.path().join("pours"));
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
        // Each sheet as a window draws it (plan 3.B), light and dark,
        // alike on one thread and eight. Its revisions hash the manifest,
        // whose project ID `init` draws at random, so they are left out.
        let first = ryueda(dir, 1, &["scene", "--block", &b]);
        let sheets: Vec<String> = first["sheets"]
            .as_array()
            .unwrap()
            .iter()
            .map(|s| s["sheet"].as_str().unwrap().to_owned())
            .collect();
        for s in &sheets {
            for theme in ["light", "dark"] {
                let args = [
                    "scene",
                    "--block",
                    b.as_str(),
                    "--sheet",
                    s.as_str(),
                    "--theme",
                    theme,
                ];
                let mut one = ryueda(dir, 1, &args);
                assert_eq!(ryueda(dir, 8, &args), one, "{name} {b} {s} {theme}");
                let o = one.as_object_mut().unwrap();
                o.remove("revision");
                o.remove("revisions");
                golden.json(&format!("schematic.{name}.{b}.{s}.scene.{theme}"), &one);
            }
        }
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
    // Its own repository, so the install lands there and never in one
    // above the temp folder (#1004).
    assert!(
        ryu_git::git_env::isolated()
            .args(["init", "-q"])
            .arg(&root)
            .status()
            .unwrap()
            .success()
    );
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

/// A dual op-amp, for the `opamp` template's golden (#758).
const OPAMP_PINS: &str = r#"[
    {"number": "2", "name": "-", "type": "input", "unit": 1},
    {"number": "3", "name": "+", "type": "input", "unit": 1},
    {"number": "1", "name": "~", "type": "output", "unit": 1},
    {"number": "6", "name": "-", "type": "input", "unit": 2},
    {"number": "5", "name": "+", "type": "input", "unit": 2},
    {"number": "7", "name": "~", "type": "output", "unit": 2},
    {"number": "8", "name": "V+", "type": "power_in"},
    {"number": "4", "name": "V-", "type": "power_in"}]"#;

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

/// Git in `dir`, reading no user's config, with a fixed identity and
/// dates: whether it succeeded, and what it said on stderr.
fn git_try(dir: &Path, args: &[&str]) -> (bool, String) {
    let out = std::process::Command::new("git")
        .env("GIT_CONFIG_GLOBAL", dir.join("..").join("no-gitconfig"))
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .env("GIT_AUTHOR_NAME", "Test")
        .env("GIT_AUTHOR_EMAIL", "t@example.com")
        .env("GIT_COMMITTER_NAME", "Test")
        .env("GIT_COMMITTER_EMAIL", "t@example.com")
        .env("GIT_AUTHOR_DATE", "2026-01-01T00:00:00Z")
        .env("GIT_COMMITTER_DATE", "2026-01-01T00:00:00Z")
        .args([
            "-c",
            "commit.gpgsign=false",
            "-c",
            "init.defaultBranch=main",
            "-c",
            "core.autocrlf=false",
        ])
        .args(args)
        .current_dir(dir)
        .output()
        .unwrap();
    (
        out.status.success(),
        String::from_utf8_lossy(&out.stderr).into_owned(),
    )
}

/// [`git_try`], which must succeed.
fn git(dir: &Path, args: &[&str]) {
    let (ok, said) = git_try(dir, args);
    assert!(ok, "git {args:?}: {said}");
}

/// The styled breakout committed, then edited as `tests/git.rs`'s
/// `edited_breakout` edits it (plan 4.B): its `diff`, `diff --netlist` and
/// `diff --layout` (the commit, which the manifest's random project ID
/// moves, left out), each block's compare SVG and the compare PDF, alike
/// on one thread and eight.
fn compared(golden: &mut Golden, dir: &Path) {
    git(dir, &["init", "-q"]);
    git(dir, &["add", "."]);
    git(dir, &["commit", "-q", "-m", "styled"]);
    for args in [
        vec!["component", "edit", "breakout/r1", "--value", "10k"],
        vec![
            "component",
            "add",
            "breakout",
            "--of",
            "r_4k7",
            "--id",
            "r3",
        ],
        vec!["connect", "breakout/r3/1", "SDA"],
        vec!["connect", "breakout/r3/2", "rail:3V3"],
        vec!["component", "delete", "breakout/c3"],
        vec!["net", "rename", "breakout/scl", "I2C_SCL"],
        vec!["place", "breakout", "--move", "shift J1 right 4"],
    ] {
        ryueda(dir, 1, &args);
    }
    let strip = |mut v: Value| {
        v["from"]["commit"] = Value::Null;
        v
    };
    for (name, args) in [
        ("diff.breakout", vec!["diff"]),
        ("diff.breakout.netlist", vec!["diff", "--netlist"]),
        ("diff.breakout.layout", vec!["diff", "--layout"]),
    ] {
        let one = strip(ryueda(dir, 1, &args));
        assert_eq!(strip(ryueda(dir, 8, &args)), one, "{name}");
        golden.json(name, &one);
    }
    for b in blocks_of(dir) {
        let args = [
            "render",
            "--block",
            &b,
            "--compare",
            "HEAD",
            "--out",
            "c.svg",
        ];
        ryueda(dir, 1, &args);
        let svg = std::fs::read(dir.join("c.svg")).unwrap();
        ryueda(dir, 8, &args);
        assert_eq!(std::fs::read(dir.join("c.svg")).unwrap(), svg, "{b}");
        golden.got.insert(
            format!("schematic.breakout.compare.{b}.svg"),
            blake3::hash(&svg).to_hex()[..32].to_owned(),
        );
    }
    // Each block's sheets as a window draws them against HEAD (plan
    // 13.A), and as HEAD draws them, light and dark, alike on one thread
    // and eight. The commit (the manifest's random project ID moves it)
    // and the revisions are left out.
    for b in blocks_of(dir) {
        let first = ryueda(dir, 1, &["scene", "--block", &b, "--compare", "HEAD"]);
        let sheets: Vec<(String, bool)> = first["sheets"]
            .as_array()
            .unwrap()
            .iter()
            .map(|s| {
                (
                    s["sheet"].as_str().unwrap().to_owned(),
                    s["removed"] == true,
                )
            })
            .collect();
        for (s, removed) in &sheets {
            for theme in ["light", "dark"] {
                for (what, flag) in [("compare", "--compare"), ("rev", "--rev")] {
                    if *removed && what == "compare" {
                        continue;
                    }
                    let args = [
                        "scene",
                        "--block",
                        b.as_str(),
                        "--sheet",
                        s.as_str(),
                        "--theme",
                        theme,
                        flag,
                        "HEAD",
                    ];
                    let mut one = ryueda(dir, 1, &args);
                    assert_eq!(ryueda(dir, 8, &args), one, "{b} {s} {theme} {what}");
                    let o = one.as_object_mut().unwrap();
                    o.remove("revision");
                    o.remove("revisions");
                    o.remove("commit");
                    if let Some(a) = o.get_mut("against") {
                        a["commit"] = Value::Null;
                    }
                    golden.json(
                        &format!("schematic.breakout.{what}.{b}.{s}.scene.{theme}"),
                        &one,
                    );
                }
            }
        }
    }
    ryueda(dir, 1, &["output", "--compare", "HEAD", "--out", "c.pdf"]);
    let pdf = std::fs::read(dir.join("c.pdf")).unwrap();
    golden.got.insert(
        "schematic.breakout.compare.pdf".to_owned(),
        blake3::hash(&pdf).to_hex()[..32].to_owned(),
    );
}

/// Every file under `dir` but its manifest (whose project ID is random),
/// by its path from there, in path order.
fn files_of(dir: &Path) -> BTreeMap<String, Vec<u8>> {
    let mut out = BTreeMap::new();
    let mut stack = vec![dir.to_path_buf()];
    while let Some(d) = stack.pop() {
        for e in std::fs::read_dir(&d).unwrap() {
            let e = e.unwrap();
            if e.file_name() == ".git" {
                continue;
            }
            if e.file_type().unwrap().is_dir() {
                stack.push(e.path());
            } else {
                let rel = e
                    .path()
                    .strip_prefix(dir)
                    .unwrap()
                    .to_string_lossy()
                    .replace('\\', "/");
                if rel != "ryueda.json" {
                    out.insert(rel, std::fs::read(e.path()).unwrap());
                }
            }
        }
    }
    out
}

/// Semantic clashes on the breakout (plan 4.D): each side adds a
/// resistor as R5 and a net AUX, ours joins SDA into SCL and theirs hangs
/// its resistor on SDA. The status (commits left out) and the merged
/// files after every item is settled by its first repair, alike on one
/// thread and eight.
fn clashes(golden: &mut Golden, repo: &Path) {
    git(repo, &["init", "-q"]);
    let board = repo.join("board");
    std::fs::create_dir_all(&board).unwrap();
    ryueda(&board, 1, &["init"]);
    breakout_at(&board);
    git(repo, &["add", "-A"]);
    git(repo, &["commit", "-q", "-m", "base"]);
    git(repo, &["checkout", "-q", "-b", "theirs"]);
    for args in [
        vec![
            "component",
            "add",
            "breakout",
            "--of",
            "r_4k7",
            "--id",
            "r9",
            "--refdes",
            "R5",
        ],
        vec!["net", "add", "breakout", "AUX", "--id", "aux_t"],
        vec!["connect", "breakout/r9/1", "AUX"],
        vec!["connect", "breakout/r9/2", "SDA"],
    ] {
        ryueda(&board, 1, &args);
    }
    git(repo, &["add", "-A"]);
    git(repo, &["commit", "-q", "-m", "theirs"]);
    git(repo, &["checkout", "-q", "main"]);
    for args in [
        vec![
            "component",
            "add",
            "breakout",
            "--of",
            "r_4k7",
            "--id",
            "r8",
            "--refdes",
            "R5",
        ],
        vec!["net", "add", "breakout", "AUX", "--id", "aux_o"],
        vec!["net", "join", "breakout/SDA", "SCL"],
    ] {
        ryueda(&board, 1, &args);
    }
    git(repo, &["add", "-A"]);
    git(repo, &["commit", "-q", "-m", "ours"]);
    let run = |threads: usize| {
        let _ = git_try(repo, &["merge", "--no-commit", "-q", "theirs"]);
        let mut status = ryueda(&board, threads, &["merge", "status"]);
        for side in ["ours", "theirs", "base"] {
            status[side]["commit"] = Value::Null;
        }
        let mut settled = false;
        for _ in 0..8 {
            let now = ryueda(&board, threads, &["merge", "status"]);
            let Some(c) = now["conflicts"]
                .as_array()
                .unwrap()
                .iter()
                .find(|c| c["repairs"].as_array().is_some_and(|r| !r.is_empty()))
                .cloned()
            else {
                settled = true;
                break;
            };
            let repair = c["repairs"][0]["id"].as_str().unwrap().to_owned();
            ryueda(
                &board,
                threads,
                &[
                    "merge",
                    "resolve",
                    c["id"].as_str().unwrap(),
                    "--repair",
                    &repair,
                ],
            );
        }
        assert!(
            settled,
            "not settled in eight rounds: {}",
            ryueda(&board, threads, &["merge", "status"])["conflicts"]
        );
        let mut h = blake3::Hasher::new();
        for (path, bytes) in files_of(&board) {
            h.update(path.as_bytes());
            h.update(&bytes);
        }
        // The repairs changed files the merge staged, which `merge
        // --abort` refuses to reset: a hard reset ends the merge as well.
        git(repo, &["reset", "-q", "--hard"]);
        (status, h.finalize().to_hex()[..32].to_owned())
    };
    let one = run(1);
    assert_eq!(run(8), one, "one thread and eight");
    golden.json("merge.breakout.clashes", &one.0);
    golden
        .got
        .insert("merge.breakout.clashes.files".to_owned(), one.1);
}

/// The breakout laid out on two branches that add a capacitor each in
/// the same free space (as `tests/merge.rs`'s `laid_out_branches`),
/// merged: `merge status` (the commits, which the manifest's random
/// project ID moves, left out) and the layout `relayout` writes, alike on
/// one thread and eight.
fn merged(golden: &mut Golden, repo: &Path) {
    git(repo, &["init", "-q"]);
    let board = repo.join("board");
    std::fs::create_dir_all(&board).unwrap();
    ryueda(&board, 1, &["init"]);
    breakout_at(&board);
    ryueda(&board, 1, &["layout"]);
    git(repo, &["add", "-A"]);
    git(repo, &["commit", "-q", "-m", "laid out"]);
    git(repo, &["checkout", "-q", "-b", "theirs"]);
    for args in [
        vec![
            "component",
            "add",
            "breakout",
            "--of",
            "c_100n",
            "--id",
            "c97",
            "--refdes",
            "C97",
        ],
        vec!["connect", "breakout/c97/1", "SCL"],
        vec!["component", "edit", "breakout/r1", "--value", "47k"],
    ] {
        ryueda(&board, 1, &args);
    }
    git(repo, &["add", "-A"]);
    git(repo, &["commit", "-q", "-m", "theirs"]);
    git(repo, &["checkout", "-q", "main"]);
    for args in [
        vec![
            "component",
            "add",
            "breakout",
            "--of",
            "c_100n",
            "--id",
            "c98",
            "--refdes",
            "C98",
        ],
        vec!["connect", "breakout/c98/1", "SCL"],
    ] {
        ryueda(&board, 1, &args);
    }
    git(repo, &["add", "-A"]);
    git(repo, &["commit", "-q", "-m", "ours"]);
    let run = |threads: usize| {
        let (merged, _) = git_try(repo, &["merge", "-q", "theirs"]);
        assert!(!merged, "both sides changed the layout");
        let mut status = ryueda(&board, threads, &["merge", "status"]);
        for side in ["ours", "theirs", "base"] {
            status[side]["commit"] = Value::Null;
        }
        let areas: Vec<String> = status["conflicts"]
            .as_array()
            .unwrap()
            .iter()
            .filter(|c| c["kind"] == "layout_conflict")
            .map(|c| c["id"].as_str().unwrap().to_owned())
            .collect();
        let mut args = vec!["merge", "resolve"];
        args.extend(areas.iter().map(String::as_str));
        args.extend(["--repair", "relayout"]);
        ryueda(&board, threads, &args);
        let layout = std::fs::read(board.join("blocks/breakout/layout.json")).unwrap();
        git(repo, &["merge", "--abort"]);
        (status, layout)
    };
    let one = run(1);
    assert_eq!(run(8), one, "one thread and eight");
    golden.json("merge.breakout.status", &one.0);
    golden.got.insert(
        "merge.breakout.layout".to_owned(),
        blake3::hash(&one.1).to_hex()[..32].to_owned(),
    );
}
