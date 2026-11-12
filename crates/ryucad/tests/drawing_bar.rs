//! Design 417's bars for drawings (plan 9.D), on one thread: the
//! 1,000-hole plate's standard drawing (front, top, right and an
//! isometric) built cold within 1.25 times ryukei's own `project::view`
//! calls on the same four views; built by a new session over a warm disk
//! cache under 1 s; and its PDF written under 0.5 s. With
//! `RYUCAD_BARS=1` the bars are asserted as they are, else ten times
//! looser. Release builds only: a debug build takes minutes.
//!
//! `RYUCAD_BARS=1 cargo test -p ryucad --release --test drawing_bar -- --nocapture`

mod common;

use common::must;
use ryucad::cache::Disk;
use ryucad::doc::id::Id;
use ryucad::session::Session;
use ryucad::units::Unit;
use ryukei::geom::{Rigid, Vector3};
use ryukei::project::{Placed, View, ViewOptions};
use serde_json::{Value, json};
use std::path::Path;
use std::sync::Arc;
use std::time::Instant;

/// How much looser than recorded the bars are: ten times without
/// `RYUCAD_BARS`.
fn slack() -> f64 {
    if std::env::var_os("RYUCAD_BARS").is_some() {
        1.0
    } else {
        10.0
    }
}

/// Runs `f` on a pool of one thread, in seconds.
fn one_thread<T: Send>(f: impl FnOnce() -> T + Send) -> (f64, T) {
    let pool = rayon::ThreadPoolBuilder::new()
        .num_threads(1)
        .build()
        .unwrap();
    let t = Instant::now();
    let out = pool.install(f);
    (t.elapsed().as_secs_f64(), out)
}

fn open(root: &Path, cache: &Path) -> Session {
    Session::open(root)
        .unwrap()
        .with_disk(Some(Arc::new(Disk::open(cache, [9; 32]))))
}

/// The plate of `tests/bar.rs`: 40 × 25 × 0.5 m, a 40 × 25 pattern of
/// ⌀0.6 m holes a metre apart, and drawing `d` of its standard views.
fn plate(s: &mut Session) {
    must(s, "part.create", json!({"part": "p"}));
    let add = |s: &mut Session, mut f: Value| {
        f["part"] = json!("p");
        must(s, "feature.add", f)
    };
    add(
        s,
        json!({"type": "box", "id": "b", "params": {"x": 40, "y": 25, "z": 0.5}}),
    );
    add(
        s,
        json!({"type": "sketch", "id": "t", "placement": {"origin": [0, 0, 0.5]}}),
    );
    must(
        s,
        "sketch.edit",
        json!({"part": "p", "sketch": "t", "ops": [
            {"op": "add", "type": "point", "at": [0.5, 0.5]},
            {"op": "add", "type": "fix", "a": "p1"}]}),
    );
    add(
        s,
        json!({"type": "hole", "id": "h", "sketch": "t", "diameter": 0.6, "depth": "through_all"}),
    );
    add(
        s,
        json!({"type": "pattern", "id": "grid", "features": ["h"], "direction": "x",
               "spacing": 1, "count": 40, "direction2": "y", "spacing2": 1, "count2": 25}),
    );
}

#[test]
#[cfg_attr(debug_assertions, ignore = "minutes in a debug build")]
fn the_1000_hole_plates_drawing_builds_within_its_bars() {
    let dir = tempfile::tempdir().unwrap();
    let (root, cache) = (dir.path().join("project"), dir.path().join("cache"));
    std::fs::create_dir_all(&root).unwrap();
    ryucad::server::init(&root, Some(Unit::M)).unwrap();
    let mut s = open(&root, &cache);
    plate(&mut s);
    // The part rebuilt first: the bar is the drawing's.
    must(&mut s, "rebuild", json!({"part": "p"}));
    let (cold, made) = one_thread(|| {
        must(
            &mut s,
            "drawing.create",
            json!({"part": "p", "drawing": "d", "name": "Plate", "size": "ansi_e", "views": "standard"}),
        )
    });
    let views = made["sheets"][0]["views"].as_array().unwrap().clone();
    assert_eq!(views.len(), 4, "{made}");
    // ryukei alone, on the same body and the same four views.
    let model = s
        .model(
            &Id::parse("p", "part").unwrap(),
            &ryucad::doc::assembly::Rev::Working,
        )
        .unwrap();
    let body = model.occurrences[0].solids[0].built.body.clone();
    let vec3 = |v: &Value| {
        Vector3::new(
            v[0].as_f64().unwrap(),
            v[1].as_f64().unwrap(),
            v[2].as_f64().unwrap(),
        )
    };
    let (kernel, pieces) = one_thread(|| {
        views
            .iter()
            .map(|v| {
                let view = View::new(vec3(&v["towards"]), vec3(&v["up"])).unwrap();
                let placed = [Placed {
                    body: &body,
                    place: Rigid::IDENTITY,
                }];
                ryukei::project::view(&placed, &view, &ViewOptions::default())
                    .unwrap()
                    .pieces
                    .len()
            })
            .sum::<usize>()
    });
    // A new session over the warm disk cache: the part and every
    // projection come from it.
    let (warm, _) = one_thread(|| {
        let mut w = open(&root, &cache);
        must(&mut w, "drawing.get", json!({"part": "p", "drawing": "d"}))
    });
    let out = dir.path().join("d.pdf");
    let (pdf, _) = one_thread(|| {
        must(
            &mut s,
            "drawing.export",
            json!({"part": "p", "drawing": "d", "out": out.to_str().unwrap()}),
        )
    });
    let bytes = std::fs::metadata(&out).unwrap().len();
    println!(
        "the 1,000-hole plate's standard drawing, one thread, {} {} with {} cores: cold {cold:.2} s \
         against ryukei's own views {kernel:.2} s ({pieces} pieces; {:.2} times), warm {warm:.3} s, \
         PDF {pdf:.3} s ({bytes} bytes)",
        std::env::consts::OS,
        std::env::consts::ARCH,
        std::thread::available_parallelism().map_or(0, |n| n.get()),
        cold / kernel,
    );
    let k = slack();
    let mut missed = Vec::new();
    if cold > 1.25 * kernel * k {
        missed.push(format!("cold {cold:.2} s past 1.25 × {kernel:.2} s"));
    }
    if warm > 1.0 * k {
        missed.push(format!("warm {warm:.3} s past 1 s"));
    }
    if pdf > 0.5 * k {
        missed.push(format!("PDF {pdf:.3} s past 0.5 s"));
    }
    assert!(missed.is_empty(), "{}", missed.join("; "));
}
