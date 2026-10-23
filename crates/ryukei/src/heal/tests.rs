use super::*;
use crate::geom::{Rigid, Vector3};
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
    use crate::boolean::{BooleanOp, Tool, ToolKey, boolean};
    let outer = primitive::block(OpKey(1), 4.0, 4.0, 4.0, &Rigid::IDENTITY)
        .unwrap()
        .0;
    let inner = primitive::sphere(OpKey(2), 1.0, &Rigid::IDENTITY)
        .unwrap()
        .0;
    let hollow = boolean(
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
    .0;
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
    uses[0].sense = uses[0].sense.flipped();
    let e = uses[0].edge;
    let names = names::names(OpKey(7), &s);
    assert_eq!(
        heal(OpKey(7), &s, &HealOptions::default()).unwrap_err(),
        HealError::Misoriented { at: names.edges[e] }.into()
    );
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
