//! A stand-in for `ryucad` in step-hunt's own tests: it answers the
//! commands step-hunt sends as a part that always builds would, except in
//! the stage `STEP_HUNT_FAKE` names (`<stage>:<how>`), where it fails
//! (`fail`), panics (`crash`), hangs (`hang`), answers other counts
//! (`counts`), measures another volume (`volume`) or exports other bytes
//! (`bytes`); `keyed` fails an import except one `--into` another part, as
//! healing that depends on the part ID does; `keyed2` answers two parts
//! failing, of which only the second (`lid`) builds `--into` another part.
//! As the real one, it refuses `init` in a folder holding a project.
//! With `STEP_HUNT_FAKE_MODES` set, its `import --help` names `--solid`
//! (a binary with reference imports), and with `unmeshed` there a
//! reference's `check` finds `reference_faces_unmeshed`.
//! `STEP_HUNT_FAKE_LOG` names a file each call's arguments are appended to.
//! `defeature:refuse` refuses the defeature, typed; `defeature:fail`
//! untyped; `defeature:checkfail` builds it and fails its `check`.
//! `blends --help` names `--below`.

use std::path::PathBuf;

use serde_json::{Value, json};

fn answer(v: Value) -> ! {
    println!("{}", json!({"ok": true, "result": v, "warnings": []}));
    std::process::exit(0);
}

fn refuse(code: &str, details: Value) -> ! {
    println!(
        "{}",
        json!({"ok": false, "error": {"code": code, "message": "fake", "details": details}})
    );
    std::process::exit(1);
}

fn main() {
    let mut args: Vec<String> = std::env::args().skip(1).collect();
    let mut dir = PathBuf::from(".");
    if args.first().map(String::as_str) == Some("-C") {
        dir = PathBuf::from(args.remove(1));
        args.remove(0);
    }
    let stage = std::env::var("STEP_HUNT_STAGE").unwrap_or_default();
    if let Ok(log) = std::env::var("STEP_HUNT_FAKE_LOG") {
        use std::io::Write;
        let mut f = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(log)
            .expect("the log");
        // One write a call, so parallel calls' lines don't interleave.
        f.write_all(format!("{}\n", args.join(" ")).as_bytes())
            .expect("logging");
    }
    let modes = std::env::var("STEP_HUNT_FAKE_MODES").unwrap_or_default();
    if args.first().map(String::as_str) == Some("import") && args.iter().any(|a| a == "--help") {
        if modes.is_empty() {
            println!("Usage: ryucad import <FILE> [--part <PART>]");
        } else {
            println!("Usage: ryucad import <FILE> [--part <PART>] [--solid]");
        }
        std::process::exit(0);
    }
    if args.first().map(String::as_str) == Some("blends") && args.iter().any(|a| a == "--help") {
        println!("Usage: ryucad blends <PART> [SOLID] [--tolerance <LENGTH>] [--below <LENGTH>]");
        std::process::exit(0);
    }
    let fake = std::env::var("STEP_HUNT_FAKE").unwrap_or_default();
    let how = match fake.split_once(':') {
        Some((s, h)) if s == stage => h.to_owned(),
        _ => String::new(),
    };
    match how.as_str() {
        "crash" => panic!("a fake crash in {stage}"),
        "hang" => std::thread::sleep(std::time::Duration::from_secs(3600)),
        _ => {}
    }
    let held = dir.join("imported.step");
    let cmd = args.first().map(String::as_str).unwrap_or_default();
    match cmd {
        "version" => answer(json!({"ryucad": "fake", "commit": "0000000"})),
        "init" if dir.join("ryucad.json").exists() => {
            refuse("project_exists", json!({"path": dir}))
        }
        "init" => {
            std::fs::write(dir.join("ryucad.json"), "{}\n").expect("writing the manifest");
            answer(json!({}))
        }
        "part" => answer(json!({})),
        "import" => {
            std::fs::copy(&args[1], &held).expect("the file to import");
            let part = args
                .iter()
                .position(|a| a == "--part")
                .map_or("body".to_owned(), |k| args[k + 1].clone());
            let faces = if how == "counts" { 7 } else { 6 };
            let mut feature = json!({"id": "body", "type": "import", "ok": true, "hash": "h",
                                     "counts": {"faces": faces, "edges": 12, "vertices": 8}});
            let into = args.iter().any(|a| a == "--into");
            let lid = args
                .windows(2)
                .any(|w| w[0] == "--product" && w[1] == "lid");
            let failed = json!({"id": "body", "type": "import", "ok": false, "error":
                {"code": "import_failed", "message": "fake", "details": {"reason": "faces_overlap", "step": ["#1"]}}});
            if how == "fail" || (how == "keyed" && !into) || (how == "keyed2" && !(into && lid)) {
                feature = failed.clone();
            }
            let mut parts = vec![json!({"part": part, "kind": "part", "features": [feature],
                                        "product": {"id": "box", "name": "box"}})];
            if how == "keyed2" && !into {
                parts.push(json!({"part": "lid", "kind": "part", "features": [failed],
                                  "product": {"id": "lid", "name": "lid"}}));
            }
            answer(json!({"parts": parts}))
        }
        "inspect" => answer(json!({"bodies": [{"solid": "body",
            "bounds": {"min": [0.0, 0.0, 0.0], "max": [0.03, 0.04, 0.0]}}]})),
        "blends" => answer(json!({"blends": [{"index": 0}], "corners": []})),
        "feature" => answer(json!({})),
        "rebuild" => {
            let f = match how.as_str() {
                "refuse" => json!({"id": "defeature", "ok": false, "error":
                    {"code": "tweak_failed", "message": "fake", "details": {"reason": "blend_lost"}}}),
                "fail" => json!({"id": "defeature", "ok": false, "error":
                    {"code": "internal", "message": "fake", "details": {}}}),
                _ => json!({"id": "defeature", "ok": true}),
            };
            answer(json!({"parts": [{"part": "body", "features": [f]}]}))
        }
        "check" if how == "checkfail" => refuse(
            "check_failed",
            json!({"findings": [{"code": "faces_intersect", "severity": "error"}]}),
        ),
        "check" if how == "fail" => refuse(
            "check_failed",
            json!({"findings": [{"code": "faces_intersect", "severity": "error"}]}),
        ),
        "check" if modes == "unmeshed" && args.get(1).is_some_and(|p| p == "ref") => answer(
            json!({"passed": true, "findings": [{"code": "reference_faces_unmeshed",
                                                  "severity": "warning"}]}),
        ),
        "check" => answer(json!({"passed": true, "findings": []})),
        "export" if how == "fail" => refuse("export_unrepresentable", json!({"reason": "pcurve"})),
        "export" => {
            let out = &args[args.iter().position(|a| a == "--out").expect("--out") + 1];
            let mut bytes = std::fs::read(&held).expect("an import first");
            if how == "bytes" {
                bytes.extend_from_slice(dir.to_string_lossy().as_bytes());
            }
            std::fs::write(out, bytes).expect("writing the export");
            answer(json!({"out": out}))
        }
        // The second project's volume differs.
        "mass" if how == "volume" && dir.ends_with("p2") => {
            answer(json!({"volume": {"value": "2 mm^3", "m3": 2e-9, "exact": true}}))
        }
        "mass" => answer(json!({"volume": {"value": "1 mm^3", "m3": 1e-9, "exact": true}})),
        other => refuse("usage", json!({"command": other})),
    }
}
