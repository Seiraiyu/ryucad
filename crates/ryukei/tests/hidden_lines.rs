//! Hidden lines (design 326, plan 10.B): views of placed bodies against
//! closed forms.

use core::f64::consts::TAU;
use ryukei::blend::{BlendKind, EdgeBlend, blend};
use ryukei::boolean::{BooleanOp, Tool, ToolKey, boolean};
use ryukei::geom::{Curve, Frame, Plane, Point3, Rigid, Surface, Vector3};
use ryukei::name::OpKey;
use ryukei::project::{
    Class, Piece, Placed, Projection, Source, View, ViewOptions, Visibility, view, view_with,
};
use ryukei::topo::Body;
use ryukei::work::Work;
use ryukei::{Error, ViewError, primitive, sheet};
use ryukei_testkit::projection::{Oracle, judge, placed_bodies, size};

fn look(t: Vector3) -> View {
    let up = if t.cross(Vector3::new(0.0, 0.0, 1.0)).length() > 0.1 {
        Vector3::new(0.0, 0.0, 1.0)
    } else {
        Vector3::new(0.0, 1.0, 0.0)
    };
    View::new(t, up).unwrap()
}

fn at(body: &Body) -> Placed<'_> {
    Placed {
        body,
        place: Rigid::IDENTITY,
    }
}

fn sees(placed: &[Placed], v: &View) -> Projection {
    let p = view(placed, v, &ViewOptions::default()).unwrap();
    for q in &p.pieces {
        assert_ne!(q.visibility, Visibility::Uncertain, "{q:?}");
    }
    p
}

/// The 3D curve a piece is a stretch of.
fn curve_of<'a>(p: &'a Projection, bodies: &'a [&Body], q: &Piece) -> &'a Curve {
    match q.source {
        Source::Edge { body, edge, .. } => {
            let b = bodies[body as usize];
            b.curve(b.edge(edge).unwrap().curve).unwrap()
        }
        Source::Silhouette {
            body,
            face,
            ordinal,
            ..
        } => {
            &p.silhouettes[body as usize]
                .branches
                .iter()
                .find(|s| s.face == face && s.ordinal == ordinal)
                .unwrap()
                .curve
        }
    }
}

/// A piece's length in space, certified to a part in 10⁹.
fn length(p: &Projection, bodies: &[&Body], q: &Piece) -> f64 {
    ryukei::measure::curve_length(curve_of(p, bodies, q), q.image.range, 1e-9)
        .unwrap()
        .value()
}

fn hidden(p: &Projection, bodies: &[&Body]) -> f64 {
    p.pieces
        .iter()
        .filter(|q| matches!(q.visibility, Visibility::Hidden(_)))
        .map(|q| length(p, bodies, q))
        .sum()
}

/// Visible, and no piece it lies on is nearer.
fn drawn(p: &Projection, q: &Piece) -> bool {
    let mid = |q: &Piece| (q.depth[0] + q.depth[1]) / 2.0;
    q.visibility == Visibility::Visible
        && q.overlaps
            .iter()
            .all(|&o| mid(&p.pieces[o as usize]) <= mid(q))
}

fn drawn_image_length(p: &Projection) -> f64 {
    p.pieces
        .iter()
        .filter(|q| drawn(p, q))
        .map(|q| {
            q.image
                .point(q.image.range[0])
                .distance(q.image.point(q.image.range[1]))
        })
        .sum()
}

#[test]
fn a_block_from_all_26_directions() {
    let (dx, dy, dz) = (0.01, 0.02, 0.03);
    let (b, _) = primitive::block(OpKey(1), dx, dy, dz, &Rigid::IDENTITY).unwrap();
    for i in -1..=1 {
        for j in -1..=1 {
            for k in -1..=1 {
                let zeros = [i, j, k].iter().filter(|&&c| c == 0).count();
                if zeros == 3 {
                    continue;
                }
                let v = look(Vector3::new(f64::from(i), f64::from(j), f64::from(k)));
                let p = sees(&[at(&b)], &v);
                let h = hidden(&p, &[&b]);
                let want = match zeros {
                    0 => dx + dy + dz,
                    1 => [dx, dy, dz][[i, j, k].iter().position(|&c| c == 0).unwrap()],
                    _ => 0.0,
                };
                assert!((h - want).abs() < 1e-12, "({i}, {j}, {k}): {h} hidden");
                if zeros == 2 {
                    let face = match [i, j, k].iter().position(|&c| c != 0).unwrap() {
                        0 => 2.0 * (dy + dz),
                        1 => 2.0 * (dx + dz),
                        _ => 2.0 * (dx + dy),
                    };
                    let d = drawn_image_length(&p);
                    assert!((d - face).abs() < 1e-12, "({i}, {j}, {k}): {d} drawn");
                }
            }
        }
    }
}

#[test]
fn a_cylinder_side_on_draws_its_rulings_and_rims() {
    let (c, _) = primitive::cylinder(OpKey(2), 0.01, 0.03, &Rigid::IDENTITY).unwrap();
    let p = sees(&[at(&c)], &look(Vector3::new(0.0, -1.0, 0.0)));
    assert!((drawn_image_length(&p) - (2.0 * 0.03 + 2.0 * 0.02)).abs() < 1e-12);
    let rulings: Vec<&Piece> = p
        .pieces
        .iter()
        .filter(|q| q.class == Class::Silhouette)
        .collect();
    assert_eq!(rulings.len(), 2);
    assert!(rulings.iter().all(|q| q.visibility == Visibility::Visible));
}

#[test]
fn a_spheres_silhouette_is_visible_from_every_side() {
    let (s, _) = primitive::sphere(OpKey(3), 0.02, &Rigid::IDENTITY).unwrap();
    for t in [
        Vector3::new(0.0, 0.0, 1.0),
        Vector3::new(1.0, 0.0, 0.0),
        Vector3::new(1.0, -2.0, 3.0),
    ] {
        let p = sees(&[at(&s)], &look(t));
        assert!(p.pieces.iter().all(|q| q.visibility == Visibility::Visible));
        let total: f64 = p.pieces.iter().map(|q| length(&p, &[&s], q)).sum();
        assert!((total - TAU * 0.02).abs() < 1e-5, "{t:?}: {total}");
    }
}

fn drilled() -> Body {
    let (plate, _) = primitive::block(OpKey(4), 0.04, 0.03, 0.01, &Rigid::IDENTITY).unwrap();
    let (pin, _) = primitive::cylinder(OpKey(5), 0.004, 0.02, &Rigid::IDENTITY).unwrap();
    let tool = Tool {
        key: ToolKey(1),
        body: &pin,
        place: Rigid::translation(Vector3::new(0.02, 0.015, -0.005)),
    };
    boolean(OpKey(6), BooleanOp::Subtract, &plate, &[tool])
        .unwrap()
        .0
}

/// The pieces of the circle edges at height `z`.
fn rims(p: &Projection, b: &Body, z: f64) -> Vec<usize> {
    (0..p.pieces.len())
        .filter(|&i| match p.pieces[i].source {
            Source::Edge { edge, .. } => {
                let c = b.curve(b.edge(edge).unwrap().curve).unwrap();
                matches!(c, Curve::Circle(_)) && (c.point(0.0).z - z).abs() < 1e-12
            }
            Source::Silhouette { .. } => false,
        })
        .collect()
}

#[test]
fn a_drilled_plate_hides_its_bottom_rim_and_its_bore_behind_its_front() {
    let d = drilled();
    let iso = sees(&[at(&d)], &look(Vector3::new(1.0, -1.0, 1.0)));
    let bottom = rims(&iso, &d, 0.0);
    let top = rims(&iso, &d, 0.01);
    assert!(!bottom.is_empty() && !top.is_empty());
    assert!(
        bottom
            .iter()
            .all(|&i| matches!(iso.pieces[i].visibility, Visibility::Hidden(_)))
    );
    assert!(
        top.iter()
            .all(|&i| iso.pieces[i].visibility == Visibility::Visible)
    );
    let rim: f64 = top
        .iter()
        .map(|&i| length(&iso, &[&d], &iso.pieces[i]))
        .sum();
    assert!((rim - TAU * 0.004).abs() < 1e-5);
    let side = sees(&[at(&d)], &look(Vector3::new(0.0, -1.0, 0.0)));
    let bore: Vec<&Piece> = side
        .pieces
        .iter()
        .filter(|q| q.class == Class::Silhouette)
        .collect();
    assert!(!bore.is_empty());
    assert!(bore.iter().all(|q| q.visibility == Visibility::Hidden(1)));
    let total: f64 = bore.iter().map(|q| length(&side, &[&d], q)).sum();
    assert!((total - 0.02).abs() < 1e-9, "{total}");
}

#[test]
fn a_fillets_rails_are_drawn_smooth() {
    let (dx, dz) = (0.02, 0.04);
    let (b, _) = primitive::block(OpKey(7), dx, 0.03, dz, &Rigid::IDENTITY).unwrap();
    let on = |p: Point3| (p.x - dx).abs() < 1e-12 && (p.z - dz).abs() < 1e-12;
    let e = b
        .edges()
        .find(|&e| {
            let ed = b.edge(e).unwrap();
            on(b.vertex(ed.start).unwrap().point) && on(b.vertex(ed.end).unwrap().point)
        })
        .unwrap();
    let fillet = EdgeBlend {
        edge: b.edge(e).unwrap().name,
        kind: BlendKind::Fillet { radius: 0.004 },
    };
    let (r, _) = blend(OpKey(8), &b, &[fillet]).unwrap();
    let p = sees(&[at(&r)], &look(Vector3::new(1.0, -1.0, 1.0)));
    let mut smooth = std::collections::BTreeSet::new();
    for q in &p.pieces {
        if let (Source::Edge { edge, .. }, Class::Smooth) = (q.source, q.class) {
            smooth.insert(edge);
        }
    }
    assert_eq!(smooth.len(), 2);
    assert!(
        p.pieces
            .iter()
            .all(|q| matches!(q.class, Class::Sharp | Class::Smooth | Class::Silhouette))
    );
}

#[test]
fn a_block_behind_a_block_is_behind_both_its_faces() {
    let (front, _) = primitive::block(OpKey(9), 0.03, 0.03, 0.03, &Rigid::IDENTITY).unwrap();
    let (back, _) = primitive::block(OpKey(10), 0.01, 0.01, 0.01, &Rigid::IDENTITY).unwrap();
    let placed = [
        at(&front),
        Placed {
            body: &back,
            place: Rigid::translation(Vector3::new(0.01, 0.05, 0.01)),
        },
    ];
    let p = sees(&placed, &look(Vector3::new(0.0, -1.0, 0.0)));
    let behind: Vec<&Piece> = p
        .pieces
        .iter()
        .filter(|q| matches!(q.source, Source::Edge { body: 1, .. }))
        .collect();
    assert_eq!(behind.len(), 12);
    assert!(
        behind.iter().all(|q| q.visibility == Visibility::Hidden(2)),
        "{behind:?}"
    );
    // Nothing of the front block is behind anything.
    assert!(
        p.pieces
            .iter()
            .filter(|q| matches!(q.source, Source::Edge { body: 0, .. }))
            .all(|q| q.visibility == Visibility::Visible)
    );
}

#[test]
fn a_sheet_in_front_hides_a_block_and_draws_its_boundary() {
    let (b, _) = primitive::block(OpKey(11), 0.03, 0.03, 0.03, &Rigid::IDENTITY).unwrap();
    let f = Frame::new(
        Point3::new(-0.01, -0.01, -0.01),
        Vector3::new(0.0, -1.0, 0.0),
        Vector3::new(1.0, 0.0, 0.0),
    )
    .unwrap();
    let plane = Surface::Plane(Plane::new(f));
    let (s, _) = sheet::from_surface(OpKey(12), &plane, [[0.0, 0.05], [0.0, 0.05]]).unwrap();
    let p = sees(&[at(&b), at(&s)], &look(Vector3::new(0.0, -1.0, 0.0)));
    for q in &p.pieces {
        match q.source {
            Source::Edge { body: 0, .. } => {
                assert_eq!(q.visibility, Visibility::Hidden(1), "{q:?}")
            }
            Source::Edge { body: 1, .. } => {
                assert_eq!(q.class, Class::Boundary);
                assert_eq!(q.visibility, Visibility::Visible);
            }
            _ => {}
        }
    }
}

#[test]
fn one_thread_and_eight_draw_alike() {
    let d = drilled();
    let (s, _) = primitive::sphere(OpKey(13), 0.01, &Rigid::IDENTITY).unwrap();
    let placed = [
        at(&d),
        Placed {
            body: &s,
            place: Rigid::translation(Vector3::new(0.02, 0.015, 0.03)),
        },
    ];
    let v = look(Vector3::new(1.0, -2.0, 3.0));
    let run = |threads| {
        rayon::ThreadPoolBuilder::new()
            .num_threads(threads)
            .build()
            .unwrap()
            .install(|| view(&placed, &v, &ViewOptions::default()).unwrap())
    };
    assert_eq!(run(1), run(8));
}

/// A pin pressed through a plate (bodies that interfere, as ryucad's
/// `interference` takes a press fit to be meant): the pin's visibility
/// changes where it pierces the plate's faces, where no image curve
/// crosses it.
#[test]
fn a_pin_through_a_plate_is_hidden_inside_it_and_behind_its_bottom() {
    let (plate, _) = primitive::block(OpKey(14), 0.03, 0.03, 0.01, &Rigid::IDENTITY).unwrap();
    let (pin, _) = primitive::cylinder(OpKey(15), 0.003, 0.03, &Rigid::IDENTITY).unwrap();
    let placed = [
        at(&plate),
        Placed {
            body: &pin,
            place: Rigid::translation(Vector3::new(0.015, 0.015, -0.01)),
        },
    ];
    let moved = pin
        .transformed(&Rigid::translation(Vector3::new(0.015, 0.015, -0.01)))
        .unwrap();
    let bodies = [&plate, &moved];
    let p = sees(&placed, &look(Vector3::new(1.0, -1.0, 1.0)));
    // Each silhouette ruling of the pin, where (x - 15 mm) = (y - 15 mm) =
    // ±3/√2 mm, runs 10 mm below the plate, where a ray towards (1, -1, 1)
    // reaches the plate's bottom inside it and leaves by its top or a side
    // (behind two faces: at z = -10 mm it meets z = 0 at (27.1, 7.1) mm or
    // (22.9, 2.9) mm), 10 mm inside it (behind the one it leaves by) and
    // 10 mm above it (seen).
    let rulings: Vec<&Piece> = p
        .pieces
        .iter()
        .filter(|q| matches!(q.source, Source::Silhouette { body: 1, .. }))
        .collect();
    let mut by: [[f64; 3]; 2] = [[0.0; 3]; 2];
    let mut faces = Vec::new();
    for q in &rulings {
        let Source::Silhouette { face, ordinal, .. } = q.source else {
            unreachable!()
        };
        if !faces.contains(&(face, ordinal)) {
            faces.push((face, ordinal));
        }
        let r = faces.iter().position(|&f| f == (face, ordinal)).unwrap();
        let k = match q.visibility {
            Visibility::Visible => 0,
            Visibility::Hidden(1) => 1,
            Visibility::Hidden(2) => 2,
            other => panic!("{other:?}: {q:?}"),
        };
        by[r][k] += length(&p, &bodies, q);
    }
    assert_eq!(faces.len(), 2, "{rulings:?}");
    for lengths in by {
        for l in lengths {
            assert!((l - 0.01).abs() < 1e-9, "{by:?}");
        }
    }
}

/// A sphere's askew silhouette circle cut by its face's loops into a
/// stretch across the circle's start (plan 10.C's property test, sheet
/// seed 14): the stretch runs past the range its fitted pcurve was made
/// over, so the pcurve must be fitted again over it, and every point of
/// the pcurve lie on the curve.
#[test]
fn a_silhouette_across_its_circles_start_keeps_its_pcurve_on_it() {
    use ryukei::project::silhouettes;
    let bytes = ryukei_testkit::race::seed_bytes(14);
    let (sheets, _) = ryukei_testkit::sheet_bytes::sheet(&bytes).unwrap();
    let b = &sheets[0];
    let v = View::new(
        Vector3::new(
            -0.8438747602138779,
            -0.5937262282849576,
            -0.6568354826440128,
        ),
        Vector3::new(0.0, 0.0, 1.0),
    )
    .unwrap();
    let s = silhouettes(b, &v).unwrap();
    assert!(!s.branches.is_empty());
    for br in &s.branches {
        let surface = b.surface(b.face(br.face).unwrap().surface).unwrap();
        for k in 0..=16 {
            let x = br.range[0] + (br.range[1] - br.range[0]) * k as f64 / 16.0;
            let uv = br.pcurve.0.point(br.pcurve.1[0] + br.pcurve.1[1] * x);
            let gap = surface.point(uv.x, uv.y).distance(br.curve.point(x));
            assert!(gap <= 2.0 * br.tolerance, "{gap} at {x} on {br:?}");
        }
    }
}

/// Views the property test found judged wrong (plan 10.C), each held to
/// a fine mesh: race body 15's edge where a torus's silhouette ends on
/// it, and race body 18's torus silhouette at its image's cusps, where
/// visibility changes with no image crossing. Release only: tracing
/// these tori takes minutes in a debug build.
#[test]
#[cfg_attr(debug_assertions, ignore = "release only")]
fn visibility_changes_at_silhouette_ends_and_cusps() {
    use ryukei_testkit::projection::{Oracle, judge, look, placed_bodies, size};
    for (n, t) in [
        (
            15,
            [
                0.9793780808084567,
                -0.43247718930990686,
                0.35915854484882126,
            ],
        ),
        (
            15,
            [-0.604310299052824, 0.9999058082224215, -0.6282954476028557],
        ),
        (
            18,
            [0.9659554944336881, 0.5175783622980419, -0.8360049876987496],
        ),
        (
            18,
            [0.8753073327918315, -0.5493856012986855, -0.8721076118134168],
        ),
    ] {
        let b = ryukei_testkit::race::bodies(24).into_iter().nth(n).unwrap();
        let v = look(Vector3::new(t[0], t[1], t[2]));
        let placed = [Placed {
            body: &b,
            place: Rigid::IDENTITY,
        }];
        let p = view(&placed, &v, &ViewOptions::default()).unwrap();
        let bodies = placed_bodies(&placed);
        let o = Oracle::new(&bodies, &v, size(&bodies) * 1e-3).unwrap();
        let j = judge(&p, &bodies, &o);
        assert!(j.wrong.is_empty(), "race {n} {t:?}: {:?}", j.wrong);
    }
}

/// #643's edge (design 484, plan 17.B): ryucad's radial engine crankcase
/// in small, its 60 mm case with bosses (22 mm) joined and bored through
/// along the given angles. Seen from ryucad's isometric corner with all
/// nine bosses, a boss's saddle with the case is a NURBS whose image
/// cusps, which was refused `ViewError::Image` (the fifth boss alone
/// drew).
fn crankcase_twin(bosses: &[f64]) -> Body {
    let case = primitive::cylinder(
        OpKey(0x6430),
        0.06,
        0.08,
        &Rigid::translation(Vector3::new(0.0, 0.0, -0.04)),
    )
    .unwrap()
    .0;
    let mut body = case;
    for (k, &deg) in bosses.iter().enumerate() {
        let (s, c) = ryukei::math::sin_cos(deg.to_radians());
        let along = Vector3::new(c, s, 0.0);
        let place = |r: f64| {
            Rigid::from_frame(
                &Frame::new(
                    Point3::new(r * c, r * s, 0.0),
                    along,
                    Vector3::new(0.0, 0.0, 1.0),
                )
                .unwrap(),
            )
        };
        let boss = primitive::cylinder(OpKey(0x6431), 0.022, 0.02, &Rigid::IDENTITY)
            .unwrap()
            .0;
        let bore = primitive::cylinder(OpKey(0x6432), 0.015, 0.04, &Rigid::IDENTITY)
            .unwrap()
            .0;
        body = boolean(
            OpKey(0x6440 + k as u128),
            BooleanOp::Unite,
            &body,
            &[Tool {
                key: ToolKey(1),
                body: &boss,
                place: place(0.05),
            }],
        )
        .unwrap()
        .0;
        body = boolean(
            OpKey(0x6460 + k as u128),
            BooleanOp::Subtract,
            &body,
            &[Tool {
                key: ToolKey(1),
                body: &bore,
                place: place(0.04),
            }],
        )
        .unwrap()
        .0;
    }
    body
}

#[test]
#[cfg_attr(debug_assertions, ignore = "release only: booleans of a saddle")]
fn the_crankcases_boss_saddle_draws_from_the_corner() {
    let body = crankcase_twin(&[0.0, 40.0, 80.0, 120.0, 160.0, 200.0, 240.0, 280.0, 320.0]);
    let v = look(Vector3::new(1.0, -1.0, 1.0));
    let p = view(&[at(&body)], &v, &ViewOptions::default()).unwrap_or_else(|e| panic!("{e:?}"));
    let uncertain = p
        .pieces
        .iter()
        .filter(|q| q.visibility == Visibility::Uncertain)
        .count();
    assert!(
        uncertain * 10 <= p.pieces.len(),
        "{uncertain} of {}",
        p.pieces.len()
    );
}

/// A torus from the front (design 484): its view draws, the radial
/// engine's exhaust ring's case, with at most a tenth of its pieces
/// uncertain.
#[test]
fn a_torus_from_the_front_draws() {
    let (b, _) = primitive::torus(OpKey(0x6470), 0.03, 0.01, &Rigid::IDENTITY).unwrap();
    for t in [Vector3::new(0.0, -1.0, 0.0), Vector3::new(1.0, 0.0, 0.0)] {
        let p = view(&[at(&b)], &look(t), &ViewOptions::default())
            .unwrap_or_else(|e| panic!("{t:?}: {e:?}"));
        let uncertain = p
            .pieces
            .iter()
            .filter(|q| q.visibility == Visibility::Uncertain)
            .count();
        assert!(
            uncertain * 10 <= p.pieces.len(),
            "{uncertain} of {}",
            p.pieces.len()
        );
    }
}

/// The kind of surface of the face a refusal names, or of the edge's
/// first face.
fn refused_at(body: &Body, e: &Error) -> String {
    let at = match e {
        Error::InvalidView(ViewError::Uncertain { at } | ViewError::Image { at }) => *at,
        _ => return format!("{e:?}"),
    };
    for f in body.faces() {
        let face = body.face(f).unwrap();
        if face.name == at {
            let s = format!("{:?}", body.surface(face.surface).unwrap());
            return format!("face {at:?}: {}", s.split('(').next().unwrap_or(""));
        }
    }
    format!("edge {at:?}")
}

/// The race bodies' isometric views on one thread (design 484's bar):
/// every refusal takes at most 10 s with `RYUKEI_BARS=1` on the bar
/// machine (cox-worker-2), 100 s otherwise; each is printed with what it
/// refused on and its time, and the total. Minutes: plans 17.B's and
/// 17.C's bars run it with `--ignored`.
#[test]
#[ignore = "minutes: the bars run it"]
fn race_views_refuse_quickly() {
    let bar = if std::env::var("RYUKEI_BARS").as_deref() == Ok("1") {
        10.0
    } else {
        100.0
    };
    let pool = rayon::ThreadPoolBuilder::new()
        .num_threads(1)
        .build()
        .unwrap();
    let v = look(Vector3::new(1.0, -1.0, 1.0));
    let (mut drawn, mut total, mut slow) = (0, 0.0, Vec::new());
    pool.install(|| {
        for (i, body) in ryukei_testkit::race::bodies(200).iter().enumerate() {
            let start = std::time::Instant::now();
            let r = view(&[at(body)], &v, &ViewOptions::default());
            let secs = start.elapsed().as_secs_f64();
            total += secs;
            if secs > 10.0 {
                eprintln!(
                    "race {i}: {} in {secs:.2} s",
                    if r.is_ok() { "drawn" } else { "refused" }
                );
            }
            match r {
                Ok(_) => drawn += 1,
                Err(e) => {
                    eprintln!(
                        "race {i}: refused on {} in {secs:.2} s",
                        refused_at(body, &e)
                    );
                    if secs > bar {
                        slow.push((i, secs));
                    }
                }
            }
        }
    });
    eprintln!("race bodies iso, one thread: {drawn} drawn, {total:.1} s in all");
    assert!(slow.is_empty(), "refusals past {bar} s: {slow:?}");
}

/// The crankcase twin with all nine bosses from the drawing's front, top
/// and right (design 484): no piece the mesh contradicts, and at most a
/// tenth of the pieces uncertain in each view (#643). The bosses come in
/// pairs mirrored in the plane of the view and the axis (40° and 320°,
/// …), so a ray from a piece meets its twin's edge exactly: plan 17.C
/// settles such a ray by the edge's two faces' crossings, and certifies a
/// saddle's spline image lying on the rim's circle as an overlap. Plan
/// 17.B's count on cox-worker-4 was 656 of 1,571 pieces uncertain from
/// the front, 1,701 of 2,023 from the top, 131 of 1,163 from the right;
/// plan 17.C's is 37 of 1,571, 13 of 1,786 and 49 of 1,061. What remains
/// is mostly rays through edges in a pattern the two-face rule leaves
/// unsure (Task 10's tally on #643).
#[test]
#[cfg_attr(debug_assertions, ignore = "release only: booleans of saddles")]
fn the_crankcase_twins_views_agree_with_the_mesh() {
    let body = crankcase_twin(&[0.0, 40.0, 80.0, 120.0, 160.0, 200.0, 240.0, 280.0, 320.0]);
    for (t, up) in [
        (Vector3::new(0.0, -1.0, 0.0), Vector3::new(0.0, 0.0, 1.0)),
        (Vector3::new(0.0, 0.0, 1.0), Vector3::new(0.0, 1.0, 0.0)),
        (Vector3::new(1.0, 0.0, 0.0), Vector3::new(0.0, 0.0, 1.0)),
    ] {
        let v = View::new(t, up).unwrap();
        let placed = [at(&body)];
        let p = view(&placed, &v, &ViewOptions::default()).unwrap();
        let uncertain = p
            .pieces
            .iter()
            .filter(|q| q.visibility == Visibility::Uncertain)
            .count();
        eprintln!("{t:?}: {uncertain} of {} pieces uncertain", p.pieces.len());
        assert!(
            uncertain * 10 <= p.pieces.len(),
            "{t:?}: {uncertain} of {}",
            p.pieces.len()
        );
        let bodies = placed_bodies(&placed);
        let o = Oracle::new(&bodies, &v, size(&bodies) * 1e-3).unwrap();
        let j = judge(&p, &bodies, &o);
        assert!(j.wrong.is_empty(), "{t:?}: {:?}", j.wrong);
    }
}

/// #643 (plan 17.C): the crankcase twin seen from the drawing's front,
/// where the bosses at 40° and 320° (and each such pair) are mirror
/// images in a plane square to the view, so their saddles' images
/// coincide and a ray from the back saddle leaves through the front
/// saddle's edge. At most a tenth of its pieces uncertain, none the mesh
/// contradicts.
#[test]
#[cfg_attr(debug_assertions, ignore = "release only: booleans of saddles")]
fn the_crankcase_twins_front_view_mostly_certifies() {
    let body = crankcase_twin(&[0.0, 40.0, 80.0, 120.0, 160.0, 200.0, 240.0, 280.0, 320.0]);
    let v = View::new(Vector3::new(0.0, -1.0, 0.0), Vector3::new(0.0, 0.0, 1.0)).unwrap();
    let placed = [at(&body)];
    let p = view(&placed, &v, &ViewOptions::default()).unwrap();
    let uncertain = p
        .pieces
        .iter()
        .filter(|q| q.visibility == Visibility::Uncertain)
        .count();
    eprintln!("front: {uncertain} of {} pieces uncertain", p.pieces.len());
    assert!(
        uncertain * 10 <= p.pieces.len(),
        "{uncertain} of {}",
        p.pieces.len()
    );
    let bodies = placed_bodies(&placed);
    let o = Oracle::new(&bodies, &v, size(&bodies) * 1e-3).unwrap();
    let j = judge(&p, &bodies, &o);
    assert!(j.wrong.is_empty(), "{:?}", j.wrong);
}

/// #767 (plan 17.C): race bodies 16, 56 and 229, spline revolutions whose
/// formula's branches `fit_contour` couldn't certify whole, drawn from
/// ryucad's isometric corner: drawn with no piece the mesh contradicts,
/// or refused `Uncertain`.
#[test]
#[ignore = "minutes: the trace after a failed fit (#767)"]
fn the_slow_revolutions_draw_or_refuse() {
    let bodies = ryukei_testkit::race::bodies(200);
    let v = look(Vector3::new(1.0, -1.0, 1.0));
    for i in [16, 56, 229] {
        let placed = [at(&bodies[i])];
        let w = Work::unlimited();
        match view_with(&placed, &v, &ViewOptions::default(), &w) {
            Ok(p) => {
                let bs = placed_bodies(&placed);
                let o = Oracle::new(&bs, &v, size(&bs) * 1e-3).unwrap();
                let j = judge(&p, &bs, &o);
                assert!(j.wrong.is_empty(), "race {i}: {:?}", j.wrong);
                eprintln!("race {i}: drawn, {} units", w.spent());
            }
            Err(Error::InvalidView(ViewError::Uncertain { .. })) => {
                eprintln!("race {i}: refused, {} units", w.spent());
            }
            Err(e) => panic!("race {i}: {e:?}"),
        }
    }
}
