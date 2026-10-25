//! Design 59's bar for patterns: a plate drilled by a 1,000-hole linear
//! pattern rebuilds, on one thread, within twice the time ryukei itself
//! takes to cut the same block by the same 1,000 cylinders in one
//! boolean. A hole's tool starts on its sketch's plane, so on a face its
//! top is flush with the face; ryukei is timed on that geometry too, and
//! beside it on ryukei-bench's (rods out of both faces), so the cost of
//! flush tools stays visible (issue #77). The block is 40 × 25 ×
//! 0.5 m, the holes 0.6 m across a metre apart. Release builds only: in a
//! debug build it takes minutes.

use ryucad::doc::id::Id;
use ryucad::proto::Request;
use ryucad::server::handle;
use ryucad::session::Session;
use ryucad::units::Unit;
use ryukei::boolean::{BooleanOp, Tool, boolean};
use ryukei::geom::{Rigid, Vector3};
use ryukei::name::{OpKey, ToolKey};
use ryukei::topo::Body;
use serde_json::{Value, json};
use std::time::Instant;

fn must(s: &mut Session, method: &str, params: Value) -> Value {
    handle(s, Request::from_rpc(method, Some(params)).unwrap())
        .unwrap_or_else(|e| panic!("{method}: {e}"))
}

/// The best of two runs of `f` on one thread, in milliseconds, and what
/// the last run returned.
fn best<T: Send>(f: impl Fn() -> T + Sync) -> (f64, T) {
    let pool = rayon::ThreadPoolBuilder::new()
        .num_threads(1)
        .build()
        .unwrap();
    let mut fastest = f64::INFINITY;
    let mut last = None;
    for _ in 0..2 {
        let t = Instant::now();
        let out = pool.install(&f);
        fastest = fastest.min(t.elapsed().as_secs_f64() * 1e3);
        last = Some(out);
    }
    (fastest, last.expect("two runs"))
}

/// ryukei alone: the block minus 1,000 cylinders `h` long from `z`, in
/// milliseconds.
fn kernel(block: &Body, h: f64, z: f64) -> f64 {
    let (rod, _) = ryukei::primitive::cylinder(OpKey(3), 0.3, h, &Rigid::IDENTITY).unwrap();
    let places: Vec<Rigid> = (0..25)
        .flat_map(|j| (0..40).map(move |i| (i, j)))
        .map(|(i, j)| Rigid::translation(Vector3::new(f64::from(i) + 0.5, f64::from(j) + 0.5, z)))
        .collect();
    let (ms, (cut, _)) = best(|| {
        let tools: Vec<Tool> = places
            .iter()
            .enumerate()
            .map(|(i, p)| Tool {
                key: ToolKey(i as u128 + 1),
                body: &rod,
                place: *p,
            })
            .collect();
        boolean(OpKey(2), BooleanOp::Subtract, block, &tools).expect("ryukei's boolean")
    });
    assert_eq!(cut.counts().faces, 6 + 1000);
    ms
}

#[test]
#[cfg_attr(debug_assertions, ignore = "minutes in a debug build")]
fn a_1000_hole_pattern_rebuilds_within_twice_ryukeis_own_boolean() {
    let (block, _) = ryukei::primitive::block(OpKey(1), 40.0, 25.0, 0.5, &Rigid::IDENTITY).unwrap();
    // Out of both faces, as ryukei-bench has it; flush with the top, as a
    // hole drilled from a sketch on the top is.
    let out = kernel(&block, 1.0, -0.25);
    let flush = kernel(&block, 0.75, -0.25);

    // ryucad: the plate, a sketch on its top with one point, a hole
    // through all, and a 40 × 25 pattern of it.
    let dir = tempfile::tempdir().unwrap();
    ryucad::server::init(dir.path(), Some(Unit::M)).unwrap();
    let mut s = Session::open(dir.path()).unwrap();
    must(&mut s, "part.create", json!({"part": "p"}));
    let add = |s: &mut Session, f: Value| {
        let mut f = f;
        f["part"] = json!("p");
        must(s, "feature.add", f)
    };
    add(
        &mut s,
        json!({"type": "box", "id": "b", "params": {"x": 40, "y": 25, "z": 0.5}}),
    );
    add(
        &mut s,
        json!({"type": "sketch", "id": "t", "placement": {"origin": [0, 0, 0.5]}}),
    );
    must(
        &mut s,
        "sketch.edit",
        json!({"part": "p", "sketch": "t", "ops": [
            {"op": "add", "type": "point", "at": [0.5, 0.5]},
            {"op": "add", "type": "fix", "a": "p1"}
        ]}),
    );
    add(
        &mut s,
        json!({"type": "hole", "id": "h", "sketch": "t", "diameter": 0.6, "depth": "through_all"}),
    );
    add(
        &mut s,
        json!({"type": "pattern", "id": "grid", "features": ["h"], "direction": "x",
               "spacing": 1, "count": 40, "direction2": "y", "spacing2": 1, "count2": 25}),
    );
    let part = s
        .project()
        .part(&Id::parse("p", "part").unwrap())
        .unwrap()
        .clone();
    let (ryucad_ms, r) = best(|| ryucad::rebuild::rebuild_part(&part));
    assert_eq!(r.failed(), 0, "{:?}", r.features);
    let (_, last) = r.features.last().unwrap();
    let plate = last
        .as_ref()
        .unwrap()
        .solid(&Id::parse("b", "feature").unwrap())
        .unwrap();
    assert_eq!(plate.body.counts().faces, 6 + 1000);

    println!(
        "1000 holes, one thread, {} {} with {} cores: ryucad {ryucad_ms:.0} ms; ryukei \
         {flush:.0} ms flush, {out:.0} ms out of both faces",
        std::env::consts::OS,
        std::env::consts::ARCH,
        std::thread::available_parallelism().map_or(0, |n| n.get()),
    );
    assert!(
        ryucad_ms < 2.0 * flush,
        "ryucad {ryucad_ms:.0} ms vs ryukei {flush:.0} ms on the same geometry"
    );
    // Flush tools cost the kernel about 9 times as much (issue #77); this
    // fails if that grows, and keeps passing once it is fixed.
    assert!(
        flush < 12.0 * out,
        "ryukei {flush:.0} ms flush vs {out:.0} ms out of both faces"
    );
}
