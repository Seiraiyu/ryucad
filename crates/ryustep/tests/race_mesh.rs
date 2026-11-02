//! Issues #288 and #278: which of the race's bodies mesh slowly, by the
//! hash of their dumps, so a body is the same body at every commit.
//! Ignored: minutes in release. `RACE_SLOW_S` (default 1) is the time
//! past which a body is listed; `RACE_DUMP` writes the slowest body's dump
//! and encoding to that folder.

use ryukei::mesh::{MeshParams, tessellate};
use std::time::Instant;

fn fnv(s: &str) -> u64 {
    let mut h = 0xcbf2_9ce4_8422_2325u64;
    for b in s.bytes() {
        h ^= u64::from(b);
        h = h.wrapping_mul(0x0000_0100_0000_01b3);
    }
    h
}

#[test]
#[ignore = "minutes: the race's bodies through STEP, each meshed; run by name in release"]
fn the_race_bodies_mesh_times() {
    let slow: f64 = std::env::var("RACE_SLOW_S")
        .ok()
        .and_then(|s| s.parse().ok())
        .unwrap_or(1.0);
    let bodies = ryukei_testkit::race::writable(1600);
    let step = ryustep::write(
        &ryustep::WriteModel {
            products: vec![ryustep::Product {
                id: "race".into(),
                name: "race".into(),
                bodies,
                ..Default::default()
            }],
        },
        &ryustep::WriteOptions {
            unit: ryustep::LengthUnit::Millimeter,
            time_stamp: "2026-10-30T00:00:00".into(),
            ..Default::default()
        },
    )
    .unwrap();
    let read = ryustep::read(
        &step,
        &ryustep::ReadOptions {
            key: ryukei::name::OpKey(7),
            heal: Default::default(),
        },
    )
    .unwrap();
    let healed: Vec<_> = read
        .bodies
        .iter()
        .filter_map(|b| b.result.as_ref().ok().map(|r| r.0.clone()))
        .collect();
    println!("healed {}", healed.len());
    let params = MeshParams::new(1e-3, 15f64.to_radians());
    let mut worst = (0.0f64, 0usize);
    for (i, b) in healed.iter().enumerate() {
        let t = Instant::now();
        let m = tessellate(b, &params).unwrap();
        let s = t.elapsed().as_secs_f64();
        if s > worst.0 {
            worst = (s, i);
        }
        if s > slow {
            println!(
                "slow healed[{i}] dump {:016x} {s:.2} s {} triangles {} faces faults {}",
                fnv(&b.dump()),
                m.welded_triangles().count(),
                b.faces().count(),
                m.faults.len()
            );
        }
    }
    if let Ok(dir) = std::env::var("RACE_DUMP") {
        let b = &healed[worst.1];
        std::fs::write(format!("{dir}/race-slow.dump"), b.dump()).unwrap();
        std::fs::write(
            format!("{dir}/race-slow.body"),
            ryukei::codec::encode_body(b),
        )
        .unwrap();
    }
}
