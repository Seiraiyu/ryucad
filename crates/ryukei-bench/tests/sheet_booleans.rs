//! Booleans on sheets head to head with OCCT (design 295, plan 12.D):
//! sheets trimmed by solids against `BRepAlgoAPI_BooleanOperation`'s
//! common and cut with the sheet's shell as the argument, and solids and
//! sheets divided by sheets against `BRepAlgoAPI_Splitter`. OCCT reads
//! each operand from a STEP file ryustep writes, in millimetres (its
//! measures are scaled back to meters). ryukei's areas and volumes must
//! hold OCCT's, piece by piece in sorted order, to a millionth; both
//! kernels' times are printed, never raced (design 295 races sewing
//! only). SMLib's shim has no booleans of shells and takes no part.
//! Devcontainer only (`--features occt`); run in release.

use core::f64::consts::TAU;
use ryukei::boolean::{BooleanOp, Divided, Tool, ToolKey, boolean, divide};
use ryukei::geom::{Cylinder, Frame, Plane, Point3, Rigid, Surface, Vector3};
use ryukei::measure::{Entity, Properties, properties};
use ryukei::name::OpKey;
use ryukei::primitive;
use ryukei::sheet::{from_surface, remove};
use ryukei::topo::{Body, BodyKind};
use ryukei_bench::occt;
use ryustep::{LengthUnit, Product, WriteModel, WriteOptions, write};
use std::path::Path;
use std::time::Instant;

/// Square millimetres and cubic millimetres in meters.
const MM2: f64 = 1e-6;
const MM3: f64 = 1e-9;

fn to_occt(dir: &Path, name: &str, body: &Body) -> occt::Solid {
    let bytes = write(
        &WriteModel {
            products: vec![Product {
                id: name.into(),
                name: name.into(),
                bodies: vec![body.clone()],
                ..Default::default()
            }],
        },
        &WriteOptions {
            unit: LengthUnit::Millimeter,
            ..WriteOptions::default()
        },
    )
    .unwrap();
    let path = dir.join(format!("{name}.stp"));
    std::fs::write(&path, &bytes).unwrap();
    occt::Solid::read_step(&path, false).unwrap()
}

fn tool(body: &Body) -> Tool<'_> {
    Tool {
        key: ToolKey(2),
        body,
        place: Rigid::IDENTITY,
    }
}

fn plane(origin: Point3, z: Vector3, x: Vector3, rect: [[f64; 2]; 2]) -> Body {
    let frame = Frame::new(origin, z, x).unwrap();
    from_surface(OpKey(1), &Surface::Plane(Plane::new(frame)), rect)
        .unwrap()
        .0
}

fn flat(z: f64, half: f64) -> Body {
    plane(
        Point3::new(0.0, 0.0, z),
        Vector3::new(0.0, 0.0, 1.0),
        Vector3::new(1.0, 0.0, 0.0),
        [[-half, half], [-half, half]],
    )
}

fn block(at: [f64; 3], size: [f64; 3]) -> Body {
    let place = Rigid::translation(Vector3::new(at[0], at[1], at[2]));
    primitive::block(OpKey(3), size[0], size[1], size[2], &place)
        .unwrap()
        .0
}

/// Trims judged by their total area rather than face by face. STEP gives
/// the band (a whole turn of a cylinder) a seam at `u = 0`, on +x, so OCCT's
/// common with the block on +x is that half band cut in two at the seam
/// (two faces of `π / 2`), where ryukei's face has no seam: one face of `π`,
/// the closed form (radius 1, height 1, half a turn). Measured on cox-worker-2
/// on 2026-11-02, both totals agreed within 1e-13.
const SEAMED: [&str; 1] = ["band by a block"];

/// A body's faces' areas, or its solids' volumes, sorted.
fn measures(bodies: &[&Body]) -> Vec<f64> {
    let mut out = Vec::new();
    for b in bodies {
        match b.kind() {
            BodyKind::Solid => match properties(b, Entity::Body).unwrap() {
                Properties::Body { volume, .. } => out.push(volume.value()),
                p => panic!("{p:?}"),
            },
            _ => {
                for f in b.faces() {
                    match properties(b, Entity::Face(f)).unwrap() {
                        Properties::Face { area, .. } => out.push(area.value()),
                        p => panic!("{p:?}"),
                    }
                }
            }
        }
    }
    out.sort_by(f64::total_cmp);
    out
}

/// Whether two sorted lists agree to a millionth, printing both.
fn agree(case: &str, ours: &[f64], theirs: &[f64]) -> bool {
    let ok = ours.len() == theirs.len()
        && ours
            .iter()
            .zip(theirs)
            .all(|(a, b)| (a - b).abs() <= 1e-6 * a.abs().max(b.abs()).max(1e-9));
    println!(
        "{case}: ryukei {ours:?}, OCCT {theirs:?}{}",
        if ok { "" } else { "  DISAGREE" }
    );
    ok
}

#[test]
fn trims_and_divisions_agree_with_occt() {
    let dir = std::env::temp_dir().join("ryukei-bench-sheet-booleans");
    std::fs::create_dir_all(&dir).unwrap();
    let mut failed = Vec::new();
    // Trims: a sheet and a solid tool.
    let band = from_surface(
        OpKey(1),
        &Surface::Cylinder(Cylinder::new(Frame::WORLD, 1.0).unwrap()),
        [[0.0, TAU], [0.0, 2.0]],
    )
    .unwrap()
    .0;
    let ball = primitive::sphere(OpKey(4), 1.0, &Rigid::IDENTITY)
        .unwrap()
        .0;
    let skin = remove(OpKey(5), &ball, &[]).unwrap().0;
    let plate = block([0.0, 0.0, 0.0], [2.0, 2.0, 0.5]);
    let rod = primitive::cylinder(
        OpKey(6),
        0.4,
        2.0,
        &Rigid::translation(Vector3::new(1.0, 1.0, -0.5)),
    )
    .unwrap()
    .0;
    let drilled = boolean(OpKey(7), BooleanOp::Subtract, &plate, &[tool(&rod)])
        .unwrap()
        .0;
    let section = plane(
        Point3::new(1.0, 1.0, 0.25),
        Vector3::new(0.0, 1.0, 0.0),
        Vector3::new(1.0, 0.0, 0.0),
        [[-2.0, 2.0], [-1.0, 1.0]],
    );
    let trims: [(&str, &Body, Body); 4] = [
        (
            "square by a block",
            &flat(0.5, 2.0),
            block([0.0, 0.0, 0.0], [2.0, 2.0, 1.0]),
        ),
        (
            "band by a block",
            &band,
            block([0.0, -2.0, 0.5], [2.0, 4.0, 1.0]),
        ),
        (
            "skin by a block",
            &skin,
            block([-2.0, -2.0, 0.5], [4.0, 4.0, 4.0]),
        ),
        ("section by a drilled plate", &section, drilled),
    ];
    for (k, (case, sheet, solid)) in trims.iter().enumerate() {
        let theirs_sheet = to_occt(&dir, &format!("trim{k}-sheet"), sheet);
        let theirs_solid = to_occt(&dir, &format!("trim{k}-solid"), solid);
        for op in [BooleanOp::Intersect, BooleanOp::Subtract] {
            let t = Instant::now();
            let ours = boolean(OpKey(8), op, sheet, &[tool(solid)]).unwrap().0;
            let ours_s = t.elapsed().as_secs_f64();
            let t = Instant::now();
            let theirs = theirs_sheet
                .boolean(op, std::slice::from_ref(&theirs_solid), false)
                .unwrap();
            let theirs_s = t.elapsed().as_secs_f64();
            println!("{case} {op:?}: ryukei {ours_s:.3} s, OCCT {theirs_s:.3} s");
            let mut a: Vec<f64> = theirs.face_areas().iter().map(|a| a * MM2).collect();
            a.sort_by(f64::total_cmp);
            let mut ours = measures(&[&ours]);
            if SEAMED.contains(case) {
                ours = vec![ours.iter().sum()];
                a = vec![a.iter().sum()];
            }
            if !agree(&format!("{case} {op:?}"), &ours, &a) {
                failed.push(format!("{case} {op:?}"));
            }
        }
    }
    // Divisions: a target and a sheet tool.
    let cube = block([-2.0, -2.0, -2.0], [4.0, 4.0, 4.0]);
    let square = plane(
        Point3::new(0.0, 0.0, 0.0),
        Vector3::new(0.0, 0.0, 1.0),
        Vector3::new(1.0, 0.0, 0.0),
        [[0.0, 4.0], [0.0, 4.0]],
    );
    let wall = plane(
        Point3::new(1.0, 0.0, 0.0),
        Vector3::new(1.0, 0.0, 0.0),
        Vector3::new(0.0, 1.0, 0.0),
        [[-1.0, 5.0], [-1.0, 1.0]],
    );
    let divisions: [(&str, &Body, &Body); 4] = [
        (
            "box by a plane",
            &block([0.0, 0.0, 0.0], [2.0, 2.0, 2.0]),
            &flat(0.5, 3.0),
        ),
        ("ball by a plane", &ball, &flat(0.3, 2.0)),
        ("box by a ball's skin", &cube, &skin),
        ("sheet by a wall", &square, &wall),
    ];
    for (k, (case, target, sheet)) in divisions.iter().enumerate() {
        let theirs_target = to_occt(&dir, &format!("divide{k}-target"), target);
        let theirs_tool = to_occt(&dir, &format!("divide{k}-tool"), sheet);
        let t = Instant::now();
        let (d, _): (Divided, _) = divide(OpKey(9), target, &[tool(sheet)]).unwrap();
        let ours_s = t.elapsed().as_secs_f64();
        let t = Instant::now();
        let theirs = theirs_target
            .split(std::slice::from_ref(&theirs_tool))
            .unwrap();
        let theirs_s = t.elapsed().as_secs_f64();
        println!("{case}: ryukei {ours_s:.3} s, OCCT {theirs_s:.3} s");
        let pieces: Vec<&Body> = d.front.iter().chain(&d.back).chain(&d.apart).collect();
        let mut b: Vec<f64> = if target.kind() == BodyKind::Solid {
            theirs.solids().iter().map(|s| s.volume * MM3).collect()
        } else {
            theirs.face_areas().iter().map(|a| a * MM2).collect()
        };
        b.sort_by(f64::total_cmp);
        if !agree(case, &measures(&pieces), &b) {
            failed.push(case.to_string());
        }
    }
    assert!(failed.is_empty(), "{failed:?}");
}
