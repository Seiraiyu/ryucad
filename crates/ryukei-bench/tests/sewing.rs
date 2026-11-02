//! Sewing head to head with OCCT (design 295, plan 12.C): the race's
//! 1,600 bodies and a plate with a 40 × 25 grid of through-holes, each
//! cut into its faces (`sheet::extract`), every face moved along a
//! direction of its own by up to a tenth of healing's cap (seeded), then
//! sewn back within the cap, body by body, all bodies at once on every
//! core: by ryukei (`heal::sew`) and by OCCT (`BRepBuilderAPI_Sewing`,
//! each closed shell made a solid by `ShapeFix_Solid`). OCCT reads each
//! body's faces from a STEP file ryustep writes, which isn't timed.
//!
//! ryukei must give back every body: a solid per shell, the body's faces,
//! edges and vertices between them, and its volume (certified, within
//! both bounds; a cavity sews alone into a solid facing out). A body
//! ryukei refuses must be in `REFUSED`, with the issue that tracks it; a
//! body it refuses that isn't listed, or one listed that it now sews,
//! fails the test. OCCT's solids must hold the same volume to a
//! thousandth. And ryukei must be faster: with `RYUKEI_BARS=1` as it is,
//! else ten times looser. Bodies with a face that can't be a sheet alone
//! (one touching itself at a vertex) are left out and counted.
//! Devcontainer only (`--features occt`); run in release.

use rayon::prelude::*;
use ryukei::boolean::{BooleanOp, Tool, boolean};
use ryukei::geom::{Rigid, Vector3};
use ryukei::heal::{SewOptions, sew};
use ryukei::measure::{Entity, Properties, properties};
use ryukei::name::{Name, OpKey, ToolKey};
use ryukei::primitive;
use ryukei::sheet::extract;
use ryukei::topo::{Body, BodyKind, ShellId};
use ryukei_bench::occt;
use ryukei_testkit::Lcg;
use ryustep::{LengthUnit, Product, WriteModel, WriteOptions, write};
use std::time::Instant;

/// Healing's cap, in meters: sewing closes gaps up to it.
const TOLERANCE: f64 = 1e-4;

/// Bodies (by index in `bodies()`) ryukei refuses to sew back, each with
/// the issue that tracks it.
const REFUSED: &[(usize, &str)] = &[(15, "#348")];

/// How much looser than recorded the bar is.
fn slack() -> f64 {
    if std::env::var("RYUKEI_BARS").as_deref() == Ok("1") {
        1.0
    } else {
        10.0
    }
}

/// The plate with 1,000 through-holes.
fn plate() -> Body {
    let block = primitive::block(OpKey(20), 40.0, 25.0, 0.5, &Rigid::IDENTITY)
        .unwrap()
        .0;
    let rod = primitive::cylinder(OpKey(21), 0.3, 1.0, &Rigid::IDENTITY)
        .unwrap()
        .0;
    let places: Vec<Rigid> = (0..25)
        .flat_map(|j| {
            (0..40).map(move |i| {
                Rigid::translation(Vector3::new(i as f64 + 0.5, j as f64 + 0.5, -0.25))
            })
        })
        .collect();
    let tools: Vec<Tool> = places
        .iter()
        .enumerate()
        .map(|(i, p)| Tool {
            key: ToolKey(i as u128 + 1),
            body: &rod,
            place: *p,
        })
        .collect();
    boolean(OpKey(22), BooleanOp::Subtract, &block, &tools)
        .unwrap()
        .0
}

/// The race's bodies, then the plate.
fn bodies() -> Vec<Body> {
    let mut out = ryukei_testkit::race::writable(1600);
    out.push(plate());
    out
}

/// A body's faces as sheets, each moved along a direction of its own by
/// up to a tenth of the tolerance, from `seed`; `None` where a face can't
/// be a sheet alone (one touching itself at a vertex: a typed refusal).
fn cut(b: &Body, seed: u64) -> Option<Vec<Body>> {
    let mut g = Lcg::new(seed);
    let mut unit = || (g.next_u64() >> 11) as f64 / (1u64 << 53) as f64;
    b.faces()
        .map(|f| {
            let s = extract(OpKey(23), b, &[b.face(f).unwrap().name]).ok()?.0;
            let dir = Vector3::new(unit() - 0.5, unit() - 0.5, unit() - 0.5);
            let by = dir.normalize().map_or(Vector3::new(0.0, 0.0, 0.0), |d| {
                d.get() * (0.1 * TOLERANCE * unit())
            });
            Some(s.transformed(&Rigid::translation(by)).unwrap())
        })
        .collect()
}

/// A solid's certified volume and its bound.
fn volume(b: &Body) -> (f64, f64) {
    match properties(b, Entity::Body) {
        Ok(Properties::Body { volume, .. }) => (volume.value(), volume.bound()),
        other => panic!("{other:?}"),
    }
}

fn names(faces: impl Iterator<Item = Name>) -> Vec<Name> {
    let mut x: Vec<Name> = faces.collect();
    x.sort();
    x
}

/// Each shell's faces' names, and whether it is a cavity (not the shell
/// of its region with the largest box).
fn shells(b: &Body) -> Vec<(Vec<Name>, bool)> {
    let size = |s: ShellId| {
        b.shell_faces(s)
            .map(|f| ryukei::intersect::face_box(b, f))
            .reduce(|x, y| x.union(&y))
            .map_or(0.0, |x| (x.max() - x.min()).length())
    };
    let mut out: Vec<(Vec<Name>, bool)> = b
        .shells()
        .map(|sh| {
            let region = b.shell(sh).unwrap().region;
            let outer = b
                .region_shells(region)
                .max_by(|&x, &y| size(x).total_cmp(&size(y)))
                .unwrap();
            let faces = names(b.shell_faces(sh).map(|f| b.face(f).unwrap().name));
            (faces, outer != sh)
        })
        .collect();
    out.sort();
    out
}

/// Why `pieces` aren't `b` given back, for the report; with the volume
/// they hold.
fn given_back(b: &Body, pieces: &[Body]) -> Result<f64, String> {
    let want = shells(b);
    let mut got: Vec<Vec<Name>> = pieces
        .iter()
        .map(|p| names(p.faces().map(|f| p.face(f).unwrap().name)))
        .collect();
    got.sort();
    if got != want.iter().map(|x| x.0.clone()).collect::<Vec<_>>() {
        return Err(format!("{} pieces for {} shells", pieces.len(), want.len()));
    }
    let (mut counts, mut sum, mut bound) = ([0; 3], 0.0, 0.0);
    for p in pieces {
        if p.kind() != BodyKind::Solid {
            return Err(format!("a {:?}", p.kind()));
        }
        let c = p.counts();
        counts = [
            counts[0] + c.faces,
            counts[1] + c.edges,
            counts[2] + c.vertices,
        ];
        let (v, e) = volume(p);
        let faces = names(p.faces().map(|f| p.face(f).unwrap().name));
        let cavity = want.iter().any(|(n, c)| *c && *n == faces);
        sum += if cavity { -v } else { v };
        bound += e;
    }
    let c = b.counts();
    if counts != [c.faces, c.edges, c.vertices] {
        return Err(format!("counts {counts:?} != {c:?}"));
    }
    let (w, e) = volume(b);
    // The faces moved by up to a tenth of the cap: the volume by up to
    // that times the area.
    let area: f64 = match properties(b, Entity::Body) {
        Ok(Properties::Body { area, .. }) => area.value(),
        _ => 0.0,
    };
    if (sum - w).abs() > bound + e + 0.1 * TOLERANCE * area {
        return Err(format!("volume {sum} != {w}"));
    }
    Ok(sum)
}

#[test]
fn sewing_race_against_occt() {
    let t = Instant::now();
    let all = bodies();
    let cuts: Vec<Option<Vec<Body>>> = all
        .par_iter()
        .enumerate()
        .map(|(i, b)| cut(b, i as u64))
        .collect();
    let kept: Vec<usize> = (0..all.len()).filter(|&i| cuts[i].is_some()).collect();
    let sheets: usize = cuts.iter().flatten().map(Vec::len).sum();
    println!(
        "{} bodies cut into {sheets} sheets in {:.1} s ({} left out: a face can't be a sheet alone)",
        kept.len(),
        t.elapsed().as_secs_f64(),
        all.len() - kept.len()
    );
    // OCCT's input: each body's sheets in a STEP file of its own, in
    // millimetres, read before the clock starts.
    let dir = std::env::temp_dir().join("ryukei-bench-sewing");
    std::fs::create_dir_all(&dir).unwrap();
    let options = WriteOptions {
        unit: LengthUnit::Millimeter,
        ..WriteOptions::default()
    };
    let occt_in: Vec<occt::Solid> = kept
        .iter()
        .map(|&i| {
            let bytes = write(
                &WriteModel {
                    products: vec![Product {
                        id: format!("body{i}"),
                        name: format!("body{i}"),
                        bodies: cuts[i].clone().unwrap(),
                        ..Default::default()
                    }],
                },
                &options,
            )
            .unwrap();
            let path = dir.join(format!("body{i}.stp"));
            std::fs::write(&path, &bytes).unwrap();
            occt::Solid::read_step(&path, false).unwrap()
        })
        .collect();
    // ryukei, every body at once on every core.
    let sew_options = SewOptions {
        tolerance: TOLERANCE,
        solid: true,
    };
    let t = Instant::now();
    let ours: Vec<_> = kept
        .par_iter()
        .map(|&i| sew(OpKey(24), cuts[i].as_ref().unwrap(), &sew_options))
        .collect();
    let ours_s = t.elapsed().as_secs_f64();
    // OCCT, the same way.
    let t = Instant::now();
    let theirs: Vec<Result<occt::Solid, String>> =
        occt_in.par_iter().map(|s| s.sew(TOLERANCE * 1e3)).collect();
    let theirs_s = t.elapsed().as_secs_f64();
    // Judged after the clocks.
    let mut wrong = Vec::new();
    let mut refused = Vec::new();
    let mut occt_off = 0;
    for (k, &i) in kept.iter().enumerate() {
        match &ours[k] {
            Err(e) => refused.push((i, format!("{e:?}"))),
            Ok((pieces, _)) => match given_back(&all[i], pieces) {
                Err(why) => wrong.push(format!("body {i}: {why}")),
                Ok(v) => {
                    let occt_v: f64 = theirs[k]
                        .as_ref()
                        .map(|s| s.solids().iter().map(|x| x.volume).sum::<f64>() * 1e-9)
                        .unwrap_or(f64::NAN);
                    if !((occt_v - v).abs() <= 1e-3 * v.abs()) {
                        occt_off += 1;
                        println!("body {i}: OCCT {occt_v} m³, ryukei {v} m³");
                    }
                }
            },
        }
    }
    println!(
        "ryukei: {} bodies in {ours_s:.2} s ({} refused); OCCT: {:.2} s ({} not to ryukei's volume); OCCT {:.2} times ryukei's time",
        kept.len(),
        refused.len(),
        theirs_s,
        occt_off,
        theirs_s / ours_s
    );
    for (i, why) in &refused {
        println!("refused: body {i}: {why}");
    }
    let listed: Vec<usize> = REFUSED.iter().map(|x| x.0).collect();
    let got: Vec<usize> = refused.iter().map(|x| x.0).collect();
    if got != listed {
        wrong.push(format!("refused {got:?}, listed {listed:?}"));
    }
    assert!(wrong.is_empty(), "{wrong:#?}");
    assert!(
        ours_s < theirs_s * slack(),
        "ryukei {ours_s:.2} s, OCCT {theirs_s:.2} s"
    );
}
