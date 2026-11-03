//! Sheet-metal benchmarks (design 298, plan 11.D): recognizing,
//! unfolding and refolding a tray of four flanges set back with reliefs,
//! a jog extruded from a bent strip, and the tray's flat drilled across
//! its bend lines.

use core::f64::consts::FRAC_PI_2;
use divan::black_box;
use ryukei::boolean::{BooleanOp, Tool, boolean};
use ryukei::geom::{Frame, Point3, Rigid};
use ryukei::name::{OpKey, ToolKey};
use ryukei::primitive::{block, cylinder};
use ryukei::sheet_metal::{
    Allowance, EdgeFlange, FlangePosition, FlatMap, Measure, RefoldSpec, Relief, SheetOptions,
    UnfoldSpec, flange, recognize, refold, unfold,
};
use ryukei::topo::Body;
use ryukei_testkit::sheet_metal::{Leg, strip};
use ryukei_testkit::sheet_metal_bytes::{about_the_plate, edge_at};

const T: f64 = 0.001;
const R: f64 = 0.0015;

fn main() {
    divan::main();
}

fn tray() -> Body {
    let (px, py) = (0.04, 0.03);
    let plate = block(OpKey(1), px, py, T, &Rigid::IDENTITY).unwrap().0;
    let specs: Vec<EdgeFlange> = [
        Point3::new(px, 0.5 * py, T),
        Point3::new(0.5 * px, py, T),
        Point3::new(0.0, 0.5 * py, T),
        Point3::new(0.5 * px, 0.0, T),
    ]
    .into_iter()
    .map(|p| EdgeFlange {
        edge: edge_at(&plate, p),
        side: None,
        angle: FRAC_PI_2,
        length: 0.01,
        measured: Measure::Tangent,
        radius: R,
        position: FlangePosition::MaterialInside,
        setbacks: [0.005; 2],
        reliefs: [Relief::Rectangular {
            width: 0.001,
            depth: 0.001,
        }; 2],
    })
    .collect();
    flange(OpKey(2), &plate, &specs, &SheetOptions::default())
        .unwrap()
        .0
}

fn jog() -> Body {
    let turn = |angle| Leg::Turn { angle, radius: R };
    strip(
        OpKey(3),
        &[
            Leg::Run(0.01),
            turn(FRAC_PI_2),
            Leg::Run(0.02),
            turn(-FRAC_PI_2),
            Leg::Run(0.01),
        ],
        T,
        0.02,
    )
}

fn part(kind: &str) -> (Body, UnfoldSpec) {
    let allowance = Allowance::KFactor(0.44);
    if kind == "tray" {
        let b = tray();
        let m = recognize(&b, &SheetOptions::default()).unwrap();
        let spec = about_the_plate(&m, allowance);
        (b, spec)
    } else {
        (jog(), UnfoldSpec::all(allowance))
    }
}

#[divan::bench(args = ["tray", "jog"])]
fn recognizing(bencher: divan::Bencher, kind: &str) {
    let (b, _) = part(kind);
    bencher.bench(|| black_box(recognize(&b, &SheetOptions::default()).unwrap()));
}

#[divan::bench(args = ["tray", "jog"])]
fn unfolding(bencher: divan::Bencher, kind: &str) {
    let (b, spec) = part(kind);
    let m = recognize(&b, &SheetOptions::default()).unwrap();
    bencher.bench(|| black_box(unfold(OpKey(4), &b, &m, &spec).unwrap()));
}

/// The tray's flat, drilled across each strip's near bend line.
fn drilled(b: &Body, spec: &UnfoldSpec) -> (Body, FlatMap) {
    let m = recognize(b, &SheetOptions::default()).unwrap();
    let u = unfold(OpKey(4), b, &m, spec).unwrap();
    let hole = cylinder(OpKey(5), 0.0008, T + 0.0004, &Rigid::IDENTITY)
        .unwrap()
        .0;
    let places: Vec<Rigid> = u
        .map
        .strips
        .iter()
        .map(|s| {
            let y = 0.5 * (s.span[0] + s.span[1]);
            let f = Frame::new(
                s.frame.to_world(0.0, y, -0.0002),
                s.frame.z().get(),
                s.frame.x().get(),
            )
            .unwrap();
            Rigid::from_frame(&f)
        })
        .collect();
    let tools: Vec<Tool> = places
        .iter()
        .enumerate()
        .map(|(i, p)| Tool {
            key: ToolKey(i as u128 + 1),
            body: &hole,
            place: *p,
        })
        .collect();
    let flat = boolean(OpKey(6), BooleanOp::Subtract, &u.body, &tools)
        .unwrap()
        .0;
    (flat, u.map)
}

#[divan::bench(args = ["tray", "jog"], consts = [false, true])]
fn refolding<const DRILLED: bool>(bencher: divan::Bencher, kind: &str) {
    let (b, spec) = part(kind);
    let (flat, map) = if DRILLED {
        drilled(&b, &spec)
    } else {
        let m = recognize(&b, &SheetOptions::default()).unwrap();
        let u = unfold(OpKey(4), &b, &m, &spec).unwrap();
        (u.body, u.map)
    };
    bencher.bench(|| black_box(refold(OpKey(7), &flat, &map, &RefoldSpec::default()).unwrap()));
}
