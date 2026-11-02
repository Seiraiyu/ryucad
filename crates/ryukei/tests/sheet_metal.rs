//! Sheet metal against closed forms: bent strips recognized and unfolded,
//! their flats' lengths the textbook's, their patterns' outlines and bend
//! lines, and their flat maps through the codec.

use ryukei::Error;
use ryukei::SheetMetalError;
use ryukei::boolean::{BooleanOp, Tool, boolean};
use ryukei::codec::{decode_flat_map, encode_flat_map};
use ryukei::geom::{Frame, Point3, Rigid, Surface, Vector3};
use ryukei::measure::{Entity, MassOptions, Properties, mass, properties};
use ryukei::mesh::{MeshParams, tessellate};
use ryukei::name::{OpKey, ToolKey};
use ryukei::primitive::{block, cylinder, frustum};
use ryukei::sheet_metal::{
    Allowance, BendDirection, RefoldSpec, SheetModel, SheetOptions, UnfoldSpec, recognize, refold,
    unfold,
};
use ryukei::topo::{Body, EntityId, FaceId};
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

/// An L bracket's flat, unfolded with a K-factor of a half, cut by `tool`
/// placed on its strip's near bend line `y` along it, turned by `turn`
/// about the flat's normal.
fn cut_flat(tool: &Body, y: f64, turn: f64) -> (Body, ryukei::sheet_metal::FlatMap) {
    let b = strip(
        K,
        &[Leg::Run(0.02), self::turn(FRAC_PI_2), Leg::Run(0.015)],
        T,
        W,
    );
    let m = model(&b);
    let u = unfold(K, &b, &m, &from_start(&m, Allowance::KFactor(0.5))).unwrap();
    let s = &u.map.strips[0];
    let at = s.frame.to_world(0.0, s.span[0] + y, -0.001);
    let (sn, cs) = ryukei::math::sin_cos(turn);
    let along = s.frame.x().get() * cs + s.frame.y().get() * sn;
    let frame = Frame::new(at, s.frame.z().get(), along).unwrap();
    let (flat, _) = boolean(
        OpKey(98),
        BooleanOp::Subtract,
        &u.body,
        &[Tool {
            key: ToolKey(1),
            body: tool,
            place: Rigid::from_frame(&frame),
        }],
    )
    .unwrap();
    (flat, u.map)
}

fn area(b: &Body, f: FaceId) -> (f64, f64) {
    match properties(b, Entity::Face(f)).unwrap() {
        Properties::Face { area, .. } => (area.value(), area.bound()),
        _ => unreachable!("a face"),
    }
}

/// A round hole and a slot at thirty degrees, cut in the flat across a
/// bend line, refold exactly: their walls in the bend are bent surfaces,
/// the result checks clean and meshes, keeps the flat's volume (a K-factor
/// of a half keeps volume), and each side of the bend has its flat area
/// scaled by `(r + z) θ / ba`.
#[test]
fn holes_and_slots_cut_across_a_bend_line_refold() {
    let hole = cylinder(OpKey(99), 0.0008, 0.003, &Rigid::IDENTITY)
        .unwrap()
        .0;
    let slot = block(
        OpKey(99),
        0.004,
        0.0012,
        0.003,
        &Rigid::translation(Vector3::new(-0.002, -0.0006, 0.0)),
    )
    .unwrap()
    .0;
    for (tool, turn) in [(hole, 0.0), (slot, PI / 6.0)] {
        let (flat, map) = cut_flat(&tool, 0.005, turn);
        let (folded, _) = refold(K, &flat, &map, &RefoldSpec::default()).unwrap();
        assert!(folded.faces().any(|f| {
            let s = folded.face(f).unwrap().surface;
            matches!(folded.surface(s).unwrap(), Surface::Bent(_))
        }));
        let mesh = tessellate(&folded, &MeshParams::new(1e-5, 0.3)).unwrap();
        assert!(mesh.faults.is_empty(), "{:?}", mesh.faults);
        let volume = |b: &Body| mass(b, 1.0, &MassOptions::default()).unwrap().volume;
        let (a, c) = (volume(&flat), volume(&folded));
        assert!(
            (a.value() - c.value()).abs() <= a.bound() + c.bound(),
            "{a:?} against {c:?}"
        );
        let s = &map.strips[0];
        let kappa = s.angle / s.length;
        let mut sides = 0;
        for f in folded.faces() {
            let Surface::Cylinder(cy) = folded.surface(folded.face(f).unwrap().surface).unwrap()
            else {
                continue;
            };
            // The bend's sides, not the hole's wall.
            if (cy.radius() - R).abs() > 1e-12 && (cy.radius() - R - T).abs() > 1e-12 {
                continue;
            }
            let name = folded.face(f).unwrap().name;
            let Some(EntityId::Face(g)) = flat.by_name(name) else {
                panic!("the flat's side");
            };
            let ((fa, fb), (ga, gb)) = (area(&folded, f), area(&flat, g));
            let want = ga * cy.radius() * kappa;
            assert!(
                (fa - want).abs() <= fb + gb * cy.radius() * kappa,
                "{fa} against {want}"
            );
            sides += 1;
        }
        assert_eq!(sides, 2);
    }
}

/// Folding one strip of a channel's two folds that bend and carries the
/// rest rigidly: the first flange's corner and the middle flange come
/// back, the second strip stays flat.
#[test]
fn a_partial_refold_folds_only_the_bends_named() {
    let b = strip(
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
    let m = model(&b);
    let u = unfold(K, &b, &m, &from_start(&m, Allowance::KFactor(0.5))).unwrap();
    let first = u.map.strips[0].bend;
    let spec = RefoldSpec {
        bends: Some(vec![first]),
    };
    let (half, _) = refold(K, &u.body, &u.map, &spec).unwrap();
    let hm = model(&half);
    assert_eq!(hm.bends.len(), 1);
    // Unfolding what is left gives the whole flat again.
    let again = unfold(K, &half, &hm, &from_start(&hm, Allowance::KFactor(0.5))).unwrap();
    same_shape(&u.body, &again.body, 1e-12);
    let other = ryukei::name::Name::from_u128(3);
    assert_eq!(
        refold(
            K,
            &u.body,
            &u.map,
            &RefoldSpec {
                bends: Some(vec![other])
            }
        )
        .unwrap_err(),
        Error::from(SheetMetalError::NoSuchBend { name: other })
    );
}

/// A countersink cut across a bend line in the flat has no radial walls
/// in the bend; a flange lengthened in the flat until it folds through
/// another collides.
#[test]
fn refolds_that_cant_be_made_are_refused() {
    let sink = frustum(OpKey(99), 0.0004, 0.0016, 0.003, &Rigid::IDENTITY)
        .unwrap()
        .0;
    let (flat, map) = cut_flat(&sink, 0.005, 0.0);
    assert!(matches!(
        refold(K, &flat, &map, &RefoldSpec::default()).unwrap_err(),
        Error::InvalidSheetMetal(SheetMetalError::NotRadial { .. })
    ));
    // A C: up, back half way and down, its last flange made long enough
    // in the flat to fold through its first.
    let c = strip(
        K,
        &[
            Leg::Run(0.02),
            turn(FRAC_PI_2),
            Leg::Run(0.01),
            turn(FRAC_PI_2),
            Leg::Run(0.01),
            turn(FRAC_PI_2),
            Leg::Run(0.005),
        ],
        T,
        W,
    );
    let m = model(&c);
    let u = unfold(K, &c, &m, &from_start(&m, Allowance::KFactor(0.5))).unwrap();
    let bx = u
        .body
        .vertices()
        .fold((f64::INFINITY, f64::NEG_INFINITY), |a, v| {
            let x = u.body.vertex(v).unwrap().point.x;
            (a.0.min(x), a.1.max(x))
        });
    let tab = block(
        OpKey(99),
        0.03,
        T,
        W,
        &Rigid::translation(Vector3::new(bx.1 - 0.001, 0.0, 0.0)),
    )
    .unwrap()
    .0;
    let (long, _) = boolean(
        OpKey(98),
        BooleanOp::Unite,
        &u.body,
        &[Tool {
            key: ToolKey(1),
            body: &tab,
            place: Rigid::IDENTITY,
        }],
    )
    .unwrap();
    assert!(matches!(
        refold(K, &long, &u.map, &RefoldSpec::default()).unwrap_err(),
        Error::InvalidSheetMetal(SheetMetalError::FoldCollides { .. })
    ));
}
