//! Every long entry point under a work budget and a cancel (plan 15.A):
//! it charges, the same units on one thread and eight, a budget of what it
//! spends is enough, one unit less is `BudgetExceeded` (judged alike on
//! every thread count), and a cancel set before it starts is `Cancelled`.

use ryukei::Error;
use ryukei::boolean::{BooleanOp, Tool, ToolKey, boolean};
use ryukei::geom::{Rigid, Vector3};
use ryukei::name::OpKey;
use ryukei::primitive;
use ryukei::topo::Body;
use ryukei::work::Work;
use std::sync::atomic::AtomicBool;

fn pool(threads: usize) -> rayon::ThreadPool {
    rayon::ThreadPoolBuilder::new()
        .num_threads(threads)
        .build()
        .unwrap()
}

/// What `call` spends, after checking the rules above.
fn bounded<T>(name: &str, call: impl Fn(&Work) -> Result<T, Error> + Sync) -> u64
where
    T: Send + std::fmt::Debug,
{
    let free = Work::unlimited();
    if let Err(e) = call(&free) {
        panic!("{name}: {e:?}");
    }
    let spent = free.spent();
    assert!(spent > 0, "{name} charged nothing");
    let mut over = Vec::new();
    for threads in [1, 8] {
        let pool = pool(threads);
        let w = Work::unlimited();
        pool.install(|| call(&w)).unwrap();
        assert_eq!(w.spent(), spent, "{name} on {threads} threads");
        let w = Work::unlimited().with_budget(spent);
        assert!(
            pool.install(|| call(&w)).is_ok(),
            "{name}: a budget of what it spends"
        );
        let w = Work::unlimited().with_budget(spent - 1);
        match pool.install(|| call(&w)) {
            Err(Error::BudgetExceeded { spent: s, budget }) => {
                assert_eq!(budget, spent - 1);
                assert!(s > budget);
                over.push(s);
            }
            other => panic!("{name} on {threads} threads, one unit short: {other:?}"),
        }
    }
    assert_eq!(
        over[0], over[1],
        "{name}: where it stops depends on threads"
    );
    let flag = AtomicBool::new(true);
    let w = Work::unlimited().with_cancel(&flag);
    assert!(
        matches!(call(&w), Err(Error::Cancelled)),
        "{name}: cancelled"
    );
    spent
}

fn at(x: f64, y: f64, z: f64) -> Rigid {
    Rigid::translation(Vector3::new(x, y, z))
}

/// A 10 mm cube with a 3 mm rod through it: a boolean of a few traces.
fn drilled() -> Body {
    let (cube, _) = primitive::block(OpKey(1), 0.01, 0.01, 0.01, &Rigid::IDENTITY).unwrap();
    let (rod, _) = primitive::cylinder(OpKey(2), 0.0015, 0.02, &at(0.005, 0.005, -0.005)).unwrap();
    let tool = Tool {
        key: ToolKey(1),
        body: &rod,
        place: Rigid::IDENTITY,
    };
    boolean(OpKey(3), BooleanOp::Subtract, &cube, &[tool])
        .unwrap()
        .0
}

#[test]
fn the_check_is_bounded() {
    let body = drilled();
    let spent = bounded("check_intersections", |w| {
        ryukei::check::check_intersections_with(&body, w)
    });
    let w = Work::unlimited();
    assert_eq!(
        ryukei::check::check_intersections_with(&body, &w).unwrap(),
        ryukei::check::check_intersections(&body)
    );
    eprintln!("check_intersections: {spent} units");
}

#[test]
fn a_boolean_is_bounded() {
    let (cube, _) = primitive::block(OpKey(1), 0.01, 0.01, 0.01, &Rigid::IDENTITY).unwrap();
    let (rod, _) = primitive::cylinder(OpKey(2), 0.0015, 0.02, &at(0.005, 0.005, -0.005)).unwrap();
    let (ball, _) = primitive::sphere(OpKey(4), 0.003, &at(0.01, 0.01, 0.01)).unwrap();
    let tools = [
        Tool {
            key: ToolKey(1),
            body: &rod,
            place: Rigid::IDENTITY,
        },
        Tool {
            key: ToolKey(2),
            body: &ball,
            place: Rigid::IDENTITY,
        },
    ];
    let plain = boolean(OpKey(3), BooleanOp::Subtract, &cube, &tools)
        .unwrap()
        .0;
    let spent = bounded("boolean", |w| {
        ryukei::boolean::boolean_with(OpKey(3), BooleanOp::Subtract, &cube, &tools, w)
    });
    let w = Work::unlimited();
    let (with, _) =
        ryukei::boolean::boolean_with(OpKey(3), BooleanOp::Subtract, &cube, &tools, &w).unwrap();
    assert_eq!(with.dump(), plain.dump());
    eprintln!("boolean: {spent} units");
}

#[test]
fn a_mesh_is_bounded() {
    use ryukei::mesh::{MeshParams, tessellate, tessellate_with};
    let body = drilled();
    let params = MeshParams::new(1e-5, 0.3);
    let plain = tessellate(&body, &params).unwrap();
    let spent = bounded("tessellate", |w| tessellate_with(&body, &params, w));
    let w = Work::unlimited();
    let with = tessellate_with(&body, &params, &w).unwrap();
    assert_eq!(with.positions, plain.positions);
    eprintln!("tessellate: {spent} units");
}

#[test]
fn healing_is_bounded() {
    use ryukei::heal::{HealOptions, heal, heal_with, seamed};
    let body = drilled();
    let shape = seamed(&body, 1e-7).unwrap();
    let plain = heal(OpKey(7), &shape, &HealOptions::default()).unwrap().0;
    let spent = bounded("heal", |w| {
        heal_with(OpKey(7), &shape, &HealOptions::default(), w)
    });
    let w = Work::unlimited();
    let with = heal_with(OpKey(7), &shape, &HealOptions::default(), &w)
        .unwrap()
        .0;
    assert_eq!(with.dump(), plain.dump());
    eprintln!("heal: {spent} units");
}

#[test]
fn a_reference_is_bounded() {
    use ryukei::heal::{ReferenceOptions, reference, reference_with, seamed};
    let body = drilled();
    let shape = seamed(&body, 1e-7).unwrap();
    let opts = ReferenceOptions::default();
    let plain = reference(OpKey(7), &shape, &opts).unwrap().0;
    let spent = bounded("reference", |w| reference_with(OpKey(7), &shape, &opts, w));
    let w = Work::unlimited();
    let with = reference_with(OpKey(7), &shape, &opts, &w).unwrap().0;
    assert_eq!(with.dump(), plain.dump());
    eprintln!("reference: {spent} units");
}

#[test]
fn a_blend_is_bounded() {
    use ryukei::blend::{BlendKind, EdgeBlend, blend, blend_with};
    let (cube, _) = primitive::block(OpKey(1), 0.01, 0.01, 0.01, &Rigid::IDENTITY).unwrap();
    let edge = cube.edges().next().unwrap();
    let blends = [EdgeBlend {
        edge: cube.edge(edge).unwrap().name,
        kind: BlendKind::Fillet { radius: 0.002 },
    }];
    let plain = blend(OpKey(5), &cube, &blends).unwrap().0;
    let spent = bounded("blend", |w| blend_with(OpKey(5), &cube, &blends, w));
    let w = Work::unlimited();
    assert_eq!(
        blend_with(OpKey(5), &cube, &blends, &w).unwrap().0.dump(),
        plain.dump()
    );
    eprintln!("blend: {spent} units");
}

#[test]
fn offsets_and_shells_are_bounded() {
    use ryukei::offset::{OffsetSpec, ShellSpec, offset_body_with, shell_with};
    let (cube, _) = primitive::block(OpKey(1), 0.01, 0.01, 0.01, &Rigid::IDENTITY).unwrap();
    let grown = OffsetSpec {
        distance: 0.001,
        faces: vec![],
    };
    let spent = bounded("offset_body", |w| {
        offset_body_with(OpKey(8), &cube, &grown, w)
    });
    eprintln!("offset_body: {spent} units");
    let hollow = ShellSpec {
        thickness: 0.001,
        outward: false,
        open: vec![],
        faces: vec![],
    };
    let spent = bounded("shell", |w| shell_with(OpKey(9), &cube, &hollow, w));
    eprintln!("shell: {spent} units");
}

#[test]
fn tweaks_are_bounded() {
    use ryukei::name::{FaceRole, Name, Role};
    use ryukei::tweak::{
        MoveSpec, OffsetFacesSpec, ReplaceSpec, move_faces_with, offset_faces_with,
        replace_faces_with,
    };
    let (cube, _) = primitive::block(OpKey(1), 0.01, 0.01, 0.01, &Rigid::IDENTITY).unwrap();
    let top = Name::derive(OpKey(1), Role::Face(FaceRole::PlusZ), &[]);
    let side = Name::derive(OpKey(1), Role::Face(FaceRole::PlusX), &[]);
    let up = MoveSpec {
        faces: vec![top],
        motion: Rigid::translation(ryukei::geom::Vector3::new(0.0, 0.0, 0.002)),
        tangent: false,
    };
    let spent = bounded("move_faces", |w| move_faces_with(OpKey(8), &cube, &up, w));
    eprintln!("move_faces: {spent} units");
    let out = OffsetFacesSpec {
        faces: vec![(top, 0.001), (side, 0.002)],
        tangent: false,
    };
    let spent = bounded("offset_faces", |w| {
        offset_faces_with(OpKey(8), &cube, &out, w)
    });
    eprintln!("offset_faces: {spent} units");
    let plane = ryukei::geom::Surface::Plane(ryukei::geom::Plane::new(
        ryukei::geom::Frame::new(
            ryukei::geom::Point3::new(0.0, 0.0, 0.012),
            ryukei::geom::Vector3::new(0.0, 0.0, 1.0),
            ryukei::geom::Vector3::new(1.0, 0.0, 0.0),
        )
        .unwrap(),
    ));
    let flush = ReplaceSpec {
        faces: vec![(top, plane)],
    };
    let spent = bounded("replace_faces", |w| {
        replace_faces_with(OpKey(8), &cube, &flush, w)
    });
    eprintln!("replace_faces: {spent} units");
}

#[test]
fn thickening_a_sheet_is_bounded() {
    use ryukei::name::{FaceRole, Name, Role};
    use ryukei::offset::{ThickenSpec, thicken_with};
    let (cube, _) = primitive::block(OpKey(1), 0.01, 0.01, 0.01, &Rigid::IDENTITY).unwrap();
    let top = Name::derive(OpKey(1), Role::Face(FaceRole::PlusZ), &[]);
    let (open, _) = ryukei::sheet::remove(OpKey(2), &cube, &[top]).unwrap();
    let spec = ThickenSpec {
        front: 0.001,
        back: 0.0005,
        faces: vec![],
    };
    let spent = bounded("thicken", |w| thicken_with(OpKey(10), &open, &spec, w));
    eprintln!("thicken: {spent} units");
}

#[test]
fn blending_a_sheet_is_bounded() {
    use ryukei::blend::{BlendKind, EdgeBlend, blend_with};
    use ryukei::name::{FaceRole, Name, Role};
    let (cube, _) = primitive::block(OpKey(1), 0.01, 0.01, 0.01, &Rigid::IDENTITY).unwrap();
    let face = |r| Name::derive(OpKey(1), Role::Face(r), &[]);
    let (open, _) = ryukei::sheet::remove(OpKey(2), &cube, &[face(FaceRole::PlusZ)]).unwrap();
    // An upright edge, ending in a ball corner and at the open rim.
    let mut faces = [face(FaceRole::PlusX), face(FaceRole::PlusY)];
    faces.sort();
    let blends: Vec<EdgeBlend> = [
        Name::derive(OpKey(1), Role::Edge, &faces),
        {
            let mut f = [face(FaceRole::MinusZ), face(FaceRole::PlusX)];
            f.sort();
            Name::derive(OpKey(1), Role::Edge, &f)
        },
        {
            let mut f = [face(FaceRole::MinusZ), face(FaceRole::PlusY)];
            f.sort();
            Name::derive(OpKey(1), Role::Edge, &f)
        },
    ]
    .map(|edge| EdgeBlend {
        edge,
        kind: BlendKind::Fillet { radius: 0.001 },
    })
    .to_vec();
    let spent = bounded("blend a sheet", |w| {
        blend_with(OpKey(10), &open, &blends, w)
    });
    eprintln!("blend a sheet: {spent} units");
}

#[test]
fn filling_is_bounded() {
    use ryukei::fill::{FillBoundary, FillOptions, FillSide, fill_with};
    use ryukei::geom::{Curve, Cylinder, Frame, KnotVector, NurbsCurve, Point3, Surface};
    use ryukei::name::{Name, Role, SheetRole};
    use std::sync::Arc;
    // A quad tangent to a quarter cylinder sheet's side: the correction is
    // fitted and every side's angle certified.
    let cyl = Surface::Cylinder(Cylinder::new(Frame::WORLD, 1.0).unwrap());
    let (sheet, _) = ryukei::sheet::from_surface(
        OpKey(1),
        &cyl,
        [[0.0, core::f64::consts::FRAC_PI_2], [0.0, 1.0]],
    )
    .unwrap();
    let face = Name::derive(OpKey(1), Role::Sheet(SheetRole::Face), &[]);
    let side = Name::derive(OpKey(1), Role::Sheet(SheetRole::SideU1), &[face]);
    let p = Point3::new;
    let bezier = |pts: &[Point3]| {
        let k = KnotVector::clamped(2, &[0.0, 1.0], &[3, 3]).unwrap();
        FillSide::Curve {
            curve: Curve::Nurbs(Arc::new(NurbsCurve::new(k, pts.to_vec(), None).unwrap())),
            range: [0.0, 1.0],
        }
    };
    let boundary = FillBoundary {
        sides: vec![
            FillSide::Edge {
                body: &sheet,
                edge: side,
                tangent: true,
            },
            bezier(&[p(0.0, 1.0, 1.0), p(-0.5, 1.0, 1.0), p(-1.0, 0.5, 1.0)]),
            bezier(&[p(-1.0, 0.5, 1.0), p(-1.2, 0.1, 0.5), p(-1.0, 0.5, 0.0)]),
            bezier(&[p(-1.0, 0.5, 0.0), p(-0.5, 1.0, 0.0), p(0.0, 1.0, 0.0)]),
        ],
    };
    let options = FillOptions {
        tolerance: 1e-7,
        angle: 0.01,
    };
    let spent = bounded("fill", |w| fill_with(OpKey(10), &boundary, &options, w));
    eprintln!("fill: {spent} units");
}

/// A square of side `s` in `frame`, its loop keyed `lp` and its segments
/// from `first`, as `tests/along.rs` and `tests/loft.rs` make them.
fn square_in(
    frame: ryukei::geom::Frame,
    lp: u128,
    first: u128,
    s: f64,
) -> ryukei::sweep::ValidProfile {
    use ryukei::geom::{Curve2, Line2, Point2};
    use ryukei::sweep::{Loop, LoopKey, Profile, Segment, SegmentKey, ValidProfile};
    let h = s / 2.0;
    let pts = [(-h, -h), (h, -h), (h, h), (-h, h)];
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
        frame,
        loops: vec![Loop {
            key: LoopKey(lp),
            segments,
        }],
    })
    .unwrap()
}

#[test]
fn sweeps_along_paths_and_lofts_are_bounded() {
    use core::f64::consts::FRAC_PI_2;
    use ryukei::geom::{Circle, Curve, Frame, Line, Point3};
    use ryukei::sweep::{AlongOptions, LoftOptions, Path, PathSegment, SegmentKey};
    let part = |key: u128, curve: Curve, range: [f64; 2]| PathSegment {
        key: SegmentKey(key),
        curve,
        range,
    };
    let line = |from: Point3, dir: Vector3| Curve::Line(Line::new(from, dir).unwrap());
    // Along x by 2, a quarter turn of radius 1 towards y, along y by 2:
    // `tests/along.rs`'s bent path.
    let f = Frame::new(
        Point3::new(2.0, 1.0, 0.0),
        Vector3::new(0.0, 0.0, 1.0),
        Vector3::new(1.0, 0.0, 0.0),
    )
    .unwrap();
    let path = Path {
        segments: vec![
            part(
                1,
                line(Point3::ORIGIN, Vector3::new(1.0, 0.0, 0.0)),
                [0.0, 2.0],
            ),
            part(
                2,
                Curve::Circle(Circle::new(f, 1.0).unwrap()),
                [-FRAC_PI_2, 0.0],
            ),
            part(
                3,
                line(Point3::new(3.0, 1.0, 0.0), Vector3::new(0.0, 1.0, 0.0)),
                [0.0, 2.0],
            ),
        ],
    };
    let across = Frame::new(
        Point3::ORIGIN,
        Vector3::new(1.0, 0.0, 0.0),
        Vector3::new(0.0, 1.0, 0.0),
    )
    .unwrap();
    let profile = square_in(across, 1, 10, 0.4);
    let opts = AlongOptions::default();
    let spent = bounded("along", |w| {
        ryukei::sweep::along_with(OpKey(10), &profile, &path, &opts, w)
    });
    eprintln!("along: {spent} units");
    // A frustum: `tests/loft.rs`'s first loft, which checks itself.
    let at = |h: f64| {
        Frame::new(
            Point3::new(0.0, 0.0, h),
            Vector3::new(0.0, 0.0, 1.0),
            Vector3::new(1.0, 0.0, 0.0),
        )
        .unwrap()
    };
    let sections = [
        square_in(at(0.0), 1, 10, 2.0),
        square_in(at(1.0), 1, 20, 1.0),
    ];
    let opts = LoftOptions::default();
    let spent = bounded("loft", |w| {
        ryukei::sweep::loft_with(OpKey(11), &sections, &opts, w)
    });
    eprintln!("loft: {spent} units");
}

#[test]
fn mass_and_distance_are_bounded() {
    use ryukei::measure::{DistanceOptions, Entity, MassOptions, distance_with, mass_with};
    let (cube, _) = primitive::block(OpKey(1), 0.01, 0.01, 0.01, &Rigid::IDENTITY).unwrap();
    let (ball, _) = primitive::sphere(OpKey(4), 0.003, &at(0.03, 0.0, 0.0)).unwrap();
    let spent = bounded("mass", |w| {
        mass_with(&ball, 7850.0, &MassOptions::default(), w)
    });
    eprintln!("mass: {spent} units");
    let spent = bounded("distance", |w| {
        distance_with(
            (&cube, Entity::Body, &Rigid::IDENTITY),
            (&ball, Entity::Body, &Rigid::IDENTITY),
            &DistanceOptions::default(),
            w,
        )
    });
    eprintln!("distance: {spent} units");
}

/// Every `Meter::free()` says why it is unmetered, and none is left
/// waiting for a task of plan 15.A.
#[test]
fn every_free_meter_says_why() {
    fn walk(dir: &std::path::Path, out: &mut Vec<std::path::PathBuf>) {
        for e in std::fs::read_dir(dir).unwrap() {
            let p = e.unwrap().path();
            if p.is_dir() {
                walk(&p, out);
            } else if p.extension().is_some_and(|x| x == "rs") {
                out.push(p);
            }
        }
    }
    let mut files = Vec::new();
    walk(
        &std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("src"),
        &mut files,
    );
    let mut bad = Vec::new();
    for f in files {
        let text = std::fs::read_to_string(&f).unwrap();
        let lines: Vec<&str> = text.lines().collect();
        for (i, l) in lines.iter().enumerate() {
            if l.contains("until Task") {
                bad.push(format!("{}:{}: {l}", f.display(), i + 1));
            }
            if l.contains("Meter::free()") && !l.contains("fn free()") {
                let said = l.contains("// unmetered:")
                    || (i > 0 && lines[i - 1].contains("// unmetered:"));
                if !said {
                    bad.push(format!("{}:{}: {l}", f.display(), i + 1));
                }
            }
        }
    }
    assert!(bad.is_empty(), "{}", bad.join("\n"));
}
#[test]
fn silhouettes_are_bounded() {
    use ryukei::project::{View, silhouettes_with};
    let (torus, _) = primitive::torus(OpKey(12), 0.03, 0.01, &Rigid::IDENTITY).unwrap();
    let v = View::new(Vector3::new(1.0, 0.2, 1.0), Vector3::new(0.0, 0.0, 1.0)).unwrap();
    let spent = bounded("silhouettes", |w| silhouettes_with(&torus, &v, w));
    eprintln!("silhouettes: {spent} units");
}

/// A unit costs about the same time whichever surfaces it encloses
/// (#353): the drilled block's check, its NURBS twin's, and #260's first
/// sweep find (sides on sweep surfaces, built and checked) spend units
/// within a factor of 8 of each other's rates. Unweighted, the sweep's
/// units were each about 18 times the block's (cox-worker-2: 103,000
/// against 1,900,000 a second). A ratio of rates, but still of times, so
/// it is asserted only with `RYUKEI_BARS=1`, on a quiet machine; the
/// rates and ratios are always printed (`full.yml` runs it in release).
#[test]
#[cfg_attr(debug_assertions, ignore = "release only")]
fn units_follow_time_across_surfaces() {
    let analytic = drilled();
    let mut spline = analytic.clone();
    ryukei_testkit::nurbs::convert_body(&mut spline).unwrap();
    let timed = |f: &(dyn Fn(&Work) + Sync)| {
        pool(1).install(|| {
            let w = Work::unlimited();
            let t = std::time::Instant::now();
            f(&w);
            w.spent() as f64 / t.elapsed().as_secs_f64()
        })
    };
    let check_analytic = |w: &Work| {
        ryukei::check::check_intersections_with(&analytic, w).unwrap();
    };
    let check_spline = |w: &Work| {
        ryukei::check::check_intersections_with(&spline, w).unwrap();
    };
    let hex = concat!(
        "0adfdfdfdfdfdfdfdfdfdfdfdfdfdfdfdf0000000000000008dfdfdfdfdfdfdfdfdfdfdf",
        "dfdfdfdfdfdfdfdfdfdfdfdfdfdfdfdfdfdfdfdfdfdfdfdfdfdfdfdfdfdfdfdfdfdfdfdf",
        "dfdfdfdfdfdfdfdfdfdfdfdfdfdfdfdfdfdfdfdfdfdfdfdfdfdfdfdfdfdfdfdfdfdf",
    );
    let data: Vec<u8> = (0..hex.len())
        .step_by(2)
        .map(|i| u8::from_str_radix(&hex[i..i + 2], 16).unwrap())
        .collect();
    let sweep = |w: &Work| {
        let (made, _, _) = ryukei_testkit::along_bytes::outcome_with(&data, w).expect("a sweep");
        made.expect("builds");
    };
    // The best of three of the quick ones: the least disturbed by the
    // machine; the sweep takes seconds.
    let best = |f: &(dyn Fn(&Work) + Sync)| (0..3).map(|_| timed(f)).fold(0.0, f64::max);
    let ra = best(&check_analytic);
    for (what, r) in [("spline", best(&check_spline)), ("sweep", timed(&sweep))] {
        eprintln!("{what} {r:.0}/s, analytic {ra:.0}/s, ratio {:.2}", ra / r);
        assert!(
            std::env::var("RYUKEI_BARS").as_deref() != Ok("1")
                || (r / ra > 1.0 / 8.0 && r / ra < 8.0),
            "{what} {r:.0}/s, analytic {ra:.0}/s"
        );
    }
}
