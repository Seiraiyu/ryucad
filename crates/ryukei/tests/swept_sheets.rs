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

#[test]
fn a_square_with_a_hole_is_a_planar_face_named_as_a_start_cap() {
    use ryukei::measure::FaceShape;
    use ryukei::sweep::planar;
    let outer = square(1, 10, (0.0, 0.0), 1.0);
    let hole = square(2, 20, (0.25, 0.25), 0.5);
    let p = ValidProfile::new(Profile {
        frame: Frame::WORLD,
        loops: vec![outer, hole],
    })
    .unwrap();
    let (b, journal) = planar(KEY, &p).unwrap();
    sound(&b, 0.75, 1e-12);
    assert_eq!(counts(&b), (1, 8, 8, 8));
    assert_eq!(journal.events().len(), 17);
    let f = b.faces().next().unwrap();
    let Properties::Face {
        shape: FaceShape::Plane { normal, .. },
        ..
    } = properties(&b, Entity::Face(f)).unwrap()
    else {
        panic!("a plane");
    };
    assert_eq!(normal.get(), Vector3::new(0.0, 0.0, 1.0));
    let lp = Name::derive(KEY, Role::Sweep(SweepRole::Loop), &[Name::from_u128(1)]);
    let cap = Name::derive(KEY, Role::Sweep(SweepRole::StartCap), &[lp]);
    assert_eq!(b.face(f).unwrap().name, cap);
    // Chains, or no loops, make no planar face.
    let zig = polyline(3, 30, &[(2.0, 0.0), (3.0, 0.0)]);
    let open = surface(vec![square(1, 10, (0.0, 0.0), 1.0)], vec![zig]);
    assert_eq!(
        planar(KEY, &open).unwrap_err(),
        ryukei::Error::InvalidSheet(ryukei::SheetError::OpenProfile)
    );
}

/// The profile frame whose plane holds the z axis: `x` is x, `y` is z.
fn upright() -> Frame {
    Frame::new(
        ryukei::geom::Point3::ORIGIN,
        Vector3::new(0.0, -1.0, 0.0),
        Vector3::new(1.0, 0.0, 0.0),
    )
    .unwrap()
}

fn upright_surface(chains: Vec<Chain>) -> ValidProfile {
    let profile = Profile {
        frame: upright(),
        loops: vec![],
    };
    ValidProfile::surface(profile, chains).unwrap()
}

fn z_axis() -> ryukei::geom::Axis {
    ryukei::geom::Axis::new(ryukei::geom::Point3::ORIGIN, Vector3::new(0.0, 0.0, 1.0)).unwrap()
}

#[test]
fn a_line_turns_into_a_band_and_a_quarter() {
    use core::f64::consts::TAU;
    use ryukei::sweep::revolve;
    let wall = || polyline(1, 10, &[(1.0, 0.0), (1.0, 1.0)]);
    let p = upright_surface(vec![wall()]);
    let (band, _) = revolve(KEY, &p, z_axis(), 0.0, TAU).unwrap();
    sound(&band, TAU, 1e-3);
    assert_eq!(counts(&band), (1, 2, 2, 2));
    let (quarter, _) = revolve(KEY, &p, z_axis(), 0.0, FRAC_PI_2).unwrap();
    sound(&quarter, FRAC_PI_2, 1e-3);
    assert_eq!(counts(&quarter), (1, 4, 4, 4));
}

#[test]
fn a_chain_ending_on_the_axis_makes_a_pole_or_an_apex() {
    use core::f64::consts::{SQRT_2, TAU};
    use ryukei::sweep::revolve;
    // A quarter circle from the equator to the pole: a hemisphere, no
    // vertex at the pole.
    let f = Frame2::new(Point2::ORIGIN, Vector2::new(1.0, 0.0), true).unwrap();
    let c = Curve2::Circle2(Circle2::new(f, 1.0).unwrap());
    let quarter = Chain {
        key: ChainKey(1),
        segments: vec![Segment {
            key: SegmentKey(10),
            curve: c,
            t0: 0.0,
            t1: FRAC_PI_2,
        }],
    };
    let (dome, _) = revolve(KEY, &upright_surface(vec![quarter]), z_axis(), 0.0, TAU).unwrap();
    sound(&dome, TAU, 1e-3);
    assert_eq!(counts(&dome), (1, 1, 1, 1));
    // A slanted line to the axis: a cone with its apex a vertex, whole
    // and in part.
    let slant = || upright_surface(vec![polyline(2, 20, &[(1.0, 0.0), (0.0, 1.0)])]);
    let (cone, _) = revolve(KEY, &slant(), z_axis(), 0.0, TAU).unwrap();
    sound(&cone, PI * SQRT_2, 2e-3);
    assert_eq!(counts(&cone), (1, 1, 2, 1));
    let (part, _) = revolve(KEY, &slant(), z_axis(), 0.0, FRAC_PI_2).unwrap();
    sound(&part, PI * SQRT_2 / 4.0, 1e-3);
    assert_eq!(counts(&part), (1, 3, 3, 3));
}

#[test]
fn chains_on_the_axis_are_refused() {
    use core::f64::consts::TAU;
    use ryukei::SweepError;
    use ryukei::sweep::revolve;
    let pinched = upright_surface(vec![polyline(1, 10, &[(1.0, 0.0), (0.0, 0.5), (1.0, 1.0)])]);
    assert_eq!(
        revolve(KEY, &pinched, z_axis(), 0.0, TAU).unwrap_err(),
        SweepError::PinchedOnAxis {
            segment: SegmentKey(11)
        }
        .into()
    );
    let along = upright_surface(vec![polyline(2, 20, &[(0.0, 0.0), (0.0, 1.0), (1.0, 1.0)])]);
    assert_eq!(
        revolve(KEY, &along, z_axis(), 0.0, FRAC_PI_2).unwrap_err(),
        SweepError::ChainOnAxis {
            segment: SegmentKey(20)
        }
        .into()
    );
}

mod along_paths {
    use super::*;
    use core::f64::consts::TAU;
    use ryukei::geom::{Circle, Curve, Line, Point3};
    use ryukei::sweep::{AlongOptions, Path, PathSegment, along};

    fn part(key: u128, curve: Curve, range: [f64; 2]) -> PathSegment {
        PathSegment {
            key: SegmentKey(key),
            curve,
            range,
        }
    }

    fn straight(from: Point3, dir: Vector3) -> Curve {
        Curve::Line(Line::new(from, dir).unwrap())
    }

    fn quarter(key: u128, c: (f64, f64), r: f64, a: f64) -> PathSegment {
        let x = Vector3::new(1.0, 0.0, 0.0);
        let f = Frame::new(Point3::new(c.0, c.1, 0.0), Vector3::new(0.0, 0.0, 1.0), x).unwrap();
        part(
            key,
            Curve::Circle(Circle::new(f, r).unwrap()),
            [a, a + FRAC_PI_2],
        )
    }

    /// Along `x` by 2, a quarter turn of radius 1 towards `y`, along `y`
    /// by 2: `4 + π/2` long.
    fn bent() -> Path {
        Path {
            segments: vec![
                part(
                    1,
                    straight(Point3::ORIGIN, Vector3::new(1.0, 0.0, 0.0)),
                    [0.0, 2.0],
                ),
                quarter(2, (2.0, 1.0), 1.0, -FRAC_PI_2),
                part(
                    3,
                    straight(Point3::new(3.0, 1.0, 0.0), Vector3::new(0.0, 1.0, 0.0)),
                    [0.0, 2.0],
                ),
            ],
        }
    }

    /// A rounded rectangle `10 + π` long, its corners of radius 0.5.
    fn rounded_rectangle() -> Path {
        let (x, y) = (Vector3::new(1.0, 0.0, 0.0), Vector3::new(0.0, 1.0, 0.0));
        Path {
            segments: vec![
                part(1, straight(Point3::ORIGIN, x), [0.0, 2.0]),
                quarter(2, (2.0, 0.5), 0.5, -FRAC_PI_2),
                part(3, straight(Point3::new(2.5, 0.5, 0.0), y), [0.0, 3.0]),
                quarter(4, (2.0, 3.5), 0.5, 0.0),
                part(5, straight(Point3::new(2.0, 4.0, 0.0), -x), [0.0, 2.0]),
                quarter(6, (0.0, 3.5), 0.5, FRAC_PI_2),
                part(7, straight(Point3::new(-0.5, 3.5, 0.0), -y), [0.0, 3.0]),
                quarter(8, (0.0, 0.5), 0.5, PI),
            ],
        }
    }

    /// A chain across the path's start at the origin heading along `x`:
    /// the profile's `x` is `y`, its `y` is `z`.
    fn across(chain: Chain) -> ValidProfile {
        let frame = Frame::new(
            Point3::ORIGIN,
            Vector3::new(1.0, 0.0, 0.0),
            Vector3::new(0.0, 1.0, 0.0),
        )
        .unwrap();
        ValidProfile::surface(
            Profile {
                frame,
                loops: vec![],
            },
            vec![chain],
        )
        .unwrap()
    }

    /// Upright, 0.2 high.
    fn post() -> Chain {
        polyline(1, 10, &[(0.0, -0.1), (0.0, 0.1)])
    }

    #[test]
    fn a_post_round_a_bend_is_a_ribbon() {
        let (b, _) = along(KEY, &across(post()), &bent(), &AlongOptions::default()).unwrap();
        sound(&b, 0.2 * (4.0 + FRAC_PI_2), 1e-3);
        // A face per leg, rails at the four sections, two laterals a leg.
        assert_eq!(counts(&b), (3, 10, 8, 8));
        // A side is named from its segment and its path segment.
        let segment = Name::derive(KEY, Role::Sweep(SweepRole::Segment), &[Name::from_u128(10)]);
        let leg = Name::derive(
            KEY,
            Role::Sweep(SweepRole::PathSegment),
            &[Name::from_u128(2)],
        );
        let side = Name::derive(KEY, Role::Sweep(SweepRole::Side), &[segment, leg]);
        assert!(matches!(b.by_name(side), Some(EntityId::Face(_))));
    }

    #[test]
    fn a_twisted_bar_carries_its_chains_end_on_its_sweep() {
        let line = Path {
            segments: vec![part(
                1,
                straight(Point3::ORIGIN, Vector3::new(1.0, 0.0, 0.0)),
                [0.0, 2.0],
            )],
        };
        let flat = polyline(1, 10, &[(-0.1, 0.0), (0.1, 0.0)]);
        let opts = AlongOptions {
            twist: FRAC_PI_2,
            ..AlongOptions::default()
        };
        let (b, _) = along(KEY, &across(flat), &line, &opts).unwrap();
        // A helicoid: `L ∫ sqrt(1 + k² r²) dr` over the width, `k` the
        // twist a unit of length.
        let k = FRAC_PI_2 / 2.0;
        let steps = 2000;
        let h = 0.2 / f64::from(steps);
        let area: f64 = (0..steps)
            .map(|i| {
                let r = -0.1 + h * (f64::from(i) + 0.5);
                2.0 * h * ryukei::math::sqrt(1.0 + k * k * r * r)
            })
            .sum();
        sound(&b, area, 1e-4);
        assert_eq!(counts(&b), (1, 4, 4, 4));
    }

    #[test]
    fn a_post_round_one_circle_is_a_band() {
        let f = Frame::new(
            Point3::new(0.0, 2.0, 0.0),
            Vector3::new(0.0, 0.0, 1.0),
            Vector3::new(0.0, -1.0, 0.0),
        )
        .unwrap();
        let ring = Path {
            segments: vec![part(
                1,
                Curve::Circle(Circle::new(f, 2.0).unwrap()),
                [0.0, TAU],
            )],
        };
        let (b, _) = along(KEY, &across(post()), &ring, &AlongOptions::default()).unwrap();
        sound(&b, 0.2 * TAU * 2.0, 1e-3);
        assert_eq!(counts(&b), (1, 2, 2, 2));
    }

    #[test]
    fn a_post_round_a_closed_path_closes_at_its_sections() {
        let (b, _) = along(
            KEY,
            &across(post()),
            &rounded_rectangle(),
            &AlongOptions::default(),
        )
        .unwrap();
        sound(&b, 0.2 * (10.0 + PI), 1e-3);
        // Eight faces and sections, sixteen laterals and vertices.
        assert_eq!(counts(&b), (8, 24, 16, 16));
    }
}

mod lofts {
    use super::*;
    use core::f64::consts::TAU;
    use ryukei::LoftError;
    use ryukei::geom::Point3;
    use ryukei::sweep::{LoftOptions, loft};

    /// Chains in the plane `z = h`.
    fn at(h: f64, chains: Vec<Chain>) -> ValidProfile {
        let frame = Frame::new(
            Point3::new(0.0, 0.0, h),
            Vector3::new(0.0, 0.0, 1.0),
            Vector3::new(1.0, 0.0, 0.0),
        )
        .unwrap();
        ValidProfile::surface(
            Profile {
                frame,
                loops: vec![],
            },
            chains,
        )
        .unwrap()
    }

    #[test]
    fn two_lines_loft_to_a_rectangle_named_by_both() {
        let sections = [
            at(0.0, vec![polyline(1, 10, &[(0.0, 0.0), (1.0, 0.0)])]),
            at(1.0, vec![polyline(1, 20, &[(0.0, 0.0), (1.0, 0.0)])]),
        ];
        let (b, _) = loft(KEY, &sections, &LoftOptions::default()).unwrap();
        sound(&b, 1.0, 2e-3);
        assert_eq!(counts(&b), (1, 4, 4, 4));
        let stand =
            |r: SweepRole, k: u128| Name::derive(KEY, Role::Sweep(r), &[Name::from_u128(k)]);
        let side = Name::derive(
            KEY,
            Role::Sweep(SweepRole::Side),
            &[
                stand(SweepRole::Loop, 1),
                stand(SweepRole::Segment, 10),
                stand(SweepRole::Segment, 20),
            ],
        );
        assert!(matches!(b.by_name(side), Some(EntityId::Face(_))));
    }

    #[test]
    fn bent_chains_loft_through_three_sections() {
        let ell = |h: f64, k: u128, s: f64| {
            at(h, vec![polyline(1, k, &[(0.0, s), (0.0, 0.0), (s, 0.0)])])
        };
        let sections = [ell(0.0, 10, 1.0), ell(1.0, 20, 1.2), ell(2.0, 30, 1.0)];
        let (b, _) = loft(KEY, &sections, &LoftOptions::default()).unwrap();
        // About two walls `2 × 1.1`.
        sound(&b, 4.4, 0.2);
        assert_eq!(counts(&b), (2, 7, 6, 6));
    }

    #[test]
    fn chains_match_by_key_and_count() {
        let a = at(0.0, vec![polyline(1, 10, &[(0.0, 0.0), (1.0, 0.0)])]);
        let other = at(1.0, vec![polyline(2, 20, &[(0.0, 0.0), (1.0, 0.0)])]);
        assert_eq!(
            loft(KEY, &[a.clone(), other], &LoftOptions::default()).unwrap_err(),
            LoftError::LoopMismatch {
                section: 1,
                loop_: LoopKey(1)
            }
            .into()
        );
        let longer = at(
            1.0,
            vec![polyline(1, 20, &[(0.0, 0.0), (1.0, 0.0), (1.0, 1.0)])],
        );
        assert_eq!(
            loft(KEY, &[a, longer], &LoftOptions::default()).unwrap_err(),
            LoftError::SegmentCountMismatch {
                section: 1,
                loop_: LoopKey(1)
            }
            .into()
        );
    }

    #[test]
    fn a_closed_loft_of_a_chain_is_a_band_with_closed_laterals() {
        let sections: Vec<ValidProfile> = (0..4)
            .map(|k| {
                let a = TAU * f64::from(k) / 4.0;
                let (s, c) = ryukei::math::sin_cos(a);
                let frame = Frame::new(
                    Point3::new(3.0 * c, 3.0 * s, 0.0),
                    Vector3::new(-s, c, 0.0),
                    Vector3::new(c, s, 0.0),
                )
                .unwrap();
                let key = 10 * (k as u128 + 1);
                ValidProfile::surface(
                    Profile {
                        frame,
                        loops: vec![],
                    },
                    vec![polyline(1, key, &[(-0.5, 0.0), (0.5, 0.0)])],
                )
                .unwrap()
            })
            .collect();
        let opts = LoftOptions {
            closed: true,
            ..LoftOptions::default()
        };
        let (b, _) = loft(KEY, &sections, &opts).unwrap();
        // About the annulus between radii 2.5 and 3.5.
        sound(&b, 6.0 * PI, 2.0);
        assert_eq!(counts(&b), (1, 2, 2, 2));
    }
}

#[test]
fn loops_as_surfaces_in_every_sweep() {
    use core::f64::consts::TAU;
    use ryukei::geom::{Circle, Curve, Line, Point3};
    use ryukei::sweep::{AlongOptions, LoftOptions, Path, PathSegment, along, loft, revolve};
    let sq = |frame: Frame, k: u128, at: (f64, f64)| {
        ValidProfile::surface(
            Profile {
                frame,
                loops: vec![square(1, k, at, 0.5)],
            },
            vec![],
        )
        .unwrap()
    };
    // A full turn: a closed sheet of four faces, no laminar edge.
    let (ring, _) = revolve(KEY, &sq(upright(), 10, (1.0, 0.0)), z_axis(), 0.0, TAU).unwrap();
    assert_eq!(ring.kind(), BodyKind::Sheet);
    assert_eq!(check(&ring), []);
    assert_eq!(counts(&ring), (4, 4, 4, 0));
    // Along a line and a quarter turn: two legs of four sides.
    let x = Vector3::new(1.0, 0.0, 0.0);
    // Up `z` by 2, then a quarter turn of radius 1 towards `x`.
    let f = Frame::new(Point3::new(1.0, 0.0, 2.0), Vector3::new(0.0, 1.0, 0.0), x).unwrap();
    let path = Path {
        segments: vec![
            PathSegment {
                key: SegmentKey(1),
                curve: Curve::Line(Line::new(Point3::ORIGIN, Vector3::new(0.0, 0.0, 1.0)).unwrap()),
                range: [0.0, 2.0],
            },
            PathSegment {
                key: SegmentKey(2),
                curve: Curve::Circle(Circle::new(f, 1.0).unwrap()),
                range: [PI, 3.0 * FRAC_PI_2],
            },
        ],
    };
    let (bar, _) = along(
        KEY,
        &sq(Frame::WORLD, 10, (-0.25, -0.25)),
        &path,
        &AlongOptions::default(),
    )
    .unwrap();
    assert_eq!(bar.kind(), BodyKind::Sheet);
    assert_eq!(check(&bar), []);
    assert_eq!(counts(&bar).0, 8);
    assert_eq!(counts(&bar).3, 8);
    // A loft of two squares: four faces, open at both ends.
    let lifted = Frame::new(Point3::new(0.0, 0.0, 1.0), Vector3::new(0.0, 0.0, 1.0), x).unwrap();
    let (tube, _) = loft(
        KEY,
        &[sq(Frame::WORLD, 10, (0.0, 0.0)), sq(lifted, 20, (0.0, 0.0))],
        &LoftOptions::default(),
    )
    .unwrap();
    assert_eq!(tube.kind(), BodyKind::Sheet);
    assert_eq!(check(&tube), []);
    assert_eq!(counts(&tube), (4, 12, 8, 8));
}
