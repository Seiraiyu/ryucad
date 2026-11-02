//! Seeded blends on tolerant twins (design 299, plan 14.B): each of 96
//! cases from `ryukei_testkit::blend_bytes`, its body moved off its faces
//! by one of `tolerant_bytes::MOVES` and healed with tightening off, the
//! same edges blended in the twin. Wherever the exact blend succeeds, the
//! tolerant one must too, check clean, cross nothing, mesh, and enclose
//! the exact result's volume within both bounds and its tolerances'
//! strip. Release only.

use ryukei::blend::blend;
use ryukei::check::{Fault, check, check_intersections};
use ryukei::measure::{Entity, Properties, properties};
use ryukei::mesh::{MeshParams, tessellate};
use ryukei::name::OpKey;
use ryukei::topo::Body;
use ryukei_testkit::Lcg;
use ryukei_testkit::tolerant_bytes::tolerant_blend_case;

fn volume(b: &Body) -> (f64, f64, f64) {
    let Properties::Body { volume, area } = properties(b, Entity::Body).unwrap() else {
        unreachable!()
    };
    (volume.value(), volume.bound(), area.value())
}

fn worst(b: &Body) -> f64 {
    b.edges()
        .map(|e| b.edge(e).unwrap().tolerance.get())
        .chain(b.vertices().map(|v| b.vertex(v).unwrap().tolerance.get()))
        .fold(0.0, f64::max)
}

#[test]
#[cfg_attr(
    debug_assertions,
    ignore = "minutes in a debug build: CI runs it in release"
)]
fn tolerant_blends_match_their_exact_twins() {
    let mut g = Lcg::new(0x746f_6c62_6c65_6e64);
    let (mut compared, mut skipped) = (0, 0);
    let mut failures = Vec::new();
    for seed in 0..96 {
        let data: Vec<u8> = (0..128).map(|_| (g.next_u64() >> 56) as u8).collect();
        let Some(case) = tolerant_blend_case(&data) else {
            skipped += 1;
            continue;
        };
        let Ok((exact, _)) = blend(OpKey(0xb1e), &case.exact.body, &case.exact.blends) else {
            skipped += 1;
            continue;
        };
        compared += 1;
        let what = format!("seed {seed}, moved by {:e}", case.by);
        let t0 = std::time::Instant::now();
        let t = match blend(OpKey(0xb1e), &case.tolerant.body, &case.tolerant.blends) {
            Ok((t, _)) => t,
            Err(e) => {
                failures.push(format!("{what}: {e:?}"));
                continue;
            }
        };
        let took = t0.elapsed().as_secs_f64();
        if took > 10.0 {
            println!("{what}: {took:.1} s");
        }
        if !check(&t).is_empty() {
            failures.push(format!("{what}: {:?}", check(&t)));
            continue;
        }
        let crossing: Vec<Fault> = check_intersections(&t)
            .into_iter()
            .filter(|f| !matches!(f, Fault::CannotCheck { .. }))
            .collect();
        if !crossing.is_empty() {
            failures.push(format!("{what}: {crossing:?}"));
            continue;
        }
        match tessellate(&t, &MeshParams::new(1e-3, 0.3)) {
            Ok(m) if m.faults.is_empty() => {}
            other => {
                failures.push(format!("{what}: mesh {:?}", other.map(|m| m.faults)));
                continue;
            }
        }
        let ((ve, be, ae), (vt, bt, _)) = (volume(&exact), volume(&t));
        let strip = 2.0 * worst(&t) * ae;
        if (vt - ve).abs() > be + bt + strip {
            failures.push(format!(
                "{what}: volume {vt} against {ve} (bounds {be}, {bt}, strip {strip})"
            ));
        }
    }
    println!("{compared} compared, {skipped} skipped");
    assert!(failures.is_empty(), "{failures:#?}");
    assert!(compared >= 32, "only {compared} cases compared");
}
