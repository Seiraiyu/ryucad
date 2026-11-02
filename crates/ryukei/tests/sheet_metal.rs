//! Sheet metal against closed forms: bent strips recognized and unfolded,
//! their flats' lengths the textbook's, their patterns' outlines and bend
//! lines, and their flat maps through the codec.

use ryukei::Error;
use ryukei::SheetMetalError;
use ryukei::codec::{decode_flat_map, encode_flat_map};
use ryukei::geom::Point3;
use ryukei::name::OpKey;
use ryukei::sheet_metal::{
    Allowance, BendDirection, RefoldSpec, SheetModel, SheetOptions, UnfoldSpec, recognize, refold,
    unfold,
};
use ryukei::topo::Body;
use ryukei_testkit::sheet_metal::{Leg, strip};
use std::f64::consts::{FRAC_PI_2, PI};

const K: OpKey = OpKey(11);
const T: f64 = 0.001;
const R: f64 = 0.0015;
const W: f64 = 0.012;

fn turn(angle: f64) -> Leg {
    Leg::Turn { angle, radius: R }
}

fn model(b: &Body) -> SheetModel {
    recognize(b, &SheetOptions::default()).unwrap()
}

/// The unfold spec holding still the flange the strip starts with, the
/// one along `x` from the origin.
fn from_start(m: &SheetModel, allowance: Allowance) -> UnfoldSpec {
    let first = m
        .flanges
        .iter()
        .find(|f| {
            f.sides.iter().any(|s| {
                s.plane.frame().z().get().y.abs() > 1.0 - 1e-12
                    && s.plane.frame().origin().y.abs() < 1e-12
            })
        })
        .expect("the first flange");
    UnfoldSpec {
        fixed: Some(first.sides[0].faces[0]),
        ..UnfoldSpec::all(allowance)
    }
}

/// The flat's reach along `x` from the fixed flange's start.
fn length(b: &Body) -> f64 {
    let xs: Vec<f64> = b.vertices().map(|v| b.vertex(v).unwrap().point.x).collect();
    xs.iter().copied().fold(f64::NEG_INFINITY, f64::max)
        - xs.iter().copied().fold(f64::INFINITY, f64::min)
}

/// An L bracket at any angle unfolds to `L₁ + L₂ - 2 (r + t) tan(θ / 2) +
/// ba`, its legs `L₁`, `L₂` to the outer mould line, `ba = θ (r + k t)`.
#[test]
fn brackets_unfold_to_the_textbook_length() {
    let (a, c) = (0.02, 0.015);
    for theta in [PI / 6.0, PI / 3.0, FRAC_PI_2, 0.75 * PI] {
        let b = strip(K, &[Leg::Run(a), turn(theta), Leg::Run(c)], T, W);
        let m = model(&b);
        assert_eq!(m.bends.len(), 1);
        assert!((m.bends[0].angle - theta).abs() < 1e-12);
        for k in [0.0, 0.44, 1.0] {
            let u = unfold(K, &b, &m, &from_start(&m, Allowance::KFactor(k))).unwrap();
            // The legs to the outer mould line, where the outer sides meet.
            let reach = (R + T) * ryukei::math::tan(0.5 * theta);
            let (l1, l2) = (a + reach, c + reach);
            let want = l1 + l2 - 2.0 * reach + theta * (R + k * T);
            let got = length(&u.body);
            assert!(
                (got - want).abs() < 1e-12,
                "θ {theta}, k {k}: {got} against {want}"
            );
        }
    }
}

#[test]
fn channels_fold_one_way_and_jogs_both() {
    let channel = strip(
        K,
        &[
            Leg::Run(0.01),
            turn(FRAC_PI_2),
            Leg::Run(0.02),
            turn(FRAC_PI_2),
            Leg::Run(0.01),
        ],
        T,
        W,
    );
    let jog = strip(
        K,
        &[
            Leg::Run(0.01),
            turn(FRAC_PI_2),
            Leg::Run(0.02),
            turn(-FRAC_PI_2),
            Leg::Run(0.01),
        ],
        T,
        W,
    );
    let ba = FRAC_PI_2 * (R + 0.4 * T);
    for (b, ways) in [
        (channel, [BendDirection::Down; 2]),
        (jog, [BendDirection::Down, BendDirection::Up]),
    ] {
        let m = model(&b);
        let u = unfold(K, &b, &m, &from_start(&m, Allowance::KFactor(0.4))).unwrap();
        assert!((length(&u.body) - (0.04 + 2.0 * ba)).abs() < 1e-12);
        let p = &u.pattern;
        assert_eq!(p.loops.len(), 1);
        assert!((p.loops[0].area() - (0.04 + 2.0 * ba) * W).abs() < 1e-15);
        let got: Vec<BendDirection> = p.bend_lines.iter().map(|l| l.direction).collect();
        assert_eq!(got, ways);
        for l in &p.bend_lines {
            assert!((l.length - ba).abs() < 1e-15);
            assert!((l.start.distance(l.end) - W).abs() < 1e-12);
        }
    }
}

#[test]
fn the_flat_map_goes_through_the_codec() {
    let b = strip(K, &[Leg::Run(0.02), turn(FRAC_PI_2), Leg::Run(0.01)], T, W);
    let m = model(&b);
    let u = unfold(K, &b, &m, &UnfoldSpec::all(Allowance::Length(0.003))).unwrap();
    let back = decode_flat_map(&encode_flat_map(&u.map)).unwrap();
    assert_eq!(back, u.map);
    assert_eq!(back.strips[0].length, 0.003);
    // The strip runs the width of the bend along its axis.
    let [y0, y1] = back.strips[0].span;
    assert!((y1 - y0 - W).abs() < 1e-15);
}

#[test]
fn a_hem_unfolds_onto_its_two_sides() {
    let hem = strip(K, &[Leg::Run(0.02), turn(PI), Leg::Run(0.01)], T, W);
    let m = model(&hem);
    let u = unfold(K, &hem, &m, &from_start(&m, Allowance::KFactor(0.5))).unwrap();
    assert!((length(&u.body) - (0.03 + PI * (R + 0.5 * T))).abs() < 1e-12);
    // Every vertex of the flat lies on one of its two sides.
    for v in u.body.vertices() {
        let p: Point3 = u.body.vertex(v).unwrap().point;
        assert!(p.y.abs() < 1e-12 || (p.y - T).abs() < 1e-12, "{p:?}");
    }
    let fixed = m.fixed;
    let refused = |spec: &UnfoldSpec| match unfold(K, &hem, &m, spec) {
        Err(Error::InvalidSheetMetal(e)) => e,
        other => panic!("{other:?}"),
    };
    let wrong = m.bends[0].inner[0];
    let spec = UnfoldSpec {
        fixed: Some(wrong),
        ..UnfoldSpec::all(Allowance::KFactor(0.5))
    };
    assert_eq!(refused(&spec), SheetMetalError::NoSuchFace { name: wrong });
    assert_eq!(m.fixed, fixed);
}

/// Every vertex of `a` named in `b`, within `tol` of where `b` has it,
/// and the same faces, edges and vertices by name.
fn same_shape(a: &Body, b: &Body, tol: f64) {
    let names = |x: &Body| {
        let mut n: Vec<_> = x
            .faces()
            .map(|f| x.face(f).unwrap().name)
            .chain(x.edges().map(|e| x.edge(e).unwrap().name))
            .chain(x.vertices().map(|v| x.vertex(v).unwrap().name))
            .collect();
        n.sort_unstable();
        n
    };
    assert_eq!(names(a), names(b));
    for v in a.vertices() {
        let va = a.vertex(v).unwrap();
        let Some(ryukei::topo::EntityId::Vertex(w)) = b.by_name(va.name) else {
            panic!("vertex {}", va.name);
        };
        let d = va.point.distance(b.vertex(w).unwrap().point);
        assert!(d <= tol, "vertex {} is {d} away", va.name);
    }
}

/// Refolding an unfolded body gives it back: its topology and names, its
/// points within ulps (not bit for bit: the fold map and its inverse
/// round); and unfolding that gives the same flat as closely.
#[test]
fn flats_refold_to_what_was_unfolded() {
    for legs in [
        &[Leg::Run(0.02), turn(FRAC_PI_2), Leg::Run(0.015)][..],
        &[Leg::Run(0.02), turn(2.0), Leg::Run(0.015)][..],
        &[
            Leg::Run(0.01),
            turn(FRAC_PI_2),
            Leg::Run(0.02),
            turn(FRAC_PI_2),
            Leg::Run(0.01),
        ][..],
        &[
            Leg::Run(0.01),
            turn(FRAC_PI_2),
            Leg::Run(0.02),
            turn(-FRAC_PI_2),
            Leg::Run(0.01),
        ][..],
        &[Leg::Run(0.02), turn(PI), Leg::Run(0.01)][..],
    ] {
        let b = strip(K, legs, T, W);
        let m = model(&b);
        let spec = from_start(&m, Allowance::KFactor(0.4));
        let u = unfold(K, &b, &m, &spec).unwrap();
        let (back, journal) = refold(K, &u.body, &u.map, &RefoldSpec::default()).unwrap();
        same_shape(&b, &back, 1e-12);
        assert!(!journal.events().is_empty());
        let again = unfold(K, &back, &model(&back), &spec).unwrap();
        same_shape(&u.body, &again.body, 1e-12);
    }
}
