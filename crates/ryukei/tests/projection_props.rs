//! Hidden lines against a mesh (design 326, plan 10.C): seeded bodies
//! from seeded directions. Release only (`cargo test -p ryukei --release
//! --test projection_props`): debug builds take minutes over a traced
//! silhouette.

use ryukei::Error;
use ryukei::geom::{Rigid, Vector3};
use ryukei::project::{Placed, ViewOptions, view};
use ryukei::topo::Body;
use ryukei_testkit::Lcg;
use ryukei_testkit::projection::{Oracle, judge, look, placed_bodies, silhouette_excess, size};

/// Cases found to disagree, by name, each with its issue: none yet.
const KNOWN: &[(&str, &str)] = &[];

fn cases() -> Vec<(String, Body)> {
    let mut out: Vec<(String, Body)> = ryukei_testkit::race::bodies(24)
        .into_iter()
        .enumerate()
        .map(|(i, b)| (format!("race {i}"), b))
        .collect();
    for seed in 0..16u64 {
        let bytes = ryukei_testkit::race::seed_bytes(seed);
        if let Some((b, _, _)) = ryukei_testkit::along_bytes::carried(&bytes) {
            out.push((format!("along {seed}"), b));
        }
        if let Some((sheets, _)) = ryukei_testkit::sheet_bytes::sheet(&bytes) {
            for (k, s) in sheets.into_iter().enumerate() {
                out.push((format!("sheet {seed}.{k}"), s));
            }
        }
    }
    out.retain(|(_, b)| b.faces().next().is_some());
    out
}

#[test]
#[cfg_attr(debug_assertions, ignore = "release only")]
fn views_agree_with_a_fine_mesh() {
    let mut rng = Lcg::new(0x10c);
    let (mut samples, mut unsure, mut uncertain, mut pieces, mut refused) = (0, 0, 0, 0, 0);
    let mut wrong = Vec::new();
    for (name, body) in cases() {
        for _ in 0..2 {
            let t = Vector3::new(
                rng.next_f64(-1.0, 1.0),
                rng.next_f64(-1.0, 1.0),
                rng.next_f64(-1.0, 1.0),
            );
            if t.length() < 0.2 {
                continue;
            }
            let v = look(t);
            let placed = [Placed {
                body: &body,
                place: Rigid::IDENTITY,
            }];
            let p = match view(&placed, &v, &ViewOptions::default()) {
                Ok(p) => p,
                Err(Error::InvalidView(e)) => {
                    eprintln!("{name}: refused {e:?}");
                    refused += 1;
                    continue;
                }
                Err(e) => panic!("{name}: {e:?}"),
            };
            let bodies = placed_bodies(&placed);
            let excess = silhouette_excess(&bodies, &p, &v);
            assert!(
                excess <= 0.0,
                "{name} {t:?}: a silhouette {excess} off square"
            );
            let Some(o) = Oracle::new(&bodies, &v, size(&bodies) * 1e-3) else {
                eprintln!("{name}: its mesh has faults, not judged");
                continue;
            };
            let j = judge(&p, &bodies, &o);
            samples += j.samples;
            unsure += j.unsure;
            uncertain += j.uncertain;
            pieces += j.pieces;
            if !j.wrong.is_empty() && !KNOWN.iter().any(|(n, _)| *n == name) {
                wrong.push((name.clone(), t, j.wrong));
            }
        }
    }
    eprintln!(
        "{samples} samples judged, {unsure} unsure; {uncertain} of {pieces} pieces uncertain; {refused} views refused"
    );
    assert!(wrong.is_empty(), "{wrong:#?}");
    assert!(samples > 1000, "too few samples judged: {samples}");
    assert!(
        uncertain * 10 <= pieces,
        "{uncertain} of {pieces} pieces uncertain"
    );
}
