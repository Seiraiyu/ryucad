//! DXF of flat patterns (design 346, plan 16.B) against the test reader:
//! layers, each entity, arcs either way round, units, notes, and the same
//! bytes twice.

mod common;

use common::dxf::read;
use ryuformats::dxf::{DxfOptions, DxfUnit, note, write};
use ryukei::geom::{
    Box2, Circle2, Curve2, Ellipse2, Frame, Frame2, KnotVector, Line2, NurbsCurve2, Point2, Vector2,
};
use ryukei::name::{Name, OpKey};
use ryukei::sheet_metal::{
    Allowance, BendDirection, BendLine, FlatPattern, PatternLoop, PatternSegment, SheetOptions,
    UnfoldSpec, recognize, unfold,
};
use ryukei_testkit::sheet_metal::{Leg, strip};
use std::f64::consts::{FRAC_PI_2, PI, TAU};
use std::sync::Arc;

fn mm() -> DxfOptions {
    DxfOptions {
        unit: DxfUnit::Mm,
        notes: true,
    }
}

/// An L: 20 mm, a quarter turn of 1 mm inside, 15 mm; 1 mm thick, 10 mm
/// wide; unfolded at a K-factor of 0.44.
fn l_pattern() -> FlatPattern {
    let b = strip(
        OpKey(1),
        &[
            Leg::Run(0.02),
            Leg::Turn {
                angle: FRAC_PI_2,
                radius: 0.001,
            },
            Leg::Run(0.015),
        ],
        0.001,
        0.01,
    );
    let m = recognize(&b, &SheetOptions::default()).unwrap();
    unfold(OpKey(2), &b, &m, &UnfoldSpec::all(Allowance::KFactor(0.44)))
        .unwrap()
        .pattern
}

fn seg(curve: Curve2, t0: f64, t1: f64, n: u128) -> PatternSegment {
    PatternSegment {
        edge: Name::from_u128(n),
        curve,
        t0,
        t1,
    }
}

fn frame2(o: [f64; 2], x: [f64; 2], ccw: bool) -> Frame2 {
    Frame2::new(Point2::new(o[0], o[1]), Vector2::new(x[0], x[1]), ccw).unwrap()
}

/// A pattern of every kind of segment: a square outline of lines, a whole
/// circle walked backwards, a half circle on a clockwise frame, half an
/// ellipse and a quadratic spline.
fn kinds() -> FlatPattern {
    let p = |x: f64, y: f64| Point2::new(x, y);
    let line = |a: Point2, b: Point2| Curve2::Line2(Line2::through(a, b).unwrap());
    let sq = [p(0.0, 0.0), p(0.1, 0.0), p(0.1, 0.1), p(0.0, 0.1)];
    let outline = PatternLoop {
        outer: true,
        segments: (0..4)
            .map(|i| {
                seg(
                    line(sq[i], sq[(i + 1) % 4]),
                    0.0,
                    sq[i].distance(sq[(i + 1) % 4]),
                    i as u128 + 1,
                )
            })
            .collect(),
    };
    let circle =
        Curve2::Circle2(Circle2::new(frame2([0.02, 0.02], [1.0, 0.0], true), 0.005).unwrap());
    let cw = Curve2::Circle2(Circle2::new(frame2([0.05, 0.05], [0.0, 1.0], false), 0.004).unwrap());
    let ellipse = Curve2::Ellipse2(
        Ellipse2::new(frame2([0.08, 0.02], [1.0, 1.0], true), 0.006, 0.003).unwrap(),
    );
    let knots = KnotVector::clamped(2, &[0.0, 1.0], &[3, 3]).unwrap();
    let spline = Curve2::Nurbs2(Arc::new(
        NurbsCurve2::new(
            knots,
            vec![p(0.02, 0.08), p(0.05, 0.095), p(0.08, 0.08)],
            Some(vec![1.0, 0.8, 1.0]),
        )
        .unwrap(),
    ));
    let holes = PatternLoop {
        outer: false,
        segments: vec![
            seg(circle, TAU, 0.0, 10),
            seg(cw, 0.0, PI, 11),
            seg(ellipse, PI, 0.0, 12),
            seg(spline, 0.0, 1.0, 13),
        ],
    };
    FlatPattern {
        frame: Frame::WORLD,
        loops: vec![outline, holes],
        bend_lines: vec![BendLine {
            bend: Name::from_u128(20),
            start: p(0.1, 0.03),
            end: p(0.0, 0.03),
            edges: vec![Name::from_u128(21)],
            direction: BendDirection::Down,
            angle: PI / 4.0,
            radius: 0.0005,
            length: 0.001234,
            allowance: Allowance::Length(0.001234),
        }],
        thickness: 0.001,
        bounds: Box2::new(p(0.0, 0.0), p(0.1, 0.1)),
    }
}

/// Where an ARC entity starts and ends, counterclockwise.
fn arc_ends(e: &common::dxf::Entity) -> [[f64; 2]; 2] {
    let (cx, cy, r) = (e.f(10), e.f(20), e.f(40));
    [e.f(50), e.f(51)].map(|d| {
        let a = d.to_radians();
        [cx + r * ryukei::math::cos(a), cy + r * ryukei::math::sin(a)]
    })
}

fn near(a: [f64; 2], b: [f64; 2]) -> bool {
    (a[0] - b[0]).abs() < 1e-9 && (a[1] - b[1]).abs() < 1e-9
}

#[test]
fn a_bracket_writes_its_outline_bend_line_and_note() {
    let pat = l_pattern();
    let d = read(&write(&pat, &mm()).unwrap());
    let names: Vec<&str> = d.layers.iter().map(|l| l.name.as_str()).collect();
    assert_eq!(
        names,
        [
            "0",
            "OUTLINE",
            "INNER",
            "BEND_UP",
            "BEND_DOWN",
            "BEND_NOTES"
        ]
    );
    for (l, colour, lt) in [
        ("BEND_UP", 2, "DASHED"),
        ("BEND_DOWN", 4, "DASHED"),
        ("OUTLINE", 7, "CONTINUOUS"),
    ] {
        let x = d.layers.iter().find(|x| x.name == l).unwrap();
        assert_eq!((x.colour, x.linetype.as_str()), (colour, lt));
    }
    assert_eq!(d.var("$ACADVER"), "AC1015");
    assert_eq!(d.var("$INSUNITS").trim(), "4");
    let outer: usize = pat
        .loops
        .iter()
        .filter(|l| l.outer)
        .map(|l| l.segments.len())
        .sum();
    assert_eq!(d.on("OUTLINE").len(), outer);
    assert!(d.on("INNER").is_empty());
    let b = &pat.bend_lines[0];
    let layer = match b.direction {
        BendDirection::Up => "BEND_UP",
        BendDirection::Down => "BEND_DOWN",
    };
    let lines = d.on(layer);
    assert_eq!(lines.len(), 1);
    let e = lines[0];
    assert_eq!(e.kind, "LINE");
    assert!(near([e.f(10), e.f(20)], [b.start.x * 1e3, b.start.y * 1e3]));
    assert!(near([e.f(11), e.f(21)], [b.end.x * 1e3, b.end.y * 1e3]));
    // Coordinates are the shortest text that reads back to the bits.
    assert_eq!(e.f(10), b.start.x * 1e3);
    let notes = d.on("BEND_NOTES");
    assert_eq!(notes.len(), 1);
    assert_eq!(notes[0].text(1), note(b, DxfUnit::Mm, "%%d"));
    assert!(
        notes[0].text(1).ends_with("R1 K0.44"),
        "{}",
        notes[0].text(1)
    );
}

#[test]
fn every_kind_of_segment_and_arcs_either_way_round() {
    let pat = kinds();
    let d = read(&write(&pat, &mm()).unwrap());
    let inner = d.on("INNER");
    let kinds: Vec<&str> = inner.iter().map(|e| e.kind.as_str()).collect();
    assert_eq!(kinds, ["CIRCLE", "ARC", "ELLIPSE", "SPLINE"]);
    // The whole circle, walked backwards, is still a circle.
    assert_eq!(
        (inner[0].f(10), inner[0].f(20), inner[0].f(40)),
        (20.0, 20.0, 5.0)
    );
    // The half circle on a clockwise frame from t = 0 to π runs clockwise:
    // the ARC, always counterclockwise, starts where it ends.
    let s = &pat.loops[1].segments[1];
    let (a, b) = (s.curve.point(s.t0), s.curve.point(s.t1));
    let [from, to] = arc_ends(inner[1]);
    assert!(
        near(from, [b.x * 1e3, b.y * 1e3]) && near(to, [a.x * 1e3, a.y * 1e3]),
        "{from:?} {to:?}"
    );
    // The ellipse: its centre, major axis end, ratio and half a turn.
    let e = inner[2];
    assert_eq!((e.f(10), e.f(20)), (80.0, 20.0));
    assert!((e.f(40) - 0.5).abs() < 1e-15);
    assert!(((e.f(42) - e.f(41)) - PI).abs() < 1e-12);
    // The spline: rational, quadratic, six knots, three points.
    let sp = inner[3];
    assert_eq!(sp.f(70) as i64 & 4, 4);
    assert_eq!((sp.f(71), sp.all(40).len(), sp.all(10).len()), (2.0, 6, 3));
    assert_eq!(sp.all(41), [1.0, 0.8, 1.0]);
    // DOWN, a length allowance, millimetres to three places.
    assert_eq!(d.on("BEND_NOTES")[0].text(1), "DOWN 45%%d R0.5 BA1.234");
}

#[test]
fn inches_and_no_notes() {
    let pat = l_pattern();
    let opts = DxfOptions {
        unit: DxfUnit::In,
        notes: false,
    };
    let d = read(&write(&pat, &opts).unwrap());
    assert_eq!(d.var("$INSUNITS").trim(), "1");
    assert_eq!(d.var("$MEASUREMENT").trim(), "0");
    assert!(d.on("BEND_NOTES").is_empty());
    assert!(d.layers.iter().any(|l| l.name == "BEND_NOTES"));
    let b = &pat.bend_lines[0];
    let line = d
        .entities
        .iter()
        .find(|e| e.layer.starts_with("BEND_") && e.kind == "LINE")
        .unwrap();
    assert_eq!(line.f(10), b.start.x / 0.0254);
}

#[test]
fn the_same_pattern_the_same_bytes() {
    let pat = l_pattern();
    let a = write(&pat, &mm()).unwrap();
    assert_eq!(a, write(&pat.clone(), &mm()).unwrap());
    let decoded =
        ryukei::codec::decode_flat_pattern(&ryukei::codec::encode_flat_pattern(&pat)).unwrap();
    assert_eq!(a, write(&decoded, &mm()).unwrap());
    assert_eq!(
        write(&kinds(), &mm()).unwrap(),
        write(&kinds(), &mm()).unwrap()
    );
}
