//! Sweeps (plan 26.B): the same report on one thread and on eight, from
//! a fresh session each time.

use ryucad::session::Session;
use serde_json::{Value, json};

fn call(s: &mut Session, m: &str, p: Value) -> Value {
    ryucad::server::handle(s, ryucad::proto::Request::from_rpc(m, Some(p)).unwrap())
        .unwrap_or_else(|e| panic!("{m}: {e}"))
}

/// An arm turning a quarter turn into a fixed block and over a nub sunk
/// into it at rest, swept with interference, clearance and poses.
fn swept() -> Value {
    let dir = tempfile::tempdir().unwrap();
    ryucad::server::init(dir.path(), None).unwrap();
    let mut s = Session::open(dir.path()).unwrap();
    for (part, x, y, z) in [("arm", 50, 4, 4), ("block", 10, 10, 4), ("cube", 2, 2, 2)] {
        call(&mut s, "part.create", json!({"part": part}));
        call(
            &mut s,
            "feature.add",
            json!({"part": part, "type": "box", "id": "body",
            "params": {"x": format!("{x} mm"), "y": format!("{y} mm"), "z": format!("{z} mm")}}),
        );
    }
    call(
        &mut s,
        "part.create",
        json!({"part": "swing", "kind": "assembly"}),
    );
    for (id, of, at, fixed) in [
        ("arm", "arm", ["0 mm", "0 mm", "0 mm"], false),
        ("block", "block", ["30 mm", "20 mm", "0 mm"], true),
        ("nub", "cube", ["10 mm", "1 mm", "1 mm"], true),
    ] {
        call(
            &mut s,
            "feature.add",
            json!({"part": "swing", "type": "instance", "id": id,
            "of": of, "fixed": fixed, "placement": {"origin": at}}),
        );
    }
    call(
        &mut s,
        "assembly.sweep",
        json!({"part": "swing", "steps": 46,
        "interference": true, "clearance": true, "below": "50 mm", "poses": true,
        "drives": [{"instances": ["arm"], "rotate":
            {"axis": {"origin": ["0 mm", "0 mm", "0 mm"], "direction": "z"},
             "from": "0 deg", "to": "45 deg"}}]}),
    )
}

#[test]
fn a_sweep_is_the_same_on_one_thread_and_eight() {
    let answers: Vec<Value> = [1, 8]
        .into_iter()
        .map(|n| {
            rayon::ThreadPoolBuilder::new()
                .num_threads(n)
                .build()
                .unwrap()
                .install(swept)
        })
        .collect();
    assert_eq!(answers[0], answers[1]);
    assert_eq!(answers[0]["first_collision"]["index"], 22);
}
