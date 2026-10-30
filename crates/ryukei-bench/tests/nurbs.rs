//! ryukei on every surface against OCCT and SMLib (design 94, plan 8.F).
//!
//! Bodies go to OCCT through STEP (ryustep, in millimetres, which OCCT
//! reads as such) and to SMLib through USD (ryuformats' BrepArrays, in
//! meters), so every kernel works on the same exact NURBS; curves go to
//! both as exact clamped NURBS. SMLib runs in Python
//! (`tests/nurbs/reference.py`, `RYUCAD_USD_PYTHON`, else
//! `/opt/usd/bin/python`), each job in a process of its own.
//!
//! - The race: a block whose top is a lofted bicubic face, minus 1,000
//!   lofted pins poking out of both faces, on one thread, against OCCT's
//!   `BRepAlgoAPI_Cut`: ryukei within twice OCCT's time (design 94 asked
//!   it to be faster; that is left to phase 9). Its flush variant (pins standing on the
//!   block's bottom, issue #77) is measured on the first row of 40 pins
//!   and printed with its ratio to the clear pins' time per pin; SMLib's
//!   times are printed beside them.
//! - Lofts, sweeps along paths, offsets and shells: volumes against exact
//!   values where they are known, else against the references; where
//!   both references agree and ryukei doesn't, the test fails.
//! - Intersections: branch counts and the Hausdorff distance between
//!   ryukei's branches and each reference's curves.
//!
//! Devcontainer only (`--features occt,usd`); prints a report (run with
//! `--nocapture`).

use core::f64::consts::PI;
use ryukei::boolean::{BooleanOp, Tool, ToolKey, boolean};
use ryukei::geom::{Curve, Frame, KnotVector, Line, NurbsCurve, Point3, Rigid, Vector3};
use ryukei::intersect::{SurfaceHit, face_box, surface_surface};
use ryukei::measure::{Entity, Properties, properties};
use ryukei::name::{Name, OpKey};
use ryukei::offset::{OffsetSpec, ShellSpec, offset_body, shell};
use ryukei::primitive;
use ryukei::sweep::{
    AlongOptions, LoftOptions, Loop, LoopKey, Path, PathSegment, Profile, Segment, SegmentKey,
    ValidProfile, along, loft,
};
use ryukei::topo::{Body, FaceId};
use ryukei_bench::{BenchCurves, occt};
use ryukei_testkit::nurbs_race;
use serde_json::{Value, json};
use std::path::{Path as FilePath, PathBuf};
use std::sync::Arc;
use std::time::Instant;

/// A body's volume, measured by ryukei, and its bound.
fn measured(b: &Body) -> (f64, f64) {
    let Properties::Body { volume, .. } = properties(b, Entity::Body).unwrap() else {
        unreachable!()
    };
    (volume.value(), volume.bound())
}

/// `b` written as STEP in millimetres and read by OCCT, which keeps
/// millimetres: its volumes are in mm³.
fn to_occt(dir: &FilePath, name: &str, b: &Body) -> occt::Solid {
    use ryustep::{LengthUnit, Product, WriteModel, WriteOptions, write};
    let bytes = write(
        &WriteModel {
            products: vec![Product {
                id: name.into(),
                name: name.into(),
                bodies: vec![b.clone()],
                children: vec![],
                ..Default::default()
            }],
        },
        &WriteOptions {
            unit: LengthUnit::Millimeter,
            time_stamp: "2026-10-30T00:00:00".into(),
            ..WriteOptions::default()
        },
    )
    .unwrap();
    let path = dir.join(format!("{name}.stp"));
    std::fs::write(&path, bytes).unwrap();
    occt::Solid::read_step(&path, false).unwrap()
}

/// `b` written as USD (BrepArrays only, in meters) for SMLib.
fn to_usd(dir: &FilePath, name: &str, b: &Body) -> PathBuf {
    use ryuformats::Unit;
    use ryuformats::usd::stage::{Model, Options, Product, Solid, layer};
    let model = Model {
        products: vec![Product {
            id: name.into(),
            name: name.into(),
            part: name.into(),
            label: None,
            solids: vec![Solid {
                name: "s".into(),
                body: Arc::new(b.clone()),
                mesh: None,
            }],
            children: vec![],
        }],
    };
    let opts = Options {
        unit: Unit::Meter,
        brep: true,
        mesh: false,
        pipe_tolerance: 1e-7,
        layer_data: vec![],
    };
    let path = dir.join(format!("{name}.usda"));
    let text = ryuformats::usd::usda::to_string(&layer(&model, &opts).unwrap()).unwrap();
    std::fs::write(&path, text).unwrap();
    path
}

/// SMLib's results for `jobs`, one each (see `tests/nurbs/reference.py`).
fn smlib(dir: &FilePath, jobs: &[Value]) -> Vec<Value> {
    let python =
        std::env::var("RYUCAD_USD_PYTHON").unwrap_or_else(|_| "/opt/usd/bin/python".into());
    let manifest = dir.join("jobs.json");
    std::fs::write(&manifest, json!({ "jobs": jobs }).to_string()).unwrap();
    let script = FilePath::new(env!("CARGO_MANIFEST_DIR")).join("tests/nurbs/reference.py");
    let out = std::process::Command::new(&python)
        .arg(&script)
        .arg(&manifest)
        .output()
        .unwrap_or_else(|e| panic!("{python}: {e}"));
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    serde_json::from_slice(&out.stdout).unwrap()
}

/// A curve as `reference.py` reads it: its exact NURBS form, clamped.
fn curve_json(c: &Curve, range: [f64; 2]) -> Value {
    let mut b = BenchCurves::default();
    b.push(c, range).unwrap();
    let [degree, nk, np, rational] = [0, 1, 2, 3].map(|k| b.spec[k] as usize);
    let d = &b.data;
    let points: Vec<[f64; 3]> = (0..np)
        .map(|i| {
            let at = 2 * nk + 3 * i;
            [d[at], d[at + 1], d[at + 2]]
        })
        .collect();
    let weights: Vec<f64> = if rational == 1 {
        d[2 * nk + 3 * np..].to_vec()
    } else {
        Vec::new()
    };
    json!({
        "degree": degree,
        "knots": d[..nk].to_vec(),
        "mults": d[nk..2 * nk].iter().map(|&m| m as usize).collect::<Vec<_>>(),
        "points": points,
        "weights": weights,
    })
}

/// A reference's volume from its result, or what it said instead.
fn reference_volume(r: &Value) -> Result<f64, String> {
    r["volume"]
        .as_f64()
        .ok_or_else(|| r["error"].as_str().unwrap_or("no volume").to_string())
}

/// Judges a volume: against `exact` where it is known (within ryukei's
/// bound and a millionth), else, when both references agree within
/// `rel`, against them.
fn judge(
    name: &str,
    ryukei: (f64, f64),
    exact: Option<f64>,
    occt: Result<f64, String>,
    smlib: Result<f64, String>,
    rel: f64,
) {
    let show = |r: &Result<f64, String>| match r {
        Ok(v) => format!("{v:.9}"),
        Err(e) => format!("refused ({})", e.chars().take(60).collect::<String>()),
    };
    println!(
        "{name:<34} ryukei {:.9} ± {:.1e}  exact {}  OCCT {}  SMLib {}",
        ryukei.0,
        ryukei.1,
        exact.map_or("-".into(), |v| format!("{v:.9}")),
        show(&occt),
        show(&smlib),
    );
    let (v, bound) = ryukei;
    if let Some(e) = exact {
        assert!(
            (v - e).abs() <= bound + 1e-6 * e.abs(),
            "{name}: ryukei {v} against {e}"
        );
    } else if let (Ok(o), Ok(s)) = (&occt, &smlib)
        && (o - s).abs() <= rel * o.abs()
    {
        assert!(
            (v - o).abs() <= bound + rel * o.abs(),
            "{name}: ryukei {v} against OCCT {o} and SMLib {s}"
        );
    }
}

fn at(x: f64, y: f64, z: f64) -> Rigid {
    Rigid::translation(Vector3::new(x, y, z))
}

/// Millimetres for OCCT: `place` with its translation scaled.
fn in_mm(place: &Rigid) -> Rigid {
    let t = place.translation_part() * 1000.0;
    Rigid::translation(Vector3::new(t.x, t.y, t.z))
}

fn one_thread<T: Send>(f: impl FnOnce() -> T + Send) -> T {
    rayon::ThreadPoolBuilder::new()
        .num_threads(1)
        .build()
        .unwrap()
        .install(f)
}

/// ryukei's cut of `pin` at `places` from `block`, and its seconds.
fn ryukei_cut(block: &Body, pin: &Body, places: &[Rigid]) -> (Body, f64) {
    let tools: Vec<Tool> = places
        .iter()
        .enumerate()
        .map(|(i, p)| Tool {
            key: ToolKey(i as u128 + 1),
            body: pin,
            place: *p,
        })
        .collect();
    let t = Instant::now();
    let (cut, _) = one_thread(|| boolean(OpKey(3), BooleanOp::Subtract, block, &tools))
        .expect("ryukei's boolean");
    (cut, t.elapsed().as_secs_f64())
}

/// OCCT's cut of the same, and its seconds; its failure as text.
fn occt_cut(
    block: &occt::Solid,
    pin: &occt::Solid,
    places: &[Rigid],
) -> (Result<occt::Solid, String>, f64) {
    let pins: Vec<occt::Solid> = places.iter().map(|p| pin.placed(&in_mm(p))).collect();
    let t = Instant::now();
    let cut = block.boolean(BooleanOp::Subtract, &pins, false);
    (cut, t.elapsed().as_secs_f64())
}

#[test]
fn the_nurbs_race_against_occt_on_one_thread() {
    let dir = tempfile::tempdir().unwrap();
    let (block, clear, flush) = (
        nurbs_race::block(),
        nurbs_race::clear_pin(),
        nurbs_race::flush_pin(),
    );
    let places = nurbs_race::places(nurbs_race::COLS * nurbs_race::ROWS);
    let row = &places[..nurbs_race::COLS];

    let (cut, ryukei_s) = ryukei_cut(&block, &clear, &places);
    assert_eq!(cut.counts().faces, 6 + places.len());
    let (row_cut, row_s) = ryukei_cut(&block, &flush, row);
    assert_eq!(row_cut.counts().faces, 6 + row.len());
    let clear_row_s = ryukei_cut(&block, &clear, row).1;

    let (ob, oc, of) = (
        to_occt(dir.path(), "block", &block),
        to_occt(dir.path(), "clear", &clear),
        to_occt(dir.path(), "flush", &flush),
    );
    let (occt_result, occt_s) = occt_cut(&ob, &oc, &places);
    let occt_result = occt_result.expect("OCCT cuts");
    assert_eq!(occt_result.faces() as usize, 6 + places.len());
    let (occt_flush, occt_flush_s) = occt_cut(&ob, &of, row);

    let usd = |name: &str, b: &Body| to_usd(dir.path(), name, b);
    let xyz = |ps: &[Rigid]| -> Vec<[f64; 3]> {
        ps.iter()
            .map(|p| {
                let t = p.translation_part();
                [t.x, t.y, t.z]
            })
            .collect()
    };
    let (ub, uc, uf) = (
        usd("block", &block),
        usd("clear", &clear),
        usd("flush", &flush),
    );
    let s = smlib(
        dir.path(),
        &[
            json!({"kind": "race", "block": ub, "pin": uc, "places": xyz(&places)}),
            json!({"kind": "race", "block": ub, "pin": uf, "places": xyz(row)}),
        ],
    );

    let (v, bound) = measured(&cut);
    println!("NURBS race, one thread, {} pins:", places.len());
    println!("  ryukei {ryukei_s:.1} s, {v:.6} m³ ± {bound:.1e}");
    println!(
        "  OCCT   {occt_s:.1} s, {:.6} m³ (OCCT's own volume)",
        occt_result.volume() * 1e-9
    );
    println!("  SMLib  {}", s[0]);
    println!(
        "Flush, the first row of {}: ryukei {row_s:.1} s ({:.1}× the clear pins' time per pin, {clear_row_s:.1} s); OCCT {}; SMLib {}",
        row.len(),
        row_s / clear_row_s,
        match &occt_flush {
            Ok(_) => format!("{occt_flush_s:.1} s"),
            Err(e) => format!("refused after {occt_flush_s:.1} s ({e})"),
        },
        s[1],
    );
    if let Some(sv) = s[0]["volume"].as_f64() {
        assert!(
            (v - sv).abs() <= bound + 1e-6 * sv,
            "ryukei {v} against SMLib {sv}"
        );
    }
    // Design 94's bar was to beat OCCT; plan 8.F measured ryukei at about
    // twice OCCT's time once OCCT read the pins right (it had read them as
    // other shapes, and lost), and keeps the gap from growing past that.
    println!("ryukei / OCCT: {:.2}", ryukei_s / occt_s);
    assert!(
        ryukei_s < 2.0 * occt_s,
        "ryukei {ryukei_s:.1} s vs OCCT {occt_s:.1} s"
    );
}

/// A square of side `s` in its frame, centred on its origin.
fn square(first: u128, s: f64) -> Loop {
    let h = s / 2.0;
    let pts = [(-h, -h), (h, -h), (h, h), (-h, h)].map(|(x, y)| ryukei::geom::Point2::new(x, y));
    Loop {
        key: LoopKey(1),
        segments: (0..4)
            .map(|i| {
                let (p, q) = (pts[i], pts[(i + 1) % 4]);
                Segment {
                    key: SegmentKey(first + i as u128),
                    curve: ryukei::geom::Curve2::Line2(ryukei::geom::Line2::through(p, q).unwrap()),
                    t0: 0.0,
                    t1: p.distance(q),
                }
            })
            .collect(),
    }
}

/// The same square as 3D lines in `frame`.
fn square_curves(frame: &Frame, s: f64) -> Vec<(Curve, [f64; 2])> {
    let h = s / 2.0;
    let pts = [(-h, -h), (h, -h), (h, h), (-h, h)].map(|(x, y)| frame.to_world(x, y, 0.0));
    (0..4)
        .map(|i| {
            let (p, q) = (pts[i], pts[(i + 1) % 4]);
            (
                Curve::Line(Line::through(p, q).unwrap()),
                [0.0, p.distance(q)],
            )
        })
        .collect()
}

fn level(z: f64) -> Frame {
    Frame::new(
        Point3::new(0.0, 0.0, z),
        Vector3::new(0.0, 0.0, 1.0),
        Vector3::new(1.0, 0.0, 0.0),
    )
    .unwrap()
}

#[test]
fn lofts_against_occt_and_smlib() {
    let dir = tempfile::tempdir().unwrap();
    // A frustum of squares: (2² + 2·1 + 1²) / 3.
    let sections: Vec<ValidProfile> = [(0.0, 2.0), (1.0, 1.0)]
        .iter()
        .enumerate()
        .map(|(k, &(z, s))| {
            ValidProfile::new(Profile {
                frame: level(z),
                loops: vec![square(10 * (k as u128 + 1), s)],
            })
            .unwrap()
        })
        .collect();
    let (frustum, _) = loft(OpKey(5), &sections, &LoftOptions::default()).unwrap();
    let curves = vec![
        square_curves(&level(0.0), 2.0),
        square_curves(&level(1.0), 1.0),
    ];
    let occt = occt::Solid::loft(&curves).map(|s| s.volume());
    let jobs = [
        json!({"kind": "loft", "sections": curves.iter().map(|s| s.iter().map(|(c, r)| curve_json(c, *r)).collect::<Vec<_>>()).collect::<Vec<_>>()}),
        json!({"kind": "volume", "file": to_usd(dir.path(), "pin", &nurbs_race::clear_pin())}),
    ];
    let s = smlib(dir.path(), &jobs);
    judge(
        "frustum of squares",
        measured(&frustum),
        Some(7.0 / 3.0),
        occt,
        reference_volume(&s[0]),
        1e-6,
    );
    // The race's pin, through STEP and USD: the same skin in every kernel.
    let pin = nurbs_race::clear_pin();
    let op = to_occt(dir.path(), "pin", &pin).volume() * 1e-9;
    judge(
        "the race's pin",
        measured(&pin),
        None,
        Ok(op),
        reference_volume(&s[1]),
        1e-6,
    );
}

/// A planar cubic path through the xz plane.
fn planar_path() -> NurbsCurve {
    let k = KnotVector::clamped(3, &[0.0, 1.0, 2.0], &[4, 1, 4]).unwrap();
    let pts = vec![
        Point3::new(0.0, 0.0, 0.0),
        Point3::new(1.0, 0.0, 0.0),
        Point3::new(2.0, 0.0, 1.0),
        Point3::new(3.0, 0.0, 1.5),
        Point3::new(4.0, 0.0, 2.0),
    ];
    NurbsCurve::new(k, pts, None).unwrap()
}

fn length(c: &NurbsCurve, [a, b]: [f64; 2]) -> f64 {
    let steps = 200_000;
    (0..steps)
        .map(|i| {
            let t = |k: usize| a + (b - a) * k as f64 / steps as f64;
            c.point(t(i)).distance(c.point(t(i + 1)))
        })
        .sum()
}

#[test]
fn sweeps_along_paths_against_occt_and_smlib() {
    let dir = tempfile::tempdir().unwrap();
    // A square of 0.3 along a planar path, square to it at its start: no
    // twist, its centroid on the path, so area times length (Pappus).
    let path_curve = planar_path();
    let s = 0.3;
    let start = Frame::new(
        Point3::ORIGIN,
        Vector3::new(1.0, 0.0, 0.0),
        Vector3::new(0.0, 1.0, 0.0),
    )
    .unwrap();
    let profile = ValidProfile::new(Profile {
        frame: start,
        loops: vec![square(10, s)],
    })
    .unwrap();
    let path = Path {
        segments: vec![PathSegment {
            key: SegmentKey(1),
            curve: Curve::Nurbs(Arc::new(path_curve.clone())),
            range: [0.0, 2.0],
        }],
    };
    let (b, _) = along(OpKey(6), &profile, &path, &AlongOptions::default()).unwrap();
    let exact = s * s * length(&path_curve, [0.0, 2.0]);
    let profile3 = square_curves(&start, s);
    let spine = vec![(Curve::Nurbs(Arc::new(path_curve.clone())), [0.0, 2.0])];
    let occt = occt::Solid::pipe(&profile3, &spine, None).map(|x| x.volume());
    let job = json!({
        "kind": "pipe",
        "profile": profile3.iter().map(|(c, r)| curve_json(c, *r)).collect::<Vec<_>>(),
        "path": spine.iter().map(|(c, r)| curve_json(c, *r)).collect::<Vec<_>>(),
    });
    let r = smlib(dir.path(), &[job]);
    judge(
        "square along a planar spline",
        measured(&b),
        Some(exact),
        occt,
        reference_volume(&r[0]),
        1e-6,
    );
}

/// The face of `b` whose surface passes through `p` and whose box holds
/// it.
fn face_at(b: &Body, p: Point3) -> FaceId {
    b.faces()
        .find(|&f| {
            let s = b.surface(b.face(f).unwrap().surface).unwrap();
            s.distance(p) <= 1e-9
                && face_box(b, f)
                    .grow(1e-9)
                    .intersects(&ryukei::geom::Box3::from_point(p))
        })
        .expect("a face there")
}

fn name_at(b: &Body, p: Point3) -> Name {
    b.face(face_at(b, p)).unwrap().name
}

/// A box grown by `d`: Steiner's formula for a box `x × y × z`.
fn grown_box([x, y, z]: [f64; 3], d: f64) -> f64 {
    x * y * z
        + 2.0 * (x * y + y * z + z * x) * d
        + PI * (x + y + z) * d * d
        + 4.0 / 3.0 * PI * d * d * d
}

#[test]
fn offsets_and_shells_against_occt_and_smlib() {
    let dir = tempfile::tempdir().unwrap();
    let size = [1.0, 0.6, 0.4];
    let (plain, _) =
        primitive::block(OpKey(1), size[0], size[1], size[2], &Rigid::IDENTITY).unwrap();
    let mut spline = plain.clone();
    ryukei_testkit::nurbs::convert_body(&mut spline).unwrap();
    let d = 0.1;
    let t = 0.05;
    let top = Point3::new(0.5, 0.3, 0.4);
    for (name, b) in [("box", &plain), ("spline box", &spline)] {
        let tag = name.replace(' ', "_");
        let (grown, _) = offset_body(
            OpKey(2),
            b,
            &OffsetSpec {
                distance: d,
                faces: vec![],
            },
        )
        .unwrap();
        let (hollow, _) = shell(
            OpKey(3),
            b,
            &ShellSpec {
                thickness: t,
                outward: false,
                open: vec![name_at(b, top)],
                faces: vec![],
            },
        )
        .unwrap();
        let ob = to_occt(dir.path(), &tag, b);
        let mm = |p: Point3| Point3::new(p.x * 1e3, p.y * 1e3, p.z * 1e3);
        let file = to_usd(dir.path(), &tag, b);
        let r = smlib(
            dir.path(),
            &[
                json!({"kind": "offset", "file": file, "distance": d}),
                json!({"kind": "shell", "file": file, "thickness": t, "open": [[top.x, top.y, top.z]]}),
            ],
        );
        judge(
            &format!("{name} grown by {d}"),
            measured(&grown),
            Some(grown_box(size, d)),
            ob.offset(d * 1e3).map(|x| x.volume() * 1e-9),
            reference_volume(&r[0]),
            1e-6,
        );
        let [x, y, z] = size;
        judge(
            &format!("{name} shelled by {t}, top open"),
            measured(&hollow),
            Some(x * y * z - (x - 2.0 * t) * (y - 2.0 * t) * (z - t)),
            ob.shell(&[mm(top)], t * 1e3).map(|x| x.volume() * 1e-9),
            reference_volume(&r[1]),
            1e-6,
        );
    }
}

/// The largest distance from a point of `a` to the polyline `b`.
fn one_way(a: &[Point3], b: &[Point3]) -> f64 {
    let seg = |p: Point3, q: Point3, x: Point3| {
        let d = q - p;
        let t = ((x - p).dot(d) / d.dot(d).max(1e-300)).clamp(0.0, 1.0);
        (p + d * t).distance(x)
    };
    a.iter()
        .map(|&x| {
            b.windows(2)
                .map(|w| seg(w[0], w[1], x))
                .fold(f64::INFINITY, f64::min)
        })
        .fold(0.0, f64::max)
}

/// The Hausdorff distance between two sets of polylines.
fn hausdorff(a: &[Vec<Point3>], b: &[Vec<Point3>]) -> f64 {
    let all = |x: &[Vec<Point3>], y: &[Vec<Point3>]| {
        x.iter()
            .map(|p| {
                y.iter()
                    .map(|q| one_way(p, q))
                    .fold(f64::INFINITY, f64::min)
            })
            .fold(0.0, f64::max)
    };
    all(a, b).max(all(b, a))
}

#[test]
fn intersections_against_occt_and_smlib() {
    let dir = tempfile::tempdir().unwrap();
    let samples = 256;
    let block = nurbs_race::block();
    let mut cylinder = primitive::cylinder(OpKey(4), 0.5, 2.0, &at(20.0, 12.0, -0.5))
        .unwrap()
        .0;
    ryukei_testkit::nurbs::convert_body(&mut cylinder).unwrap();
    let pin = nurbs_race::clear_pin()
        .transformed(&at(10.5, 7.5, 0.0))
        .unwrap();
    // A point inside the top: the middle of the face reaching highest.
    let top = block
        .faces()
        .max_by(|&x, &y| {
            face_box(&block, x)
                .max()
                .z
                .total_cmp(&face_box(&block, y).max().z)
        })
        .unwrap();
    let top_surface = block.surface(block.face(top).unwrap().surface).unwrap();
    let [du, dv] = top_surface.domain();
    let on_top = top_surface.point((du[0] + du[1]) / 2.0, (dv[0] + dv[1]) / 2.0);
    // Pairs: the block's top and a pin's side (bicubic and lofted wavy),
    // and the top and a NURBS cylinder's side.
    let cases: [(&str, &Body, Point3, Point3); 2] = [
        (
            "top and a pin's side",
            &pin,
            on_top,
            Point3::new(10.5 + nurbs_race::RADIUS, 7.5, 0.25),
        ),
        (
            "top and a spline cylinder",
            &cylinder,
            on_top,
            Point3::new(20.5, 12.0, 0.5),
        ),
    ];
    let ob = to_occt(dir.path(), "block", &block);
    let ub = to_usd(dir.path(), "block", &block);
    for (name, other, pa, pb) in cases {
        let sa = top_surface;
        let near_b = other
            .faces()
            .min_by(|&x, &y| {
                let d = |f: FaceId| {
                    other
                        .surface(other.face(f).unwrap().surface)
                        .unwrap()
                        .distance(pb)
                };
                d(x).total_cmp(&d(y))
            })
            .unwrap();
        let sb = other.surface(other.face(near_b).unwrap().surface).unwrap();
        let region = face_box(&block, top).union(&face_box(other, near_b));
        let ours: Vec<Vec<Point3>> = match surface_surface(sa, sb, &region).unwrap() {
            SurfaceHit::Meet { branches, .. } => branches
                .iter()
                .map(|br| {
                    (0..=samples)
                        .map(|k| {
                            let [t0, t1] = br.range;
                            br.curve.point(t0 + (t1 - t0) * k as f64 / samples as f64)
                        })
                        .collect()
                })
                .collect(),
            SurfaceHit::Coincident { .. } => panic!("{name}: coincident"),
        };
        let tag = name.replace(' ', "_").replace('\'', "");
        let oo = to_occt(dir.path(), &tag, other);
        let mm = |p: Point3| Point3::new(p.x * 1e3, p.y * 1e3, p.z * 1e3);
        let theirs_occt = occt::intersect_faces((&ob, mm(pa)), (&oo, mm(pb)), samples).map(|ls| {
            ls.into_iter()
                .map(|l| {
                    l.into_iter()
                        .map(|p| Point3::new(p.x * 1e-3, p.y * 1e-3, p.z * 1e-3))
                        .collect()
                })
                .collect::<Vec<Vec<Point3>>>()
        });
        let r = smlib(
            dir.path(),
            &[json!({"kind": "ssi", "a": ub, "pa": [pa.x, pa.y, pa.z],
                     "b": to_usd(dir.path(), &tag, other), "pb": [pb.x, pb.y, pb.z],
                     "samples": samples})],
        );
        let theirs_smlib: Result<Vec<Vec<Point3>>, String> = r[0]["curves"]
            .as_array()
            .map(|cs| {
                cs.iter()
                    .map(|c| {
                        c.as_array()
                            .unwrap()
                            .iter()
                            .map(|p| {
                                let p: Vec<f64> = serde_json::from_value(p.clone()).unwrap();
                                Point3::new(p[0], p[1], p[2])
                            })
                            .collect()
                    })
                    .collect()
            })
            .ok_or_else(|| r[0]["error"].to_string());
        let show = |t: &Result<Vec<Vec<Point3>>, String>| match t {
            Ok(ls) => format!(
                "{} curves, {:.1e} m from ryukei's",
                ls.len(),
                hausdorff(&ours, ls)
            ),
            Err(e) => format!("refused ({e})"),
        };
        if std::env::var_os("SCRATCH_SSI").is_some() {
            eprintln!(
                "SCRATCH ours {:?}",
                ours.iter()
                    .map(|l| (l[0], l[l.len() / 2]))
                    .collect::<Vec<_>>()
            );
            if let Ok(o) = &theirs_occt {
                eprintln!(
                    "SCRATCH occt {:?}",
                    o.iter()
                        .map(|l| (l.len(), l[0], l[l.len() / 2], l[l.len() - 1]))
                        .collect::<Vec<_>>()
                );
            }
        }
        println!(
            "{name:<30} ryukei {} branches; OCCT {}; SMLib {}",
            ours.len(),
            show(&theirs_occt),
            show(&theirs_smlib),
        );
        // Both references agreeing (as point sets within 10 µm), ryukei
        // must agree with them within as much.
        if let (Ok(o), Ok(s)) = (&theirs_occt, &theirs_smlib)
            && hausdorff(o, s) <= 1e-5
        {
            assert!(
                hausdorff(&ours, o) <= 1e-5,
                "{name}: ryukei {:.1e} m from both references",
                hausdorff(&ours, o)
            );
        }
    }
}
