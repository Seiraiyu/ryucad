//! Interference against exact overlaps of primitives, through the
//! protocol: two balls' lens, a pin pressed into a hole too small for it
//! and one that fits, a bolt laid along x, instances in subassemblies and
//! a pattern's copies, and the same report on one thread and eight.

mod common;

use common::must;
use core::f64::consts::PI;
use ryucad::session::Session;
use serde_json::{Value, json};

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

/// Issue #436: a PCBA's parts are placed by poses its file wrote in
/// single precision (a quarter turn about x whose cosine is -4.37e-8,
/// axes 1.75e-7 rad off), each standing on the board's top. Every part
/// touches the board and none shares a volume with it: nothing is left
/// unchecked.
#[test]
fn parts_placed_in_single_precision_stand_on_the_board() {
    let (_dir, mut s) = project(3.0);
    must(&mut s, "part.create", json!({"part": "chip"}));
    must(
        &mut s,
        "feature.add",
        json!({"part": "chip", "type": "box", "id": "body",
               "params": {"x": 1.6, "y": 0.45, "z": 0.8},
               "placement": {"origin": [-0.8, 0, -0.4]}}),
    );
    place(&mut s, "asm", "plate", "plate", [0.0; 3], Z, X);
    let poses: [([f64; 3], [f64; 3], [f64; 3]); 4] = [
        (
            [9.225082525937, 11.125093777813, 5.0],
            [1.748455531469e-07, -1.0, -4.371140912996e-08],
            [1.0, 1.748455531469e-07, 0.0],
        ),
        (
            [25.224945196836, 19.424959500469, 5.0],
            [-8.742277657348e-08, 1.0, -4.371140912996e-08],
            [-1.0, -8.742277657348e-08, 0.0],
        ),
        (
            [32.885147223203, 24.414858182109, 5.0],
            [1.0, 4.371138828674e-08, -4.371140912996e-08],
            [-4.371138828674e-08, 1.0, 0.0],
        ),
        (
            [6.0, 22.0, 5.0],
            [0.0, -1.0, -4.371139e-08],
            [1.0, 0.0, 0.0],
        ),
    ];
    for (i, (o, z, x)) in poses.into_iter().enumerate() {
        place(&mut s, "asm", &format!("r{i}"), "chip", o, z, x);
    }
    let v = must(&mut s, "interference", json!({"part": "asm"}));
    assert_eq!(v["pairs"], 4, "{v}");
    assert_eq!(pairs(&v), []);
    assert_eq!(v["unchecked"], json!([]), "{v}");
}

#[test]
fn mesh_verdicts_agree_with_booleans_on_solids() {
    use ryucad::interference::{Prepared, Verdict, mesh_verdict, verdict};
    use ryukei::geom::{Rigid, UnitVector3, Vector3};
    use ryukei::name::OpKey;
    let mut g = ryukei_testkit::Lcg::new(364);
    let (mut agreed, mut judged) = (0, 0);
    for k in 0..48 {
        let key = OpKey::from_bytes(format!("a{k}").as_bytes());
        let a = if k % 2 == 0 {
            ryukei::primitive::block(key, 0.01, 0.008, 0.006, &Rigid::IDENTITY)
                .unwrap()
                .0
        } else {
            ryukei::primitive::cylinder(key, 0.004, 0.01, &Rigid::IDENTITY)
                .unwrap()
                .0
        };
        let b = ryukei::primitive::block(
            OpKey::from_bytes(b"b"),
            0.004,
            0.004,
            0.004,
            &Rigid::IDENTITY,
        )
        .unwrap()
        .0;
        let rel = Rigid::rotation(UnitVector3::Z, g.next_f64(0.0, 1.0));
        let rel = Rigid::translation(Vector3::new(
            g.next_f64(-0.006, 0.012),
            g.next_f64(-0.006, 0.010),
            g.next_f64(-0.006, 0.008),
        ))
        .then_after(&rel);
        let exact = verdict(&a, &b, rel);
        let (pa, pb) = (Prepared::new(&a).unwrap(), Prepared::new(&b).unwrap());
        let meshed = mesh_verdict(&pa, &pb, &rel).unwrap();
        match exact {
            Verdict::Interferes { volume_m3, .. } if volume_m3 > 1e-9 => {
                judged += 1;
                assert!(
                    matches!(meshed, Verdict::Approximate { open: false, .. }),
                    "{k}: {meshed:?}"
                );
                agreed += 1;
            }
            Verdict::Clear => {
                judged += 1;
                assert!(
                    !matches!(meshed, Verdict::Approximate { .. }),
                    "{k}: {meshed:?}"
                );
                agreed += 1;
            }
            _ => {}
        }
    }
    // A part placed twice at one pose, or a micrometre off it: its surfaces
    // coincide, so no surface point of either is deep in the other, yet
    // the boolean gives the whole volume.
    for k in 0..2 {
        let key = OpKey::from_bytes(format!("c{k}").as_bytes());
        let a = if k == 0 {
            ryukei::primitive::block(key, 0.01, 0.008, 0.006, &Rigid::IDENTITY)
                .unwrap()
                .0
        } else {
            ryukei::primitive::cylinder(key, 0.004, 0.01, &Rigid::IDENTITY)
                .unwrap()
                .0
        };
        let pa = Prepared::new(&a).unwrap();
        for rel in [
            Rigid::IDENTITY,
            Rigid::translation(Vector3::new(1e-6, 0.0, 0.0)),
        ] {
            let exact = verdict(&a, &a, rel);
            let meshed = mesh_verdict(&pa, &pa, &rel).unwrap();
            if let Verdict::Interferes { volume_m3, .. } = exact {
                assert!(volume_m3 > 1e-9, "coincident {k}: {exact:?}");
                judged += 1;
                assert!(
                    matches!(meshed, Verdict::Approximate { open: false, .. }),
                    "coincident {k} {rel:?}: {meshed:?}"
                );
                agreed += 1;
            } else {
                // Whatever the boolean says of coincident copies, the
                // meshes must certify the shared volume.
                assert!(
                    matches!(meshed, Verdict::Approximate { open: false, .. }),
                    "coincident {k} {rel:?}: {exact:?} {meshed:?}"
                );
            }
        }
    }
    println!("mesh verdicts: {agreed} of {judged} judged agree");
    assert!(judged >= 24, "{judged}");
    assert_eq!(agreed, judged);
}

#[test]
fn mesh_verdicts_on_drilled_plates_agree_with_booleans() {
    use ryucad::interference::{Prepared, Verdict, mesh_verdict, verdict};
    use ryukei::boolean::{BooleanOp, Tool};
    use ryukei::geom::{Rigid, UnitVector3, Vector3};
    use ryukei::name::{OpKey, ToolKey};
    let at = |x: f64, y: f64, z: f64| Rigid::translation(Vector3::new(x, y, z));
    // A 20 × 10 × 1.6 mm plate with ten 1 mm holes on a 2.54 mm pitch.
    let plate0 = ryukei::primitive::block(
        OpKey::from_bytes(b"pl"),
        0.02,
        0.01,
        0.0016,
        &Rigid::IDENTITY,
    )
    .unwrap()
    .0;
    let hole =
        ryukei::primitive::cylinder(OpKey::from_bytes(b"h"), 0.0005, 0.004, &Rigid::IDENTITY)
            .unwrap()
            .0;
    let places: Vec<Rigid> = (0..5)
        .flat_map(|i| {
            [0.005 - 0.00127, 0.005 + 0.00127].map(|y| at(0.004 + i as f64 * 0.00254, y, -0.001))
        })
        .collect();
    let tools: Vec<Tool> = places
        .iter()
        .enumerate()
        .map(|(k, p)| Tool {
            key: ToolKey(OpKey::from_bytes(format!("h{k}").as_bytes()).0),
            body: &hole,
            place: *p,
        })
        .collect();
    let plate = ryukei::boolean::boolean(
        OpKey::from_bytes(b"d"),
        BooleanOp::Subtract,
        &plate0,
        &tools,
    )
    .unwrap()
    .0;
    let comp = ryukei::primitive::block(
        OpKey::from_bytes(b"c"),
        0.012,
        0.005,
        0.003,
        &Rigid::IDENTITY,
    )
    .unwrap()
    .0;
    let (pp, pc) = (
        Prepared::new(&plate).unwrap(),
        Prepared::new(&comp).unwrap(),
    );
    let mut g = ryukei_testkit::Lcg::new(5250);
    let (mut agreed, mut judged) = (0, 0);
    for k in 0..6 {
        let turn = Rigid::rotation(UnitVector3::Z, g.next_f64(0.0, 0.4));
        for dz in [0.0, -0.0005, 0.0005] {
            // The component over the holes, turned about its corner, flush,
            // sunk half a millimetre or raised as much.
            let rel = at(0.003, 0.0025, 0.0016 + dz).then_after(&turn);
            let exact = verdict(&plate, &comp, rel);
            let meshed = mesh_verdict(&pp, &pc, &rel).expect("decided within the budget");
            match exact {
                Verdict::Interferes { volume_m3, .. } if volume_m3 > 1e-9 => {
                    judged += 1;
                    assert!(
                        matches!(meshed, Verdict::Approximate { open: false, .. }),
                        "{k} {dz}: {meshed:?}"
                    );
                    agreed += 1;
                }
                Verdict::Clear => {
                    judged += 1;
                    assert!(
                        !matches!(meshed, Verdict::Approximate { .. }),
                        "{k} {dz}: {meshed:?}"
                    );
                    agreed += 1;
                }
                _ => {}
            }
            if dz == 0.0 {
                assert_eq!(meshed, Verdict::Touching, "{k}: flush only touches");
            }
            let swapped =
                mesh_verdict(&pc, &pp, &rel.inverse()).expect("decided within the budget");
            assert_eq!(
                std::mem::discriminant(&swapped),
                std::mem::discriminant(&meshed),
                "{k} {dz}: {meshed:?} {swapped:?}"
            );
        }
    }
    println!("drilled plates: {agreed} of {judged} judged agree");
    assert!(judged >= 6, "{judged}");
    assert_eq!(agreed, judged);
}
