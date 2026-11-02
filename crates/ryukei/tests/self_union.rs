//! Self-union (design 299, plan 14.C): bodies whose shells cross or whose
//! faces overlap, made into the valid solid of the points their winding
//! number is one or more at, directly and as healing repairs them.

use ryukei::check::{check, check_intersections};
use ryukei::geom::{Point3, Rigid, Vector3};
use ryukei::geom::Surface;
use ryukei::heal::{HealOptions, Shape, heal, import_names, seamed};
use ryukei::intersect::{Inclusion, point_in_body, winding};
use ryukei::mesh::{MeshParams, tessellate};
use ryukei::name::{BooleanRole, Event, ImportRole, Name, OpKey, Role};
use ryukei::primitive;
use ryukei::repair::self_union;
use ryukei::topo::Body;
use ryukei::{Error, HealError};
use ryukei_testkit::crossing::{joined, one_solid, polyhedron, turn_shell};
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

fn sound(b: &Body) {
    assert!(check(b).is_empty(), "{:?}", check(b));
    assert!(
        check_intersections(b).is_empty(),
        "{:?}",
        check_intersections(b)
    );
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
    // shells: the top is kept as two strips, each named from the top and
    // the slab's side that cuts it, under healing's repair key.
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
    sound(&h);
    let key = OpKey(Name::derive(OpKey(7), Role::Import(ImportRole::Repair), &[]).to_u128());
    for side in [left, right] {
        let piece = Name::derive(key, Role::Boolean(BooleanRole::Piece), &[top, side]);
        assert!(h.by_name(piece).is_some(), "{side:?}");
    }
}

