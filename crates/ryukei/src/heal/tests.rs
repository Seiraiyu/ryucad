use super::*;
use crate::geom::{Curve, Rigid, Surface, Vector3};
use crate::name::{Event, Role};
use crate::primitive;
use crate::topo::{FaceId, ShellId};
use crate::{HealError, check};
use std::collections::BTreeMap;

/// A body's faces, loops and edges as a shape, as a file without seams
/// would give them: no pcurves, every tolerance 0, the outer shell of
/// each solid the one with the largest box.
pub(crate) fn plain(b: &Body) -> Shape {
    shape_of(b, false)
}

/// [`plain`], with each fin's pcurve as the file's where its map is the
/// identity.
fn with_pcurves(b: &Body) -> Shape {
    shape_of(b, true)
}

fn shape_of(b: &Body, pcurves: bool) -> Shape {
    let mut s = Shape::default();
    let mut surfaces = BTreeMap::new();
    let mut curves = BTreeMap::new();
    let mut vertices = BTreeMap::new();
    let mut edges = BTreeMap::new();
    for v in b.vertices() {
        vertices.insert(v, s.vertices.len());
        s.vertices.push(Vertex {
            point: b.vertices[v].point,
            tolerance: 0.0,
            seam: false,
        });
    }
    for e in b.edges() {
        let edge = &b.edges[e];
        let c = *curves.entry(edge.curve).or_insert_with(|| {
            s.curves.push(b.curves[edge.curve].clone());
            s.curves.len() - 1
        });
        edges.insert(e, s.edges.len());
        s.edges.push(Edge {
            curve: c,
            interval: edge.interval,
            start: vertices[&edge.start],
            end: vertices[&edge.end],
            tolerance: 0.0,
        });
    }
    let mut faces: BTreeMap<FaceId, usize> = BTreeMap::new();
    for f in b.faces() {
        let face = &b.faces[f];
        let surface = *surfaces.entry(face.surface).or_insert_with(|| {
            s.surfaces.push(b.surfaces[face.surface].clone());
            s.surfaces.len() - 1
        });
        let loops = b
            .face_loops(f)
            .map(|l| match b.loops[l].vertex {
                Some(v) => Loop::Vertex(vertices[&v]),
                None => Loop::Edges(
                    b.loop_fins(l)
                        .map(|fin| Use {
                            edge: edges[&b.fins[fin].edge],
                            sense: b.fins[fin].sense,
                            pcurve: b.fins[fin]
                                .pcurve
                                .filter(|p| pcurves && p.map == [0.0, 1.0])
                                .map(|p| {
                                    s.curves2.push(b.curves2[p.curve].clone());
                                    s.curves2.len() - 1
                                }),
                        })
                        .collect(),
                ),
            })
            .collect();
        faces.insert(f, s.faces.len());
        s.faces.push(Face {
            surface,
            sense: face.sense,
            loops,
            tolerance: 0.0,
        });
    }
    for r in b.regions().skip(1) {
        let mut shells: Vec<(f64, usize)> = Vec::new();
        for sh in b.region_shells(r).collect::<Vec<ShellId>>() {
            let list: Vec<usize> = b.shell_faces(sh).map(|f| faces[&f]).collect();
            let size = b
                .shell_faces(sh)
                .map(|f| crate::intersect::face_box(b, f))
                .reduce(|x, y| x.union(&y))
                .map_or(0.0, |x| (x.max() - x.min()).length());
            shells.push((size, s.shells.len()));
            s.shells.push(Shell {
                faces: list,
                closed: true,
            });
        }
        let outer = shells
            .iter()
            .copied()
            .reduce(|a, b| if b.0 > a.0 { b } else { a })
            .expect("a shell")
            .1;
        s.solids.push(Solid {
            outer,
            voids: shells.iter().map(|x| x.1).filter(|&x| x != outer).collect(),
        });
    }
    s
}

fn tilted() -> Rigid {
    Rigid::rotation(Vector3::new(1.0, 2.0, 0.5).normalize().unwrap(), 0.7)
}

fn heal_ok(s: &Shape) -> (Body, Journal, HealReport) {
    let out = heal(OpKey(7), s, &HealOptions::default()).unwrap();
    assert_eq!(check::check(&out.0), []);
    out
}

#[test]
fn a_block_heals_to_the_same_block() {
    let b = primitive::block(OpKey(1), 1.0, 2.0, 3.0, &tilted())
        .unwrap()
        .0;
    let (h, journal, report) = heal_ok(&plain(&b));
    assert_eq!(h.counts(), b.counts());
    assert_eq!(report, HealReport::default());
    // Every face, edge and vertex is created, with its import name.
    let created: Vec<Name> = journal
        .events()
        .iter()
        .map(|e| match e {
            Event::Created(n) => *n,
            e => panic!("{e:?}"),
        })
        .collect();
    assert_eq!(created.len(), 6 + 12 + 8);
    let names = names::names(OpKey(7), &plain(&b));
    let mut all: Vec<Name> = names
        .faces
        .iter()
        .chain(&names.edges)
        .chain(&names.vertices)
        .copied()
        .collect();
    all.sort();
    let mut got = created.clone();
    got.sort();
    assert_eq!(got, all);
}

#[test]
fn names_follow_geometry_not_order() {
    let b = primitive::block(OpKey(1), 1.0, 2.0, 3.0, &tilted())
        .unwrap()
        .0;
    let s = plain(&b);
    // The same faces listed the other way round, each loop starting
    // elsewhere.
    let mut t = s.clone();
    t.faces.reverse();
    for f in &mut t.faces {
        if let Loop::Edges(uses) = &mut f.loops[0] {
            uses.rotate_left(1);
        }
    }
    let n = t.faces.len();
    for sh in &mut t.shells {
        for f in &mut sh.faces {
            *f = n - 1 - *f;
        }
    }
    let (a, _, _) = heal_ok(&s);
    let (c, _, _) = heal_ok(&t);
    let names = |b: &Body| {
        let mut x: Vec<Name> = b.names.keys().copied().collect();
        x.sort();
        x
    };
    assert_eq!(names(&a), names(&c));
    // Another key, other names.
    let d = heal(OpKey(8), &s, &HealOptions::default()).unwrap().0;
    assert!(names(&a).iter().all(|n| d.by_name(*n).is_none()));
}

#[test]
fn equal_hashes_take_ordinals_by_their_neighbours() {
    let hashes = [5, 9, 5, 5];
    let near = [vec![3], vec![1], vec![2], vec![2]];
    let n = names::named(OpKey(1), ImportRole::Edge, &hashes, &near);
    let by = |h: u128, o: u128| {
        Name::derive(
            OpKey(1),
            Role::Import(ImportRole::Edge),
            &[Name::from_u128(h), Name::from_u128(o)],
        )
    };
    assert_eq!(n, [by(5, 2), by(9, 0), by(5, 0), by(5, 1)]);
}

#[test]
fn turned_and_closed_primitives_heal() {
    // Primitives without seam edges or poles bounded: a cylinder's side
    // is one face between two circles.
    for b in [
        primitive::cylinder(OpKey(1), 1.0, 2.0, &tilted())
            .unwrap()
            .0,
        primitive::cone(OpKey(1), 1.0, 2.0, &tilted()).unwrap().0,
        primitive::frustum(OpKey(1), 1.0, 0.4, 2.0, &tilted())
            .unwrap()
            .0,
        primitive::sphere(OpKey(1), 1.0, &tilted()).unwrap().0,
        primitive::torus(OpKey(1), 2.0, 0.5, &tilted()).unwrap().0,
    ] {
        let (h, _, _) = heal_ok(&plain(&b));
        assert_eq!(h.counts(), b.counts());
    }
}

#[test]
fn a_cavity_is_built_in_its_solid() {
    let hollow = hollow();
    assert_eq!(hollow.counts().shells, 2);
    let s = plain(&hollow);
    assert_eq!(s.solids.len(), 1);
    assert_eq!(s.solids[0].voids.len(), 1);
    let (h, _, _) = heal_ok(&s);
    assert_eq!(h.counts(), hollow.counts());
}

#[test]
fn an_open_shell_names_its_free_edges() {
    let b = primitive::block(OpKey(1), 1.0, 2.0, 3.0, &Rigid::IDENTITY)
        .unwrap()
        .0;
    let mut s = plain(&b);
    let names = names::names(OpKey(7), &s);
    let gone = s.faces.len() - 1;
    let Loop::Edges(uses) = &s.faces[gone].loops[0] else {
        panic!()
    };
    let mut free: Vec<Name> = uses.iter().map(|u| names.edges[u.edge]).collect();
    free.sort();
    s.faces[gone].loops.clear();
    s.faces[gone].loops.push(Loop::Vertex(0));
    // The face's own edges are now used once.
    match heal(OpKey(7), &s, &HealOptions::default()) {
        Err(Error::InvalidHeal(HealError::OpenShell { free_edges })) => {
            assert_eq!(free_edges, free)
        }
        other => panic!("{other:?}"),
    }
}

#[test]
fn a_use_run_the_wrong_way_is_misoriented() {
    let b = primitive::block(OpKey(1), 1.0, 2.0, 3.0, &Rigid::IDENTITY)
        .unwrap()
        .0;
    let mut s = plain(&b);
    let Loop::Edges(uses) = &mut s.faces[0].loops[0] else {
        panic!()
    };
    // One use of a loop against the rest: no face can be turned to agree.
    uses[0].sense = uses[0].sense.flipped();
    let names = names::names(OpKey(7), &s);
    match heal(OpKey(7), &s, &HealOptions::default()) {
        Err(Error::InvalidHeal(HealError::Misoriented { at })) => {
            assert!(names.edges.contains(&at))
        }
        other => panic!("{other:?}"),
    }
}

#[test]
fn malformed_shapes_are_refused() {
    let b = primitive::block(OpKey(1), 1.0, 2.0, 3.0, &Rigid::IDENTITY)
        .unwrap()
        .0;
    let s = plain(&b);
    let refused = |s: &Shape| {
        matches!(
            heal(OpKey(7), s, &HealOptions::default()),
            Err(Error::InvalidHeal(HealError::Malformed { .. }))
        )
    };
    let mut t = s.clone();
    t.edges[0].curve = 99;
    assert!(refused(&t));
    let mut t = s.clone();
    t.edges[0].interval = [1.0, 1.0];
    assert!(refused(&t));
    let mut t = s.clone();
    t.shells[0].faces.pop();
    assert!(refused(&t));
    let mut t = s.clone();
    t.solids.clear();
    assert!(refused(&t));
    let mut t = s.clone();
    t.vertices[0].tolerance = f64::NAN;
    assert!(refused(&t));
}

#[test]
fn approximated_faces_are_reported_and_capped() {
    let b = primitive::block(OpKey(1), 1.0, 2.0, 3.0, &Rigid::IDENTITY)
        .unwrap()
        .0;
    let mut s = plain(&b);
    s.faces[2].tolerance = 3e-7;
    let names = names::names(OpKey(7), &s);
    let (h, _, report) = heal_ok(&s);
    assert_eq!(report.approximated, [(names.faces[2], 3e-7)]);
    // The face's edges and vertices take its bound.
    let Loop::Edges(uses) = &s.faces[2].loops[0] else {
        panic!()
    };
    for u in uses {
        let e = h.by_name(names.edges[u.edge]).unwrap();
        let crate::topo::EntityId::Edge(e) = e else {
            panic!()
        };
        assert_eq!(h.edges[e].tolerance.get(), 3e-7);
    }
    assert_eq!(report.tolerances.len(), 4 + 4);
    s.faces[2].tolerance = 2e-4;
    assert_eq!(
        heal(OpKey(7), &s, &HealOptions::default()).unwrap_err(),
        HealError::GapTooLarge {
            entity: names.faces[2],
            gap: 2e-4
        }
        .into()
    );
}

#[test]
fn a_vertex_off_its_curve_is_a_gap() {
    let b = primitive::block(OpKey(1), 1.0, 2.0, 3.0, &Rigid::IDENTITY)
        .unwrap()
        .0;
    let mut s = plain(&b);
    let p = s.vertices[0].point;
    s.vertices[0].point = crate::geom::Point3::new(p.x + 1e-3, p.y, p.z);
    let names = names::names(OpKey(7), &s);
    match heal(OpKey(7), &s, &HealOptions::default()) {
        Err(Error::InvalidHeal(HealError::GapTooLarge { entity, .. })) => {
            assert!(names.vertices.contains(&entity) || names.edges.contains(&entity))
        }
        other => panic!("{other:?}"),
    }
}

/// Two cylinders crossed at right angles, united: their intersection
/// edges are splines, with fitted pcurves on both.
fn crossed() -> Body {
    use crate::boolean::{BooleanOp, Tool, ToolKey, boolean};
    let a = primitive::cylinder(
        OpKey(1),
        1.0,
        4.0,
        &Rigid::translation(Vector3::new(0.0, 0.0, -2.0)),
    )
    .unwrap()
    .0;
    let turn = Rigid::rotation(crate::geom::UnitVector3::X, core::f64::consts::FRAC_PI_2);
    let b = primitive::cylinder(
        OpKey(2),
        0.6,
        4.0,
        &Rigid::translation(Vector3::new(0.0, 2.0, 0.0)),
    )
    .unwrap()
    .0;
    boolean(
        OpKey(3),
        BooleanOp::Unite,
        &a,
        &[Tool {
            key: ToolKey(1),
            body: &b,
            place: turn,
        }],
    )
    .unwrap()
    .0
}

#[test]
fn pcurves_no_formula_gives_are_fitted() {
    let b = crossed();
    let nurbs = b
        .edges()
        .filter(|&e| matches!(b.curves[b.edges[e].curve], crate::geom::Curve::Nurbs(_)))
        .count();
    assert!(nurbs > 0);
    let (h, _, report) = heal_ok(&plain(&b));
    assert_eq!(h.counts(), b.counts());
    // Fitted pcurves within resolution need no larger tolerance.
    assert!(report.tolerances.iter().all(|t| t.1 <= 1e-7), "{report:?}");
}

#[test]
fn a_files_pcurves_are_used_where_they_fit() {
    let b = crossed();
    let s = with_pcurves(&b);
    assert!(!s.curves2.is_empty());
    let (h, _, _) = heal_ok(&s);
    // The intersection edges' pcurves are the file's, bit for bit.
    let theirs: Vec<String> = s.curves2.iter().map(crate::topo::dump::curve2).collect();
    let used = h
        .fins()
        .filter_map(|f| h.fins[f].pcurve)
        .filter(|p| theirs.contains(&crate::topo::dump::curve2(&h.curves2[p.curve])))
        .count();
    assert!(used > 0);
    // A pcurve off its edge is not used: the edge's is fitted instead.
    let mut t = s.clone();
    for c in &mut t.curves2 {
        if let crate::geom::Curve2::Nurbs2(n) = c {
            let pts: Vec<crate::geom::Point2> = n
                .control_points()
                .map(|p| crate::geom::Point2::new(p.x + 0.1, p.y))
                .collect();
            *c = crate::geom::Curve2::Nurbs2(std::sync::Arc::new(
                crate::geom::NurbsCurve2::new(n.knots().clone(), pts, Some(n.weights().to_vec()))
                    .unwrap(),
            ));
        }
    }
    heal_ok(&t);
}

/// Each entity's geometry as text, sorted: what a round trip must keep.
fn geometry(b: &Body) -> (Vec<String>, Vec<String>, Vec<String>) {
    use crate::topo::dump;
    let mut s: Vec<String> = b
        .faces()
        .map(|f| {
            format!(
                "{} {:?}",
                dump::surface(&b.surfaces[b.faces[f].surface]),
                b.faces[f].sense
            )
        })
        .collect();
    let mut c: Vec<String> = b
        .edges()
        .map(|e| dump::curve(&b.curves[b.edges[e].curve]))
        .collect();
    let mut p: Vec<String> = b
        .vertices()
        .map(|v| dump::point(b.vertices[v].point))
        .collect();
    s.sort();
    c.sort();
    p.sort();
    (s, c, p)
}

/// `b` written with seams and healed again: the same faces on the same
/// surfaces, edges on the same curves and vertices at the same points;
/// and healing what it gives written again is the same body.
pub(crate) fn round_trip(b: &Body) -> Body {
    let s = seamed(b, 1e-7).unwrap();
    // Every face of the shape is cut open: no periodic face wraps.
    for (k, f) in s.faces.iter().enumerate() {
        for l in &f.loops {
            if let Loop::Edges(uses) = l {
                for u in uses {
                    if let Some(p) = u.pcurve {
                        let e = &s.edges[u.edge];
                        assert!(
                            crate::pcurve::fits(
                                &s.surfaces[f.surface],
                                &s.curves[e.curve],
                                e.interval,
                                1e-7,
                                &(s.curves2[p].clone(), [0.0, 1.0])
                            ),
                            "face {k}: a pcurve off its edge"
                        );
                    }
                }
            }
        }
    }
    let (h, _, _) = heal(OpKey(9), &s, &HealOptions::default()).unwrap();
    assert_eq!(check::check(&h), [], "{}", h.dump());
    assert_eq!(h.counts(), b.counts());
    assert_eq!(geometry(&h), geometry(b));
    let again = heal(
        OpKey(9),
        &seamed(&h, 1e-7).unwrap(),
        &HealOptions::default(),
    )
    .unwrap()
    .0;
    assert_eq!(again.dump(), h.dump());
    h
}

#[test]
fn primitives_round_trip_through_seams() {
    let t = tilted();
    let k = OpKey(1);
    for b in [
        primitive::block(k, 1.0, 2.0, 3.0, &t).unwrap().0,
        primitive::cylinder(k, 1.0, 2.0, &t).unwrap().0,
        primitive::cone(k, 1.0, 2.0, &t).unwrap().0,
        primitive::frustum(k, 1.0, 0.4, 2.0, &t).unwrap().0,
        primitive::sphere(k, 1.0, &t).unwrap().0,
        primitive::torus(k, 2.0, 0.5, &t).unwrap().0,
    ] {
        round_trip(&b);
    }
}

#[test]
fn seams_and_poles_are_reported() {
    let b = primitive::sphere(OpKey(1), 1.0, &tilted()).unwrap().0;
    let s = seamed(&b, 1e-7).unwrap();
    // One face, one loop: the seam there and back between the poles.
    assert_eq!(s.faces.len(), 1);
    let Loop::Edges(uses) = &s.faces[0].loops[0] else {
        panic!()
    };
    assert_eq!(uses.len(), 2);
    assert_eq!(uses[0].edge, uses[1].edge);
    let (_, journal, report) = heal_ok(&s);
    assert_eq!(report.seams.len(), 1);
    assert_eq!(report.poles.len(), 2);
    assert!(
        journal
            .events()
            .iter()
            .any(|e| *e == Event::Deleted(report.seams[0]))
    );
}

/// Edge `e` of a shape moved by `by`, onto a curve of its own.
fn move_edge(s: &mut Shape, e: usize, by: Vector3) {
    let c = s.curves[s.edges[e].curve].transformed(&Rigid::translation(by));
    s.curves.push(c);
    s.edges[e].curve = s.curves.len() - 1;
}

/// The middle of edge `e`.
fn middle(s: &Shape, e: usize) -> crate::geom::Point3 {
    let x = &s.edges[e];
    s.curves[x.curve].point((x.interval[0] + x.interval[1]) / 2.0)
}

/// The built entity a name names.
fn built(b: &Body, n: Name) -> crate::topo::EntityId {
    b.by_name(n)
        .unwrap_or_else(|| panic!("{n} is not in the body"))
}

#[test]
fn gaps_under_the_cap_are_covered_by_tolerances() {
    use crate::topo::EntityId;
    let b = primitive::block(OpKey(1), 1.0, 2.0, 3.0, &Rigid::IDENTITY)
        .unwrap()
        .0;
    let mut s = plain(&b);
    // Off its faces, away from the block's middle (at right angles to the
    // edge), by 3 µm.
    let away = (middle(&s, 0) - crate::geom::Point3::new(0.5, 1.0, 1.5))
        .normalize()
        .unwrap();
    move_edge(&mut s, 0, away.get() * 3e-6);
    let names = names::names(OpKey(7), &s);
    let (h, _, report) = heal_ok(&s);
    let EntityId::Edge(e) = built(&h, names.edges[0]) else {
        panic!()
    };
    // Its farthest from either face's plane.
    let gap = h
        .edge_fins(e)
        .map(|f| {
            let face = h.loops[h.fins[f].loop_].face;
            h.surfaces[h.faces[face].surface].distance(middle(&s, 0))
        })
        .fold(0.0, f64::max);
    // The edge's tolerance also covers its vertices, 3 µm from its ends.
    let t = h.edges[e].tolerance.get();
    assert!(
        gap < 3e-6 && (3e-6..=3e-6 * 1.001).contains(&t),
        "{t} for {gap}"
    );
    for v in [h.edges[e].start, h.edges[e].end] {
        let tv = h.vertices[v].tolerance.get();
        assert!((3e-6..=3e-6 * 1.001).contains(&tv), "{tv}");
    }
    assert!(report.tolerances.iter().any(|x| x.0 == names.edges[0]));
    assert_eq!(report.tolerances.len(), 3);
}

#[test]
fn gaps_on_analytic_surfaces_are_certified_tightly() {
    use crate::geom::{Circle, Curve};
    use crate::topo::EntityId;
    let b = primitive::cylinder(OpKey(1), 1.0, 2.0, &tilted())
        .unwrap()
        .0;
    let mut s = plain(&b);
    // One rim 2 µm wider than the cylinder.
    let rim = (0..s.edges.len())
        .find(|&e| matches!(s.curves[s.edges[e].curve], Curve::Circle(_)))
        .unwrap();
    let Curve::Circle(c) = &s.curves[s.edges[rim].curve] else {
        panic!()
    };
    let wider = Circle::new(*c.frame(), c.radius() + 2e-6).unwrap();
    s.curves.push(Curve::Circle(wider));
    s.edges[rim].curve = s.curves.len() - 1;
    // The gap from the cylinder, certified, is the 2 µm.
    let x = &s.edges[rim];
    let side = s
        .surfaces
        .iter()
        .find(|f| matches!(f, crate::geom::Surface::Cylinder(_)))
        .unwrap();
    let gap = gaps::edge_gap(&s.curves[x.curve], x.interval, side);
    assert!((2e-6..=2e-6 * 1.01 + 2e-10).contains(&gap), "{gap}");
    let names = names::names(OpKey(7), &s);
    let (h, _, _) = heal_ok(&s);
    let EntityId::Edge(e) = built(&h, names.edges[rim]) else {
        panic!()
    };
    assert!(h.edges[e].tolerance.get() >= gap);
}

#[test]
fn a_gap_past_the_cap_is_refused_with_its_size() {
    let b = primitive::block(OpKey(1), 1.0, 2.0, 3.0, &Rigid::IDENTITY)
        .unwrap()
        .0;
    let mut s = plain(&b);
    let away = (middle(&s, 0) - crate::geom::Point3::new(0.5, 1.0, 1.5))
        .normalize()
        .unwrap();
    move_edge(&mut s, 0, away.get() * 2e-4);
    let names = names::names(OpKey(7), &s);
    let e = &s.edges[0];
    let ours = [
        names.edges[0],
        names.vertices[e.start],
        names.vertices[e.end],
    ];
    match heal(OpKey(7), &s, &HealOptions::default()) {
        Err(Error::InvalidHeal(HealError::GapTooLarge { entity, gap })) => {
            assert!(ours.contains(&entity));
            assert!((1e-4..=2e-4 * (1.0 + 1e-9)).contains(&gap), "{gap}");
        }
        other => panic!("{other:?}"),
    }
    // A larger cap takes it.
    let wide = HealOptions {
        tolerance_cap: 1e-3,
    };
    let (h, _, _) = heal(OpKey(7), &s, &wide).unwrap();
    assert_eq!(check::check(&h), []);
}

/// A shape whose faces each have their own edges and vertices, as a
/// surface model writes them: face `k`'s copies moved by `nudge` along a
/// direction of its own.
fn unshared(s: &Shape, nudge: f64) -> Shape {
    let mut t = s.clone();
    t.edges.clear();
    t.vertices.clear();
    for k in 0..s.faces.len() {
        let dir = Vector3::new((k * 7 % 5) as f64 - 2.0, (k * 3 % 5) as f64 - 2.0, 1.0)
            .normalize()
            .unwrap();
        let by = Rigid::translation(dir.get() * nudge);
        let mut vertices: BTreeMap<usize, usize> = BTreeMap::new();
        let mut copy = |t: &mut Shape, v: usize| {
            *vertices.entry(v).or_insert_with(|| {
                t.vertices.push(Vertex {
                    point: by.apply_point(s.vertices[v].point),
                    ..s.vertices[v]
                });
                t.vertices.len() - 1
            })
        };
        let mut loops = s.faces[k].loops.clone();
        for l in &mut loops {
            match l {
                Loop::Vertex(v) => *v = copy(&mut t, *v),
                Loop::Edges(uses) => {
                    for u in uses {
                        let e = s.edges[u.edge];
                        t.curves.push(s.curves[e.curve].transformed(&by));
                        let (start, end) = (copy(&mut t, e.start), copy(&mut t, e.end));
                        t.edges.push(Edge {
                            curve: t.curves.len() - 1,
                            start,
                            end,
                            ..e
                        });
                        u.edge = t.edges.len() - 1;
                        u.pcurve = None;
                    }
                }
            }
        }
        t.faces[k].loops = loops;
    }
    t
}

#[test]
fn faces_with_their_own_edges_are_sewn() {
    let b = primitive::block(OpKey(1), 1.0, 2.0, 3.0, &tilted())
        .unwrap()
        .0;
    let s = unshared(&plain(&b), 2e-7);
    assert_eq!(s.edges.len(), 24);
    let (h, journal, report) = heal_ok(&s);
    assert_eq!(h.counts(), b.counts());
    assert_eq!((report.sewn.len(), report.merged.len()), (12, 8));
    // Each sewn edge merges two of the file's, each vertex three.
    let mut merges: Vec<usize> = journal
        .events()
        .iter()
        .filter_map(|e| match e {
            Event::Merged { parents, child } => {
                assert!(h.by_name(*child).is_some());
                Some(parents.len())
            }
            _ => None,
        })
        .collect();
    merges.sort();
    assert_eq!(merges, [vec![2; 12], vec![3; 8]].concat());
    // The tolerances cover where the copies were.
    assert!(report.tolerances.iter().all(|t| t.1 <= 1e-6));
    // A cylinder's rims are closed edges.
    let c = primitive::cylinder(OpKey(1), 1.0, 2.0, &tilted())
        .unwrap()
        .0;
    let (h, _, report) = heal_ok(&unshared(&plain(&c), 2e-7));
    assert_eq!(h.counts(), c.counts());
    assert_eq!(report.sewn.len(), 2);
}

#[test]
fn open_shells_sewn_together_are_one_solid() {
    let b = primitive::block(OpKey(1), 1.0, 2.0, 3.0, &Rigid::IDENTITY)
        .unwrap()
        .0;
    let mut s = unshared(&plain(&b), 1e-7);
    // Two open shells of three faces each, as a surface model gives them.
    s.shells = vec![
        Shell {
            faces: vec![0, 1, 2],
            closed: false,
        },
        Shell {
            faces: vec![3, 4, 5],
            closed: false,
        },
    ];
    s.solids = vec![
        Solid {
            outer: 0,
            voids: vec![],
        },
        Solid {
            outer: 1,
            voids: vec![],
        },
    ];
    let (h, _, _) = heal_ok(&s);
    assert_eq!(h.counts(), b.counts());
}

#[test]
fn sewing_refuses_three_faces_at_an_edge_and_leaves_open_shells_open() {
    let b = primitive::block(OpKey(1), 1.0, 2.0, 3.0, &Rigid::IDENTITY)
        .unwrap()
        .0;
    let mut s = unshared(&plain(&b), 0.0);
    // Face 0 twice: its edges have two candidates each that coincide.
    let mut twin = s.faces[0].clone();
    for l in &mut twin.loops {
        if let Loop::Edges(uses) = l {
            for u in uses {
                s.edges.push(s.edges[u.edge]);
                u.edge = s.edges.len() - 1;
            }
        }
    }
    s.faces.push(twin);
    s.shells[0].faces.push(s.faces.len() - 1);
    assert!(matches!(
        heal(OpKey(7), &s, &HealOptions::default()),
        Err(Error::InvalidHeal(HealError::NonManifold { .. }))
    ));
    // Without a face, its neighbours' copies of its edges stay free.
    let mut s = unshared(&plain(&b), 1e-7);
    s.faces.pop();
    s.shells[0].faces.retain(|&f| f < s.faces.len());
    match heal(OpKey(7), &s, &HealOptions::default()) {
        Err(Error::InvalidHeal(HealError::OpenShell { free_edges })) => {
            assert_eq!(free_edges.len(), 4)
        }
        other => panic!("{other:?}"),
    }
}

/// A plate with a round hole through it.
fn plate() -> Body {
    use crate::boolean::{BooleanOp, Tool, ToolKey, boolean};
    let block = primitive::block(OpKey(1), 4.0, 3.0, 1.0, &Rigid::IDENTITY)
        .unwrap()
        .0;
    let rod = primitive::cylinder(
        OpKey(2),
        0.5,
        3.0,
        &Rigid::translation(Vector3::new(1.5, 1.2, -1.0)),
    )
    .unwrap()
    .0;
    boolean(
        OpKey(3),
        BooleanOp::Subtract,
        &block,
        &[Tool {
            key: ToolKey(1),
            body: &rod,
            place: Rigid::IDENTITY,
        }],
    )
    .unwrap()
    .0
}

/// A face of the shape with two loops, the hole's loop's index, and the
/// face's index.
fn holed_face(s: &Shape) -> (usize, usize) {
    let k = s
        .faces
        .iter()
        .position(|f| f.loops.len() == 2 && matches!(s.surfaces[f.surface], Surface::Plane(_)))
        .unwrap();
    // The hole's loop is the one on the circle.
    let hole = s.faces[k]
        .loops
        .iter()
        .position(|l| match l {
            Loop::Edges(u) => matches!(s.curves[s.edges[u[0].edge].curve], Curve::Circle(_)),
            Loop::Vertex(_) => false,
        })
        .unwrap();
    (k, hole)
}

fn reverse_loop(s: &mut Shape, k: usize, l: usize) {
    let Loop::Edges(uses) = &mut s.faces[k].loops[l] else {
        panic!()
    };
    uses.reverse();
    for u in uses {
        u.sense = u.sense.flipped();
    }
}

#[test]
fn a_loop_run_against_its_face_is_reversed() {
    let b = plate();
    let s = plain(&b);
    let (k, hole) = holed_face(&s);
    let names = names::names(OpKey(7), &s);
    for l in [hole, 1 - hole] {
        let mut t = s.clone();
        reverse_loop(&mut t, k, l);
        let (h, journal, report) = heal_ok(&t);
        assert_eq!(h.counts(), b.counts());
        assert_eq!(report.reversed, [names.faces[k]]);
        assert!(journal.events().contains(&Event::Modified(names.faces[k])));
    }
    // Loops the file runs the right way are left alone.
    assert_eq!(heal_ok(&s).2.reversed, []);
}

#[test]
fn loops_that_cross_have_no_direction() {
    use crate::geom::Circle;
    let b = plate();
    let mut s = plain(&b);
    let (k, hole) = holed_face(&s);
    // The hole's circle moved over the plate's edge: it reads as an outer
    // loop running clockwise.
    let Loop::Edges(uses) = &s.faces[k].loops[hole] else {
        panic!()
    };
    let e = uses[0].edge;
    let Curve::Circle(c) = s.curves[s.edges[e].curve].clone() else {
        panic!()
    };
    let moved = crate::geom::Frame::new(
        c.frame().origin() + Vector3::new(-1.3, 0.0, 0.0),
        c.frame().z().get(),
        c.frame().x().get(),
    )
    .unwrap();
    s.curves
        .push(Curve::Circle(Circle::new(moved, c.radius()).unwrap()));
    s.edges[e].curve = s.curves.len() - 1;
    let names = names::names(OpKey(7), &s);
    assert_eq!(
        heal(OpKey(7), &s, &HealOptions::default()).unwrap_err(),
        HealError::SelfCrossingLoop {
            face: names.faces[k]
        }
        .into()
    );
}

/// Face `k` of a shape turned over: its sense and every loop reversed.
fn turn_face(s: &mut Shape, k: usize) {
    s.faces[k].sense = s.faces[k].sense.flipped();
    for l in 0..s.faces[k].loops.len() {
        if matches!(s.faces[k].loops[l], Loop::Edges(_)) {
            reverse_loop(s, k, l);
        }
    }
}

#[test]
fn faces_against_their_neighbours_are_turned_over() {
    let b = plate();
    let s = plain(&b);
    let names = names::names(OpKey(7), &s);
    // Every face but the least-named, one at a time.
    let least = (0..s.faces.len()).min_by_key(|&k| names.faces[k]).unwrap();
    for k in (0..s.faces.len()).filter(|&k| k != least) {
        let mut t = s.clone();
        turn_face(&mut t, k);
        let names = names::names(OpKey(7), &t);
        let (h, journal, report) = heal_ok(&t);
        assert_eq!(h.counts(), b.counts());
        assert_eq!(report.turned, [names.faces[k]]);
        assert!(journal.events().contains(&Event::Modified(names.faces[k])));
    }
}

/// A block 4 m on a side with a ball of radius 1 cut out of its middle.
fn hollow() -> Body {
    use crate::boolean::{BooleanOp, Tool, ToolKey, boolean};
    let outer = primitive::block(OpKey(1), 4.0, 4.0, 4.0, &Rigid::IDENTITY)
        .unwrap()
        .0;
    let inner = primitive::sphere(OpKey(2), 1.0, &Rigid::IDENTITY)
        .unwrap()
        .0;
    boolean(
        OpKey(3),
        BooleanOp::Subtract,
        &outer,
        &[Tool {
            key: ToolKey(1),
            body: &inner,
            place: Rigid::translation(Vector3::new(2.0, 2.0, 2.0)),
        }],
    )
    .unwrap()
    .0
}

/// Two shapes as one, each keeping its own shells and solids.
fn joined(a: &Shape, b: &Shape) -> Shape {
    let mut s = a.clone();
    let (ns, nc, nc2) = (s.surfaces.len(), s.curves.len(), s.curves2.len());
    let (nv, ne, nf, nsh) = (
        s.vertices.len(),
        s.edges.len(),
        s.faces.len(),
        s.shells.len(),
    );
    s.surfaces.extend(b.surfaces.iter().cloned());
    s.curves.extend(b.curves.iter().cloned());
    s.curves2.extend(b.curves2.iter().cloned());
    s.vertices.extend(b.vertices.iter().copied());
    s.edges.extend(b.edges.iter().map(|e| Edge {
        curve: e.curve + nc,
        start: e.start + nv,
        end: e.end + nv,
        ..*e
    }));
    s.faces.extend(b.faces.iter().map(|f| {
        Face {
            surface: f.surface + ns,
            loops: f
                .loops
                .iter()
                .map(|l| match l {
                    Loop::Edges(list) => Loop::Edges(
                        list.iter()
                            .map(|u| Use {
                                edge: u.edge + ne,
                                pcurve: u.pcurve.map(|c| c + nc2),
                                ..*u
                            })
                            .collect(),
                    ),
                    Loop::Vertex(v) => Loop::Vertex(v + nv),
                })
                .collect(),
            ..f.clone()
        }
    }));
    s.shells.extend(b.shells.iter().map(|sh| Shell {
        faces: sh.faces.iter().map(|f| f + nf).collect(),
        closed: sh.closed,
    }));
    s.solids.extend(b.solids.iter().map(|so| Solid {
        outer: so.outer + nsh,
        voids: so.voids.iter().map(|v| v + nsh).collect(),
    }));
    s
}

#[test]
fn a_shell_inside_out_is_turned_over() {
    let b = primitive::block(OpKey(1), 1.0, 2.0, 3.0, &tilted())
        .unwrap()
        .0;
    let mut s = plain(&b);
    for k in 0..s.faces.len() {
        turn_face(&mut s, k);
    }
    let names = names::names(OpKey(7), &s);
    let (h, _, report) = heal_ok(&s);
    assert_eq!(h.counts(), b.counts());
    let mut all = names.faces.clone();
    all.sort();
    assert_eq!(report.turned, all);
}

#[test]
fn a_cavity_facing_out_is_turned_into_it() {
    let b = hollow();
    let mut s = plain(&b);
    let ball = (0..s.faces.len())
        .find(|&k| matches!(s.surfaces[s.faces[k].surface], Surface::Sphere(_)))
        .unwrap();
    turn_face(&mut s, ball);
    let names = names::names(OpKey(7), &s);
    let (h, _, report) = heal_ok(&s);
    assert_eq!(h.counts(), b.counts());
    assert_eq!(report.turned, [names.faces[ball]]);
}

#[test]
fn cavities_follow_geometry_not_the_file() {
    let b = hollow();
    let s = plain(&b);
    let solid = s.solids[0].clone();
    // The cavity named the outer shell, and the shells as two solids.
    let mut swapped = s.clone();
    swapped.solids = vec![Solid {
        outer: solid.voids[0],
        voids: vec![solid.outer],
    }];
    let mut apart = s.clone();
    apart.solids = vec![
        Solid {
            outer: solid.outer,
            voids: vec![],
        },
        Solid {
            outer: solid.voids[0],
            voids: vec![],
        },
    ];
    for t in [swapped, apart] {
        let (h, _, report) = heal_ok(&t);
        assert_eq!(h.counts(), b.counts());
        assert_eq!(report.turned, []);
    }
}

#[test]
fn a_solid_in_a_cavity_is_a_solid_of_its_own() {
    let core = primitive::block(
        OpKey(4),
        0.4,
        0.4,
        0.4,
        &Rigid::translation(Vector3::new(1.8, 1.8, 1.8)),
    )
    .unwrap()
    .0;
    let s = joined(&plain(&hollow()), &plain(&core));
    let (h, _, _) = heal_ok(&s);
    // The outer void region, the hollow block and the core.
    assert_eq!(h.counts().regions, 3);
    assert_eq!(h.counts().shells, 3);
    let mut shells: Vec<usize> = h
        .regions()
        .skip(1)
        .map(|r| h.region_shells(r).count())
        .collect();
    shells.sort();
    assert_eq!(shells, [1, 2]);
}

#[test]
fn shells_that_cross_are_refused() {
    let a = primitive::block(OpKey(1), 4.0, 4.0, 4.0, &Rigid::IDENTITY)
        .unwrap()
        .0;
    let b = primitive::block(
        OpKey(2),
        4.0,
        4.0,
        4.0,
        &Rigid::translation(Vector3::new(2.0, 1.5, 1.3)),
    )
    .unwrap()
    .0;
    let s = joined(&plain(&a), &plain(&b));
    let r = heal(OpKey(7), &s, &HealOptions::default());
    assert!(
        matches!(r, Err(Error::InvalidHeal(HealError::ShellsCross { a, b })) if a < b),
        "{r:?}"
    );
}
