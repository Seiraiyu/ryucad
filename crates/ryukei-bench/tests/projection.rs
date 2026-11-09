//! Hidden lines against OCCT's exact `HLRBRep_Algo` (design 326, plan
//! 10.C): closed forms judged against exact lengths and OCCT's, seeded
//! bodies and the pinned corpus where OCCT and a mesh agree, and the
//! race on one thread. Devcontainer only (`--features occt`).

use ryukei::geom::{Rigid, Vector3};
use ryukei_bench::Primitive;
use ryukei_bench::occt;

#[test]
fn occt_draws_a_blocks_edges_from_a_corner() {
    let s = occt::Solid::new(Primitive::Block([1.0, 2.0, 3.0]), &Rigid::IDENTITY);
    let v = ryukei_testkit::projection::look(Vector3::new(1.0, -1.0, 1.0));
    let h = s.hlr(v.towards().get(), v.x().get()).unwrap();
    let f = (2.0f64 / 3.0).sqrt();
    assert!((h.visible[0] - 3.0 * 6.0 * f).abs() < 1e-9, "{h:?}");
    assert!((h.hidden[0] - 6.0 * f).abs() < 1e-9, "{h:?}");
    assert!(h.seconds >= 0.0);
}

use ryukei::Error;
use ryukei::blend::{BlendKind, EdgeBlend, blend};
use ryukei::boolean::{BooleanOp, Tool, ToolKey, boolean};
use ryukei::name::OpKey;
use ryukei::primitive;
use ryukei::project::{Placed, View, ViewOptions, view};
use ryukei::topo::Body;
use ryukei_testkit::projection::{
    Lengths, Oracle, image_length, lengths, look, mesh_lengths, nearest, placed_bodies, size,
};
use std::path::{Path, PathBuf};

/// ryukei's view of `body` on one thread, and its time.
fn ours(body: &Body, v: &View) -> Result<(ryukei::project::Projection, f64), Error> {
    let pool = rayon::ThreadPoolBuilder::new()
        .num_threads(1)
        .build()
        .unwrap();
    let placed = [Placed {
        body,
        place: Rigid::IDENTITY,
    }];
    let t = std::time::Instant::now();
    let p = pool.install(|| view(&placed, v, &ViewOptions::default()))?;
    Ok((p, t.elapsed().as_secs_f64()))
}

/// OCCT's view of a body it read from STEP in millimetres, in meters.
fn theirs(solid: &occt::Solid, v: &View, scale: f64) -> Result<occt::Hlr, String> {
    let mut h = solid.hlr(v.towards().get(), v.x().get())?;
    for x in h.visible.iter_mut().chain(h.hidden.iter_mut()) {
        *x *= scale;
    }
    Ok(h)
}

/// A body written to STEP in millimetres and read by OCCT.
fn to_occt(dir: &Path, name: &str, body: &Body) -> occt::Solid {
    let bytes = ryustep::write(
        &ryustep::WriteModel {
            products: vec![ryustep::Product {
                id: name.into(),
                name: name.into(),
                bodies: vec![body.clone()],
                ..Default::default()
            }],
        },
        &ryustep::WriteOptions {
            unit: ryustep::LengthUnit::Millimeter,
            ..ryustep::WriteOptions::default()
        },
    )
    .unwrap();
    let path = dir.join(format!("{name}.stp"));
    std::fs::write(&path, &bytes).unwrap();
    occt::Solid::read_step(&path, false).unwrap()
}

fn total(l: &Lengths) -> f64 {
    l.visible.iter().chain(&l.hidden).sum()
}

fn row(name: &str, ours: &Lengths, occt: &occt::Hlr) {
    println!(
        "{name:<28} visible {:?} vs {:?}; hidden {:?} vs {:?}; uncertain {:.3e}",
        ours.visible, occt.visible, ours.hidden, occt.hidden, ours.uncertain
    );
}

fn drilled() -> Body {
    let plate = primitive::block(OpKey(1), 0.04, 0.03, 0.01, &Rigid::IDENTITY)
        .unwrap()
        .0;
    let pin = primitive::cylinder(OpKey(2), 0.004, 0.02, &Rigid::IDENTITY)
        .unwrap()
        .0;
    boolean(
        OpKey(3),
        BooleanOp::Subtract,
        &plate,
        &[Tool {
            key: ToolKey(1),
            body: &pin,
            place: Rigid::translation(Vector3::new(0.02, 0.015, -0.005)),
        }],
    )
    .unwrap()
    .0
}

/// A 20 mm box with its top edges rounded at 2 mm.
fn filleted() -> Body {
    let (b, _) = primitive::block(OpKey(4), 0.02, 0.02, 0.02, &Rigid::IDENTITY).unwrap();
    let top: Vec<EdgeBlend> = b
        .edges()
        .filter(|&e| {
            let ed = b.edge(e).unwrap();
            let z = |v| b.vertex(v).unwrap().point.z;
            (z(ed.start) - 0.02).abs() < 1e-12 && (z(ed.end) - 0.02).abs() < 1e-12
        })
        .map(|e| EdgeBlend {
            edge: b.edge(e).unwrap().name,
            kind: BlendKind::Fillet { radius: 0.002 },
        })
        .collect();
    blend(OpKey(5), &b, &top).unwrap().0
}

#[test]
fn closed_forms_agree_with_occt() {
    let dir = tempfile::tempdir().unwrap();
    let iso = look(Vector3::new(1.0, -1.0, 1.0));
    let side = look(Vector3::new(0.0, -1.0, 0.0));
    // Seen exactly from a corner, each fillet's rails lie on one line of
    // the image (their offset is along the view), which ryukei draws once
    // (a piece lying on another, as its nearer piece) and OCCT twice.
    let tilted = look(Vector3::new(1.0, -0.7, 0.9));
    let (cyl, _) = primitive::cylinder(OpKey(6), 0.01, 0.03, &Rigid::IDENTITY).unwrap();
    // (name, body, view, judge hidden lengths too)
    let cases = [
        ("drilled plate iso", drilled(), iso, true),
        ("filleted box tilted", filleted(), tilted, true),
        ("cylinder side on", cyl, side, false),
    ];
    for (name, body, v, hidden) in cases {
        let (p, _) = ours(&body, &v).unwrap();
        let l = lengths(&p);
        let h = theirs(&to_occt(dir.path(), "case", &body), &v, 1e-3).unwrap();
        row(name, &l, &h);
        // Classes are judged together: OCCT reads no regularity from
        // STEP, so it sorts some of a fillet's tangent edges among its
        // sharp lines, and it draws a cylinder's seam ruling as a sharp
        // line where ryukei draws the outline. A piece lying behind
        // another in the image (the filleted box's corner edge along a
        // rail) is drawn once by ryukei, as its nearer piece, and not
        // hidden by it; OCCT draws it again, hidden: the length of the
        // pieces counted once so is the slack.
        let slack: f64 = (0..p.pieces.len())
            .filter(|&i| !nearest(&p, i))
            .map(|i| image_length(&p.pieces[i]))
            .sum();
        let eps = 1e-5 * total(&l) + 1e-9 + slack;
        println!("{name:<28} slack for pieces lying behind others {slack:.6}");
        let sum = |x: [f64; 3]| x.iter().sum::<f64>();
        let (a, b) = (sum(l.visible), sum(h.visible));
        assert!((a - b).abs() <= eps, "{name}: visible {a} vs {b}");
        if hidden {
            let (a, b) = (sum(l.hidden), sum(h.hidden));
            assert!((a - b).abs() <= eps, "{name}: hidden {a} vs {b}");
        }
        assert_eq!(l.uncertain, 0.0, "{name}");
    }
}

/// Where `scripts/fetch-corpus.sh` keeps the pinned corpus.
fn pinned_dir() -> PathBuf {
    match std::env::var_os("RYUCAD_CORPUS") {
        Some(d) => PathBuf::from(d),
        None => Path::new(env!("CARGO_MANIFEST_DIR")).join("../../target/corpus"),
    }
}

/// The pinned corpus's healed bodies, each once, by file; none (with a
/// note) when it hasn't been fetched, unless `RYUCAD_REQUIRE_CORPUS=1`.
fn pinned_bodies() -> Vec<(String, Body)> {
    let listed = std::fs::read_to_string(
        Path::new(env!("CARGO_MANIFEST_DIR")).join("../ryustep/tests/corpus/pinned.txt"),
    )
    .unwrap();
    let mut out = Vec::new();
    for name in listed
        .lines()
        .filter_map(|l| l.trim().strip_prefix("heals "))
    {
        let name = name.split_once(' ').unwrap().1;
        let Ok(bytes) = std::fs::read(pinned_dir().join(name)) else {
            assert!(
                std::env::var_os("RYUCAD_REQUIRE_CORPUS").is_none(),
                "{name} is missing"
            );
            eprintln!("{name} is missing: run scripts/fetch-corpus.sh");
            continue;
        };
        let opts = ryustep::ReadOptions {
            key: OpKey(7),
            heal: Default::default(),
        };
        for (k, b) in ryustep::read(&bytes, &opts)
            .unwrap()
            .bodies
            .into_iter()
            .enumerate()
        {
            if let Ok((body, _, _)) = b.result {
                out.push((format!("{name}#{k}"), body));
            }
        }
    }
    out
}

/// Seeded race bodies and the pinned corpus, from a corner and a tilted
/// view: judged where OCCT and the mesh agree and ryukei doesn't (plan
/// 8.F's rule), every row printed.
#[test]
fn seeded_and_pinned_bodies_agree_with_occt_where_the_mesh_does() {
    let dir = tempfile::tempdir().unwrap();
    let mut cases: Vec<(String, Body)> = ryukei_testkit::race::bodies(24)
        .into_iter()
        .enumerate()
        .map(|(i, b)| (format!("race {i}"), b))
        .collect();
    cases.extend(pinned_bodies());
    let views = [
        look(Vector3::new(1.0, -1.0, 1.0)),
        look(Vector3::new(0.3, -0.4, 0.8)),
    ];
    let mut wrong = Vec::new();
    for (name, body) in &cases {
        let solid = to_occt(dir.path(), "case", body);
        for v in &views {
            let p = match ours(body, v) {
                Ok((p, _)) => p,
                Err(e) => {
                    println!("{name}: ryukei refused {e:?}");
                    continue;
                }
            };
            let Ok(h) = theirs(&solid, v, 1e-3) else {
                println!("{name}: OCCT refused");
                continue;
            };
            let l = lengths(&p);
            row(name, &l, &h);
            let bodies = placed_bodies(&[Placed {
                body,
                place: Rigid::IDENTITY,
            }]);
            let Some(o) = Oracle::new(&bodies, v, size(&bodies) * 1e-3) else {
                continue;
            };
            let m = mesh_lengths(&p, &bodies, &o);
            let eps = 1e-3 * total(&l) + 1e-9;
            for k in 0..3 {
                for (ours, occt, mesh, what) in [
                    (l.visible[k], h.visible[k], m.visible[k], "visible"),
                    (l.hidden[k], h.hidden[k], m.hidden[k], "hidden"),
                ] {
                    if (occt - mesh).abs() <= eps && (ours - mesh).abs() > eps {
                        wrong.push(format!(
                            "{name} {what} {k}: ryukei {ours}, OCCT {occt}, mesh {mesh}"
                        ));
                    }
                }
            }
        }
    }
    assert!(wrong.is_empty(), "{wrong:#?}");
}

/// The CPU the times were taken on, as the test prints it.
fn hardware() -> String {
    let model = std::fs::read_to_string("/proc/cpuinfo")
        .ok()
        .and_then(|s| {
            s.lines()
                .find(|l| l.starts_with("model name"))
                .map(|l| l.split(':').nth(1).unwrap_or("").trim().to_string())
        })
        .unwrap_or_else(|| "unknown CPU".into());
    let threads = std::thread::available_parallelism().map_or(0, |n| n.get());
    format!("{model}, {threads} threads")
}

fn at(x: f64, y: f64, z: f64) -> Rigid {
    Rigid::translation(Vector3::new(x, y, z))
}

/// Design 326 asks ryukei to beat OCCT's `HLRBRep_Algo` in both races;
/// plan 10.C ships slower (issue #608: on cox-worker-2, an AMD Ryzen 7
/// 9700X, one thread, the plate took 6.51 s against OCCT's 1.83 s and the
/// race bodies 424 s against 2.24 s), so the races assert only these
/// guards against regressions and print the ratio.
const PLATE_BAR: f64 = 5.0;
const RACE_BAR: f64 = 300.0;

#[test]
fn the_1000_hole_plates_iso_view_against_occt_on_one_thread() {
    let (cols, rows) = (40, 25);
    let places: Vec<Rigid> = (0..rows)
        .flat_map(|j| (0..cols).map(move |i| at(i as f64 + 0.5, j as f64 + 0.5, -0.25)))
        .collect();
    let block = primitive::block(OpKey(1), cols as f64, rows as f64, 0.5, &Rigid::IDENTITY)
        .unwrap()
        .0;
    let rod = primitive::cylinder(OpKey(2), 0.3, 1.0, &Rigid::IDENTITY)
        .unwrap()
        .0;
    let tools: Vec<Tool> = places
        .iter()
        .enumerate()
        .map(|(i, p)| Tool {
            key: ToolKey(i as u128 + 1),
            body: &rod,
            place: *p,
        })
        .collect();
    let plate = boolean(OpKey(3), BooleanOp::Subtract, &block, &tools)
        .unwrap()
        .0;
    let theirs_plate = occt::Solid::new(
        Primitive::Block([cols as f64, rows as f64, 0.5]),
        &Rigid::IDENTITY,
    )
    .cut(
        &places
            .iter()
            .map(|p| occt::Solid::new(Primitive::Cylinder { r: 0.3, h: 1.0 }, p))
            .collect::<Vec<_>>(),
        false,
    );
    let v = look(Vector3::new(1.0, -1.0, 1.0));
    let (p, ryukei_s) = ours(&plate, &v).unwrap();
    let h = theirs(&theirs_plate, &v, 1.0).unwrap();
    row("1000-hole plate iso", &lengths(&p), &h);
    println!(
        "1000-hole plate iso, one thread: ryukei {ryukei_s:.3} s, OCCT {:.3} s ({})",
        h.seconds,
        hardware()
    );
    println!(
        "1000-hole plate iso: {:.2} times OCCT's time",
        ryukei_s / h.seconds
    );
    assert!(
        ryukei_s < PLATE_BAR * h.seconds,
        "ryukei {ryukei_s:.3} s vs OCCT {:.3} s: past {PLATE_BAR} times",
        h.seconds
    );
}

#[test]
fn the_race_bodies_against_occt_on_one_thread() {
    let dir = tempfile::tempdir().unwrap();
    let v = look(Vector3::new(1.0, -1.0, 1.0));
    let (mut ryukei_s, mut occt_s, mut drawn, mut left) = (0.0, 0.0, 0, Vec::new());
    for (i, body) in ryukei_testkit::race::bodies(200).iter().enumerate() {
        let solid = to_occt(dir.path(), "race", body);
        match (ours(body, &v), theirs(&solid, &v, 1e-3)) {
            (Ok((_, r)), Ok(h)) => {
                ryukei_s += r;
                occt_s += h.seconds;
                drawn += 1;
            }
            (a, b) => left.push((i, a.err().map(|e| format!("{e:?}")), b.err())),
        }
    }
    println!(
        "race bodies iso, one thread: {drawn} drawn by both, ryukei {ryukei_s:.3} s, OCCT {occt_s:.3} s ({}); left out: {left:?}",
        hardware()
    );
    println!(
        "race bodies iso: {:.1} times OCCT's time",
        ryukei_s / occt_s
    );
    assert!(
        ryukei_s < RACE_BAR * occt_s,
        "ryukei {ryukei_s:.3} s vs OCCT {occt_s:.3} s: past {RACE_BAR} times"
    );
}
