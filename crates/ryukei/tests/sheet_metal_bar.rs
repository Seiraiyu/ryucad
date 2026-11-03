//! Design 298's bar (plan 11.D): an enclosure of 40 flanges and 2,000
//! holes, a quarter of them cut across bend lines in the flat. A plate
//! 410 mm square and 1 mm thick is castellated into ten 30 mm tabs a side
//! (notches 10 mm wide and 5 mm deep, the corners notched), each tab's
//! outer edge flanged; the flat is drilled with 500 holes across bend
//! lines and 1,500 in the base, and refolded. On one thread, in release:
//! recognizing the enclosure and unfolding it within 1 s, refolding the
//! drilled flat within 3 s. With `RYUKEI_BARS=1` the bars are asserted as
//! they are, else ten times looser (CI's runners vary). Release only.

use ryukei::boolean::{BooleanOp, Tool, boolean};
use ryukei::geom::{Frame, Point3, Rigid, Vector3};
use ryukei::name::{Name, OpKey, ToolKey};
use ryukei::primitive::{block, cylinder};
use ryukei::sheet_metal::{
    Allowance, EdgeFlange, FlangePosition, Measure, RefoldSpec, Relief, SheetModel, SheetOptions,
    UnfoldSpec, flange, recognize, refold, unfold,
};
use ryukei::topo::Body;
use std::f64::consts::FRAC_PI_2;
use std::time::Instant;

const T: f64 = 0.001;
const R: f64 = 0.0015;
const TAB: f64 = 0.03;
const NOTCH: f64 = 0.01;
const DEPTH: f64 = 0.005;
const TABS: usize = 10;
const P: f64 = TABS as f64 * TAB + (TABS + 1) as f64 * NOTCH;

/// A bar in seconds: as it is with `RYUKEI_BARS` set, else ten times.
fn bar(secs: f64) -> f64 {
    if std::env::var_os("RYUKEI_BARS").is_some() {
        secs
    } else {
        10.0 * secs
    }
}

fn edge_at(b: &Body, p: Point3) -> Name {
    let middle = |e| {
        let ed = b.edge(e).unwrap();
        b.curve(ed.curve)
            .unwrap()
            .point(0.5 * (ed.interval[0] + ed.interval[1]))
    };
    let e = b
        .edges()
        .min_by(|&x, &y| middle(x).distance(p).total_cmp(&middle(y).distance(p)))
        .unwrap();
    b.edge(e).unwrap().name
}

fn subtract(key: u64, target: &Body, tools: &[(Body, Rigid)]) -> Body {
    let tools: Vec<Tool> = tools
        .iter()
        .enumerate()
        .map(|(i, (body, place))| Tool {
            key: ToolKey(i as u128 + 1),
            body,
            place: *place,
        })
        .collect();
    boolean(OpKey(key as u128), BooleanOp::Subtract, target, &tools)
        .unwrap()
        .0
}

/// Tab centres along a side, from its start.
fn centres() -> impl Iterator<Item = f64> {
    (0..TABS).map(|i| NOTCH + i as f64 * (TAB + NOTCH) + 0.5 * TAB)
}

fn castellated() -> Body {
    let plate = block(OpKey(1), P, P, T, &Rigid::IDENTITY).unwrap().0;
    let mut tools = Vec::new();
    // Notches: along each side, NOTCH wide and DEPTH deep, between tabs and
    // at the corners (a corner square DEPTH x DEPTH is enough there, but a
    // notch NOTCH long keeps the shapes alike).
    for i in 0..=TABS {
        let s = i as f64 * (TAB + NOTCH);
        // -y side, +y side: notch spans x in [s, s + NOTCH].
        for (x, y, dx, dy) in [
            (s, -0.001, NOTCH, DEPTH + 0.001),
            (s, P - DEPTH, NOTCH, DEPTH + 0.001),
            (-0.001, s, DEPTH + 0.001, NOTCH),
            (P - DEPTH, s, DEPTH + 0.001, NOTCH),
        ] {
            let b = block(OpKey(3), dx, dy, T + 0.002, &Rigid::IDENTITY)
                .unwrap()
                .0;
            tools.push((b, Rigid::translation(Vector3::new(x, y, -0.001))));
        }
    }
    subtract(10, &plate, &tools)
}

fn spec(edge: Name) -> EdgeFlange {
    EdgeFlange {
        edge,
        side: None,
        angle: FRAC_PI_2,
        length: 0.02,
        measured: Measure::Tangent,
        radius: R,
        position: FlangePosition::BendOutside,
        setbacks: [0.0, 0.0],
        reliefs: [Relief::None; 2],
    }
}

fn base_spec(m: &SheetModel) -> UnfoldSpec {
    let base = m
        .flanges
        .iter()
        .find(|f| {
            f.sides.iter().any(|s| {
                s.plane.frame().z().get().z.abs() > 1.0 - 1e-12
                    && s.plane.frame().origin().z.abs() < 1e-12
            })
        })
        .expect("the base");
    UnfoldSpec {
        fixed: Some(base.sides[0].faces[0]),
        ..UnfoldSpec::all(Allowance::KFactor(0.5))
    }
}

#[test]
#[cfg_attr(debug_assertions, ignore = "release only")]
fn the_bar() {
    let t0 = Instant::now();
    let plate = castellated();
    let mut specs = Vec::new();
    for c in centres() {
        specs.push(spec(edge_at(&plate, Point3::new(c, 0.0, T))));
        specs.push(spec(edge_at(&plate, Point3::new(c, P, T))));
        specs.push(spec(edge_at(&plate, Point3::new(0.0, c, T))));
        specs.push(spec(edge_at(&plate, Point3::new(P, c, T))));
    }
    let flanged = flange(OpKey(20), &plate, &specs, &SheetOptions::default())
        .unwrap()
        .0;
    let m = recognize(&flanged, &SheetOptions::default()).unwrap();
    assert_eq!(m.flanges.len(), 41);
    let u = unfold(OpKey(21), &flanged, &m, &base_spec(&m)).unwrap();
    eprintln!("built the flanged plate and its flat in {:?}", t0.elapsed());
    // Holes: 500 across bend lines (12 or 13 per strip), 1,500 in the base.
    let small = cylinder(OpKey(30), 0.00075, 0.003, &Rigid::IDENTITY)
        .unwrap()
        .0;
    let big = cylinder(OpKey(31), 0.0015, 0.003, &Rigid::IDENTITY)
        .unwrap()
        .0;
    let mut tools = Vec::new();
    for (i, s) in u.map.strips.iter().enumerate() {
        let n = if i % 2 == 0 { 12 } else { 13 };
        let span = s.span[1] - s.span[0];
        for j in 0..n {
            let y = s.span[0] + span * (j as f64 + 0.5) / n as f64;
            let at = s.frame.to_world(0.0, y, -0.001);
            let frame = Frame::new(at, s.frame.z().get(), s.frame.x().get()).unwrap();
            tools.push((small.clone(), Rigid::from_frame(&frame)));
        }
    }
    assert_eq!(tools.len(), 500);
    let (lo, hi) = (DEPTH + 0.008, P - DEPTH - 0.008);
    let n = 39;
    let mut k = 0;
    'grid: for i in 0..n {
        for j in 0..n {
            if k == 1500 {
                break 'grid;
            }
            let x = lo + (hi - lo) * i as f64 / (n - 1) as f64;
            let y = lo + (hi - lo) * j as f64 / (n - 1) as f64;
            tools.push((big.clone(), Rigid::translation(Vector3::new(x, y, -0.001))));
            k += 1;
        }
    }
    assert_eq!(tools.len(), 2000);
    let t1 = Instant::now();
    let flat = subtract(40, &u.body, &tools);
    eprintln!("drilled the flat in {:?}", t1.elapsed());
    let pool = rayon::ThreadPoolBuilder::new()
        .num_threads(1)
        .build()
        .unwrap();
    pool.install(|| {
        let t = Instant::now();
        let (enclosure, _) = refold(OpKey(50), &flat, &u.map, &RefoldSpec::default()).unwrap();
        let refold_s = t.elapsed();
        let t = Instant::now();
        let m = recognize(&enclosure, &SheetOptions::default()).unwrap();
        let rec = t.elapsed();
        let t = Instant::now();
        let again = unfold(OpKey(51), &enclosure, &m, &base_spec(&m)).unwrap();
        let unf = t.elapsed();
        eprintln!(
            "faces {} edges {}; refold {refold_s:?}; recognize {rec:?}; unfold {unf:?}",
            enclosure.faces().count(),
            enclosure.edges().count()
        );
        assert_eq!(again.body.faces().count(), enclosure.faces().count());
        let (unfold_bar, refold_bar) = (bar(1.0), bar(3.0));
        let unfolding = rec + unf;
        assert!(
            unfolding.as_secs_f64() <= unfold_bar,
            "recognizing and unfolding took {unfolding:?}, over {unfold_bar} s"
        );
        assert!(
            refold_s.as_secs_f64() <= refold_bar,
            "refolding took {refold_s:?}, over {refold_bar} s"
        );
    });
}
