//! Measurement of seeded bodies ([`ryukei_testkit::boolean_bytes`]): the
//! target's mass properties, and the distance from it to the first tool
//! where the case places it. Each must be a typed refusal
//! (`Unsupported`, `Uncertain`) or a value whose bound holds against
//! sampling: the volume a mesh at another chord encloses, and every pair
//! of mesh points no nearer than the distance's lower end. Any panic, and
//! any other error, is a bug.

#![no_main]

use libfuzzer_sys::fuzz_target;
use ryukei::measure::{DistanceOptions, Entity, MassOptions, distance, mass};
use ryukei::mesh::{MeshParams, tessellate};
use ryukei::{Error, MeasureError};
use ryukei_testkit::boolean_bytes::boolean_case;

fn refused(e: &Error) -> bool {
    matches!(
        e,
        Error::InvalidMeasure(MeasureError::Unsupported | MeasureError::Uncertain)
    )
}

fuzz_target!(|data: &[u8]| {
    let Some(case) = boolean_case(data) else {
        return;
    };
    let body = &case.target;
    let chord = 1e-2;
    let params = MeshParams {
        max_triangles: 200_000,
        ..MeshParams::new(chord, 0.5)
    };
    let Ok(sample) = tessellate(body, &params) else {
        return;
    };
    if !sample.faults.is_empty() {
        return;
    }
    match mass(body, 1.0, &MassOptions::default()) {
        Ok(m) => {
            let volume: f64 = sample
                .welded_triangles()
                .map(|t| {
                    let [a, b, c] = t.map(|i| sample.positions[i as usize].to_vector());
                    a.dot(b.cross(c)) / 6.0
                })
                .sum();
            let slack = 2.0 * chord * (m.area.value() + m.area.bound()) * 1.01;
            assert!(
                m.volume.holds(volume, slack),
                "{:?} against a mesh's {volume}",
                m.volume
            );
            assert!(m.volume.bound().is_finite() && m.area.bound().is_finite());
        }
        Err(e) => assert!(refused(&e), "unexpected {e:?}"),
    }
    let tools = case.tools();
    let Some(tool) = tools.first() else {
        return;
    };
    let Ok(other) = tessellate(tool.body, &params) else {
        return;
    };
    match distance(
        (body, Entity::Body, &ryukei::geom::Rigid::IDENTITY),
        (tool.body, Entity::Body, &tool.place),
        &DistanceOptions::default(),
    ) {
        Ok(d) => {
            let lo = d.distance.value() - d.distance.bound() - 1e-9;
            for p in sample.positions.iter().step_by(7) {
                for q in other.positions.iter().step_by(7) {
                    assert!(p.distance(tool.place.apply_point(*q)) >= lo, "{d:?}");
                }
            }
        }
        Err(e) => assert!(refused(&e), "unexpected {e:?}"),
    }
});
