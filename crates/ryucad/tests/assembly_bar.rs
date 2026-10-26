//! Design 72's bars for assemblies at scale: 1,000 pins of 19 sizes on a
//! fixed plate (1,001 occurrences of 20 parts), each held by one of
//! 1,000 mates (fastened, revolute, revolute driven to 30°). The
//! assembly's rebuild over parts already built solves in under 0.1 s
//! when its poses hold and under 1 s from poses jostled about a degree
//! and a tenth of a millimetre off; and a daemon started cold over a warm
//! disk cache answers `rebuild` for it in under 2 s, writing nothing new.
//! Each prints the machine it ran on. Release builds only.
//!
//! The bars are design 72's on the machine they are recorded on: with
//! `RYUCAD_BARS=1` they are asserted as they are. CI's runners differ
//! from run to run, so there they are asserted ten times looser, which
//! still catches a cost that grows out of hand.

use assert_cmd::Command;
use ryucad::doc::id::Id;
use ryucad::proto::Request;
use ryucad::server::handle;
use ryucad::session::Session;
use ryucad::units::Unit;
use ryukei_testkit::Lcg;
use serde_json::{Value, json};
use std::path::Path;
use std::time::Instant;

const PINS: usize = 1000;
const SIZES: usize = 19;

fn must(s: &mut Session, method: &str, params: Value) -> Value {
    handle(s, Request::from_rpc(method, Some(params)).unwrap())
        .unwrap_or_else(|e| panic!("{method}: {e}"))
}

fn id(s: &str) -> Id {
    Id::parse(s, "part").unwrap()
}

/// The machine, for the record: OS, architecture, cores and, where the
/// system says, the CPU's name.
fn machine() -> String {
    let cpu = std::fs::read_to_string("/proc/cpuinfo")
        .ok()
        .and_then(|c| {
            c.lines()
                .find(|l| l.starts_with("model name"))
                .and_then(|l| l.split(':').nth(1))
                .map(|m| m.trim().to_owned())
        })
        .unwrap_or_else(|| "cpu unknown".to_owned());
    format!(
        "{} {}, {} cores, {cpu}",
        std::env::consts::OS,
        std::env::consts::ARCH,
        std::thread::available_parallelism().map_or(0, |n| n.get()),
    )
}

/// How much slack the bars get: none where they are being recorded
/// (`RYUCAD_BARS` set), else ten times.
fn slack() -> f64 {
    if std::env::var_os("RYUCAD_BARS").is_some() {
        1.0
    } else {
        10.0
    }
}

/// The name of the planar face of `part`'s solid `solid` whose box is
/// flat at height `z` (m).
fn face_at(s: &mut Session, part: &str, solid: &str, z: f64) -> String {
    let r = s.rebuilt(&id(part)).unwrap().clone();
    let (_, last) = r.features.last().unwrap();
    let b = last
        .as_ref()
        .unwrap()
        .solid(&Id::parse(solid, "feature").unwrap())
        .unwrap();
    let f = b
        .body
        .faces()
        .find(|f| {
            let bx = ryukei::intersect::face_box(&b.body, *f);
            (bx.min().z - z).abs() < 1e-9 && (bx.max().z - z).abs() < 1e-9
        })
        .unwrap();
    b.body.face(f).unwrap().name.to_string()
}

/// Where pin `i` stands: a 40 × 25 grid 20 mm apart, in metres.
fn spot(i: usize) -> [f64; 2] {
    [0.01 + 0.02 * (i % 40) as f64, 0.01 + 0.02 * (i / 40) as f64]
}

/// Writes canonical JSON to `rel` under `root`; plainly, not as ryucad
/// writes (atomically, synced), which over 2,000 files costs minutes on a
/// busy disk.
fn write(root: &Path, rel: &str, v: &Value) {
    let path = rel.split('/').fold(root.to_path_buf(), |p, s| p.join(s));
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(&path, ryucad::doc::canonical::to_string(v)).unwrap();
}

/// Pin `i`'s instance file, at its spot on the plate's top, turned and
/// moved off by `off` (radians about z, metres in x and y).
fn pin(i: usize, off: [f64; 3]) -> Value {
    let [x, y] = spot(i);
    let (c, s) = (ryukei::math::cos(off[0]), ryukei::math::sin(off[0]));
    json!({"format": 4, "kind": "ryucad.feature", "type": "instance",
           "part": format!("pin{}", i % SIZES), "rev": "working",
           "pose": {"origin_m": [x + off[1], y + off[2], 0.01], "z": [0, 0, 1], "x": [c, s, 0]}})
}

/// The project in `root`: parts `plate` (820 × 520 × 10 mm) and `pin0`
/// to `pin18` (radius 2 to 3.8 mm, 30 mm long), and assembly `asm` with
/// the plate fixed, 1,000 pins and a mate for each, its poses holding.
fn project(root: &Path) -> Session {
    ryucad::server::init(root, Some(Unit::Mm)).unwrap();
    let mut s = Session::open(root).unwrap();
    must(&mut s, "part.create", json!({"part": "plate"}));
    must(
        &mut s,
        "feature.add",
        json!({"part": "plate", "type": "box", "id": "slab",
               "params": {"x": 820, "y": 520, "z": 10}}),
    );
    for k in 0..SIZES {
        let p = format!("pin{k}");
        must(&mut s, "part.create", json!({"part": p}));
        must(
            &mut s,
            "feature.add",
            json!({"part": p, "type": "cylinder", "id": "shank",
                   "params": {"radius": 2.0 + 0.1 * k as f64, "height": 30}}),
        );
    }
    let top = face_at(&mut s, "plate", "slab", 0.01);
    let feet: Vec<String> = (0..SIZES)
        .map(|k| face_at(&mut s, &format!("pin{k}"), "shank", 0.0))
        .collect();
    must(
        &mut s,
        "part.create",
        json!({"part": "asm", "kind": "assembly"}),
    );
    let mut list = vec![json!("base")];
    write(
        root,
        "parts/asm/features/base.json",
        &json!({"format": 4, "kind": "ryucad.feature", "type": "instance",
                "part": "plate", "rev": "working", "fixed": true,
                "pose": {"origin_m": [0, 0, 0], "z": [0, 0, 1], "x": [1, 0, 0]}}),
    );
    for i in 0..PINS {
        let [x, y] = spot(i);
        // Fastened, free to turn, or turned to 30°: a quarter of each
        // turned is solved into place below.
        let (form, angle) = match i % 4 {
            0 | 1 => ("fastened", None),
            2 => ("revolute", None),
            _ => ("revolute", Some("30 deg")),
        };
        let mut m = json!({"format": 4, "kind": "ryucad.feature", "type": "mate", "form": form,
            "a": {"instance": ["base"], "at": {"feature": "slab", "name": top},
                  "offset": {"origin": [format!("{} mm", x * 1e3), format!("{} mm", y * 1e3), "0 mm"]}},
            "b": {"instance": [format!("p{i}")], "at": {"feature": "shank", "name": feet[i % SIZES]},
                  "offset": {"flip": true}}});
        if let Some(a) = angle {
            m["angle"] = json!(a);
        }
        write(
            root,
            &format!("parts/asm/features/p{i}.json"),
            &pin(i, [0.0; 3]),
        );
        write(root, &format!("parts/asm/features/m{i}.json"), &m);
        list.push(json!(format!("p{i}")));
        list.push(json!(format!("m{i}")));
    }
    write(
        root,
        "parts/asm/part.json",
        &json!({"format": 2, "kind": "ryucad.part", "part_kind": "assembly", "features": list}),
    );
    s.refresh().unwrap();
    // The driven quarter turned where the solve puts them.
    let solved = must(&mut s, "assembly.solve", json!({"part": "asm"}));
    for (inst, at) in solved["instances"].as_object().unwrap() {
        if at["moved"] == true {
            let rel = format!("parts/asm/features/{inst}.json");
            let mut v: Value =
                serde_json::from_str(&std::fs::read_to_string(root.join(&rel)).unwrap()).unwrap();
            v["pose"] = at["pose"].clone();
            write(root, &rel, &v);
        }
    }
    s.refresh().unwrap();
    s
}

/// The best of three runs of the assembly's rebuild over its parts as
/// `s` has them built, in milliseconds, and the last result's warnings.
fn assembly_ms(s: &mut Session) -> (f64, Vec<Value>) {
    let part = s.project().part(&id("asm")).unwrap().clone();
    let mut fastest = f64::INFINITY;
    let mut warnings = Vec::new();
    for _ in 0..3 {
        let t = Instant::now();
        let r = s.rebuilt_as(part.clone()).unwrap();
        fastest = fastest.min(t.elapsed().as_secs_f64() * 1e3);
        assert_eq!(
            r.failed(),
            0,
            "{:?}",
            r.features.iter().find(|(_, r)| r.is_err())
        );
        assert!(r.solve.as_ref().unwrap().report.converged);
        warnings = r.warnings(&id("asm"));
    }
    (fastest, warnings)
}

#[test]
#[cfg_attr(debug_assertions, ignore = "minutes in a debug build")]
fn a_thousand_mates_solve_held_and_jostled() {
    let dir = tempfile::tempdir().unwrap();
    let mut s = project(dir.path());
    let built = s.rebuilt(&id("asm")).unwrap();
    assert_eq!(
        built.warnings(&id("asm")),
        Vec::<Value>::new(),
        "the poses hold"
    );
    let (held, warnings) = assembly_ms(&mut s);
    assert!(warnings.is_empty(), "{warnings:?}");
    // Each pin about a degree and a tenth of a millimetre off.
    let mut g = Lcg::new(72);
    for i in 0..PINS {
        let mut v = pin(i, [0.0; 3]);
        let path = format!("parts/asm/features/p{i}.json");
        let was: Value =
            serde_json::from_str(&std::fs::read_to_string(dir.path().join(&path)).unwrap())
                .unwrap();
        v["pose"] = was["pose"].clone();
        let off = |g: &mut Lcg, d: f64| g.next_f64(-d, d);
        for k in 0..2 {
            let o = v["pose"]["origin_m"][k].as_f64().unwrap();
            v["pose"]["origin_m"][k] = json!(o + off(&mut g, 1e-4));
        }
        let z = [off(&mut g, 0.0175), off(&mut g, 0.0175), 1.0];
        v["pose"]["z"] = json!(z);
        write(dir.path(), &path, &v);
    }
    s.refresh().unwrap();
    let (jostled, warnings) = assembly_ms(&mut s);
    assert!(
        warnings.iter().any(|w| w["code"] == "assembly_stale"),
        "{warnings:?}"
    );
    println!(
        "1,000 mates over 1,001 occurrences of 20 parts, on {}: held {held:.1} ms, jostled \
         {jostled:.1} ms",
        machine()
    );
    assert!(held < 100.0 * slack(), "held: {held:.1} ms");
    assert!(jostled < 1000.0 * slack(), "jostled: {jostled:.1} ms");
}

/// Every file under `dir`, for counting what a cache holds.
fn files(dir: &Path) -> usize {
    let mut n = 0;
    let mut todo = vec![dir.to_path_buf()];
    while let Some(d) = todo.pop() {
        for e in std::fs::read_dir(d).unwrap().flatten() {
            if e.file_type().unwrap().is_dir() {
                todo.push(e.path());
            } else {
                n += 1;
            }
        }
    }
    n
}

#[test]
#[cfg_attr(debug_assertions, ignore = "minutes in a debug build")]
fn a_cold_daemon_over_a_warm_cache_answers_within_two_seconds() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().join("p");
    std::fs::create_dir(&root).unwrap();
    drop(project(&root));
    let (run, cache) = (dir.path().join("run"), dir.path().join("cache"));
    let ryucad = |args: &[&str]| -> Value {
        let out = Command::cargo_bin("ryucad")
            .unwrap()
            .current_dir(&root)
            .args(args)
            .env("RYUCAD_RUNTIME_DIR", &run)
            .env("RYUCAD_CONFIG_DIR", dir.path().join("config"))
            .env("RYUCAD_CACHE_DIR", &cache)
            .env("RYUCAD_IDLE_TIMEOUT", "60")
            .output()
            .unwrap();
        serde_json::from_slice(&out.stdout)
            .unwrap_or_else(|_| panic!("{args:?}: {}", String::from_utf8_lossy(&out.stderr)))
    };
    // A daemon fills the cache, and stops.
    let t = Instant::now();
    let first = ryucad(&["rebuild", "asm"]);
    let cold = t.elapsed().as_secs_f64() * 1e3;
    assert_eq!(first["ok"], true, "{first}");
    ryucad(&["daemon", "stop"]);
    let entries = files(&cache);
    assert!(entries >= 20, "every part's body: {entries}");
    // A new one, over the warm cache.
    let t = Instant::now();
    let again = ryucad(&["rebuild", "asm"]);
    let warm = t.elapsed().as_secs_f64() * 1e3;
    assert_eq!(again["ok"], true, "{again}");
    assert_eq!(
        again["result"]["parts"][0]["hash"],
        first["result"]["parts"][0]["hash"]
    );
    assert_eq!(files(&cache), entries, "nothing built, so nothing written");
    // What a request that builds nothing costs with 2,000 feature files to
    // check for freshness (issue #25): the best of five.
    let mut request = f64::INFINITY;
    for _ in 0..5 {
        let t = Instant::now();
        assert_eq!(ryucad(&["part", "list"])["ok"], true);
        request = request.min(t.elapsed().as_secs_f64() * 1e3);
    }
    ryucad(&["daemon", "stop"]);
    println!(
        "a cold daemon's rebuild of 1,001 occurrences, on {}: {cold:.0} ms filling the cache, \
         {warm:.0} ms over it; then {request:.0} ms a `part list`",
        machine()
    );
    assert!(warm < 2000.0 * slack(), "warm: {warm:.0} ms");
}
