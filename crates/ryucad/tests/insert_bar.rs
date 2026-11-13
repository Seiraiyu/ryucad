//! Design 521's bar: a flattened KiCad board of a hundred or more
//! reference bodies, inserted into a part off the identity; a warm rebuild
//! of the inserting part after a placement edit is under 1 s, and no
//! inserted solid is meshed again. The board is exported by
//! `scripts/export-pcba.sh` (KiCad, devcontainer) and never committed:
//! `RYUCAD_PCBA=<board.step>`.

mod bar_project;

use ryucad::proto::Request;
use ryucad::server::handle;
use ryucad::session::Session;
use serde_json::{Value, json};
use std::time::Instant;

/// The bar itself with `RYUCAD_BARS`, else ten times looser.
fn slack() -> f64 {
    if std::env::var_os("RYUCAD_BARS").is_some() {
        1.0
    } else {
        10.0
    }
}

fn must(s: &mut Session, method: &str, params: Value) -> Value {
    handle(s, Request::from_rpc(method, Some(params.clone())).unwrap())
        .unwrap_or_else(|e| panic!("{method} {params}: {e:?}"))
}

#[test]
#[ignore = "a flattened KiCad board: RYUCAD_PCBA=<board.step>"]
fn an_inserted_board_moves_in_under_a_second_and_meshes_nothing_again() {
    let file = std::env::var("RYUCAD_PCBA").expect("RYUCAD_PCBA");
    let dir = tempfile::tempdir().unwrap();
    ryucad::server::init(dir.path(), None).unwrap();
    let mut s = Session::open(dir.path()).unwrap();
    must(
        &mut s,
        "import",
        json!({"path": file, "part": "board", "flatten": true}),
    );
    must(&mut s, "part.create", json!({"part": "enclosure"}));
    must(
        &mut s,
        "feature.add",
        json!({"part": "enclosure", "type": "insert", "id": "pcb", "of": "board",
               "rev": "working",
               "placement": {"origin": ["10 mm", "5 mm", "3 mm"], "z": [0, 0, 1], "x": [0, 1, 0]}}),
    );
    let r = must(&mut s, "rebuild", json!({"part": "enclosure"}));
    let count = r["parts"][0]["features"][0]["insert"]["count"]
        .as_u64()
        .unwrap();
    assert!(
        count >= 100,
        "a board of {count} bodies is too small for the bar"
    );
    // The source's meshes, made once, and the inserted ones moved from them.
    must(&mut s, "mesh.get", json!({"part": "board"}));
    must(&mut s, "mesh.get", json!({"part": "enclosure"}));
    let meshed = ryucad::mesh::kernel_meshes();
    // A placement edit, warm.
    must(
        &mut s,
        "feature.edit",
        json!({"part": "enclosure", "id": "pcb",
               "placement": {"origin": ["12 mm", "5 mm", "3 mm"]}}),
    );
    let t = Instant::now();
    must(&mut s, "rebuild", json!({"part": "enclosure"}));
    let warm = t.elapsed().as_secs_f64();
    must(&mut s, "mesh.get", json!({"part": "enclosure"}));
    assert_eq!(
        ryucad::mesh::kernel_meshes(),
        meshed,
        "an inserted solid was meshed again"
    );
    println!(
        "insert bar on {}: {count} bodies, warm rebuild after a placement edit {warm:.3} s",
        bar_project::machine()
    );
    assert!(warm < 1.0 * slack(), "{warm:.3} s");
}
