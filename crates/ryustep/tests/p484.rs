//! #917: the race's p484, alone.

use ryukei::mesh::{MeshFaultKind, MeshParams, tessellate};

/// The race's p484 (step_bar's race file's 485th part: blend seed 267's
/// body), written to STEP and read back healed under the key ryucad's
/// import gives part `p484`'s feature `body`, meshes at 1 mm and 15
/// degrees without a BadDomain face (red in step_bar on every platform
/// on main since 2026-11-16).
#[test]
fn the_race_p484_meshes_after_a_round_trip() {
    let body = ryukei_testkit::race::blend_body(267).unwrap();
    let bytes = ryustep::write(
        &ryustep::WriteModel {
            products: vec![ryustep::Product {
                id: "p484".into(),
                name: "p484".into(),
                bodies: vec![body],
                ..Default::default()
            }],
        },
        &ryustep::WriteOptions {
            unit: ryustep::LengthUnit::Millimeter,
            time_stamp: "2026-10-27T00:00:00".into(),
            ..Default::default()
        },
    )
    .unwrap();
    let back = ryustep::read(
        &bytes,
        &ryustep::ReadOptions {
            // ryucad's op_key(part p484, feature body).
            key: ryukei::name::OpKey::from_bytes(b"ryucad.feature\0p484\0body"),
            heal: ryukei::heal::HealOptions {
                open: ryukei::heal::Open::Sheet,
                ..Default::default()
            },
        },
    )
    .unwrap();
    let healed = &back.bodies[0].result.as_ref().unwrap().0;
    let mesh = tessellate(healed, &MeshParams::new(1e-3, 15f64.to_radians())).unwrap();
    let bad: Vec<String> = mesh
        .faults
        .iter()
        .filter(|f| f.kind == MeshFaultKind::BadDomain)
        .map(|f| format!("{}", healed.face(f.face).unwrap().name))
        .collect();
    assert!(bad.is_empty(), "BadDomain faces {bad:?}");
}
