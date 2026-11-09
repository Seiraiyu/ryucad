//! Hidden lines (design 326, plan 10.B): views of placed bodies against
//! closed forms.

use core::f64::consts::TAU;
use ryukei::blend::{BlendKind, EdgeBlend, blend};
use ryukei::boolean::{BooleanOp, Tool, ToolKey, boolean};
use ryukei::geom::{Curve, Frame, Plane, Point3, Rigid, Surface, Vector3};
use ryukei::name::OpKey;
use ryukei::project::{
    Class, Piece, Placed, Projection, Source, View, ViewOptions, Visibility, view,
};
use ryukei::topo::Body;
use ryukei::{primitive, sheet};

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
