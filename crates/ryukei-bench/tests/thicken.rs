//! Thickened sheets head to head with OCCT (design 295, plan 12.E):
//! sheets of planes, cylinders, spheres and a spline patch, an L of two
//! box faces and an open box, each thickened one way at a time by ryukei
//! (`offset::thicken` with `back` or `front` zero) and by OCCT's
//! `BRepOffset_MakeOffset` in thickening mode (a negative offset for the
//! back). OCCT reads each sheet from a STEP file ryustep writes, in
//! millimetres (its volumes are scaled back to meters). Where a case has
//! a closed form, ryukei must hold it within its bound; where OCCT holds
//! it too and ryukei doesn't, or (cases with no closed form) where the
//! two kernels differ by more than a millionth, the test fails. Both
//! kernels' times are printed, never raced (design 295 races sewing
//! only). SMLib's shim has no thickening and takes no part. Devcontainer
//! only (`--features occt`); run in release.

use core::f64::consts::{FRAC_PI_2, PI};
use ryukei::geom::KnotVector;
use ryukei::geom::{Cylinder, Frame, NurbsSurface, Plane, Point3, Rigid, Sphere, Surface};
use ryukei::measure::{Entity, Properties, properties};
use ryukei::name::{FaceRole, Name, OpKey, Role};
use ryukei::offset::{ThickenSpec, thicken};
use ryukei::primitive;
use ryukei::sheet::{extract, from_surface, remove};
use ryukei::topo::Body;
use ryukei_bench::occt;
use ryustep::{LengthUnit, Product, WriteModel, WriteOptions, write};
use std::path::Path;
use std::sync::Arc;
use std::time::Instant;

/// Cubic millimetres in cubic meters, and millimetres in meters.
const MM3: f64 = 1e-9;
const MM: f64 = 1e3;

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

fn cube(x: f64) -> f64 {
    x * x * x
}

/// A case: its name, its sheet and its volume's closed form for
/// `(front, back)`, if it has one.
type Case = (&'static str, Body, Option<fn(f64, f64) -> f64>);

fn cases() -> Vec<Case> {
    let patch = |s: Surface, rect| from_surface(OpKey(1), &s, rect).unwrap().0;
    let (block, _) = primitive::block(OpKey(2), 1.0, 0.6, 0.4, &Rigid::IDENTITY).unwrap();
    let face = |r| Name::derive(OpKey(2), Role::Face(r), &[]);
    // A bicubic bump over the unit square, its middle control points
    // lifted.
    let knots = || KnotVector::clamped(3, &[0.0, 1.0], &[4, 4]).unwrap();
    let mut points = Vec::new();
    for i in 0..4 {
        for j in 0..4 {
            let lift = if (1..3).contains(&i) && (1..3).contains(&j) {
                0.2
            } else {
                0.0
            };
            points.push(Point3::new(i as f64 / 3.0, j as f64 / 3.0, lift));
        }
    }
    let bump = NurbsSurface::new([knots(), knots()], points, None).unwrap();
    vec![
        (
            "square",
            patch(
                Surface::Plane(Plane::new(Frame::WORLD)),
                [[0.0, 1.0], [0.0, 1.0]],
            ),
            Some(|f, b| f + b),
        ),
        (
            "quarter cylinder",
            patch(
                Surface::Cylinder(Cylinder::new(Frame::WORLD, 1.0).unwrap()),
                [[0.0, FRAC_PI_2], [0.0, 0.5]],
            ),
            Some(|f, b| FRAC_PI_2 * 0.5 * ((1.0 + f) * (1.0 + f) - (1.0 - b) * (1.0 - b)) / 2.0),
        ),
        (
            "sphere's patch",
            patch(
                Surface::Sphere(Sphere::new(Frame::WORLD, 1.0).unwrap()),
                [[0.0, 1.0], [0.2, 0.9]],
            ),
            Some(|f, b| {
                let lat = ryukei::math::sin(0.9) - ryukei::math::sin(0.2);
                lat * (cube(1.0 + f) - cube(1.0 - b)) / 3.0
            }),
        ),
        (
            "L",
            extract(
                OpKey(3),
                &block,
                &[face(FaceRole::PlusZ), face(FaceRole::MinusY)],
            )
            .unwrap()
            .0,
            Some(|f, b| 1.0 * 1.0 * (f + b) + PI * f * f / 4.0 - b * b),
        ),
        (
            "open box",
            remove(OpKey(3), &block, &[face(FaceRole::PlusZ)])
                .unwrap()
                .0,
            None,
        ),
        (
            "spline bump",
            patch(Surface::Nurbs(Arc::new(bump)), [[0.0, 1.0], [0.0, 1.0]]),
            None,
        ),
    ]
}

#[test]
fn thickening_agrees_with_occt() {
    let dir = std::env::temp_dir().join("ryukei-bench-thicken");
    std::fs::create_dir_all(&dir).unwrap();
    let mut failed = Vec::new();
    for (k, (case, sheet, exact)) in cases().into_iter().enumerate() {
        let theirs_sheet = to_occt(&dir, &format!("sheet{k}"), &sheet);
        for (f, b) in [(0.05, 0.0), (0.0, 0.03)] {
            let t = Instant::now();
            let spec = ThickenSpec {
                front: f,
                back: b,
                faces: Vec::new(),
            };
            let ours = thicken(OpKey(9), &sheet, &spec).map(|(s, _)| s);
            let ours_s = t.elapsed().as_secs_f64();
            let t = Instant::now();
            let theirs = theirs_sheet.thicken(if f > 0.0 { f * MM } else { -b * MM });
            let theirs_s = t.elapsed().as_secs_f64();
            println!("{case} ({f}, {b}): ryukei {ours_s:.3} s, OCCT {theirs_s:.3} s");
            let ours = match ours {
                Ok(s) => match properties(&s, Entity::Body).unwrap() {
                    Properties::Body { volume, .. } => volume,
                    p => panic!("{p:?}"),
                },
                Err(e) => {
                    println!("  ryukei refused: {e:?}");
                    failed.push(format!("{case} ({f}, {b}): ryukei refused"));
                    continue;
                }
            };
            let theirs = theirs.map(|s| s.volume() * MM3);
            println!(
                "  ryukei {ours:?}, OCCT {theirs:?}, exact {:?}",
                exact.map(|x| x(f, b))
            );
            match (exact, theirs) {
                (Some(x), theirs) => {
                    let x = x(f, b);
                    let ours_ok = (ours.value() - x).abs() <= ours.bound() + 1e-9;
                    let theirs_ok = theirs.is_ok_and(|v| (v - x).abs() <= 1e-6 * x);
                    if !ours_ok && theirs_ok {
                        failed.push(format!("{case} ({f}, {b}): OCCT holds the closed form"));
                    } else if !ours_ok {
                        failed.push(format!("{case} ({f}, {b}): off the closed form"));
                    }
                }
                (None, Ok(v)) => {
                    if (ours.value() - v).abs() > 1e-6 * v.abs() + ours.bound() {
                        failed.push(format!("{case} ({f}, {b}): DISAGREE"));
                    }
                }
                (None, Err(e)) => println!("  OCCT refused: {e}"),
            }
        }
    }
    assert!(failed.is_empty(), "{failed:#?}");
}
