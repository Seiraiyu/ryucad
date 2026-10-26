//! Interference against exact overlaps of primitives, through the
//! protocol: two balls' lens, a pin pressed into a hole too small for it
//! and one that fits, a bolt laid along x, instances in subassemblies and
//! a pattern's copies, and the same report on one thread and eight.

use core::f64::consts::PI;
use ryucad::proto::Request;
use ryucad::server::handle;
use ryucad::session::Session;
use serde_json::{Value, json};

fn call(s: &mut Session, method: &str, params: Value) -> ryucad::error::Result<Value> {
    handle(s, Request::from_rpc(method, Some(params)).unwrap())
}

fn must(s: &mut Session, method: &str, params: Value) -> Value {
    call(s, method, params).unwrap_or_else(|e| panic!("{method}: {e}"))
}

/// A project in millimetres with parts `ball` (radius 10 mm), `pin`
/// (radius 3 mm, 20 mm long) and `plate` (40 × 30 × 5 mm with a hole of
/// radius `hole` mm at (20, 15)), and an empty assembly `asm`.
fn project(hole: f64) -> (tempfile::TempDir, Session) {
    let dir = tempfile::tempdir().unwrap();
    ryucad::server::init(dir.path(), None).unwrap();
    let mut s = Session::open(dir.path()).unwrap();
    for (part, f) in [
        (
            "ball",
            json!({"type": "sphere", "id": "b", "params": {"radius": 10}}),
        ),
        (
            "pin",
            json!({"type": "cylinder", "id": "c", "params": {"radius": 3, "height": 20}}),
        ),
        (
            "plate",
            json!({"type": "box", "id": "slab", "params": {"x": 40, "y": 30, "z": 5}}),
        ),
    ] {
        must(&mut s, "part.create", json!({"part": part}));
        let mut f = f;
        f["part"] = json!(part);
        must(&mut s, "feature.add", f);
    }
    must(
        &mut s,
        "feature.add",
        json!({"part": "plate", "type": "cylinder", "id": "hole",
               "params": {"radius": hole, "height": 5}, "placement": {"origin": [20, 15, 0]},
               "operation": "cut", "targets": ["slab"]}),
    );
    must(
        &mut s,
        "part.create",
        json!({"part": "asm", "kind": "assembly"}),
    );
    (dir, s)
}

/// Places `of` in assembly `asm` as `id` at `origin` (mm), its z and x
/// axes along `z` and `x`.
fn place(
    s: &mut Session,
    asm: &str,
    id: &str,
    of: &str,
    origin: [f64; 3],
    z: [f64; 3],
    x: [f64; 3],
) {
    must(
        s,
        "feature.add",
        json!({"part": asm, "type": "instance", "id": id, "of": of,
               "placement": {"origin": origin, "z": z, "x": x}}),
    );
}

const Z: [f64; 3] = [0.0, 0.0, 1.0];
const X: [f64; 3] = [1.0, 0.0, 0.0];

/// The interferences' pairs and volumes in mm³.
fn pairs(v: &Value) -> Vec<(String, String, f64)> {
    v["interferences"]
        .as_array()
        .unwrap()
        .iter()
        .map(|i| {
            (
                i["a"].as_str().unwrap().to_owned(),
                i["b"].as_str().unwrap().to_owned(),
                i["volume_m3"].as_f64().unwrap() * 1e9,
            )
        })
        .collect()
}

fn near(got: f64, want: f64, rel: f64) {
    assert!((got - want).abs() <= rel * want, "{got} against {want}");
}

#[test]
fn two_balls_share_a_lens() {
    let (_dir, mut s) = project(3.0);
    place(&mut s, "asm", "a", "ball", [0.0; 3], Z, X);
    place(&mut s, "asm", "b", "ball", [15.0, 0.0, 0.0], Z, X);
    let v = must(&mut s, "interference", json!({"part": "asm"}));
    let got = pairs(&v);
    assert_eq!(got.len(), 1, "{v}");
    assert_eq!(
        (got[0].0.as_str(), got[0].1.as_str()),
        ("asm/a/b", "asm/b/b")
    );
    // A lens of two balls of radius R whose centres are d apart; the
    // mesh's facets turn by up to 15°, so it holds a little less.
    let (r, d) = (10.0, 15.0);
    near(
        got[0].2,
        PI * (4.0 * r + d) * (2.0 * r - d) * (2.0 * r - d) / 12.0,
        1e-2,
    );
    // Its box, its mesh's: from x = 5 mm to 10 mm, a little inside.
    let bx = &v["interferences"][0]["box"];
    near(bx["min_m"][0].as_f64().unwrap(), 0.005, 1e-3);
    near(bx["max_m"][0].as_f64().unwrap(), 0.010, 1e-3);
    // Apart, they touch nowhere.
    let (_dir, mut s) = project(3.0);
    place(&mut s, "asm", "a", "ball", [0.0; 3], Z, X);
    place(&mut s, "asm", "b", "ball", [25.0, 0.0, 0.0], Z, X);
    let v = must(&mut s, "interference", json!({"part": "asm"}));
    assert_eq!((v["pairs"].as_u64(), pairs(&v).len()), (Some(0), 0), "{v}");
}

#[test]
fn a_pin_pressed_into_a_small_hole_and_one_that_fits() {
    let (_dir, mut s) = project(2.95);
    place(&mut s, "asm", "plate", "plate", [0.0; 3], Z, X);
    place(&mut s, "asm", "pin", "pin", [20.0, 15.0, -5.0], Z, X);
    let v = must(&mut s, "interference", json!({"part": "asm"}));
    let got = pairs(&v);
    assert_eq!(got.len(), 1, "{v}");
    // The ring between the pin and the hole, through the plate.
    near(got[0].2, PI * (3.0 * 3.0 - 2.95 * 2.95) * 5.0, 1e-2);
    // The faces: the hole's wall and the plate's top and bottom; the
    // pin's side.
    let faces = &v["interferences"][0]["faces"];
    assert_eq!(faces["a"].as_array().unwrap().len(), 3, "{faces}");
    assert_eq!(faces["b"].as_array().unwrap().len(), 1, "{faces}");
    // A pin the hole's size touches it all round, and is clear.
    let (_dir, mut s) = project(3.0);
    place(&mut s, "asm", "plate", "plate", [0.0; 3], Z, X);
    place(&mut s, "asm", "pin", "pin", [20.0, 15.0, -5.0], Z, X);
    let v = must(&mut s, "interference", json!({"part": "asm"}));
    assert_eq!(v["pairs"], 1, "{v}");
    assert_eq!(pairs(&v), []);
    assert_eq!(v["unchecked"], json!([]), "{v}");
}

#[test]
fn a_pin_laid_along_x_through_the_plate() {
    let (_dir, mut s) = project(3.0);
    place(&mut s, "asm", "plate", "plate", [0.0; 3], Z, X);
    // Its axis along x at y = 5 mm, half in the plate's thickness at z = 2.5 mm.
    place(
        &mut s,
        "asm",
        "pin",
        "pin",
        [-5.0, 5.0, 2.5],
        X,
        [0.0, 1.0, 0.0],
    );
    let v = must(&mut s, "interference", json!({"part": "asm"}));
    let got = pairs(&v);
    assert_eq!(got.len(), 1, "{v}");
    // 15 mm of the pin lies in x from 0 to 40; a slab of the circle 5 mm
    // thick about its middle.
    let (r, h): (f64, f64) = (3.0, 2.5);
    let segment = 2.0 * (h * (r * r - h * h).sqrt() + r * r * ryukei::math::asin(h / r));
    near(got[0].2, segment * 15.0, 2e-3);
}

#[test]
fn subassemblies_and_copies_are_paired_within_and_across() {
    let (_dir, mut s) = project(3.0);
    // Two balls overlapping inside `pair`, and `pair` placed twice, the
    // second overlapping the first.
    must(
        &mut s,
        "part.create",
        json!({"part": "pair", "kind": "assembly"}),
    );
    place(&mut s, "pair", "l", "ball", [0.0; 3], Z, X);
    place(&mut s, "pair", "r", "ball", [15.0, 0.0, 0.0], Z, X);
    place(&mut s, "asm", "p1", "pair", [0.0; 3], Z, X);
    place(&mut s, "asm", "p2", "pair", [0.0, 15.0, 0.0], Z, X);
    let v = must(&mut s, "interference", json!({"part": "asm"}));
    let got: Vec<(String, String)> = pairs(&v).into_iter().map(|(a, b, _)| (a, b)).collect();
    let want = [
        ("asm/p1/l/b", "asm/p1/r/b"),
        ("asm/p1/l/b", "asm/p2/l/b"),
        ("asm/p1/r/b", "asm/p2/r/b"),
        ("asm/p2/l/b", "asm/p2/r/b"),
    ];
    let want: Vec<(String, String)> = want
        .iter()
        .map(|(a, b)| ((*a).to_owned(), (*b).to_owned()))
        .collect();
    assert_eq!(got, want, "{v}");
    // The diagonals' boxes overlap, but they are 21 mm apart.
    assert_eq!(
        (v["pairs"].as_u64(), v["clear"].as_u64()),
        (Some(6), Some(2))
    );
    // Only those with a side in the second copy of the pair.
    let v = must(
        &mut s,
        "interference",
        json!({"part": "asm", "instances": ["p2/l"]}),
    );
    assert_eq!(pairs(&v).len(), 2, "{v}");
    // A row of three pins 4 mm apart, each overlapping the next.
    place(&mut s, "asm", "pin", "pin", [100.0, 0.0, 0.0], Z, X);
    must(
        &mut s,
        "feature.add",
        json!({"part": "asm", "type": "instance_pattern", "id": "row", "form": "linear",
               "instances": ["pin"], "direction": "x", "spacing": "4 mm", "count": 3}),
    );
    let v = must(
        &mut s,
        "interference",
        json!({"part": "asm", "instances": ["pin", "row:1/pin", "row:2/pin"]}),
    );
    let got: Vec<(String, String)> = pairs(&v).into_iter().map(|(a, b, _)| (a, b)).collect();
    assert_eq!(
        got,
        [
            ("asm/pin/c".to_owned(), "asm/row:1/pin/c".to_owned()),
            ("asm/row:1/pin/c".to_owned(), "asm/row:2/pin/c".to_owned()),
        ],
        "{v}"
    );
}

#[test]
fn one_thread_and_eight_agree() {
    let report = |threads: usize| {
        let (_dir, mut s) = project(2.95);
        place(&mut s, "asm", "plate", "plate", [0.0; 3], Z, X);
        place(&mut s, "asm", "pin", "pin", [20.0, 15.0, -5.0], Z, X);
        for k in 0..6 {
            let id = format!("ball{k}");
            place(&mut s, "asm", &id, "ball", [6.0 * k as f64, 0.0, 0.0], Z, X);
        }
        let pool = rayon::ThreadPoolBuilder::new()
            .num_threads(threads)
            .build()
            .unwrap();
        let mut v = pool.install(|| must(&mut s, "interference", json!({"part": "asm"})));
        v.as_object_mut().unwrap().remove("revision");
        v
    };
    assert_eq!(report(1), report(8));
}
