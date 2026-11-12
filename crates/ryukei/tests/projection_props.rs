//! Hidden lines against a mesh (design 326, plan 10.C): seeded bodies
//! from seeded directions. Release only (`cargo test -p ryukei --release
//! --test projection_props`): debug builds take minutes over a traced
//! silhouette.

use ryukei::geom::{Curve, Rigid, Surface, Vector3};
use ryukei::project::{Placed, ViewOptions, view};
use ryukei::topo::Body;
use ryukei::{Error, ViewError};
use ryukei_testkit::Lcg;
use ryukei_testkit::projection::{
    Disagreement, Oracle, judge, look, placed_bodies, silhouette_excess, size,
};

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

#[derive(Default)]
struct Tally {
    samples: usize,
    unsure: usize,
    uncertain: usize,
    pieces: usize,
    refused: usize,
    images: Vec<String>,
}

type Wrong = Vec<(String, Vector3, Vec<Disagreement>)>;

/// One view of `body` judged against the mesh, into `tally` and `wrong`.
fn judge_one(name: &str, body: &Body, t: Vector3, tally: &mut Tally, wrong: &mut Wrong) {
    let v = look(t);
    let placed = [Placed {
        body,
        place: Rigid::IDENTITY,
    }];
    let p = match view(&placed, &v, &ViewOptions::default()) {
        Ok(p) => p,
        Err(Error::InvalidView(e)) => {
            eprintln!("{name}: refused {e:?}");
            if matches!(e, ViewError::Image { .. }) {
                tally.images.push(name.to_owned());
            }
            tally.refused += 1;
            return;
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
        return;
    };
    let j = judge(&p, &bodies, &o);
    if j.uncertain > 0 {
        eprintln!("{name}: {} of {} pieces uncertain", j.uncertain, j.pieces);
    }
    tally.samples += j.samples;
    tally.unsure += j.unsure;
    tally.uncertain += j.uncertain;
    tally.pieces += j.pieces;
    if !j.wrong.is_empty() && !KNOWN.iter().any(|(n, _)| *n == name) {
        wrong.push((name.to_owned(), t, j.wrong));
    }
}

#[test]
#[cfg_attr(debug_assertions, ignore = "release only")]
fn views_agree_with_a_fine_mesh() {
    let mut rng = Lcg::new(0x10c);
    let mut tally = Tally::default();
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
            judge_one(&name, &body, t, &mut tally, &mut wrong);
        }
    }
    let Tally {
        samples,
        unsure,
        uncertain,
        pieces,
        refused,
        ..
    } = tally;
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

/// Views square to a torus face's axis, and along a spline edge's
/// tangent (plan 17.B), against the same mesh: none refused for an image,
/// none wrong, and of the square views at most a tenth of pieces
/// uncertain. Views along an edge's tangent stack its pieces in the
/// image, where rays meet edges exactly (#643): their uncertain share is
/// printed, not held (164 of 694 pieces on cox-worker-4, plan 17.B).
#[test]
#[cfg_attr(debug_assertions, ignore = "release only")]
fn square_and_cusp_views_agree_with_a_fine_mesh() {
    let mut rng = Lcg::new(0x17b);
    let (mut square, mut cusp) = (Tally::default(), Tally::default());
    let mut wrong = Vec::new();
    for (name, body) in cases() {
        let mut views = Vec::new();
        for f in body.faces() {
            let s = body.surface(body.face(f).unwrap().surface).unwrap();
            if let Surface::Torus(_) = s {
                let z = s.frame().unwrap().z().get();
                let t = Vector3::new(
                    rng.next_f64(-1.0, 1.0),
                    rng.next_f64(-1.0, 1.0),
                    rng.next_f64(-1.0, 1.0),
                );
                if let Some(u) = z.cross(t).normalize() {
                    views.push((false, u.get()));
                }
            }
        }
        if let Some(e) = body.edges().find(|&e| {
            let edge = body.edge(e).unwrap();
            matches!(body.curve(edge.curve).unwrap(), Curve::Nurbs(_))
        }) {
            let edge = body.edge(e).unwrap();
            let [a, b] = edge.interval;
            let c = body.curve(edge.curve).unwrap();
            if let Some(tan) = c.tangent(a + (b - a) * rng.next_f64(0.1, 0.9)) {
                views.push((true, tan.get()));
            }
        }
        for (along, t) in views {
            let (kind, tally) = if along {
                ("cusp", &mut cusp)
            } else {
                ("square", &mut square)
            };
            judge_one(&format!("{name} {kind}"), &body, t, tally, &mut wrong);
        }
    }
    for (kind, t) in [("square", &square), ("cusp", &cusp)] {
        eprintln!(
            "{kind}: {} samples judged, {} unsure; {} of {} pieces uncertain; {} views refused",
            t.samples, t.unsure, t.uncertain, t.pieces, t.refused
        );
    }
    assert!(wrong.is_empty(), "{wrong:#?}");
    for t in [&square, &cusp] {
        assert!(t.images.is_empty(), "refused for an image: {:?}", t.images);
    }
    assert!(
        square.uncertain * 10 <= square.pieces,
        "{} of {} pieces uncertain",
        square.uncertain,
        square.pieces
    );
}
