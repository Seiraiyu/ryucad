//! Seeded booleans on tolerant twins (design 299, plan 14.A): each of 96
//! cases from `ryukei_testkit::boolean_bytes`, its operands moved off
//! their faces by one of `tolerant_bytes::MOVES` and healed with
//! tightening off. Wherever the exact boolean succeeds, the tolerant one
//! must too, check clean, cross nothing, mesh, and enclose the exact
//! result's volume within both bounds and its tolerances' strip, or be
//! refused as past the tolerance cap (design 299: the operands' own
//! tolerances may already be near it). Release only.

use ryukei::boolean::boolean;
use ryukei::check::{Fault, check, check_intersections};
use ryukei::measure::{Entity, Properties, properties};
use ryukei::mesh::{MeshParams, tessellate};
use ryukei::name::OpKey;
use ryukei::topo::Body;
use ryukei::{BooleanError, Error};
use ryukei_testkit::Lcg;
use ryukei_testkit::tolerant_bytes::tolerant_case;

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
fn tolerant_booleans_match_their_exact_twins() {
    let mut g = Lcg::new(0x746f_6c65_7261_6e74);
    let (mut compared, mut skipped, mut capped) = (0, 0, 0);
    let mut failures = Vec::new();
    for seed in 0..96 {
        let data: Vec<u8> = (0..96).map(|_| (g.next_u64() >> 56) as u8).collect();
        let Some(case) = tolerant_case(&data) else {
            skipped += 1;
            continue;
        };
        let Ok((exact, _)) = boolean(
            OpKey(9),
            case.exact.op,
            &case.exact.target,
            &case.exact.tools(),
        ) else {
            skipped += 1;
            continue;
        };
        compared += 1;
        let what = format!("seed {seed}, moved by {:e}", case.by);
        let t = match boolean(
            OpKey(9),
            case.tolerant.op,
            &case.tolerant.target,
            &case.tolerant.tools(),
        ) {
            Ok((t, _)) => t,
            Err(Error::InvalidBoolean(BooleanError::ToleranceTooLarge { .. })) => {
                capped += 1;
                continue;
            }
            Err(e) => {
                failures.push(format!("{what}: {e:?}"));
                continue;
            }
        };
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
        if exact.faces().next().is_none() {
            if t.faces().next().is_some() {
                failures.push(format!(
                    "{what}: the exact result is empty, the tolerant one isn't"
                ));
            }
            continue;
        }
        let ((ve, be, ae), (vt, bt, _)) = (volume(&exact), volume(&t));
        let strip = 2.0 * worst(&t) * ae;
        if (vt - ve).abs() > be + bt + strip {
            failures.push(format!(
                "{what}: volume {vt} against {ve} (bounds {be}, {bt}, strip {strip})"
            ));
        }
    }
    println!("{compared} compared ({capped} past the cap), {skipped} skipped");
    assert!(compared >= 48, "only {compared} cases compared");
    assert!(failures.is_empty(), "{failures:#?}");
}
