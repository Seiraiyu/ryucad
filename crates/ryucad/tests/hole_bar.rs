//! Design 533's bars for holes (plan 28.D), on one thread: the 1,000-hole
//! plate tapped M6 rebuilds within 1.1 times the same plate drilled ⌀5
//! (the tap drill: the same bodies, only the intent costs); a fill of
//! 10,000 places computes its lattice and clipping within 0.5 s (added to
//! the part in memory: its holes are never drilled); and `holes.list`
//! answers on the tapped 1,000-hole plate within 50 ms warm, its rows
//! listed once already. With `RYUCAD_BARS=1` the bars are asserted as
//! they are, else ten times looser. Every timing names the machine.
//! Release builds only: a debug build takes minutes.
//!
//! `RYUCAD_BARS=1 cargo test -p ryucad --release --test hole_bar -- --nocapture`

mod common;

use common::must;
use ryucad::doc::id::Id;
use ryucad::session::Session;
use serde_json::{Value, json};
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

/// What a timing ran on.
fn machine() -> String {
    format!(
        "{} {} with {} cores",
        std::env::consts::OS,
        std::env::consts::ARCH,
        std::thread::available_parallelism().map_or(0, |n| n.get())
    )
}

fn pool() -> rayon::ThreadPool {
    rayon::ThreadPoolBuilder::new()
        .num_threads(1)
        .build()
        .unwrap()
}

/// The best of two runs of `f` on one thread, in seconds, and what the
/// last returned.
fn best<T: Send>(f: impl Fn() -> T + Sync) -> (f64, T) {
    let pool = pool();
    let mut fastest = f64::INFINITY;
    let mut last = None;
    for _ in 0..2 {
        let t = Instant::now();
        let out = pool.install(&f);
        fastest = fastest.min(t.elapsed().as_secs_f64());
        last = Some(out);
    }
    (fastest, last.expect("two runs"))
}

fn add(s: &mut Session, mut f: Value) -> Value {
    f["part"] = json!("p");
    must(s, "feature.add", f)
}

/// Part `p` in millimetres: a plate `b` of `size` (x, y; 10 mm thick), a
/// sketch on its top with a point at (5, 5), and hole `h` there with
/// `fields`.
fn seeded(size: [f64; 2], fields: Value) -> (tempfile::TempDir, Session) {
    let dir = tempfile::tempdir().unwrap();
    ryucad::server::init(dir.path(), None).unwrap();
    let mut s = Session::open(dir.path()).unwrap();
    must(&mut s, "part.create", json!({"part": "p"}));
    add(
        &mut s,
        json!({"type": "box", "id": "b", "params": {"x": size[0], "y": size[1], "z": 10}}),
    );
    add(
        &mut s,
        json!({"type": "sketch", "id": "t", "placement": {"origin": [0, 0, 10]}}),
    );
    must(
        &mut s,
        "sketch.edit",
        json!({"part": "p", "sketch": "t", "ops": [
            {"op": "add", "type": "point", "at": [5, 5]},
            {"op": "add", "type": "fix", "a": "p1"}]}),
    );
    let mut h = json!({"type": "hole", "id": "h", "sketch": "t"});
    for (k, v) in fields.as_object().unwrap() {
        h[k] = v.clone();
    }
    add(&mut s, h);
    (dir, s)
}

/// [`seeded`]'s 400 × 250 plate with a 40 × 25 pattern of `h` 10 mm
/// apart: 1,000 holes.
fn plate(fields: Value) -> (tempfile::TempDir, Session) {
    let (d, mut s) = seeded([400.0, 250.0], fields);
    add(
        &mut s,
        json!({"type": "pattern", "id": "grid", "features": ["h"], "direction": "x",
               "spacing": 10, "count": 40, "direction2": "y", "spacing2": 10, "count2": 25}),
    );
    (d, s)
}

fn p() -> Id {
    Id::parse("p", "part").unwrap()
}

#[test]
#[cfg_attr(debug_assertions, ignore = "minutes in a debug build")]
fn a_tapped_1000_hole_plate_rebuilds_within_1_1_times_a_plain_one() {
    let (_a, plain) = plate(json!({"diameter": 5, "depth": "through_all"}));
    let (_b, tapped) = plate(json!({"spec": "M6x1.0-6H THRU"}));
    let time = |s: &Session| {
        let part = s.project().part(&p()).unwrap().clone();
        best(|| ryucad::rebuild::rebuild_part(&part))
    };
    let (plain_s, pr) = time(&plain);
    let (tapped_s, tr) = time(&tapped);
    for r in [&pr, &tr] {
        assert_eq!(r.failed(), 0, "{:?}", r.features);
        let (_, last) = r.features.last().unwrap();
        let b = last
            .as_ref()
            .unwrap()
            .solid(&Id::parse("b", "feature").unwrap())
            .unwrap();
        assert_eq!(b.body.counts().faces, 6 + 1000);
    }
    println!(
        "1000 holes, one thread, {}: tapped M6 {:.0} ms, plain ⌀5 {:.0} ms ({:.3}×)",
        machine(),
        tapped_s * 1e3,
        plain_s * 1e3,
        tapped_s / plain_s
    );
    assert!(
        tapped_s <= 1.1 * slack() * plain_s,
        "tapped {tapped_s:.3} s against plain {plain_s:.3} s"
    );
}

#[test]
#[cfg_attr(debug_assertions, ignore = "slow in a debug build")]
fn a_10000_place_fill_clips_within_half_a_second() {
    // A ⌀2 seed at (5, 5) on a 1,000 mm square: 100 × 100 places 10 mm
    // apart, the seed's among them.
    let (_d, mut s) = seeded(
        [1000.0, 1000.0],
        json!({"diameter": 2, "depth": "through_all"}),
    );
    let b = must(&mut s, "inspect", json!({"part": "p", "feature": "b"}))["bodies"][0].clone();
    let top = b["faces"]
        .as_array()
        .unwrap()
        .iter()
        .find(|f| {
            let sign = if f["reversed"] == true { -1.0 } else { 1.0 };
            f["surface"]["type"] == "plane"
                && f["surface"]["frame"]["z"][2].as_f64().unwrap() * sign == 1.0
        })
        .map(|f| f["name"].as_str().unwrap().to_owned())
        .unwrap();
    let result = s.rebuilt(&p()).unwrap().clone();
    let mut part = s.project().part(&p()).unwrap().clone();
    let fid = Id::parse("f", "feature").unwrap();
    let file = json!({"format": ryucad::doc::hole::PLACES_FORMAT, "kind": "ryucad.feature",
        "type": "pattern", "features": ["h"], "targets": ["b"], "form": "fill",
        "face": {"feature": "b", "name": top}, "spacing": "10 mm"});
    let feature =
        ryucad::doc::files::Feature::from_json(&file, ryucad::units::Unit::Mm, &part.env).unwrap();
    let ryucad::doc::files::Feature::Pattern(pattern) = feature.clone() else {
        unreachable!("a pattern")
    };
    part.features.insert(fid.clone(), Ok(feature));
    part.file.as_mut().unwrap().features.push(fid.clone());
    let (secs, motions) =
        best(|| ryucad::pattern::motions(&part, &fid, &pattern, &result).unwrap());
    assert_eq!(motions.len() + 1, 10_000);
    println!(
        "a 10,000-place fill clipped, one thread, {}: {:.0} ms",
        machine(),
        secs * 1e3
    );
    assert!(secs <= 0.5 * slack(), "{secs:.3} s");
}

#[test]
#[cfg_attr(debug_assertions, ignore = "minutes in a debug build")]
fn holes_answer_within_50_ms_warm_on_the_1000_hole_plate() {
    let (_d, mut s) = plate(json!({"spec": "M6x1.0-6H THRU"}));
    let pool = pool();
    let list = |s: &mut Session| {
        let t = Instant::now();
        let v = pool.install(|| must(s, "holes.list", json!({"part": "p"})));
        (t.elapsed().as_secs_f64(), v)
    };
    let (cold, v) = list(&mut s);
    assert_eq!(v["holes"].as_array().unwrap().len(), 1000);
    let warm = (0..2).map(|_| list(&mut s).0).fold(f64::INFINITY, f64::min);
    println!(
        "holes on 1000 holes, one thread, {}: {:.1} ms warm ({:.0} ms cold)",
        machine(),
        warm * 1e3,
        cold * 1e3
    );
    assert!(warm <= 0.05 * slack(), "{warm:.4} s");
}
