//! Bodies read from STEP with spline faces take part in booleans: a
//! lofted frustum and a sphere in NURBS form written and read back, then
//! drilled and halved, against exact volumes; and the committed corpus's
//! spline-filleted body drilled through a fillet (release only).

use core::f64::consts::PI;
use ryukei::boolean::{BooleanOp, Tool, ToolKey, boolean};
use ryukei::check::{check, check_intersections};
use ryukei::geom::{Curve2, Frame, Line2, Point2, Point3, Rigid, Surface, Vector3};
use ryukei::heal::HealOptions;
use ryukei::mesh::{MeshParams, tessellate};
use ryukei::name::OpKey;
use ryukei::primitive;
use ryukei::sweep::{LoftOptions, Loop, LoopKey, Profile, Segment, SegmentKey, ValidProfile, loft};
use ryukei::topo::Body;
use ryukei_testkit::mesh::volume_and_area;
use ryustep::{LengthUnit, Product, ReadOptions, WriteModel, WriteOptions, read, write};

/// `b` written to STEP in meters and read back, healed.
fn through_step(b: &Body) -> Body {
    let bytes = write(
        &WriteModel {
            products: vec![Product {
                id: "part".into(),
                name: "part".into(),
                bodies: vec![b.clone()],
                children: vec![],
                ..Default::default()
            }],
        },
        &WriteOptions {
            unit: LengthUnit::Meter,
            file_name: "drill".into(),
            time_stamp: "2026-10-27T00:00:00".into(),
            ..WriteOptions::default()
        },
    )
    .unwrap();
    let m = read(
        &bytes,
        &ReadOptions {
            key: OpKey(5),
            heal: Default::default(),
        },
    )
    .unwrap();
    m.bodies[0].result.clone().unwrap().0
}

fn spline_faces(b: &Body) -> usize {
    b.faces()
        .filter(|&f| {
            matches!(
                b.surface(b.face(f).unwrap().surface),
                Some(Surface::Nurbs(_))
            )
        })
        .count()
}

/// The volume of a sound body, checked and meshed at a chord of a
/// millimetre.
fn volume(b: &Body) -> f64 {
    assert_eq!(check(b), []);
    assert_eq!(check_intersections(b), []);
    let m = tessellate(b, &MeshParams::new(1e-3, 0.2)).unwrap();
    assert!(m.faults.is_empty(), "{:?}", m.faults);
    volume_and_area(&m).0
}

fn near(got: f64, want: f64) {
    assert!(
        (got - want).abs() <= 2e-3 * want.abs().max(1e-3),
        "{got} against {want}"
    );
}

fn cut(op: BooleanOp, target: &Body, tool: &Body, place: Rigid) -> Body {
    let tools = [Tool {
        key: ToolKey(1),
        body: tool,
        place,
    }];
    boolean(OpKey(9), op, target, &tools).unwrap().0
}

/// A square of side `s` in the plane `z = h`.
fn square(h: f64, first: u128, s: f64) -> ValidProfile {
    let d = s / 2.0;
    let pts = [(-d, -d), (d, -d), (d, d), (-d, d)];
    let segments = (0..4)
        .map(|i| {
            let (p, q) = (pts[i], pts[(i + 1) % 4]);
            let (p, q) = (Point2::new(p.0, p.1), Point2::new(q.0, q.1));
            Segment {
                key: SegmentKey(first + i as u128),
                curve: Curve2::Line2(Line2::through(p, q).unwrap()),
                t0: 0.0,
                t1: p.distance(q),
            }
        })
        .collect();
    ValidProfile::new(Profile {
        frame: Frame::new(
            Point3::new(0.0, 0.0, h),
            Vector3::new(0.0, 0.0, 1.0),
            Vector3::new(1.0, 0.0, 0.0),
        )
        .unwrap(),
        loops: vec![Loop {
            key: LoopKey(1),
            segments,
        }],
    })
    .unwrap()
}

#[test]
fn spline_bodies_read_from_step_are_drilled() {
    let (frustum, _) = loft(
        OpKey(3),
        &[square(0.0, 10, 2.0), square(1.0, 20, 1.0)],
        &LoftOptions::default(),
    )
    .unwrap();
    let frustum = through_step(&frustum);
    assert_eq!(spline_faces(&frustum), 4);
    let rod = primitive::cylinder(OpKey(2), 0.2, 3.0, &Rigid::IDENTITY)
        .unwrap()
        .0;
    let drilled = cut(
        BooleanOp::Subtract,
        &frustum,
        &rod,
        Rigid::translation(Vector3::new(0.0, 0.0, -1.0)),
    );
    near(volume(&drilled), 7.0 / 3.0 - PI * 0.04);
    let mut ball = primitive::sphere(OpKey(4), 1.0, &Rigid::IDENTITY)
        .unwrap()
        .0;
    ryukei_testkit::nurbs::convert_body(&mut ball).unwrap();
    let ball = through_step(&ball);
    assert_eq!(spline_faces(&ball), 1);
    let block = primitive::block(OpKey(7), 2.0, 3.0, 3.0, &Rigid::IDENTITY)
        .unwrap()
        .0;
    let half = cut(
        BooleanOp::Intersect,
        &ball,
        &block,
        Rigid::translation(Vector3::new(0.0, -1.5, -1.5)),
    );
    near(volume(&half), 2.0 / 3.0 * PI);
}

/// The committed corpus's body with spline fillets (NIST's CTC 05),
/// drilled through a fillet and through its plate: what the drill takes
/// and what it leaves make up the body. Its tolerant edges, read from the
/// file, take fitted pcurves on the faces they merge into. Its own
/// self-intersection check is plan 8.F's (25 pairs it can't settle yet).
#[test]
#[cfg_attr(
    debug_assertions,
    ignore = "slow in a debug build: CI runs it in release"
)]
fn the_corpus_spline_body_is_drilled() {
    let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/corpus");
    let bytes = std::fs::read(dir.join("nist_ctc_05_asme1_rd.stp")).unwrap();
    let m = read(
        &bytes,
        &ReadOptions {
            key: OpKey(7),
            heal: Default::default(),
        },
    )
    .unwrap();
    let body = m
        .bodies
        .iter()
        .find_map(|b| b.result.as_ref().ok())
        .unwrap()
        .0
        .clone();
    assert_eq!(spline_faces(&body), 9);
    let size = |b: &Body| {
        let bx = b
            .faces()
            .map(|f| ryukei::intersect::face_box(b, f))
            .reduce(|x, y| x.union(&y))
            .unwrap();
        (bx.max() - bx.min()).length()
    };
    let mesh_volume = |b: &Body| {
        assert_eq!(check(b), []);
        let m = tessellate(b, &MeshParams::new(size(&body) * 2e-4, 0.3)).unwrap();
        assert_eq!(m.faults, []);
        volume_and_area(&m).0
    };
    let whole = mesh_volume(&body);
    let rod = primitive::cylinder(OpKey(2), 0.004, 1.0, &Rigid::IDENTITY)
        .unwrap()
        .0;
    for (x, y) in [(0.1417, 0.0619), (-0.0812, -0.0739)] {
        let place = Rigid::translation(Vector3::new(x, y, -0.5));
        let left = cut(BooleanOp::Subtract, &body, &rod, place);
        let taken = cut(BooleanOp::Intersect, &body, &rod, place);
        let (l, t) = (mesh_volume(&left), mesh_volume(&taken));
        assert!(t > 0.0);
        assert!(
            (l + t - whole).abs() <= 1e-3 * whole,
            "{l} + {t} against {whole}"
        );
    }
}

/// Issue #204: OCCT's half turn of a spline profile has its two cut
/// faces on one plane (y = 0), either side of the axis edge, as other CAD
/// writes faces on one surface. A hole through its top and bottom, clear
/// of that plane, renames nothing the file had: the boolean merges only
/// what it made.
#[test]
fn a_hole_renames_nothing_an_imported_body_had() {
    use ryukei::topo::EntityId;
    use std::collections::BTreeSet;
    let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/data/occt");
    let bytes = std::fs::read(dir.join("spline_revolve.stp")).unwrap();
    let m = read(
        &bytes,
        &ReadOptions {
            key: OpKey(7),
            heal: Default::default(),
        },
    )
    .unwrap();
    let body = m.bodies[0].result.clone().unwrap().0;
    assert_eq!(body.counts().faces, 5);
    let names = |b: &Body| -> BTreeSet<_> {
        b.faces()
            .map(EntityId::Face)
            .chain(b.edges().map(EntityId::Edge))
            .chain(b.vertices().map(EntityId::Vertex))
            .map(|x| b.name_of(x).unwrap())
            .collect()
    };
    // A rod 0.4 mm across, 0.5 mm off the plane, through both ends.
    let rod = primitive::cylinder(OpKey(2), 2e-4, 0.01, &Rigid::IDENTITY)
        .unwrap()
        .0;
    let drilled = cut(
        BooleanOp::Subtract,
        &body,
        &rod,
        Rigid::translation(Vector3::new(0.0, 5e-4, -1e-3)),
    );
    assert_eq!(drilled.counts().faces, 6);
    let lost: Vec<_> = names(&body).difference(&names(&drilled)).copied().collect();
    assert_eq!(lost, []);
}

/// Every body the committed corpus and the OCCT files heal, healed again
/// with tightening off (design 299, plan 14.A): drilled through its box's
/// middle by a rod a fortieth of its size, the untightened body must
/// drill wherever the tightened one does, what the drill takes and leaves
/// making up the body. Release only.
#[test]
#[cfg_attr(
    debug_assertions,
    ignore = "minutes in a debug build: CI runs it in release"
)]
fn bodies_drill_untightened_wherever_they_drill_tightened() {
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("tests");
    let mut files: Vec<std::path::PathBuf> = Vec::new();
    for dir in [root.join("corpus"), root.join("data/occt")] {
        for e in std::fs::read_dir(dir).unwrap() {
            let p = e.unwrap().path();
            let ext = p
                .extension()
                .and_then(|x| x.to_str())
                .unwrap_or("")
                .to_ascii_lowercase();
            if ext == "stp" || ext == "step" {
                files.push(p);
            }
        }
    }
    files.sort();
    let mut failures = Vec::new();
    let mut drilled = 0;
    for path in &files {
        let bytes = std::fs::read(path).unwrap();
        let heal = |tighten: bool| {
            read(
                &bytes,
                &ReadOptions {
                    key: OpKey(7),
                    heal: HealOptions {
                        tighten,
                        ..HealOptions::default()
                    },
                },
            )
            .map(|m| m.bodies)
        };
        let (Ok(tight), Ok(loose)) = (heal(true), heal(false)) else {
            continue;
        };
        for (k, (a, b)) in tight.iter().zip(&loose).enumerate() {
            let (Ok((a, _, _)), Ok((b, _, _))) = (&a.result, &b.result) else {
                continue;
            };
            let what = format!("{} body {k}", path.display());
            let bx = a
                .faces()
                .map(|f| ryukei::intersect::face_box(a, f))
                .reduce(|x, y| x.union(&y))
                .unwrap();
            let size = (bx.max() - bx.min()).length();
            let mid = bx.min() + (bx.max() - bx.min()) * 0.5;
            let h = bx.max().z - bx.min().z + size;
            let place = Rigid::translation(Vector3::new(mid.x, mid.y, bx.min().z - size / 2.0));
            let rod = primitive::cylinder(OpKey(40), size / 40.0, h, &place)
                .unwrap()
                .0;
            let tools = [Tool {
                key: ToolKey(1),
                body: &rod,
                place: Rigid::IDENTITY,
            }];
            if boolean(OpKey(41), BooleanOp::Subtract, a, &tools).is_err() {
                continue;
            }
            let mesh_volume = |x: &Body| {
                tessellate(x, &MeshParams::new(size * 2e-4, 0.3))
                    .ok()
                    .filter(|m| m.faults.is_empty())
                    .map(|m| volume_and_area(&m).0)
            };
            let cut = boolean(OpKey(41), BooleanOp::Subtract, b, &tools);
            let kept = boolean(OpKey(42), BooleanOp::Intersect, b, &tools);
            match (cut, kept) {
                (Ok((c, _)), Ok((t, _))) => {
                    drilled += 1;
                    if !check(&c).is_empty() || !check(&t).is_empty() {
                        failures.push(format!("{what}: {:?} {:?}", check(&c), check(&t)));
                        continue;
                    }
                    match (mesh_volume(b), mesh_volume(&c), mesh_volume(&t)) {
                        (Some(w), Some(l), Some(r)) if (l + r - w).abs() <= 1e-3 * w => {}
                        other => failures.push(format!("{what}: volumes {other:?}")),
                    }
                }
                (c, t) => failures.push(format!("{what}: {:?} {:?}", c.err(), t.err())),
            }
        }
    }
    println!("{drilled} bodies drilled untightened");
    assert!(drilled > 0);
    assert!(failures.is_empty(), "{failures:#?}");
}

/// Every body the committed corpus and the OCCT files heal, healed again
/// with tightening off (design 299, plan 14.B), filleted as
/// `ryukei-bench`'s pinned bodies are: the longest straight edge between
/// two planes with its tangent chain, at a hundredth of the body's size,
/// halved down to a 1600th where the faces round it can't take it, the
/// next longest where none will do (five at most). Wherever the tightened
/// body fillets, the untightened one must fillet the same edge at the
/// same radius, check clean, and enclose the tightened result's volume
/// within a thousandth. Release only.
#[test]
#[cfg_attr(
    debug_assertions,
    ignore = "minutes in a debug build: CI runs it in release"
)]
fn bodies_fillet_untightened_wherever_they_fillet_tightened() {
    use ryukei::BlendError;
    use ryukei::blend::{BlendKind, EdgeBlend, blend, tangent_chain};
    use ryukei::geom::Curve;
    use ryukei::topo::EdgeId;
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("tests");
    let mut files: Vec<std::path::PathBuf> = Vec::new();
    for dir in [root.join("corpus"), root.join("data/occt")] {
        for e in std::fs::read_dir(dir).unwrap() {
            let p = e.unwrap().path();
            let ext = p
                .extension()
                .and_then(|x| x.to_str())
                .unwrap_or("")
                .to_ascii_lowercase();
            if ext == "stp" || ext == "step" {
                files.push(p);
            }
        }
    }
    files.sort();
    let mid = |b: &Body, e: EdgeId| {
        let x = b.edge(e).unwrap();
        b.curve(x.curve)
            .unwrap()
            .point(0.5 * (x.interval[0] + x.interval[1]))
    };
    // The chain through `edge` filleted at `radius`.
    let fillet = |b: &Body, edge: EdgeId, radius: f64| {
        let chain = tangent_chain(b, edge).unwrap();
        let blends: Vec<EdgeBlend> = chain
            .iter()
            .map(|&e| EdgeBlend {
                edge: b.edge(e).unwrap().name,
                kind: BlendKind::Fillet { radius },
            })
            .collect();
        blend(OpKey(43), b, &blends)
    };
    let mut failures = Vec::new();
    let mut filleted = 0;
    for path in &files {
        let bytes = std::fs::read(path).unwrap();
        let heal = |tighten: bool| {
            read(
                &bytes,
                &ReadOptions {
                    key: OpKey(7),
                    heal: HealOptions {
                        tighten,
                        ..HealOptions::default()
                    },
                },
            )
            .map(|m| m.bodies)
        };
        let (Ok(tight), Ok(loose)) = (heal(true), heal(false)) else {
            continue;
        };
        for (k, (a, b)) in tight.iter().zip(&loose).enumerate() {
            let (Ok((a, _, _)), Ok((b, _, _))) = (&a.result, &b.result) else {
                continue;
            };
            let what = format!("{} body {k}", path.display());
            let bx = a
                .faces()
                .map(|f| ryukei::intersect::face_box(a, f))
                .reduce(|x, y| x.union(&y))
                .unwrap();
            let size = (bx.max() - bx.min()).length();
            let planar = |f| {
                matches!(
                    a.surface(a.face(f).unwrap().surface),
                    Some(Surface::Plane(_))
                )
            };
            let len = |e: EdgeId| {
                let x = a.edge(e).unwrap();
                x.interval[1] - x.interval[0]
            };
            let mut edges: Vec<EdgeId> = a
                .edges()
                .filter(|&e| {
                    let x = a.edge(e).unwrap();
                    matches!(a.curve(x.curve), Some(Curve::Line(_)))
                        && a.edge_fins(e).all(|fin| {
                            let lp = a.fin(fin).unwrap().loop_;
                            planar(a.loop_(lp).unwrap().face)
                        })
                })
                .collect();
            edges.sort_by(|&x, &y| len(y).total_cmp(&len(x)));
            // The tightened body's fillet: the edge and the radius it took.
            let found = edges.iter().take(5).find_map(|&e| {
                let mut radius = size / 100.0;
                loop {
                    match fillet(a, e, radius) {
                        Ok((r, _)) => return Some((e, radius, r)),
                        Err(ryukei::Error::InvalidBlend(
                            BlendError::Overflow { .. } | BlendError::RadiusTooLarge { .. },
                        )) if radius > size / 1600.0 => radius /= 2.0,
                        Err(_) => return None,
                    }
                }
            });
            let Some((e, radius, rounded)) = found else {
                continue;
            };
            // The same edge in the untightened body: the one whose middle
            // is nearest, within the tolerances.
            let m = mid(a, e);
            let twin = b
                .edges()
                .min_by(|&x, &y| mid(b, x).distance(m).total_cmp(&mid(b, y).distance(m)))
                .unwrap();
            filleted += 1;
            let r = match fillet(b, twin, radius) {
                Ok((r, _)) => r,
                Err(err) => {
                    failures.push(format!("{what}: at {radius} m: {err:?}"));
                    continue;
                }
            };
            if !check(&r).is_empty() {
                failures.push(format!("{what}: {:?}", check(&r)));
                continue;
            }
            let mesh_volume = |x: &Body| {
                tessellate(x, &MeshParams::new(size * 1e-3, 0.3))
                    .ok()
                    .filter(|m| m.faults.is_empty())
                    .map(|m| volume_and_area(&m).0)
            };
            match (mesh_volume(&rounded), mesh_volume(&r)) {
                (Some(want), Some(got)) if (got - want).abs() <= 1e-3 * want => {}
                other => failures.push(format!("{what}: volumes {other:?}")),
            }
        }
    }
    println!("{filleted} bodies filleted untightened");
    assert!(filleted > 0);
    assert!(failures.is_empty(), "{failures:#?}");
}

/// A body of the committed corpus, healed as an import heals it, with the
/// import's map from names to STEP instances.
fn corpus_body(file: &str) -> (Body, std::collections::BTreeMap<ryukei::name::Name, u64>) {
    let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/corpus");
    let bytes = std::fs::read(dir.join(file)).unwrap();
    let m = read(
        &bytes,
        &ReadOptions {
            key: OpKey(7),
            heal: Default::default(),
        },
    )
    .unwrap();
    let b = m.bodies.into_iter().find(|b| b.result.is_ok()).unwrap();
    (b.result.unwrap().0, b.instances)
}

/// Issue #265: NIST CTC 04 writes its arcs and the lines running on from
/// them to twelve digits, so they meet 3e-11 to 1.2e-10 rad apart, past
/// `ANGULAR_RESOLUTION`: the tangent chain stopped at each, and the end
/// face there (a plane running on tangentially from the arc's cylinder)
/// was one the rail never crosses, so every fillet overflowed. A tangent
/// is now judged within the turn a move of the resolution gives it, and
/// an edge's faces' normals within their curvatures times the resolution:
/// the longest straight edge's chain of twelve fillets at a 400th of the
/// part (the bench's radius after halving twice), checks clean, and takes
/// away no more than its straight fillets' cross-section along the chain.
#[test]
#[cfg_attr(
    debug_assertions,
    ignore = "slow in a debug build: CI runs it in release"
)]
fn nist_ctc_04s_longest_edges_fillet() {
    use ryukei::blend::{BlendKind, EdgeBlend, blend, tangent_chain};
    use ryukei::geom::Curve;
    use ryukei::measure::{Entity, Properties, properties};
    let (body, _) = corpus_body("nist_ctc_04_asme1_rd.stp");
    let bx = body
        .faces()
        .map(|f| ryukei::intersect::face_box(&body, f))
        .reduce(|x, y| x.union(&y))
        .unwrap();
    let size = (bx.max() - bx.min()).length();
    let planar = |f| {
        matches!(
            body.surface(body.face(f).unwrap().surface),
            Some(Surface::Plane(_))
        )
    };
    let len = |e| {
        let x = body.edge(e).unwrap();
        x.interval[1] - x.interval[0]
    };
    let mut edges: Vec<_> = body
        .edges()
        .filter(|&e| {
            let x = body.edge(e).unwrap();
            matches!(body.curve(x.curve), Some(Curve::Line(_)))
                && body.edge_fins(e).all(|fin| {
                    let lp = body.fin(fin).unwrap().loop_;
                    planar(body.loop_(lp).unwrap().face)
                })
        })
        .collect();
    edges.sort_by(|&a, &b| len(b).total_cmp(&len(a)));
    let chain = tangent_chain(&body, edges[0]).unwrap();
    assert_eq!(chain.len(), 12);
    let radius = size / 400.0;
    let blends: Vec<EdgeBlend> = chain
        .iter()
        .map(|&e| EdgeBlend {
            edge: body.edge(e).unwrap().name,
            kind: BlendKind::Fillet { radius },
        })
        .collect();
    let (rounded, _) = blend(OpKey(43), &body, &blends).unwrap();
    assert_eq!(check(&rounded), []);
    assert_eq!(check_intersections(&rounded), []);
    let volume = |b: &Body| {
        let Properties::Body { volume, .. } = properties(b, Entity::Body).unwrap() else {
            unreachable!()
        };
        (volume.value(), volume.bound())
    };
    let ((v, vb), (r, rb)) = (volume(&body), volume(&rounded));
    // Each fillet takes the spandrel `r² (1 - π/4)` round its edge: along
    // a line its length times that, round an arc (on the top or bottom
    // plane, its other face a cylinder) by Pappus, its angle times that
    // times the radius the spandrel's centroid turns at, `g` from the
    // edge into the material: inwards of a boss, outwards of a pocket.
    let spandrel = radius * radius * (1.0 - PI / 4.0);
    let g = radius * (10.0 - 3.0 * PI) / (3.0 * (4.0 - PI));
    let mut want = 0.0;
    for &e in &chain {
        let x = body.edge(e).unwrap();
        let w = x.interval[1] - x.interval[0];
        let curve = body.curve(x.curve).unwrap();
        match curve {
            Curve::Line(_) => want += w * spandrel,
            Curve::Circle(c) => {
                let mid = curve.point(0.5 * (x.interval[0] + x.interval[1]));
                let side = body
                    .edge_fins(e)
                    .map(|fin| {
                        body.face(body.loop_(body.fin(fin).unwrap().loop_).unwrap().face)
                            .unwrap()
                    })
                    .find(|f| matches!(body.surface(f.surface), Some(Surface::Cylinder(_))))
                    .expect("a cylinder beside an arc");
                let s = body.surface(side.surface).unwrap();
                let (u, v) = s.project(mid).param;
                let mut n = s.normal(u, v).unwrap().get();
                if side.sense == ryukei::topo::Sense::Reversed {
                    n = -n;
                }
                // Outward from the axis: the material is inside (a boss).
                let boss = n.dot(mid - c.frame().origin()) > 0.0;
                let rho = if boss { c.radius() - g } else { c.radius() + g };
                want += w * spandrel * rho;
            }
            other => panic!("a chain edge on {other:?}"),
        }
    }
    let taken = v - r;
    println!("{taken} m³ taken, {want} by Pappus (bounds {vb}, {rb})");
    assert!(
        (taken - want).abs() <= 1e-6 * want,
        "{taken} taken against {want}"
    );
}

/// Issue #379: NIST FTC 07's edge #1878, a circle between two spline
/// faces, was refused `Uncertain`: its faces turn tangent at both ends
/// and the convexity certificate couldn't sign the stretches there. The
/// faces' normals are within 6.5e-6 rad all along it, less than their
/// curvatures (about 330 per meter) times twice the resolution: within
/// the resolution they are tangent (two patches of one fillet surface),
/// so a chamfer there is refused as `SmoothEdge`, which says why.
#[test]
fn nist_ftc_07s_spline_seam_is_smooth() {
    use ryukei::blend::{BlendKind, Chamfer, EdgeBlend, blend};
    let (body, instances) = corpus_body("nist_ftc_07_asme1_rd.stp");
    let name = *instances
        .iter()
        .find(|&(_, &id)| id == 1878)
        .expect("edge #1878")
        .0;
    let blends = [EdgeBlend {
        edge: name,
        kind: BlendKind::Chamfer(Chamfer::Equal { distance: 5e-4 }),
    }];
    let err = blend(OpKey(43), &body, &blends).unwrap_err();
    assert_eq!(
        err,
        ryukei::Error::InvalidBlend(ryukei::BlendError::SmoothEdge { edge: name })
    );
}

/// Issue #378: NIST FTC 07's edge #1869, filleted with its tangent chain,
/// reaches a vertex where two tangent blended edges meet a third, a circle
/// between a cylinder and a spline face whose dihedral is 5e-7 rad at its
/// ends and 1.2e-4 rad between: past what the resolution explains, so it
/// isn't smooth, and a smooth join needs it smooth all along. The vertex
/// is one no blend builds (a blend ending where its faces turn tangent is
/// still to do), so the fillet is refused `Unsupported` there, naming it,
/// not `Uncertain`.
#[test]
fn nist_ftc_07s_creased_join_is_unsupported() {
    use ryukei::blend::{BlendKind, EdgeBlend, blend, tangent_chain};
    let (body, instances) = corpus_body("nist_ftc_07_asme1_rd.stp");
    let name = *instances
        .iter()
        .find(|&(_, &id)| id == 1869)
        .expect("edge #1869")
        .0;
    let edge = body
        .edges()
        .find(|&e| body.edge(e).unwrap().name == name)
        .expect("the edge");
    let blends: Vec<EdgeBlend> = tangent_chain(&body, edge)
        .expect("a chain")
        .into_iter()
        .map(|e| EdgeBlend {
            edge: body.edge(e).unwrap().name,
            kind: BlendKind::Fillet { radius: 5e-4 },
        })
        .collect();
    let err = blend(OpKey(44), &body, &blends).unwrap_err();
    assert!(
        matches!(
            err,
            ryukei::Error::InvalidBlend(ryukei::BlendError::Unsupported { .. })
        ),
        "{err:?}"
    );
}
