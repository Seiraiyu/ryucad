use super::*;
use crate::check::check_topology;
use crate::geom::{Rigid, Vector3};
use crate::heal::seamed;
use crate::heal::tests::plain;
use crate::heal::tests::{holed_face, plate, reverse_loop, turn_face};
use crate::heal::{HealOptions, heal};
use crate::mesh::{MeshParams, tessellate};
use crate::primitive;
use crate::topo::BodyKind;
use std::collections::BTreeSet;

pub(super) fn tilted() -> Rigid {
    Rigid::rotation(Vector3::new(1.0, 2.0, 0.5).normalize().unwrap(), 0.7)
}

/// A reference of `s` under key 7, checked as every reference must be:
/// its topology sound, a sheet, every face meshed.
pub(super) fn reference_ok(s: &Shape) -> (Body, ReferenceReport) {
    let (b, report) = reference(OpKey(7), s, &ReferenceOptions::default()).unwrap();
    assert_eq!(check_topology(&b), []);
    assert_eq!(b.kind(), BodyKind::Sheet);
    let m = tessellate(&b, &MeshParams::new(1e-2, 0.5)).unwrap();
    assert_eq!(m.faults, []);
    (b, report)
}

/// A body's counts but its regions: a sheet has the outer void alone,
/// a solid that and its solid region.
pub(super) fn counts(b: &Body) -> crate::topo::Counts {
    crate::topo::Counts {
        regions: 0,
        ..b.counts()
    }
}

pub(super) fn face_names(b: &Body) -> BTreeSet<Name> {
    b.faces().map(|f| b.faces[f].name).collect()
}

#[test]
fn a_block_builds_as_a_closed_sheet_named_as_healing_names_it() {
    let b = primitive::block(OpKey(1), 1.0, 2.0, 3.0, &tilted())
        .unwrap()
        .0;
    let s = plain(&b);
    let (r, report) = reference_ok(&s);
    assert_eq!(counts(&r), counts(&b));
    // Closed: no edge is laminar.
    assert_eq!(report.laminar, []);
    assert_eq!(report, ReferenceReport::default());
    let (h, _, _) = heal(OpKey(7), &s, &HealOptions::default()).unwrap();
    assert_eq!(face_names(&r), face_names(&h));
    // Every edge and vertex too: nothing was sewn or joined.
    let names = |b: &Body| -> BTreeSet<Name> {
        b.edges()
            .map(|e| b.edges[e].name)
            .chain(b.vertices().map(|v| b.vertices[v].name))
            .collect()
    };
    assert_eq!(names(&r), names(&h));
}

#[test]
fn an_open_shell_is_an_open_sheet_with_its_boundary_laminar() {
    let b = primitive::block(OpKey(1), 1.0, 2.0, 3.0, &Rigid::IDENTITY)
        .unwrap()
        .0;
    let mut s = plain(&b);
    let gone = s.faces.len() - 1;
    let Loop::Edges(uses) = &s.faces[gone].loops[0] else {
        panic!()
    };
    let names = super::names::names(OpKey(7), &s);
    let mut free: Vec<Name> = uses.iter().map(|u| names.edges[u.edge]).collect();
    free.sort();
    // The last face taken out of its shell and the shape.
    s.faces.pop();
    s.shells[0].faces.retain(|&f| f != gone);
    let (r, report) = reference_ok(&s);
    assert_eq!(r.faces().count(), 5);
    assert_eq!(report.laminar, free);
}

#[test]
fn malformed_shapes_are_refused_as_healing_refuses_them() {
    let mut s = Shape::default();
    s.vertices.push(super::super::Vertex {
        point: crate::geom::Point3::new(0.0, 0.0, 0.0),
        tolerance: -1.0,
        seam: false,
    });
    assert!(matches!(
        reference(OpKey(7), &s, &ReferenceOptions::default()),
        Err(Error::InvalidHeal(HealError::Malformed { .. }))
    ));
    // Nothing to lay out at all.
    assert!(matches!(
        reference(OpKey(7), &Shape::default(), &ReferenceOptions::default()),
        Err(Error::InvalidHeal(HealError::Malformed { .. }))
    ));
}

#[test]
fn a_files_seams_come_out_of_a_cylinder() {
    let b = primitive::cylinder(OpKey(1), 1.0, 2.0, &tilted())
        .unwrap()
        .0;
    let s = seamed(&b, 1e-7).unwrap();
    let (r, report) = reference_ok(&s);
    assert_eq!(r.faces().count(), b.faces().count());
    assert!(!report.seams.is_empty());
    let (h, _, healed) = heal(OpKey(7), &s, &HealOptions::default()).unwrap();
    assert_eq!(report.seams, healed.seams);
    assert_eq!(face_names(&r), face_names(&h));
}

#[test]
fn a_spheres_poles_go_with_its_seam() {
    let b = primitive::sphere(OpKey(1), 1.0, &tilted()).unwrap().0;
    let s = seamed(&b, 1e-7).unwrap();
    let (r, report) = reference_ok(&s);
    assert_eq!(r.faces().count(), 1);
    let (_, _, healed) = heal(OpKey(7), &s, &HealOptions::default()).unwrap();
    assert_eq!(report.seams, healed.seams);
    assert_eq!(report.poles, healed.poles);
}

#[test]
fn a_face_inside_out_is_turned_to_agree() {
    let b = plate();
    let s = plain(&b);
    let n = super::names::names(OpKey(7), &s);
    let least = (0..s.faces.len()).min_by_key(|&k| n.faces[k]).unwrap();
    let k = (0..s.faces.len()).find(|&k| k != least).unwrap();
    let mut t = s.clone();
    turn_face(&mut t, k);
    let n = super::names::names(OpKey(7), &t);
    let (r, report) = reference_ok(&t);
    assert_eq!(report.turned, [n.faces[k]]);
    assert_eq!(report.unoriented, []);
    assert_eq!(counts(&r), counts(&b));
}

#[test]
fn a_loop_run_against_its_face_is_reversed() {
    let b = plate();
    let s = plain(&b);
    let (k, hole) = holed_face(&s);
    let n = super::names::names(OpKey(7), &s);
    let mut t = s.clone();
    reverse_loop(&mut t, k, hole);
    let (_, report) = reference_ok(&t);
    assert_eq!(report.reversed, [n.faces[k]]);
}

#[test]
fn loops_that_cross_keep_the_files_orientation() {
    use crate::geom::{Circle, Curve, Frame};
    let b = plate();
    let mut s = plain(&b);
    let (k, hole) = holed_face(&s);
    // The hole's circle moved over the plate's edge, as healing's test
    // moves it: healing refuses `SelfCrossingLoop`.
    let Loop::Edges(uses) = &s.faces[k].loops[hole] else {
        panic!()
    };
    let e = uses[0].edge;
    let Curve::Circle(c) = s.curves[s.edges[e].curve].clone() else {
        panic!()
    };
    let moved = Frame::new(
        c.frame().origin() + Vector3::new(-1.3, 0.0, 0.0),
        c.frame().z().get(),
        c.frame().x().get(),
    )
    .unwrap();
    s.curves
        .push(Curve::Circle(Circle::new(moved, c.radius()).unwrap()));
    s.edges[e].curve = s.curves.len() - 1;
    let n = super::names::names(OpKey(7), &s);
    let (r, report) = reference(OpKey(7), &s, &ReferenceOptions::default()).unwrap();
    assert_eq!(check_topology(&r), []);
    assert!(report.unoriented.contains(&n.faces[k]));
    // Nothing else had to give: the piece laid out as the file has it.
    assert_eq!(report.dropped_faces, []);
    assert_eq!(r.faces().count(), b.faces().count());
}

#[test]
fn an_edge_of_three_uses_lays_its_piece_out_face_by_face() {
    let b = primitive::block(OpKey(1), 1.0, 2.0, 3.0, &tilted())
        .unwrap()
        .0;
    let mut s = plain(&b);
    // A copy of face 0 in the shell: every edge of face 0 now has three
    // uses, which healing refuses as `NonManifold`.
    let copy = s.faces[0].clone();
    s.faces.push(copy);
    let k = s.faces.len() - 1;
    s.shells[0].faces.push(k);
    let n = super::names::names(OpKey(7), &s);
    let (r, report) = reference_ok(&s);
    // Every face alone, none dropped, each keeping its name.
    assert_eq!(r.faces().count(), 7);
    let mut all = n.faces.clone();
    all.sort();
    assert_eq!(report.split_faces, all);
    assert_eq!(report.dropped_faces, []);
    assert_eq!(face_names(&r), all.iter().copied().collect());
    // Each face its own sheet: every edge laminar, 4 a face.
    assert_eq!(r.edges().count(), 28);
    assert_eq!(report.laminar.len(), 28);
    // Each name once, though 24 copies share 12 edges' names.
    let edge_names: BTreeSet<Name> = r.edges().map(|e| r.edges[e].name).collect();
    assert_eq!(edge_names.len(), 28);
    // The first copy of each edge keeps its import name.
    for g in 0..12 {
        assert!(edge_names.contains(&n.edges[g]), "edge {g} lost its name");
    }
}

#[test]
fn a_face_that_fails_alone_is_dropped() {
    let b = primitive::block(OpKey(1), 1.0, 2.0, 3.0, &Rigid::IDENTITY)
        .unwrap()
        .0;
    let mut s = plain(&b);
    let Loop::Edges(uses) = &mut s.faces[0].loops[0] else {
        panic!()
    };
    // One use of a loop against the rest, as healing's misoriented case:
    // the loop no longer runs head to tail, so face 0 can't be laid out
    // at all, and its piece can't be oriented.
    uses[0].sense = uses[0].sense.flipped();
    let n = super::names::names(OpKey(7), &s);
    let (r, report) = reference_ok(&s);
    assert_eq!(report.dropped_faces, [n.faces[0]]);
    assert_eq!(r.faces().count(), 5);
    assert!(report.unoriented.contains(&n.faces[0]));
    assert_eq!(report.split_faces.len(), 5);
}

#[test]
fn pieces_touching_at_a_vertex_each_get_a_copy_of_it() {
    // Two blocks sharing one corner point but no edge: two pieces.
    let a = primitive::block(OpKey(1), 1.0, 1.0, 1.0, &Rigid::IDENTITY)
        .unwrap()
        .0;
    let b = primitive::block(
        OpKey(2),
        1.0,
        1.0,
        1.0,
        &Rigid::translation(Vector3::new(1.0, 1.0, 1.0)),
    )
    .unwrap()
    .0;
    let mut s = plain(&a);
    let t = plain(&b);
    // `t` appended, its corner at (1, 1, 1) made `s`'s.
    let corner = s
        .vertices
        .iter()
        .position(|v| v.point == crate::geom::Point3::new(1.0, 1.0, 1.0))
        .unwrap();
    let theirs = t
        .vertices
        .iter()
        .position(|v| v.point == crate::geom::Point3::new(1.0, 1.0, 1.0))
        .unwrap();
    let (sv, se, sf, ss, sc) = (
        s.vertices.len(),
        s.edges.len(),
        s.faces.len(),
        s.surfaces.len(),
        s.curves.len(),
    );
    let map_v = |v: usize| if v == theirs { corner } else { v + sv };
    s.surfaces.extend(t.surfaces.iter().cloned());
    s.curves.extend(t.curves.iter().cloned());
    s.vertices.extend(t.vertices.iter().copied());
    for e in &t.edges {
        s.edges.push(super::super::Edge {
            curve: e.curve + sc,
            start: map_v(e.start),
            end: map_v(e.end),
            ..*e
        });
    }
    for f in &t.faces {
        let mut f = f.clone();
        f.surface += ss;
        for l in &mut f.loops {
            if let Loop::Edges(uses) = l {
                for u in uses {
                    u.edge += se;
                }
            }
        }
        s.faces.push(f);
    }
    s.shells.push(Shell {
        faces: (sf..s.faces.len()).collect(),
        closed: true,
    });
    s.solids.push(super::super::Solid {
        outer: s.shells.len() - 1,
        voids: Vec::new(),
    });
    let (r, report) = reference_ok(&s);
    assert_eq!(r.faces().count(), 12);
    assert_eq!(report.split_faces, []);
    // 8 + 8 vertices: the shared corner twice, under two names.
    let names: BTreeSet<Name> = r.vertices().map(|v| r.vertices[v].name).collect();
    assert_eq!(r.vertices().count(), 16);
    assert_eq!(names.len(), 16);
}

#[test]
fn a_gap_between_samples_is_covered() {
    use crate::geom::{Curve, Cylinder, Frame, KnotVector, NurbsCurve, Point3, Surface};
    use crate::topo::Sense;
    // A degree 1 spline through 17 points of a quarter circle on a
    // cylinder: on it at every one of the 17 even samples a gap once took,
    // off it by the sagitta halfway between (NIST FTC 10's splines).
    let n = 16;
    let r = 0.01;
    let step = core::f64::consts::FRAC_PI_2 / n as f64;
    let points: Vec<Point3> = (0..=n)
        .map(|i| {
            let (s, c) = crate::math::sin_cos(step * i as f64);
            Point3::new(r * c, r * s, 0.0)
        })
        .collect();
    let knots: Vec<f64> = (0..=n).map(|i| i as f64 / n as f64).collect();
    let mut mults = vec![1; n + 1];
    mults[0] = 2;
    mults[n] = 2;
    let kv = KnotVector::clamped(1, &knots, &mults).unwrap();
    let curve = Curve::Nurbs(std::sync::Arc::new(
        NurbsCurve::new(kv, points.clone(), None).unwrap(),
    ));
    let mut s = Shape {
        surfaces: vec![Surface::Cylinder(Cylinder::new(Frame::WORLD, r).unwrap())],
        curves: vec![curve],
        ..Shape::default()
    };
    for p in [points[0], points[n]] {
        s.vertices.push(super::super::Vertex {
            point: p,
            tolerance: 0.0,
            seam: false,
        });
    }
    s.edges.push(super::super::Edge {
        curve: 0,
        interval: [0.0, 1.0],
        start: 0,
        end: 1,
        tolerance: 0.0,
    });
    s.faces.push(super::super::Face {
        surface: 0,
        sense: Sense::Forward,
        loops: vec![Loop::Edges(vec![Use {
            edge: 0,
            sense: Sense::Forward,
            pcurve: None,
        }])],
        tolerance: 0.0,
    });
    let work = Work::unlimited();
    crate::work::run(&work, |m| {
        tolerate(&mut s, 1e-4, m);
        Ok(())
    })
    .unwrap();
    let sagitta = r * (1.0 - crate::math::cos(step / 2.0));
    assert!(
        s.edges[0].tolerance >= sagitta,
        "{} < {sagitta}",
        s.edges[0].tolerance
    );
}

/// #796: a face of no width between two coincident edges, as a file can
/// leave one where two faces meet: a block's edge split into two copies,
/// one for each of its faces, and a two-edge face along them in one
/// face's plane. It bounds nothing, so it meshes as no triangles rather
/// than as a fault drawn as a hole; every other face meshes as before.
#[test]
fn a_face_of_no_width_between_coincident_edges_meshes_empty() {
    use crate::mesh::check_mesh;
    let b = primitive::block(OpKey(1), 4.0, 3.0, 1.0, &Rigid::IDENTITY)
        .unwrap()
        .0;
    let mut s = plain(&b);
    let g = (0..s.edges.len())
        .find(|&g| matches!(s.curves[s.edges[g].curve], Curve::Line(_)))
        .unwrap();
    let mut uses = Vec::new();
    for (k, f) in s.faces.iter().enumerate() {
        for (l, lp) in f.loops.iter().enumerate() {
            if let Loop::Edges(us) = lp {
                for (j, u) in us.iter().enumerate() {
                    if u.edge == g {
                        uses.push((k, l, j, u.sense));
                    }
                }
            }
        }
    }
    let [(k1, _, _, sense1), (k2, l2, j2, sense2)] = uses[..] else {
        panic!("{uses:?}")
    };
    let copy = s.edges.len();
    s.edges.push(s.edges[g]);
    let Loop::Edges(us) = &mut s.faces[k2].loops[l2] else {
        unreachable!()
    };
    us[j2].edge = copy;
    let sliver = super::super::Face {
        surface: s.faces[k1].surface,
        sense: s.faces[k1].sense,
        loops: vec![Loop::Edges(vec![
            Use {
                edge: g,
                sense: sense1.flipped(),
                pcurve: None,
            },
            Use {
                edge: copy,
                sense: sense2.flipped(),
                pcurve: None,
            },
        ])],
        tolerance: 0.0,
    };
    s.faces.push(sliver);
    if let Some(sh) = s.shells.first_mut() {
        sh.faces.push(s.faces.len() - 1);
    }
    let (body, _) = reference(OpKey(7), &s, &ReferenceOptions::default()).unwrap();
    assert_eq!(body.faces().count(), 7);
    let mesh = tessellate(&body, &MeshParams::new(1e-2, 0.5)).unwrap();
    assert_eq!(mesh.faults, []);
    assert_eq!(check_mesh(&body, &mesh), Ok(()));
    assert_eq!(mesh.faces.len(), 7);
}
