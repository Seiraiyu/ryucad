//! The body codec over the kernel's corpora: primitives, seeded sweeps,
//! booleans and blends, their mirror images, and bodies healed from their
//! seamed shapes. Every body and journal decodes to what was encoded,
//! bit for bit, and a decoded body checks as the original does.

use ryukei::Error;
use ryukei::blend::blend;
use ryukei::boolean::boolean;
use ryukei::check::check;
use ryukei::codec::{decode_body, decode_journal, encode_body, encode_journal};
use ryukei::geom::{Frame, Plane, Point3, Rigid, Vector3};
use ryukei::heal::{HealOptions, heal, seamed};
use ryukei::name::{Journal, OpKey};
use ryukei::primitive;
use ryukei::topo::Body;
use ryukei_testkit::Lcg;
use ryukei_testkit::blend_bytes::blend_case;
use ryukei_testkit::boolean_bytes::boolean_case;
use ryukei_testkit::sweep_bytes::sweep;

fn bytes(seed: u64) -> Vec<u8> {
    let mut g = Lcg::new(seed);
    (0..96).map(|_| (g.next_u64() >> 56) as u8).collect()
}

/// The primitives, then seeded sweeps, booleans and blends, each with
/// the journal that made it.
fn corpus(seeds: u64) -> Vec<(Body, Journal)> {
    let t = Rigid::rotation(Vector3::new(1.0, 2.0, 0.5).normalize().unwrap(), 0.7);
    let k = OpKey(1);
    let mut out = vec![
        primitive::block(k, 1.0, 2.0, 3.0, &t).unwrap(),
        primitive::cylinder(k, 1.0, 2.0, &t).unwrap(),
        primitive::cone(k, 1.0, 2.0, &t).unwrap(),
        primitive::sphere(k, 1.0, &t).unwrap(),
        primitive::torus(k, 2.0, 0.5, &t).unwrap(),
    ];
    for seed in 0..seeds {
        if let Some((b, _)) = sweep(&bytes(seed)) {
            out.push((b, Journal::new()));
        }
        if let Some(case) = boolean_case(&bytes(seed))
            && let Ok(r) = boolean(OpKey(9), case.op, &case.target, &case.tools())
        {
            out.push(r);
        }
        if let Some(case) = blend_case(&bytes(seed))
            && let Ok(r) = blend(OpKey(9), &case.body, &case.blends)
        {
            out.push(r);
        }
    }
    out
}

fn round_trip(b: &Body) {
    let bytes = encode_body(b);
    let d = decode_body(&bytes).unwrap();
    assert_eq!(d.dump(), b.dump());
    assert_eq!(encode_body(&d), bytes);
    assert_eq!(check(&d), check(b));
}

#[test]
fn the_corpus_round_trips_bit_for_bit() {
    let corpus = corpus(16);
    assert!(corpus.len() > 30, "{}", corpus.len());
    for (b, j) in &corpus {
        round_trip(b);
        assert_eq!(&decode_journal(&encode_journal(j)).unwrap(), j);
    }
}

#[test]
fn mirror_images_and_healed_bodies_round_trip() {
    let plane = Plane::new(
        Frame::new(
            Point3::new(0.3, -0.2, 0.1),
            Vector3::new(1.0, 0.4, -0.7),
            Vector3::new(0.0, 1.0, 0.0),
        )
        .unwrap(),
    );
    for (b, _) in corpus(8) {
        if let Ok(m) = b.mirrored(&plane) {
            round_trip(&m);
        }
        let shape = seamed(&b, 1e-7).unwrap();
        let (h, journal, _) = heal(OpKey(3), &shape, &HealOptions::default()).unwrap();
        round_trip(&h);
        assert_eq!(decode_journal(&encode_journal(&journal)).unwrap(), journal);
    }
}

#[test]
fn every_truncation_of_a_boolean_is_refused() {
    let (b, _) = corpus(4)
        .into_iter()
        .find(|(b, j)| !j.is_empty() && b.counts().faces > 6)
        .unwrap();
    let bytes = encode_body(&b);
    for n in (0..bytes.len()).step_by(7) {
        assert!(matches!(
            decode_body(&bytes[..n]),
            Err(Error::InvalidEncoding { .. })
        ));
    }
}
