//! Blends head to head (design 43, phase 6): ryukei, OCCT and SMLib
//! round and bevel the same edges of the same bodies. ryukei's result
//! must enclose the exact volume where there is one, and OCCT's (its
//! B-rep's exact volume) to within its mesh's chord times its area
//! wherever OCCT succeeds; SMLib's fillets (it measures chamfers as a
//! chord, not as setbacks) and its refusals are reported beside them.
//! Then a plate's 1,000 hole rims filleted in one call on one thread,
//! where ryukei must beat OCCT's time. Prints a report (run with
//! `--nocapture`).

use ryukei::blend::{BlendKind, Chamfer, EdgeBlend, blend};
use ryukei::boolean::{BooleanOp, Tool, ToolKey, boolean};
use ryukei::geom::{Rigid, Vector3};
use ryukei::mesh::{MeshParams, check_mesh, tessellate};
use ryukei::name::{Name, OpKey};
use ryukei::primitive;
use ryukei::topo::Body;
use ryukei_bench::{BenchBlend, BenchKind, Primitive, occt, smlib};
use ryukei_testkit::mesh::volume_and_area;
use std::f64::consts::{FRAC_PI_2, PI};
use std::time::Instant;

const CHORD: f64 = 1e-4;
const ANGLE: f64 = 0.3;

fn at(x: f64, y: f64, z: f64) -> Rigid {
    Rigid::translation(Vector3::new(x, y, z))
}

fn ryukei_body(p: Primitive, place: &Rigid) -> Body {
    let k = OpKey(1);
    match p {
        Primitive::Block([x, y, z]) => primitive::block(k, x, y, z, place),
        Primitive::Cylinder { r, h } => primitive::cylinder(k, r, h, place),
        Primitive::Cone { r0, r1: 0.0, h } => primitive::cone(k, r0, h, place),
        Primitive::Cone { r0, r1, h } => primitive::frustum(k, r0, r1, h, place),
        Primitive::Sphere(r) => primitive::sphere(k, r, place),
        Primitive::Torus { major, minor } => primitive::torus(k, major, minor, place),
    }
    .unwrap()
    .0
}

/// A body every kernel builds: a primitive, united with or less placed
/// tools.
#[derive(Clone)]
struct Shape {
    base: (Primitive, Rigid),
    op: BooleanOp,
    tools: Vec<(Primitive, Rigid)>,
}

impl Shape {
    fn plain(p: Primitive) -> Self {
        Self {
            base: (p, Rigid::IDENTITY),
            op: BooleanOp::Unite,
            tools: Vec::new(),
        }
    }

    fn ryukei(&self) -> Body {
        let base = ryukei_body(self.base.0, &self.base.1);
        if self.tools.is_empty() {
            return base;
        }
        let bodies: Vec<Body> = self.tools.iter().map(|(p, r)| ryukei_body(*p, r)).collect();
        let tools: Vec<Tool> = bodies
            .iter()
            .enumerate()
            .map(|(i, body)| Tool {
                key: ToolKey(i as u128 + 1),
                body,
                place: Rigid::IDENTITY,
            })
            .collect();
        boolean(OpKey(2), self.op, &base, &tools).unwrap().0
    }

    fn occt(&self) -> occt::Solid {
        let base = occt::Solid::new(self.base.0, &self.base.1);
        if self.tools.is_empty() {
            return base;
        }
        let tools: Vec<occt::Solid> = self
            .tools
            .iter()
            .map(|(p, r)| occt::Solid::new(*p, r))
            .collect();
        base.boolean(self.op, &tools, false)
            .expect("OCCT builds the body")
    }

    fn smlib(&self) -> Option<smlib::Solid> {
        let mut s = smlib::Solid::new(self.base.0, &self.base.1, false);
        if self.tools.is_empty() {
            return Some(s);
        }
        for (p, r) in &self.tools {
            s.add_tool(*p, r);
        }
        s.combine_tools(self.op, false).ok().map(|()| s)
    }
}

/// ryukei's blends of the edges passing nearest each blend's point
/// (sampled at 17 points), as the C++ kernels pick them.
fn ryukei_blends(body: &Body, blends: &[BenchBlend]) -> Vec<EdgeBlend> {
    let p = |a: [f64; 3]| ryukei::geom::Point3::new(a[0], a[1], a[2]);
    let edge_at = |q: [f64; 3]| {
        body.edges()
            .min_by(|&a, &b| {
                let near = |e| {
                    let x = body.edge(e).unwrap();
                    let c = body.curve(x.curve).unwrap();
                    let [t0, t1] = x.interval;
                    (0..=16)
                        .map(|k| c.point(t0 + (t1 - t0) * f64::from(k) / 16.0).distance(p(q)))
                        .fold(f64::INFINITY, f64::min)
                };
                near(a).total_cmp(&near(b))
            })
            .unwrap()
    };
    let face_at = |e, q: [f64; 3]| -> Name {
        body.edge_fins(e)
            .map(|fin| body.loop_(body.fin(fin).unwrap().loop_).unwrap().face)
            .min_by(|&a, &b| {
                let d = |f| {
                    body.surface(body.face(f).unwrap().surface)
                        .unwrap()
                        .distance(p(q))
                };
                d(a).total_cmp(&d(b))
            })
            .map(|f| body.face(f).unwrap().name)
            .unwrap()
    };
    blends
        .iter()
        .map(|b| {
            let e = edge_at(b.at);
            let kind = match b.kind {
                BenchKind::Fillet(radius) => BlendKind::Fillet { radius },
                BenchKind::Equal(distance) => BlendKind::Chamfer(Chamfer::Equal { distance }),
                BenchKind::Distances {
                    first,
                    second,
                    first_face,
                } => BlendKind::Chamfer(Chamfer::Distances {
                    first,
                    second,
                    first_face: face_at(e, first_face),
                }),
                BenchKind::DistanceAngle {
                    distance,
                    angle,
                    face,
                } => BlendKind::Chamfer(Chamfer::DistanceAngle {
                    distance,
                    angle,
                    face: face_at(e, face),
                }),
            };
            EdgeBlend {
                edge: body.edge(e).unwrap().name,
                kind,
            }
        })
        .collect()
}

fn fillet(at: [f64; 3], r: f64) -> BenchBlend {
    BenchBlend {
        at,
        kind: BenchKind::Fillet(r),
    }
}

/// The area a fillet of radius `r` takes from a square edge.
fn corner(r: f64) -> f64 {
    (1.0 - PI / 4.0) * r * r
}

/// A case: a name, the body, its blends, and the exact volume if known.
type Case = (&'static str, Shape, Vec<BenchBlend>, Option<f64>);

/// A shaft of radius 1 along `x` and a boss of radius 0.5 on it, united.
fn shaft() -> Shape {
    let y = Vector3::new(0.0, 1.0, 0.0).normalize().unwrap();
    let along_x = Rigid::rotation(y, FRAC_PI_2).then_after(&at(0.0, 0.0, -2.0));
    Shape {
        base: (Primitive::Cylinder { r: 1.0, h: 4.0 }, along_x),
        op: BooleanOp::Unite,
        tools: vec![(Primitive::Cylinder { r: 0.5, h: 2.0 }, Rigid::IDENTITY)],
    }
}

fn cases() -> Vec<Case> {
    let block = Shape::plain(Primitive::Block([2.0, 2.0, 2.0]));
    // The twelve edges' middles of the block.
    let mut edges = Vec::new();
    for (a, b) in [(0.0, 0.0), (2.0, 0.0), (0.0, 2.0), (2.0, 2.0)] {
        edges.push([1.0, a, b]);
        edges.push([a, 1.0, b]);
        edges.push([a, b, 1.0]);
    }
    let r = 0.3;
    // The block with every edge rounded: its core, slabs on its faces,
    // quarter cylinders along its edges and eighth balls at its corners.
    let k = 2.0 - 2.0 * r;
    let rounded = k * k * k + 6.0 * k * k * r + 3.0 * PI * r * r * k + 4.0 / 3.0 * PI * r * r * r;
    let rim_d = 0.2 * (10.0 - 3.0 * PI) / (12.0 - 3.0 * PI);
    let top = [1.0, 2.0, 2.0];
    let plus_z = [1.0, 1.0, 2.0];
    vec![
        (
            "one edge of a block",
            block.clone(),
            vec![fillet(top, r)],
            Some(8.0 - 2.0 * corner(r)),
        ),
        (
            "every edge of a block",
            block.clone(),
            edges.iter().map(|&e| fillet(e, r)).collect(),
            Some(rounded),
        ),
        (
            "a cylinder's rim",
            Shape::plain(Primitive::Cylinder { r: 1.0, h: 1.0 }),
            vec![fillet([0.0, 1.0, 1.0], 0.2)],
            Some(PI - 2.0 * PI * (1.0 - rim_d) * corner(0.2)),
        ),
        (
            "an equal chamfer",
            block.clone(),
            vec![BenchBlend {
                at: top,
                kind: BenchKind::Equal(0.3),
            }],
            Some(8.0 - 0.045 * 2.0),
        ),
        (
            "a chamfer of two distances",
            block.clone(),
            vec![BenchBlend {
                at: top,
                kind: BenchKind::Distances {
                    first: 0.2,
                    second: 0.4,
                    first_face: plus_z,
                },
            }],
            Some(8.0 - 0.04 * 2.0),
        ),
        (
            "a chamfer of a distance and an angle",
            block,
            vec![BenchBlend {
                at: top,
                kind: BenchKind::DistanceAngle {
                    distance: 0.3,
                    angle: PI / 6.0,
                    face: plus_z,
                },
            }],
            Some(8.0 - 0.5 * 0.3 * 0.3 * ryukei::math::tan(PI / 6.0) * 2.0),
        ),
        (
            "a boss on a shaft (a pipe)",
            shaft(),
            vec![fillet([0.0, 0.5, 0.75f64.sqrt()], 0.1)],
            None,
        ),
    ]
}

/// ryukei's result: its mesh's volume and area, and the blend's time.
fn ryukei_result(shape: &Shape, blends: &[BenchBlend]) -> Result<(f64, f64, f64), String> {
    let body = shape.ryukei();
    let ours = ryukei_blends(&body, blends);
    let t = Instant::now();
    let (r, _) = blend(OpKey(9), &body, &ours).map_err(|e| format!("{e:?}"))?;
    let ms = t.elapsed().as_secs_f64() * 1e3;
    let m = tessellate(&r, &MeshParams::new(CHORD, ANGLE)).unwrap();
    assert!(m.faults.is_empty(), "{:?}", m.faults);
    assert_eq!(check_mesh(&r, &m), Ok(()));
    let (v, a) = volume_and_area(&m);
    Ok((v, a, ms))
}

#[test]
fn ryukei_agrees_with_occt_and_smlib() {
    println!(
        "{:<38} {:>12} {:>9} {:>12} {:>9} {:>12} {:>9} {:>12}",
        "case", "ryukei", "ms", "OCCT", "ms", "SMLib", "ms", "exact"
    );
    for (name, shape, blends, exact) in cases() {
        let (rv, ra, rms) = ryukei_result(&shape, &blends).expect(name);
        let solid = shape.occt();
        let t = Instant::now();
        let o = solid.blend(&blends);
        let oms = t.elapsed().as_secs_f64() * 1e3;
        let ov = o.as_ref().ok().map(|o| o.volume());
        let (sv, sms) = match shape.smlib() {
            Some(mut s)
                if blends
                    .iter()
                    .all(|b| matches!(b.kind, BenchKind::Fillet(_))) =>
            {
                let t = Instant::now();
                let done = s.fillet(&blends, false);
                let ms = t.elapsed().as_secs_f64() * 1e3;
                let v = done
                    .ok()
                    .map(|()| s.mesh(1e-3, ANGLE, false).soup().volume_and_area().0);
                (v, ms)
            }
            // SMLib measures chamfers as a chord across the edge.
            Some(_) => (Some(f64::NAN), 0.0),
            None => (None, 0.0),
        };
        let show = |r: Option<f64>| match r {
            None => "refused".to_owned(),
            Some(v) if v.is_nan() => "-".to_owned(),
            Some(v) => format!("{v:.6}"),
        };
        println!(
            "{name:<38} {rv:>12.6} {rms:>9.2} {:>12} {oms:>9.2} {:>12} {sms:>9.2} {:>12}",
            show(ov),
            show(sv),
            show(exact),
        );
        let slack = CHORD * ra + 1e-9;
        if let Some(x) = exact {
            assert!((rv - x).abs() <= slack, "{name}: ryukei {rv} vs exact {x}");
        }
        if let Some(ov) = ov {
            assert!((rv - ov).abs() <= slack, "{name}: ryukei {rv} vs OCCT {ov}");
        }
    }
}

/// A plate 40 × 25 × 0.5 with a grid of 1,000 holes of radius 0.3, as
/// the booleans' race has it.
fn plate() -> Shape {
    let (cols, rows) = (40, 25);
    let mut tools = Vec::new();
    for j in 0..rows {
        for i in 0..cols {
            tools.push((
                Primitive::Cylinder { r: 0.3, h: 1.0 },
                at(i as f64 + 0.5, j as f64 + 0.5, -0.25),
            ));
        }
    }
    Shape {
        base: (
            Primitive::Block([cols as f64, rows as f64, 0.5]),
            Rigid::IDENTITY,
        ),
        op: BooleanOp::Subtract,
        tools,
    }
}

#[test]
fn filleting_1000_rims_beats_occt_on_one_thread() {
    let shape = plate();
    let rims: Vec<BenchBlend> = shape
        .tools
        .iter()
        .map(|(_, r)| {
            let t = r.translation_part();
            fillet([t.x, t.y + 0.3, 0.5], 0.05)
        })
        .collect();
    let pool = rayon::ThreadPoolBuilder::new()
        .num_threads(1)
        .build()
        .unwrap();
    let body = shape.ryukei();
    let ours = ryukei_blends(&body, &rims);
    let t = Instant::now();
    let (r, _) = pool
        .install(|| blend(OpKey(9), &body, &ours))
        .expect("ryukei's fillets");
    let ryukei_ms = t.elapsed().as_secs_f64() * 1e3;
    assert_eq!(r.counts().faces, 6 + 2 * 1000);
    let t = Instant::now();
    let all = rayon::ThreadPoolBuilder::new()
        .build()
        .unwrap()
        .install(|| blend(OpKey(9), &body, &ours))
        .expect("ryukei's fillets");
    let parallel_ms = t.elapsed().as_secs_f64() * 1e3;
    assert_eq!(all.0.dump(), r.dump());

    let solid = shape.occt();
    let t = Instant::now();
    let rounded = solid.blend(&rims).expect("OCCT fillets");
    let occt_ms = t.elapsed().as_secs_f64() * 1e3;
    assert_eq!(rounded.faces(), 6 + 2 * 1000);

    println!(
        "1000 rims filleted: ryukei {ryukei_ms:.0} ms on one thread, {parallel_ms:.0} ms on all; OCCT {occt_ms:.0} ms"
    );
    assert!(
        ryukei_ms < occt_ms,
        "ryukei {ryukei_ms:.0} ms vs OCCT {occt_ms:.0} ms"
    );
}
