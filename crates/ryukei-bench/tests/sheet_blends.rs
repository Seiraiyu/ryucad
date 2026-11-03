//! Blends on sheets head to head with OCCT (design 295, plan 12.F): an L
//! of two box faces filleted and chamfered to its open ends, an open box
//! rounded inside (ball corners at the bottom, open ends at the rim) and
//! chamfered round its bottom, and a cup (a cylinder without its top)
//! filleted round its bottom rim, by ryukei (`blend::blend` on the sheet)
//! and by OCCT's `BRepFilletAPI_MakeFillet` and `_MakeChamfer` on the
//! sheet's shell, which OCCT reads from a STEP file ryustep writes, in
//! millimetres (its areas are scaled back to meters). Where a case has a
//! closed form, ryukei must hold it; where OCCT holds it too and ryukei
//! doesn't, or (no closed form) where the two kernels' areas differ by
//! more than a millionth, the test fails. OCCT's refusals are printed,
//! never judged. Both kernels' times are printed, never raced (design
//! 295 races sewing only). SMLib's shim has no blends of shells and takes
//! no part. Devcontainer only (`--features occt`); run in release.

use core::f64::consts::{FRAC_PI_2, PI};
use ryukei::blend::{BlendKind, Chamfer, EdgeBlend, blend};
use ryukei::geom::{Point3, Rigid};
use ryukei::measure::{Entity, Properties, properties};
use ryukei::name::{FaceRole, Name, OpKey, Role};
use ryukei::primitive;
use ryukei::sheet::{extract, remove};
use ryukei::topo::Body;
use ryukei_bench::{BenchBlend, BenchKind, occt};
use ryustep::{LengthUnit, Product, WriteModel, WriteOptions, write};
use std::path::Path;
use std::time::Instant;

/// Square millimetres in square meters, and millimetres in meters.
const MM2: f64 = 1e-6;
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

/// The middle of an edge, in millimetres: how OCCT picks it.
fn middle(b: &Body, name: Name) -> [f64; 3] {
    let Some(ryukei::topo::EntityId::Edge(e)) = b.by_name(name) else {
        panic!("no edge {name:?}")
    };
    let e = b.edge(e).unwrap();
    let c = b.curve(e.curve).unwrap();
    let p: Point3 = c.point((e.interval[0] + e.interval[1]) / 2.0);
    [p.x * MM, p.y * MM, p.z * MM]
}

/// A case: its name, its sheet, its blends and its area's closed form.
type Case = (&'static str, Body, Vec<EdgeBlend>, Option<f64>);

fn cases() -> Vec<Case> {
    use FaceRole::*;
    let k = OpKey(2);
    let (block, _) = primitive::block(k, 1.0, 0.6, 0.4, &Rigid::IDENTITY).unwrap();
    let face = |r| Name::derive(k, Role::Face(r), &[]);
    let edge = |a, b| {
        let mut f = [face(a), face(b)];
        f.sort();
        Name::derive(k, Role::Edge, &f)
    };
    let fillet = |edge, radius| EdgeBlend {
        edge,
        kind: BlendKind::Fillet { radius },
    };
    let chamfer = |edge, distance| EdgeBlend {
        edge,
        kind: BlendKind::Chamfer(Chamfer::Equal { distance }),
    };
    let l = extract(OpKey(3), &block, &[face(PlusZ), face(MinusY)])
        .unwrap()
        .0;
    let open = remove(OpKey(3), &block, &[face(PlusZ)]).unwrap().0;
    let bottom = [
        edge(MinusZ, PlusX),
        edge(MinusZ, MinusX),
        edge(MinusZ, PlusY),
        edge(MinusZ, MinusY),
    ];
    let upright = [
        edge(PlusX, PlusY),
        edge(PlusY, MinusX),
        edge(MinusX, MinusY),
        edge(MinusY, PlusX),
    ];
    let (cup, _) = primitive::cylinder(k, 0.3, 0.5, &Rigid::IDENTITY).unwrap();
    let cup = remove(OpKey(3), &cup, &[face(Top)]).unwrap().0;
    let rim: Vec<Name> = cup
        .edges()
        .filter(|&e| !cup.is_laminar(e))
        .map(|e| cup.edge(e).unwrap().name)
        .collect();
    // The L: the top (1 × 0.6) and the front (1 × 0.4), its edge 1 long.
    let (r, d) = (0.1, 0.08);
    let l_area = 1.0 * 0.6 + 1.0 * 0.4;
    // The open box: 1 × 0.6 × 0.4 without its top, every inner edge
    // rounded inside by `s`: flats, quarter cylinders, eighth spheres.
    let s = 0.05;
    let (a, b, h) = (1.0 - 2.0 * s, 0.6 - 2.0 * s, 0.4 - s);
    let box_area = a * b
        + 2.0 * (a + b) * h
        + FRAC_PI_2 * s * (2.0 * (a + b) + 4.0 * h)
        + 2.0 * PI * s * s;
    // The cup, radius 0.3 and 0.5 tall, its rim rounded by `s`.
    let (rc, hc) = (0.3, 0.5);
    let cup_area = PI * (rc - s) * (rc - s)
        + 2.0 * PI * rc * (hc - s)
        + 2.0 * PI * s * ((rc - s) * FRAC_PI_2 + s);
    vec![
        (
            "L filleted",
            l.clone(),
            vec![fillet(edge(PlusZ, MinusY), r)],
            Some(l_area - 2.0 * r + FRAC_PI_2 * r),
        ),
        (
            "L chamfered",
            l,
            vec![chamfer(edge(PlusZ, MinusY), d)],
            Some(l_area - 2.0 * d + d * 2f64.sqrt()),
        ),
        (
            "open box rounded",
            open.clone(),
            bottom.iter().chain(&upright).map(|&e| fillet(e, s)).collect(),
            Some(box_area),
        ),
        (
            "open box chamfered",
            open,
            bottom.iter().map(|&e| chamfer(e, 0.04)).collect(),
            None,
        ),
        (
            "cup",
            cup,
            rim.iter().map(|&e| fillet(e, s)).collect(),
            Some(cup_area),
        ),
    ]
}

#[test]
fn sheet_blends_agree_with_occt() {
    let dir = std::env::temp_dir().join("ryukei-bench-sheet-blends");
    std::fs::create_dir_all(&dir).unwrap();
    let mut failed = Vec::new();
    for (k, (case, sheet, blends, exact)) in cases().into_iter().enumerate() {
        let theirs_sheet = to_occt(&dir, &format!("sheet{k}"), &sheet);
        let bench: Vec<BenchBlend> = blends
            .iter()
            .map(|b| BenchBlend {
                at: middle(&sheet, b.edge),
                kind: match b.kind {
                    BlendKind::Fillet { radius } => BenchKind::Fillet(radius * MM),
                    BlendKind::Chamfer(Chamfer::Equal { distance }) => {
                        BenchKind::Equal(distance * MM)
                    }
                    _ => unreachable!(),
                },
            })
            .collect();
        let t = Instant::now();
        let ours = blend(OpKey(9), &sheet, &blends).map(|(s, _)| s);
        let ours_s = t.elapsed().as_secs_f64();
        let t = Instant::now();
        let theirs = theirs_sheet.blend(&bench);
        let theirs_s = t.elapsed().as_secs_f64();
        println!("{case}: ryukei {ours_s:.3} s, OCCT {theirs_s:.3} s");
        let ours = match ours {
            Ok(s) => match properties(&s, Entity::Body).unwrap() {
                Properties::Sheet { area } => area.value(),
                p => panic!("{p:?}"),
            },
            Err(e) => {
                println!("  ryukei refused: {e:?}");
                failed.push(format!("{case}: ryukei refused"));
                continue;
            }
        };
        let theirs = theirs.map(|s| s.area() * MM2);
        println!("  ryukei {ours}, OCCT {theirs:?}, exact {exact:?}");
        match (exact, theirs) {
            (Some(x), theirs) => {
                let ours_ok = (ours - x).abs() <= 1e-7 * x;
                let theirs_ok = theirs.as_ref().is_ok_and(|v| (v - x).abs() <= 1e-6 * x);
                if !ours_ok && theirs_ok {
                    failed.push(format!("{case}: OCCT holds the closed form"));
                } else if !ours_ok {
                    failed.push(format!("{case}: off the closed form"));
                }
            }
            (None, Ok(v)) => {
                if (ours - v).abs() > 1e-6 * v.abs() {
                    failed.push(format!("{case}: DISAGREE"));
                }
            }
            (None, Err(e)) => println!("  OCCT refused: {e}"),
        }
    }
    assert!(failed.is_empty(), "{failed:#?}");
}
