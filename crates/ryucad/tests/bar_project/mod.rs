//! The project design 72's bars and benchmarks build: a fixed plate and
//! 1,000 pins of 19 sizes on it (1,001 occurrences of 20 parts), each held
//! by one of 1,000 mates (fastened, revolute, revolute driven to 30°).
//! Shared by `tests/assembly_bar.rs` and `benches/assembly.rs`.

#![allow(dead_code)]

use ryucad::doc::id::Id;
use ryucad::proto::Request;
use ryucad::server::handle;
use ryucad::session::Session;
use ryucad::units::Unit;
use ryukei_testkit::Lcg;
use serde_json::{Value, json};
use std::path::Path;

pub const PINS: usize = 1000;
pub const SIZES: usize = 19;

pub fn must(s: &mut Session, method: &str, params: Value) -> Value {
    handle(s, Request::from_rpc(method, Some(params)).unwrap())
        .unwrap_or_else(|e| panic!("{method}: {e}"))
}

pub fn id(s: &str) -> Id {
    Id::parse(s, "part").unwrap()
}

/// The machine, for the record: OS, architecture, cores and, where the
/// system says, the CPU's name.
pub fn machine() -> String {
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

/// The name of the planar face of `part`'s solid `solid` whose box is
/// flat at height `z` (m).
pub fn face_at(s: &mut Session, part: &str, solid: &str, z: f64) -> String {
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
pub fn spot(i: usize) -> [f64; 2] {
    [0.01 + 0.02 * (i % 40) as f64, 0.01 + 0.02 * (i / 40) as f64]
}

/// Writes canonical JSON to `rel` under `root`; plainly, not as ryucad
/// writes (atomically, synced), which over 2,000 files costs minutes on a
/// busy disk.
pub fn write(root: &Path, rel: &str, v: &Value) {
    let path = rel.split('/').fold(root.to_path_buf(), |p, s| p.join(s));
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(&path, ryucad::doc::canonical::to_string(v)).unwrap();
}

/// Pin `i`'s instance file, at its spot on the plate's top, turned and
/// moved off by `off` (radians about z, metres in x and y).
pub fn pin(i: usize, off: [f64; 3]) -> Value {
    let [x, y] = spot(i);
    let (c, s) = (ryukei::math::cos(off[0]), ryukei::math::sin(off[0]));
    json!({"format": 4, "kind": "ryucad.feature", "type": "instance",
           "part": format!("pin{}", i % SIZES), "rev": "working",
           "pose": {"origin_m": [x + off[1], y + off[2], 0.01], "z": [0, 0, 1], "x": [c, s, 0]}})
}

/// `v`, an instance file, placing release `A` of another project's part.
fn foreign(mut v: Value, project: &str) -> Value {
    v["project"] = json!(project);
    v["rev"] = json!({"label": "A"});
    v["format"] = json!(6);
    v
}

/// The plate and the pins in `s`'s project, and the names of the plate's
/// top and each pin's foot.
fn parts(s: &mut Session) -> (String, Vec<String>) {
    must(s, "part.create", json!({"part": "plate"}));
    must(
        s,
        "feature.add",
        json!({"part": "plate", "type": "box", "id": "slab",
               "params": {"x": 820, "y": 520, "z": 10}}),
    );
    for k in 0..SIZES {
        let p = format!("pin{k}");
        must(s, "part.create", json!({"part": p}));
        must(
            s,
            "feature.add",
            json!({"part": p, "type": "cylinder", "id": "shank",
                   "params": {"radius": 2.0 + 0.1 * k as f64, "height": 30}}),
        );
    }
    let top = face_at(s, "plate", "slab", 0.01);
    let feet: Vec<String> = (0..SIZES)
        .map(|k| face_at(s, &format!("pin{k}"), "shank", 0.0))
        .collect();
    (top, feet)
}

/// The project in `root`: parts `plate` (820 × 520 × 10 mm) and `pin0`
/// to `pin18` (radius 2 to 3.8 mm, 30 mm long), and assembly `asm` with
/// the plate fixed, 1,000 pins and a mate for each, its poses holding.
pub fn project(root: &Path) -> Session {
    ryucad::server::init(root, Some(Unit::Mm)).unwrap();
    let mut s = Session::open(root).unwrap();
    let (top, feet) = parts(&mut s);
    assembly(root, s, &top, &feet, None)
}

/// [`project`] with its 20 parts in another project, a library in a
/// repository at `root/lib` releasing each as `A`, and the assembly in
/// `root/app`, placing those releases from its dependency cache
/// `root/cache` (fetched and locked).
pub fn project_over_dependency(root: &Path) -> Session {
    let lib = root.join("lib");
    std::fs::create_dir_all(&lib).unwrap();
    let git = |args: &[&str]| {
        let out = std::process::Command::new("git")
            .args(["-c", "user.name=Test", "-c", "user.email=t@example.com"])
            .args(["-c", "commit.gpgsign=false"])
            .args(args)
            .current_dir(&lib)
            .output()
            .unwrap();
        assert!(
            out.status.success(),
            "{}",
            String::from_utf8_lossy(&out.stderr)
        );
    };
    git(&["init", "-q", "-b", "main"]);
    ryucad::server::init(&lib, Some(Unit::Mm)).unwrap();
    let mut l = Session::open(&lib).unwrap();
    let (top, feet) = parts(&mut l);
    let names: Vec<String> = std::iter::once("plate".to_owned())
        .chain((0..SIZES).map(|k| format!("pin{k}")))
        .collect();
    for (i, p) in names.iter().enumerate() {
        must(
            &mut l,
            "props.set",
            json!({"part": p, "properties": {"number": format!("N-{i}")}}),
        );
    }
    git(&["add", "."]);
    git(&["commit", "-q", "-m", "parts"]);
    for p in &names {
        must(&mut l, "release.propose", json!({"part": p}));
    }
    git(&["add", "."]);
    git(&["commit", "-q", "-m", "A"]);
    let project = l.project().manifest.project.clone().unwrap();
    let app = root.join("app");
    std::fs::create_dir_all(&app).unwrap();
    ryucad::server::init(&app, Some(Unit::Mm)).unwrap();
    let mut s = Session::open(&app).unwrap();
    s.set_deps_cache(ryupdm::deps::Cache::new(root.join("cache")));
    let url = format!("file://{}", lib.display());
    must(&mut s, "deps.add", json!({"url": url, "name": "lib"}));
    assembly(&app, s, &top, &feet, Some(project.as_str()))
}

/// The assembly `asm` in `s`'s project at `root`: the plate fixed, 1,000
/// pins and a mate for each, its poses holding; each part another
/// project's release when `from` names that project.
fn assembly(
    root: &Path,
    mut s: Session,
    top: &str,
    feet: &[String],
    from: Option<&str>,
) -> Session {
    let place = |v: Value| match from {
        Some(p) => foreign(v, p),
        None => v,
    };
    must(
        &mut s,
        "part.create",
        json!({"part": "asm", "kind": "assembly"}),
    );
    let mut list = vec![json!("base")];
    write(
        root,
        "parts/asm/features/base.json",
        &place(
            json!({"format": 4, "kind": "ryucad.feature", "type": "instance",
                "part": "plate", "rev": "working", "fixed": true,
                "pose": {"origin_m": [0, 0, 0], "z": [0, 0, 1], "x": [1, 0, 0]}}),
        ),
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
            &place(pin(i, [0.0; 3])),
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
    // Another project's parts are read as the lock says, from what was
    // fetched.
    if from.is_some() {
        must(&mut s, "deps.update", json!({"offline": true}));
    }
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

/// Moves every pin of [`project`]'s assembly in `root` about a degree and
/// a tenth of a millimetre off where its mate holds it (seeded, so every
/// run moves them alike).
pub fn jostle(root: &Path) {
    let mut g = Lcg::new(72);
    for i in 0..PINS {
        let mut v = pin(i, [0.0; 3]);
        let path = format!("parts/asm/features/p{i}.json");
        let was: Value =
            serde_json::from_str(&std::fs::read_to_string(root.join(&path)).unwrap()).unwrap();
        v["pose"] = was["pose"].clone();
        let off = |g: &mut Lcg, d: f64| g.next_f64(-d, d);
        for k in 0..2 {
            let o = v["pose"]["origin_m"][k].as_f64().unwrap();
            v["pose"]["origin_m"][k] = json!(o + off(&mut g, 1e-4));
        }
        let z = [off(&mut g, 0.0175), off(&mut g, 0.0175), 1.0];
        v["pose"]["z"] = json!(z);
        write(root, &path, &v);
    }
}
