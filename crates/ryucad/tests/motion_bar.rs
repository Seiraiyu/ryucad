//! Design 529's bars for sweeps (plan 26.B). Release builds only.
//!
//! - **The cube:** 26 pieces (20.4 mm cubes on a 20 mm pitch, so
//!   neighbours share 0.4 mm at rest, and every twist intersects the
//!   pairs between the turning layer and the rest again) and a core,
//!   with nine layer motions of 13 steps, each turning its layer from
//!   -6° to 6°. All nine swept with interference, cold, under 60 s
//!   (design 529's target is 10 s: #804, ryukei's boolean of parallel
//!   faces overlapping and turned in their plane, takes ~830 ms a pair
//!   where others take ~4 ms; 40.9 s measured on cox-worker-3);
//!   again over the warm session, under 0.2 s.
//! - **5.D's 1,000-pin bar project:** one free revolute swept over 101
//!   steps without interference, under 1 s.
//!
//! With `RYUCAD_BARS=1` they are asserted as they are (recorded on
//! cox-worker-3); elsewhere ten times looser. Each prints the machine.

mod bar_project;

use bar_project::{id, machine, must};
use ryucad::session::Session;
use serde_json::{Value, json};
use std::time::Instant;

fn slack() -> f64 {
    if std::env::var_os("RYUCAD_BARS").is_some() {
        1.0
    } else {
        10.0
    }
}

/// The cube: part `piece` (a 20.4 mm cube), part `core` (10 mm), and
/// assembly `cube` with the core fixed at the middle and a piece at
/// every other point of the 3 × 3 × 3 grid of 20 mm, and nine motions
/// `layer-<axis><0|1|2>`, each turning the pieces of one layer about its
/// axis from -6° to 6° in 13 steps.
fn cube(root: &std::path::Path) -> Session {
    ryucad::server::init(root, None).unwrap();
    let mut s = Session::open(root).unwrap();
    for (part, size) in [("piece", "20.4 mm"), ("core", "10 mm")] {
        must(&mut s, "part.create", json!({"part": part}));
        must(
            &mut s,
            "feature.add",
            json!({"part": part, "type": "box", "id": "body",
            "params": {"x": size, "y": size, "z": size}}),
        );
    }
    must(
        &mut s,
        "part.create",
        json!({"part": "cube", "kind": "assembly"}),
    );
    must(
        &mut s,
        "feature.add",
        json!({"part": "cube", "type": "instance", "id": "core",
        "of": "core", "fixed": true, "placement": {"origin": ["-5 mm", "-5 mm", "-5 mm"]}}),
    );
    let mut pieces: Vec<([i32; 3], String)> = Vec::new();
    for i in -1..=1 {
        for j in -1..=1 {
            for k in -1..=1 {
                if (i, j, k) == (0, 0, 0) {
                    continue;
                }
                let name = format!("p{}{}{}", i + 1, j + 1, k + 1);
                let at = [i, j, k].map(|c| format!("{} mm", 20.0 * f64::from(c) - 10.2));
                must(
                    &mut s,
                    "feature.add",
                    json!({"part": "cube", "type": "instance",
                    "id": name, "of": "piece", "placement": {"origin": at}}),
                );
                pieces.push(([i, j, k], name));
            }
        }
    }
    for (axis, name) in ["x", "y", "z"].into_iter().enumerate() {
        for layer in -1..=1 {
            let instances: Vec<&String> = pieces
                .iter()
                .filter(|(c, _)| c[axis] == layer)
                .map(|(_, n)| n)
                .collect();
            must(
                &mut s,
                "motion.save",
                json!({"part": "cube",
                "motion": format!("layer-{name}{}", layer + 1), "steps": 13,
                "drives": [{"instances": instances, "rotate":
                    {"axis": {"origin": ["0 mm", "0 mm", "0 mm"], "direction": name},
                     "from": "-6 deg", "to": "6 deg"}}]}),
            );
        }
    }
    s
}

fn motions() -> Vec<String> {
    ["x", "y", "z"]
        .iter()
        .flat_map(|a| (0..3).map(move |l| format!("layer-{a}{l}")))
        .collect()
}

#[test]
#[cfg_attr(debug_assertions, ignore = "minutes in a debug build")]
fn nine_layer_twists_sweep_with_interference_cold_and_warm() {
    let dir = tempfile::tempdir().unwrap();
    let mut s = cube(dir.path());
    s.rebuilt(&id("cube")).unwrap();
    let sweep_all = |s: &mut Session| -> Vec<Value> {
        motions()
            .iter()
            .map(|m| {
                must(
                    s,
                    "assembly.sweep",
                    json!({"part": "cube", "motion": m, "interference": true}),
                )
            })
            .collect()
    };
    let t = Instant::now();
    let cold = sweep_all(&mut s);
    let cold_s = t.elapsed().as_secs_f64();
    for (m, v) in motions().iter().zip(&cold) {
        assert_eq!(v["steps"].as_array().unwrap().len(), 13, "{m}");
        assert!(
            v["steps"]
                .as_array()
                .unwrap()
                .iter()
                .all(|st| st["state"] == "holds"),
            "{m}"
        );
        // Neighbours share a volume at rest, so nothing is a collision.
        assert_eq!(v["first_collision"], Value::Null, "{m}");
        assert!(!v["at_rest"].as_array().unwrap().is_empty(), "{m}");
    }
    let t = Instant::now();
    let warm = sweep_all(&mut s);
    let warm_s = t.elapsed().as_secs_f64();
    assert_eq!(warm, cold);
    println!(
        "nine layer twists of 13 steps over 27 occurrences, with interference, on {}: cold \
         {cold_s:.2} s, warm {:.1} ms",
        machine(),
        warm_s * 1e3
    );
    // 10 s is the target once #804 makes the face-to-face pairs as quick
    // as the rest; until then the bar is 60 s (the coxswain's decision).
    assert!(cold_s < 60.0 * slack(), "cold {cold_s:.2} s (#804)");
    assert!(warm_s < 0.2 * slack(), "warm {warm_s:.3} s");
}

#[test]
#[cfg_attr(debug_assertions, ignore = "minutes in a debug build")]
fn a_thousand_pins_one_revolute_swept_over_101_steps() {
    let dir = tempfile::tempdir().unwrap();
    let mut s = bar_project::project(dir.path());
    s.rebuilt(&id("asm")).unwrap();
    let t = Instant::now();
    let v = must(
        &mut s,
        "assembly.sweep",
        json!({"part": "asm", "mate": "m2", "from": "0 deg", "to": "360 deg", "steps": 101}),
    );
    let secs = t.elapsed().as_secs_f64();
    assert_eq!(v["steps"].as_array().unwrap().len(), 101);
    assert!(
        v["steps"]
            .as_array()
            .unwrap()
            .iter()
            .all(|st| st["state"] == "holds"),
        "{v}"
    );
    println!(
        "one revolute of 1,000 mates over 101 steps, on {}: {:.1} ms",
        machine(),
        secs * 1e3
    );
    assert!(secs < 1.0 * slack(), "{secs:.3} s");
}
