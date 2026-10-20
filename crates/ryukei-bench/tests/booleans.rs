//! Booleans head to head (design 32, phase 6): ryukei, OCCT and SMLib
//! unite, intersect and subtract the same placed primitives. ryukei's
//! result must enclose OCCT's exact volume to within its mesh's chord
//! times its area wherever OCCT succeeds; SMLib's results (and its
//! refusals) are reported beside them. Then a block minus a grid of 1000
//! through-holes, on one thread, where ryukei must beat OCCT's time.
//! Prints a report (run with `--nocapture`).

use ryukei::boolean::{BooleanOp, Tool, ToolKey, boolean};
use ryukei::geom::{Rigid, Vector3};
use ryukei::mesh::{MeshParams, check_mesh, tessellate};
use ryukei::name::OpKey;
use ryukei::primitive;
use ryukei::topo::Body;
use ryukei_bench::Primitive;
use ryukei_bench::{occt, smlib};
use ryukei_testkit::mesh::volume_and_area;
use std::f64::consts::FRAC_PI_2;
use std::time::Instant;

const CHORD: f64 = 1e-3;
const ANGLE: f64 = 0.5;
const OPS: [BooleanOp; 3] = [BooleanOp::Unite, BooleanOp::Intersect, BooleanOp::Subtract];

fn ryukei_body(p: Primitive, at: &Rigid) -> Body {
    let k = OpKey(1);
    match p {
        Primitive::Block([x, y, z]) => primitive::block(k, x, y, z, at),
        Primitive::Cylinder { r, h } => primitive::cylinder(k, r, h, at),
        Primitive::Cone { r0, r1: 0.0, h } => primitive::cone(k, r0, h, at),
        Primitive::Cone { r0, r1, h } => primitive::frustum(k, r0, r1, h, at),
        Primitive::Sphere(r) => primitive::sphere(k, r, at),
        Primitive::Torus { major, minor } => primitive::torus(k, major, minor, at),
    }
    .unwrap()
    .0
}

fn at(x: f64, y: f64, z: f64) -> Rigid {
    Rigid::translation(Vector3::new(x, y, z))
}

/// A case: a name, the target and the tool, each placed.
type Case = (&'static str, (Primitive, Rigid), (Primitive, Rigid));

fn cases() -> Vec<Case> {
    let y_axis = Vector3::new(0.0, 1.0, 0.0).normalize().unwrap();
    let across = at(-1.5, 0.0, 0.0).then_after(&Rigid::rotation(y_axis, FRAC_PI_2));
    vec![
        (
            "block and rod through it",
            (Primitive::Block([2.0, 2.0, 2.0]), Rigid::IDENTITY),
            (Primitive::Cylinder { r: 0.5, h: 3.0 }, at(1.0, 1.0, -0.5)),
        ),
        (
            "block and ball on its corner",
            (Primitive::Block([2.0, 2.0, 2.0]), Rigid::IDENTITY),
            (Primitive::Sphere(0.8), at(2.0, 2.0, 2.0)),
        ),
        (
            "crossed cylinders",
            (Primitive::Cylinder { r: 0.5, h: 3.0 }, at(0.0, 0.0, -1.5)),
            (Primitive::Cylinder { r: 0.35, h: 3.0 }, across),
        ),
        (
            "torus and slab",
            (
                Primitive::Torus {
                    major: 1.0,
                    minor: 0.3,
                },
                Rigid::IDENTITY,
            ),
            (Primitive::Block([3.0, 3.0, 0.4]), at(-1.5, -1.5, -0.1)),
        ),
        (
            "cone and ball",
            (
                Primitive::Cone {
                    r0: 0.8,
                    r1: 0.0,
                    h: 1.5,
                },
                Rigid::IDENTITY,
            ),
            (Primitive::Sphere(0.6), at(0.2, 0.0, 1.0)),
        ),
        (
            "frustum and rod off its axis",
            (
                Primitive::Cone {
                    r0: 1.0,
                    r1: 0.5,
                    h: 1.0,
                },
                Rigid::IDENTITY,
            ),
            (Primitive::Cylinder { r: 0.3, h: 2.0 }, at(0.6, 0.1, -0.5)),
        ),
    ]
}

/// ryukei's result: its mesh's volume and area, and the time taken by
/// the boolean alone.
fn ryukei_volume(
    op: BooleanOp,
    (tp, ta): (Primitive, Rigid),
    (sp, sa): (Primitive, Rigid),
) -> (f64, f64, f64) {
    let (target, tool) = (ryukei_body(tp, &ta), ryukei_body(sp, &sa));
    let tools = [Tool {
        key: ToolKey(1),
        body: &tool,
        place: Rigid::IDENTITY,
    }];
    let t = Instant::now();
    let (body, _) = boolean(OpKey(2), op, &target, &tools).expect("ryukei's boolean");
    let ms = t.elapsed().as_secs_f64() * 1e3;
    if body.counts().faces == 0 {
        return (0.0, 0.0, ms);
    }
    let m = tessellate(&body, &MeshParams::new(CHORD, ANGLE)).unwrap();
    assert!(m.faults.is_empty(), "{:?}", m.faults);
    assert_eq!(check_mesh(&body, &m), Ok(()));
    let (v, a) = volume_and_area(&m);
    (v, a, ms)
}

#[test]
fn ryukei_agrees_with_occt() {
    println!(
        "{:<30} {:<10} {:>12} {:>9} {:>12} {:>9} {:>12} {:>9}",
        "case", "op", "ryukei", "ms", "OCCT", "ms", "SMLib", "ms"
    );
    for (name, target, tool) in cases() {
        for op in OPS {
            let (rv, ra, rms) = ryukei_volume(op, target, tool);
            let t = Instant::now();
            let o = occt::Solid::new(target.0, &target.1).boolean(
                op,
                &[occt::Solid::new(tool.0, &tool.1)],
                false,
            );
            let oms = t.elapsed().as_secs_f64() * 1e3;
            let mut s = smlib::Solid::new(target.0, &target.1, false);
            s.add_tool(tool.0, &tool.1);
            let t = Instant::now();
            let sv = s.combine_tools(op, false).map(|()| s);
            let sms = t.elapsed().as_secs_f64() * 1e3;
            let sv = sv
                .ok()
                .map(|s| s.mesh(CHORD, ANGLE, false).soup().volume_and_area().0);
            let ov = o.ok().map(|o| o.volume());
            let show = |r: Option<f64>| r.map_or("refused".to_owned(), |v| format!("{v:.6}"));
            println!(
                "{name:<30} {:<10} {rv:>12.6} {rms:>9.2} {:>12} {oms:>9.2} {:>12} {sms:>9.2}",
                format!("{op:?}"),
                show(ov),
                show(sv),
            );
            if let Some(ov) = ov {
                assert!(
                    (rv - ov).abs() <= CHORD * ra + 1e-9,
                    "{name} {op:?}: ryukei {rv} vs OCCT {ov}"
                );
            }
        }
    }
}

/// The spike's block with a 40 × 25 grid of through-holes.
fn grid() -> (Primitive, Primitive, Vec<Rigid>) {
    let (cols, rows) = (40, 25);
    let block = Primitive::Block([cols as f64, rows as f64, 0.5]);
    let rod = Primitive::Cylinder { r: 0.3, h: 1.0 };
    let mut places = Vec::new();
    for j in 0..rows {
        for i in 0..cols {
            places.push(at(i as f64 + 0.5, j as f64 + 0.5, -0.25));
        }
    }
    (block, rod, places)
}

#[test]
fn block_minus_1000_cylinders_beats_occt_on_one_thread() {
    let (block, rod, places) = grid();
    let pool = rayon::ThreadPoolBuilder::new()
        .num_threads(1)
        .build()
        .unwrap();
    let target = ryukei_body(block, &Rigid::IDENTITY);
    let tool = ryukei_body(rod, &Rigid::IDENTITY);
    let tools: Vec<Tool> = places
        .iter()
        .enumerate()
        .map(|(i, p)| Tool {
            key: ToolKey(i as u128 + 1),
            body: &tool,
            place: *p,
        })
        .collect();
    let t = Instant::now();
    let (body, _) = pool
        .install(|| boolean(OpKey(2), BooleanOp::Subtract, &target, &tools))
        .expect("ryukei's boolean");
    let ryukei_ms = t.elapsed().as_secs_f64() * 1e3;
    assert_eq!(body.counts().faces, 6 + 1000);

    let solid = occt::Solid::new(block, &Rigid::IDENTITY);
    let rods: Vec<occt::Solid> = places.iter().map(|p| occt::Solid::new(rod, p)).collect();
    let t = Instant::now();
    let cut = solid
        .boolean(BooleanOp::Subtract, &rods, false)
        .expect("OCCT cuts");
    let occt_ms = t.elapsed().as_secs_f64() * 1e3;
    assert_eq!(cut.faces(), 6 + 1000);

    println!(
        "block minus 1000 cylinders, one thread: ryukei {ryukei_ms:.0} ms, OCCT {occt_ms:.0} ms"
    );
    assert!(
        ryukei_ms < occt_ms,
        "ryukei {ryukei_ms:.0} ms vs OCCT {occt_ms:.0} ms"
    );
}
