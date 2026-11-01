//! Sheets swept from open chains and from loops as surfaces (design 295,
//! plan 12.B), through the public API: shapes, areas, checks, meshes and
//! names.

use core::f64::consts::{FRAC_PI_2, PI};
use ryukei::check::{check, check_intersections};
use ryukei::geom::{Circle2, Curve2, Frame, Frame2, Line2, Point2, Vector2, Vector3};
use ryukei::measure::{Entity, Properties, properties};
use ryukei::mesh::{MeshParams, check_mesh, tessellate};
use ryukei::name::{Name, OpKey, Role, SweepRole};
use ryukei::sweep::{
    Chain, ChainKey, Loop, LoopKey, Profile, Segment, SegmentKey, ValidProfile, extrude,
};
use ryukei::topo::{Body, BodyKind, EntityId};

const KEY: OpKey = OpKey(0x5357_4550);

fn line(key: u128, a: (f64, f64), b: (f64, f64)) -> Segment {
    let (a, b) = (Point2::new(a.0, a.1), Point2::new(b.0, b.1));
    Segment {
        key: SegmentKey(key),
        curve: Curve2::Line2(Line2::through(a, b).unwrap()),
        t0: 0.0,
        t1: a.distance(b),
    }
}

/// An open polyline through `pts`, segment keys from `first`.
fn polyline(key: u128, first: u128, pts: &[(f64, f64)]) -> Chain {
    Chain {
        key: ChainKey(key),
        segments: (0..pts.len() - 1)
            .map(|i| line(first + i as u128, pts[i], pts[i + 1]))
            .collect(),
    }
}

fn square(key: u128, first: u128, (x, y): (f64, f64), s: f64) -> Loop {
    let pts = [(x, y), (x + s, y), (x + s, y + s), (x, y + s)];
    Loop {
        key: LoopKey(key),
        segments: (0..4)
            .map(|i| line(first + i as u128, pts[i], pts[(i + 1) % 4]))
            .collect(),
    }
}

fn surface(loops: Vec<Loop>, chains: Vec<Chain>) -> ValidProfile {
    let profile = Profile {
        frame: Frame::WORLD,
        loops,
    };
    ValidProfile::surface(profile, chains).unwrap()
}

/// A sheet that checks clean, meshes with no faults, and has this area.
fn sound(b: &Body, area: f64, bound: f64) {
    assert_eq!(b.kind(), BodyKind::Sheet);
    assert_eq!(check(b), []);
    assert_eq!(check_intersections(b), []);
    let mesh = tessellate(b, &MeshParams::new(0.002, 0.3)).unwrap();
    assert!(mesh.faults.is_empty(), "{:?}", mesh.faults);
    assert_eq!(check_mesh(b, &mesh), Ok(()));
    let Properties::Sheet { area: a } = properties(b, Entity::Body).unwrap() else {
        panic!("a sheet's properties");
    };
    assert!(a.holds(area, bound), "{a:?} against {area}");
}

fn counts(b: &Body) -> (usize, usize, usize, usize) {
    let laminar = b.edges().filter(|&e| b.is_laminar(e)).count();
    (
        b.faces().count(),
        b.edges().count(),
        b.vertices().count(),
        laminar,
    )
}

fn names(b: &Body) -> Vec<Name> {
    let mut n: Vec<Name> = b.faces().map(|f| b.face(f).unwrap().name).collect();
    n.extend(b.edges().map(|e| b.edge(e).unwrap().name));
    n.extend(b.vertices().map(|v| b.vertex(v).unwrap().name));
    n.sort();
    n
}

#[test]
fn a_zigzag_extrudes_to_a_wall_either_way() {
    let zig = polyline(1, 10, &[(0.0, 0.0), (1.0, 0.0), (1.0, 1.0), (2.0, 1.0)]);
    let p = surface(vec![], vec![zig]);
    for dir in [1.0, -1.0] {
        let (b, journal) = extrude(KEY, &p, Vector3::new(0.0, 0.0, dir), 0.0, 2.0).unwrap();
        sound(&b, 6.0, 1e-9);
        // Three faces, six rails and four laterals, eight vertices; the
        // rails and the end laterals are laminar.
        assert_eq!(counts(&b), (3, 10, 8, 8));
        assert_eq!(journal.events().len(), 21);
    }
}

#[test]
fn a_chain_is_named_as_a_loops_segments_and_grows_at_its_end() {
    let short = polyline(1, 10, &[(0.0, 0.0), (1.0, 0.0), (1.0, 1.0)]);
    let long = polyline(1, 10, &[(0.0, 0.0), (1.0, 0.0), (1.0, 1.0), (2.0, 1.0)]);
    let up = Vector3::new(0.0, 0.0, 1.0);
    let (a, _) = extrude(KEY, &surface(vec![], vec![short]), up, 0.0, 1.0).unwrap();
    let (b, _) = extrude(KEY, &surface(vec![], vec![long]), up, 0.0, 1.0).unwrap();
    let segment =
        |k: u128| Name::derive(KEY, Role::Sweep(SweepRole::Segment), &[Name::from_u128(k)]);
    let side = |k: u128| Name::derive(KEY, Role::Sweep(SweepRole::Side), &[segment(k)]);
    for k in [10, 11] {
        assert!(matches!(a.by_name(side(k)), Some(EntityId::Face(_))));
    }
    let lateral = Name::derive(
        KEY,
        Role::Sweep(SweepRole::Lateral),
        &[segment(10), segment(11)],
    );
    assert!(matches!(b.by_name(lateral), Some(EntityId::Edge(_))));
    // Extending the chain renames only what was at its old end: the
    // lateral there and its two vertices.
    let (na, nb) = (names(&a), names(&b));
    let gone: Vec<&Name> = na.iter().filter(|n| !nb.contains(n)).collect();
    assert_eq!(gone.len(), 3, "{gone:?}");
}

#[test]
fn an_arc_extrudes_to_a_part_cylinder() {
    let f = Frame2::new(Point2::ORIGIN, Vector2::new(1.0, 0.0), true).unwrap();
    let c = Curve2::Circle2(Circle2::new(f, 0.5).unwrap());
    let arc = Chain {
        key: ChainKey(1),
        segments: vec![Segment {
            key: SegmentKey(10),
            curve: c,
            t0: 0.0,
            t1: FRAC_PI_2,
        }],
    };
    let (b, _) = extrude(
        KEY,
        &surface(vec![], vec![arc]),
        Vector3::new(0.0, 0.0, 1.0),
        0.0,
        1.0,
    )
    .unwrap();
    // A quarter of a cylinder of radius 0.5 and height 1.
    sound(&b, PI / 4.0, 1e-3);
    assert_eq!(counts(&b), (1, 4, 4, 4));
}

#[test]
fn loops_as_surfaces_lose_their_caps_and_keep_their_names() {
    let up = Vector3::new(0.0, 0.0, 1.0);
    let sq = || square(1, 10, (0.0, 0.0), 1.0);
    let (b, _) = extrude(KEY, &surface(vec![sq()], vec![]), up, 0.0, 2.0).unwrap();
    sound(&b, 8.0, 1e-9);
    assert_eq!(counts(&b), (4, 12, 8, 8));
    let solid = ValidProfile::new(Profile {
        frame: Frame::WORLD,
        loops: vec![sq()],
    })
    .unwrap();
    let (s, _) = extrude(KEY, &solid, up, 0.0, 2.0).unwrap();
    for f in b.faces() {
        assert!(matches!(
            s.by_name(b.face(f).unwrap().name),
            Some(EntityId::Face(_))
        ));
    }
    for e in b.edges() {
        assert!(matches!(
            s.by_name(b.edge(e).unwrap().name),
            Some(EntityId::Edge(_))
        ));
    }
    // A loop and a chain beside it make one sheet of two shells.
    let zig = polyline(2, 20, &[(2.0, 0.0), (3.0, 0.0), (3.0, 1.0)]);
    let (both, _) = extrude(KEY, &surface(vec![sq()], vec![zig]), up, 0.0, 2.0).unwrap();
    sound(&both, 12.0, 1e-9);
    assert_eq!(both.shells().count(), 2);
}
