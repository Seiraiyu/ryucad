//! Self-union (design 299, plan 14.C): bodies whose shells cross or whose
//! faces overlap, made into the valid solid of the points their winding
//! number is one or more at, directly and as healing repairs them.
use ryukei::check::{check, check_intersections};
use ryukei::geom::Surface;
use ryukei::geom::{Point3, Rigid, Vector3};
use ryukei::heal::import_names;
use ryukei::heal::seamed;
use ryukei::heal::{HealOptions, Shape, heal};
use ryukei::intersect::{Inclusion, point_in_body, winding};
use ryukei::mesh::{MeshParams, tessellate};
use ryukei::name::Event;
use ryukei::name::OpKey;
use ryukei::name::{BooleanRole, Name, Role};
use ryukei::primitive;
use ryukei::repair::self_union;
use ryukei::topo::Body;
use ryukei::{Error, HealError};
use ryukei_testkit::crossing::polyhedron;
use ryukei_testkit::crossing::{joined, one_solid, turn_shell};
use ryukei_testkit::mesh::volume_and_area;

fn block(key: u128, x: f64, y: f64, z: f64, at: [f64; 3]) -> Body {
    primitive::block(
        OpKey(key),
        x,
        y,
        z,
        &Rigid::translation(Vector3::new(at[0], at[1], at[2])),
    )
    .unwrap()
    .0
}

fn shape(b: &Body) -> Shape {
    seamed(b, 1e-7).unwrap()
}

/// A mesh's volume, within its chord.
fn volume(b: &Body) -> f64 {
    let m = tessellate(b, &MeshParams::new(1e-4, 0.2)).unwrap();
    assert!(m.faults.is_empty(), "{:?}", m.faults);
    volume_and_area(&m).0
}

/// Two 4 m blocks, the second moved by (2, 1.5, 1.3): they share a
/// 2 × 2.5 × 2.7 box.
fn two_blocks() -> (Body, Body) {
    (
        block(1, 4.0, 4.0, 4.0, [0.0; 3]),
        block(2, 4.0, 4.0, 4.0, [2.0, 1.5, 1.3]),
    )
}

const SHARED: f64 = 2.0 * 2.5 * 2.7;

fn sound(b: &Body) {
    assert!(check(b).is_empty(), "{:?}", check(b));
    assert!(
        check_intersections(b).is_empty(),
        "{:?}",
        check_intersections(b)
    );
}

#[test]
fn shells_crossing_in_one_solid_are_united() {
    let (a, b) = two_blocks();
    let s = one_solid(joined(&shape(&a), &shape(&b)));
    let (h, journal, report) = heal(OpKey(7), &s, &HealOptions::default()).unwrap();
    sound(&h);
    assert!(
        (volume(&h) - (128.0 - SHARED)).abs() < 1e-6,
        "{}",
        volume(&h)
    );
    assert_eq!(h.counts().shells, 1);
    // Three faces of each block are cut back to where they leave the
    // other.
    assert_eq!(report.repaired.len(), 6, "{:?}", report.repaired);
    for n in &report.repaired {
        assert!(journal.events().contains(&Event::Modified(*n)), "{n:?}");
    }
}

/// A ball crossing a block's top, written as one solid's two shells,
/// heals to their union wherever it crosses. A sphere's shell has no
/// vertex, so which shells enclose which was judged by one point of its
/// mesh, which could lie on either side of the block: on the coarse mesh
/// healing signs a sphere's shell on since its winding number (#239), it
/// lay inside, and the ball was cut out of the block as a cavity
/// (`self_union_props`' seed 14 in v0.9.0-pre.1's gate).
#[test]
fn a_ball_crossing_a_block_heals_to_their_union_wherever_it_crosses() {
    let r = 0.5;
    let b = block(1, 2.0, 2.0, 2.0, [0.0; 3]);
    for (k, &(x, y)) in [(0.5, 0.5), (1.0, 1.0), (1.5, 0.7)].iter().enumerate() {
        for (j, d) in [-0.2, 0.0, 0.2].into_iter().enumerate() {
            let place = Rigid::translation(Vector3::new(x, y, 2.0 + d));
            let ball = primitive::sphere(OpKey(10 + 3 * k as u128 + j as u128), r, &place)
                .unwrap()
                .0;
            let s = one_solid(joined(&shape(&b), &shape(&ball)));
            let (h, _, _) = heal(OpKey(7), &s, &HealOptions::default())
                .unwrap_or_else(|e| panic!("({x}, {y}, {d}): {e:?}"));
            sound(&h);
            // The block and the cap of the ball above its top.
            let cap = r + d;
            let want = 8.0 + core::f64::consts::PI * cap * cap * (3.0 * r - cap) / 3.0;
            let got = volume(&h);
            assert!(
                (got - want).abs() < 1e-3,
                "({x}, {y}, {d}): {got} against {want}"
            );
        }
    }
}

#[test]
fn a_cavity_crossing_its_solid_is_cut_out() {
    let (a, b) = two_blocks();
    let mut s = one_solid(joined(&shape(&a), &shape(&b)));
    turn_shell(&mut s, 1);
    let (h, _, _) = heal(OpKey(7), &s, &HealOptions::default()).unwrap();
    sound(&h);
    assert!(
        (volume(&h) - (64.0 - SHARED)).abs() < 1e-6,
        "{}",
        volume(&h)
    );
}

#[test]
fn an_outer_shell_inside_out_leaves_what_the_other_holds_outside_it() {
    let (a, b) = two_blocks();
    let mut s = one_solid(joined(&shape(&a), &shape(&b)));
    turn_shell(&mut s, 0);
    let (h, _, _) = heal(OpKey(7), &s, &HealOptions::default()).unwrap();
    sound(&h);
    assert!(
        (volume(&h) - (64.0 - SHARED)).abs() < 1e-6,
        "{}",
        volume(&h)
    );
    assert_eq!(
        point_in_body(&h, Point3::new(5.0, 5.0, 5.0)),
        Ok(Inclusion::Inside)
    );
}

#[test]
fn both_inside_out_is_refused_as_the_file_was() {
    let (a, b) = two_blocks();
    let mut s = one_solid(joined(&shape(&a), &shape(&b)));
    turn_shell(&mut s, 0);
    turn_shell(&mut s, 1);
    let r = heal(OpKey(7), &s, &HealOptions::default());
    assert!(
        matches!(r, Err(Error::InvalidHeal(HealError::ShellsCross { .. }))),
        "{r:?}"
    );
}

#[test]
fn without_repair_crossing_shells_are_refused() {
    let (a, b) = two_blocks();
    let s = one_solid(joined(&shape(&a), &shape(&b)));
    let options = HealOptions {
        repair: false,
        ..HealOptions::default()
    };
    let r = heal(OpKey(7), &s, &options);
    assert!(
        matches!(r, Err(Error::InvalidHeal(HealError::ShellsCross { .. }))),
        "{r:?}"
    );
}

#[test]
fn shells_of_two_solids_crossing_are_still_refused() {
    // Solids of a file that cross are the importer's to unite (ryustep).
    let (a, b) = two_blocks();
    let s = joined(&shape(&a), &shape(&b));
    let r = heal(OpKey(7), &s, &HealOptions::default());
    assert!(
        matches!(r, Err(Error::InvalidHeal(HealError::ShellsCross { .. }))),
        "{r:?}"
    );
}

#[test]
fn faces_overlapping_in_one_solid_cancel() {
    // Two blocks stacked, written as one solid's two shells: the top of
    // the lower lies on the bottom of the upper, facing the other way.
    let lower = block(1, 1.0, 1.0, 1.0, [0.0; 3]);
    let upper = block(2, 1.0, 1.0, 1.0, [0.0, 0.0, 1.0]);
    let s = one_solid(joined(&shape(&lower), &shape(&upper)));
    let (h, _, report) = heal(OpKey(7), &s, &HealOptions::default()).unwrap();
    sound(&h);
    assert!((volume(&h) - 2.0).abs() < 1e-9, "{}", volume(&h));
    // One block of six faces: the sides on one plane merge.
    assert_eq!(h.faces().count(), 6);
    // The two faces between the blocks went; the sides on one plane
    // merged.
    assert_eq!(report.repaired.len(), 2, "{:?}", report.repaired);
}

#[test]
fn faces_overlapping_one_way_keep_one() {
    // A block and a cavity facing out that is the block itself: every
    // face of one lies on one of the other, facing its way; the region
    // of winding two is still the block.
    let a = block(1, 1.0, 2.0, 3.0, [0.0; 3]);
    let b = block(2, 1.0, 2.0, 3.0, [0.0; 3]);
    let s = one_solid(joined(&shape(&a), &shape(&b)));
    let (h, _, _) = heal(OpKey(7), &s, &HealOptions::default()).unwrap();
    sound(&h);
    assert!((volume(&h) - 6.0).abs() < 1e-9, "{}", volume(&h));
    assert_eq!(h.faces().count(), 6);
}

#[test]
fn a_valid_body_comes_back_as_it_was() {
    let a = block(1, 1.0, 2.0, 3.0, [0.5, 0.0, 0.0]);
    let (b, journal) = self_union(OpKey(9), &a).unwrap();
    assert_eq!(
        ryukei::codec::encode_body(&b),
        ryukei::codec::encode_body(&a)
    );
    assert!(journal.is_empty());
}

/// An octahedron one of whose corners is pushed through the far side:
/// its four faces at that corner cross the four opposite.
fn pushed_through() -> Shape {
    let p = [
        Point3::new(1.0, 0.0, 0.0),
        Point3::new(0.0, 1.0, 0.0),
        Point3::new(-1.0, 0.0, 0.0),
        Point3::new(0.0, -1.0, 0.0),
        Point3::new(0.9, 0.2, -1.6),
        Point3::new(0.0, 0.0, -1.0),
    ];
    polyhedron(
        &p,
        &[
            [0, 1, 4],
            [1, 2, 4],
            [2, 3, 4],
            [3, 0, 4],
            [1, 0, 5],
            [2, 1, 5],
            [3, 2, 5],
            [0, 3, 5],
        ],
    )
}

#[test]
fn faces_crossing_in_one_shell_are_repaired_by_winding() {
    // Healing doesn't look for faces crossing within a shell: it heals.
    let (h, _, _) = heal(OpKey(7), &pushed_through(), &HealOptions::default()).unwrap();
    assert!(!check_intersections(&h).is_empty());
    let (r, journal) = self_union(OpKey(9), &h).unwrap();
    sound(&r);
    assert!(!journal.is_empty());
    // Inside where the winding number is one or more, on a grid.
    let mut tried = 0;
    for i in 0..12 {
        for j in 0..12 {
            for k in 0..12 {
                let at = |n: i32| -1.2 + 0.2 * f64::from(n) + 0.0123;
                let p = Point3::new(at(i), at(j), at(k) - 0.6);
                let (Ok(w), Ok(inc)) = (winding(&h, p), point_in_body(&r, p)) else {
                    continue;
                };
                if inc == Inclusion::Boundary {
                    continue;
                }
                tried += 1;
                assert_eq!(inc == Inclusion::Inside, w >= 1, "{p:?}: winding {w}");
            }
        }
    }
    assert!(tried > 1000, "{tried}");
}

#[test]
fn pieces_are_named_by_the_faces_that_cut_them() {
    // A slab crossing a block's top all the way across, as one solid's
    // shells. No vertex of either lies inside the other, so healing
    // doesn't see them cross (as for #318's bodies); a caller's
    // self-union repairs them. The top is kept as two strips, each named
    // from the top and the slab's side that cuts it.
    let a = block(1, 4.0, 4.0, 4.0, [0.0; 3]);
    let slab = block(2, 1.0, 6.0, 2.0, [1.5, -1.0, 3.0]);
    let s = one_solid(joined(&shape(&a), &shape(&slab)));
    let names = import_names(OpKey(7), &s);
    // The face of the shape on the plane through `p` along `n`.
    let face = |p: Point3, n: Vector3| {
        let k = (0..s.faces.len())
            .find(|&k| {
                let surf = &s.surfaces[s.faces[k].surface];
                match surf {
                    Surface::Plane(pl) => {
                        pl.frame().z().get().cross(n).length() < 1e-12 && surf.distance(p) < 1e-12
                    }
                    _ => false,
                }
            })
            .unwrap();
        names.faces[k]
    };
    let top = face(Point3::new(1.0, 1.0, 4.0), Vector3::new(0.0, 0.0, 1.0));
    let left = face(Point3::new(1.5, 0.0, 3.5), Vector3::new(1.0, 0.0, 0.0));
    let right = face(Point3::new(2.5, 0.0, 3.5), Vector3::new(1.0, 0.0, 0.0));
    let (h, _, _) = heal(OpKey(7), &s, &HealOptions::default()).unwrap();
    assert!(!check_intersections(&h).is_empty());
    let (r, _) = self_union(OpKey(9), &h).unwrap();
    sound(&r);
    for side in [left, right] {
        let piece = Name::derive(OpKey(9), Role::Boolean(BooleanRole::Piece), &[top, side]);
        assert!(r.by_name(piece).is_some(), "{side:?}");
    }
}

/// Issue #434 (KiCad's MSBR-1): pieces of one solid glued face to face,
/// one of them touching a third along an edge. Healing hands the
/// overlap to the repair, whose union meets itself along that edge, so
/// it is refused, saying the pieces touch and where.
#[test]
fn glued_pieces_touching_along_an_edge_say_so() {
    let lower = block(1, 1.0, 1.0, 1.0, [0.0; 3]);
    let upper = block(2, 1.0, 1.0, 1.0, [0.0, 0.0, 1.0]);
    let beside = block(3, 1.0, 1.0, 1.0, [1.0, 1.0, 0.0]);
    let s = one_solid(joined(
        &joined(&shape(&lower), &shape(&upper)),
        &shape(&beside),
    ));
    match heal(OpKey(7), &s, &HealOptions::default()) {
        Err(Error::InvalidHeal(HealError::PiecesTouch { at, faces, .. })) => {
            assert!(
                (at.x - 1.0).abs() < 1e-6 && (at.y - 1.0).abs() < 1e-6,
                "{at:?}"
            );
            assert!(!faces.is_empty());
        }
        other => panic!("{:?}", other.map(|x| x.0.counts())),
    }
}

/// The molex body's case (#585) in small: a prism whose underside is a
/// rational bicubic-by-linear spline, knots (0, 1, 1.01), its last span
/// flat on z = 0, standing on a block's top. Its spline face has the
/// knots and the flat span the twin promises.
#[test]
fn the_flat_span_twin_is_what_it_says() {
    let (prism, slab) = ryukei_testkit::nurbs::flat_span_twin();
    assert_eq!(check(&prism), []);
    assert_eq!(check(&slab), []);
    let splines: Vec<_> = prism
        .faces()
        .filter_map(
            |f| match prism.surface(prism.face(f).unwrap().surface).unwrap() {
                Surface::Nurbs(n) => Some(n.clone()),
                _ => None,
            },
        )
        .collect();
    assert_eq!(splines.len(), 1, "one spline face");
    let n = &splines[0];
    assert_eq!(n.knots()[0].knots(), [0.0, 1.0, 1.01]);
    assert_eq!(n.knots()[0].mults(), [4, 3, 4]);
    for i in 3..7 {
        for j in 0..2 {
            assert!(n.control_point(i, j).z.abs() <= 5e-9, "row {i}");
        }
    }
}

/// #585's case in small: the prism's flat span lies on the block's top,
/// facing it. Healing joins the two shells into one solid without a
/// repair (they only touch, and its own tests don't see the touch), so
/// the body is self-united as the molex body is: the self-union splits the
/// span off, the flat piece and the block's top cancel, and the result is
/// the two solids' union, sound. Before 18.B the self-union was refused.
#[test]
fn a_spline_with_a_flat_span_on_a_plane_self_unions_to_the_union() {
    let (prism, slab) = ryukei_testkit::nurbs::flat_span_twin();
    let sh = one_solid(joined(&shape(&prism), &shape(&slab)));
    let (h, _, _) = heal(OpKey(7), &sh, &HealOptions::default()).expect("healed");
    assert!(!check_intersections(&h).is_empty(), "the touch is a fault");
    let (u, journal) = self_union(OpKey(9), &h).expect("self-united");
    sound(&u);
    let (want, got) = (volume(&prism) + volume(&slab), volume(&u));
    assert!((got - want).abs() <= 1e-3 * want, "{got} against {want}");
    // The spline face was split: a Split event whose children are two
    // faces.
    assert!(
        journal
            .events()
            .iter()
            .any(|e| matches!(e, Event::Split { children, .. } if children.len() == 2)),
        "{journal:?}"
    );
}

/// The same with the block moved off: the prism alone is valid and its
/// self-union is itself, bit for bit, with no split.
#[test]
fn a_valid_body_with_a_flat_span_comes_back_as_it_was() {
    let (prism, _) = ryukei_testkit::nurbs::flat_span_twin();
    let (r, journal) = self_union(OpKey(9), &prism).unwrap();
    assert_eq!(
        ryukei::codec::encode_body(&r),
        ryukei::codec::encode_body(&prism)
    );
    assert!(journal.is_empty());
}
