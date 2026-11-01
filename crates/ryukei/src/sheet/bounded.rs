//! A surface bounded by a box of its parameters, as a sheet of one face
//! (design 295, plan 12.B).
//!
//! The face's sides run along the box's parameter lines: `v = v0` and
//! `v = v1` along `u`, `u = u0` and `u = u1` along `v`. A direction taken
//! whole round its period has no sides (no seams, as everywhere), so the
//! other direction's sides are closed edges; a side on a row the surface
//! collapses to a point (a pole, a cone's apex) is no edge, its two
//! corners one vertex. A cone's apex left alone by a whole turn is a lone
//! vertex of the face.

use crate::error::{Error, SheetError};
use crate::geom::{Circle, Curve, Frame, Line, Rigid, Surface};
use crate::heal::seamed::{iso, iso2};
use crate::layout::{self, Boundary, Layout, Use};
use crate::mesh::uv::collapsed_rows;
use crate::name::{Event, Journal, Name, OpKey, Role, SheetRole};
use crate::tol::LINEAR_RESOLUTION;
use crate::topo::{Body, EntityId, Sense};
use crate::{TopologyError, check};
use std::sync::Arc;

/// The names' recipe kind.
const KIND: &str = "surface";

/// The sides in loop order: role, the parameter they hold (`0` is `u`),
/// the box's end they hold it at, the corners they run between (each
/// `[u end, v end]`) and the sense the loop runs them.
const SIDES: [(SheetRole, usize, usize, [[usize; 2]; 2], Sense); 4] = [
    (SheetRole::SideV0, 1, 0, [[0, 0], [1, 0]], Sense::Forward),
    (SheetRole::SideU1, 0, 1, [[1, 0], [1, 1]], Sense::Forward),
    (SheetRole::SideV1, 1, 1, [[0, 1], [1, 1]], Sense::Reversed),
    (SheetRole::SideU0, 0, 0, [[0, 0], [0, 1]], Sense::Reversed),
];

/// `surface` bounded by `rect` (`[[u0, u1], [v0, v1]]`) as a sheet of one
/// face, its front the surface's natural normal, and the journal of what
/// it made (everything `Created`).
///
/// Each direction's range must be finite and increasing; a periodic one
/// at most a period long (a period, within a part in 10¹², is the whole
/// direction), a bounded one inside the domain. A whole surface must
/// close up (a sphere, a torus). Otherwise `SheetError::BadRect`. Sides
/// are exact parameter lines where the surface has them (planes,
/// cylinders, cones, spheres, tori, revolutions, extrusions, NURBS) and
/// otherwise fitted along the line within a certified bound, the edge's
/// tolerance (`SheetError::Uncertain` where that fails). Names come from
/// `key` alone ([`SheetRole`]), so a new box renames nothing.
pub fn from_surface(
    key: OpKey,
    surface: &Surface,
    rect: [[f64; 2]; 2],
) -> Result<(Body, Journal), Error> {
    let domain = surface.domain();
    let (pu, pv) = surface.periods();
    let periods = [pu, pv];
    let mut whole = [false; 2];
    for d in 0..2 {
        let [a, b] = rect[d];
        if !(a.is_finite() && b.is_finite() && a < b) {
            return Err(SheetError::BadRect.into());
        }
        match periods[d] {
            Some(p) => {
                let slack = 1e-12 * p;
                if b - a > p + slack {
                    return Err(SheetError::BadRect.into());
                }
                whole[d] = b - a >= p - slack;
            }
            None if a < domain[d][0] || b > domain[d][1] => {
                return Err(SheetError::BadRect.into());
            }
            None => {}
        }
    }
    // A side on a collapsed row of the domain is a point.
    let rows = collapsed_rows(surface);
    let pole = |d: usize, e: usize| rows[d][e] && rect[d][e] == domain[d][e];
    // The corners, joined where a whole direction or a pole makes two one.
    let mut class = [[0usize; 2]; 2];
    for (i, row) in class.iter_mut().enumerate() {
        for (j, c) in row.iter_mut().enumerate() {
            *c = 2 * i + j;
        }
    }
    let join = |a: [usize; 2], b: [usize; 2], class: &mut [[usize; 2]; 2]| {
        let (x, y) = (class[a[0]][a[1]], class[b[0]][b[1]]);
        for row in class.iter_mut() {
            for c in row.iter_mut() {
                if *c == y {
                    *c = x;
                }
            }
        }
    };
    for e in 0..2 {
        if whole[0] || pole(1, e) {
            join([0, e], [1, e], &mut class);
        }
        if whole[1] || pole(0, e) {
            join([e, 0], [e, 1], &mut class);
        }
    }
    let corner = |[i, j]: [usize; 2]| surface.point(rect[0][i], rect[1][j]);
    let mut out = Layout::default();
    let mut vertex_of: Vec<Option<usize>> = vec![None; 4];
    let mut vertex = |c: [usize; 2], out: &mut Layout| -> usize {
        let k = class[c[0]][c[1]];
        *vertex_of[k].get_or_insert_with(|| {
            out.vertices.push(layout::Vertex {
                point: corner(c),
                tolerance: LINEAR_RESOLUTION,
            });
            out.vertices.len() - 1
        })
    };
    out.surfaces.push(surface.clone());
    // Each side that is an edge: its role, layout edge and use.
    let mut sides: Vec<(SheetRole, [usize; 2], Use)> = Vec::new();
    for &(role, j, end, [from, to], sense) in &SIDES {
        // A side holding `u` runs along `v`. It is no edge where `u` is
        // whole (it would be a seam) or the side is a pole; likewise the
        // other way.
        let along = 1 - j;
        if whole[j] || pole(j, end) {
            continue;
        }
        let c = rect[j][end];
        let range = rect[along];
        let (curve, tolerance) = side_curve(surface, j, c, range)?;
        let (start, finish) = (vertex(from, &mut out), vertex(to, &mut out));
        out.curves.push(curve);
        out.curves2.push(iso2(j, c));
        out.edges.push(layout::Edge {
            start,
            end: finish,
            curve: out.curves.len() - 1,
            interval: range,
            tolerance: tolerance.max(LINEAR_RESOLUTION),
        });
        let u = Use {
            edge: out.edges.len() - 1,
            sense,
            pcurve: Some((out.curves2.len() - 1, [0.0, 1.0])),
        };
        sides.push((role, [class[from[0]][from[1]], class[to[0]][to[1]]], u));
    }
    let mut loops = Vec::new();
    match whole {
        [false, false] => loops.push(Boundary::Arcs(sides.iter().map(|s| s.2).collect())),
        [true, true] => {}
        _ => loops.extend(sides.iter().map(|s| Boundary::Arcs(vec![s.2]))),
    }
    // A cone's apex where a whole turn leaves no side through it.
    let mut apex = None;
    if whole[0] && matches!(surface, Surface::Cone(_)) && pole(1, 0) {
        let v = vertex([0, 0], &mut out);
        loops.push(Boundary::Lone(v));
        apex = Some(v);
    }
    if loops.is_empty() && surface.closed_genus().is_none() {
        return Err(SheetError::BadRect.into());
    }
    out.faces.push(layout::Face {
        surface: 0,
        sense: Sense::Forward,
        loops,
    });
    let built = layout::build_sheet(&out, key, Role::Merged)
        .map_err(|_| Error::Topology(TopologyError::Inconsistent))?;
    let mut b = built.body;
    b.set_recipes(true);
    let role = |r: SheetRole| Role::Sheet(r);
    let face = b.derive_name(KIND, key, role(SheetRole::Face), &[]);
    b.rename(EntityId::Face(built.faces[0]), face);
    let mut side_names = Vec::new();
    for &(r, _, u) in &sides {
        let name = b.derive_name(KIND, key, role(r), &[face]);
        b.rename(EntityId::Edge(built.edges[&u.edge]), name);
        side_names.push(name);
    }
    // A corner is named by the side the loop arrives on and the side it
    // leaves by (one side for a closed edge's vertex).
    let ends = |&(_, [from, to], u): &(SheetRole, [usize; 2], Use)| match u.sense {
        Sense::Forward => (from, to),
        Sense::Reversed => (to, from),
    };
    for (&g, &v) in &built.vertices {
        let k = vertex_of.iter().position(|&x| x == Some(g));
        let parents: Vec<Name> = if Some(g) == apex {
            vec![face]
        } else {
            let arriving = sides.iter().position(|s| Some(ends(s).1) == k);
            let leaving = sides.iter().position(|s| Some(ends(s).0) == k);
            let mut p: Vec<Name> = [arriving, leaving]
                .into_iter()
                .flatten()
                .map(|i| side_names[i])
                .collect();
            p.dedup();
            p
        };
        let name = b.derive_name(KIND, key, role(SheetRole::Corner), &parents);
        b.rename(EntityId::Vertex(v), name);
    }
    crate::pcurve::fill(&mut b)?;
    let mut journal = Journal::new();
    for f in b.faces() {
        journal.push(Event::Created(b.faces[f].name));
    }
    for e in b.edges() {
        journal.push(Event::Created(b.edges[e].name));
    }
    for v in b.vertices() {
        journal.push(Event::Created(b.vertices[v].name));
    }
    if cfg!(debug_assertions) {
        let faults = check::check(&b);
        assert!(
            faults.is_empty(),
            "a bounded surface is invalid: {faults:?}"
        );
    }
    Ok((b, journal))
}

/// The parameter line `j = c` over `range`, running with the other
/// parameter, and its tolerance: exact where the surface has it, else
/// fitted along the line.
fn side_curve(surface: &Surface, j: usize, c: f64, range: [f64; 2]) -> Result<(Curve, f64), Error> {
    if let Some(curve) = exact(surface, j, c) {
        return Ok((curve, 0.0));
    }
    if let Some(found) = iso(surface, j, c, range) {
        return Ok(found);
    }
    let lifted = crate::intersect::fit_along(surface, &iso2(j, c), [0.0, 1.0], range)
        .map_err(|_| SheetError::Uncertain)?;
    Ok((Curve::Nurbs(Arc::new(lifted.curve)), lifted.tolerance))
}

/// The exact parameter lines healing's seams never need: a plane's both
/// ways, a turned surface's circles of latitude, an extrusion's moved
/// curve.
fn exact(surface: &Surface, j: usize, c: f64) -> Option<Curve> {
    match (surface, j) {
        (Surface::Plane(p), _) => {
            let f = p.frame();
            let (o, d) = if j == 0 {
                (f.to_world(c, 0.0, 0.0), f.y())
            } else {
                (f.to_world(0.0, c, 0.0), f.x())
            };
            Line::from_unit(o, d).ok().map(Curve::Line)
        }
        (Surface::Cylinder(_) | Surface::Cone(_) | Surface::Sphere(_), 1) => {
            let f = surface.frame()?;
            let d = surface.point(0.0, c) - f.origin();
            let (rho, h) = (d.dot(f.x().get()), d.dot(f.z().get()));
            let frame = Frame::from_axes(f.origin() + f.z().get() * h, f.z(), f.x()).ok()?;
            Circle::new(frame, rho).ok().map(Curve::Circle)
        }
        (Surface::Extrusion(e), 1) => Some(
            e.curve()
                .transformed(&Rigid::translation(e.dir().get() * c)),
        ),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::check::{check, check_intersections};
    use crate::geom::{Cone, Cylinder, Plane, Sphere, Torus};
    use crate::topo::BodyKind;
    use core::f64::consts::{FRAC_PI_2, PI, TAU};

    const K: OpKey = OpKey(0x5355);

    fn frame() -> Frame {
        Frame::WORLD
    }

    fn counts(b: &Body) -> (usize, usize, usize) {
        (b.faces().count(), b.edges().count(), b.vertices().count())
    }

    fn sound(b: &Body) {
        assert_eq!(b.kind(), BodyKind::Sheet);
        assert_eq!(check(b), []);
        assert_eq!(check_intersections(b), []);
    }

    #[test]
    fn a_plane_rectangle() {
        let s = Surface::Plane(Plane::new(frame()));
        let (b, j) = from_surface(K, &s, [[0.0, 2.0], [-1.0, 1.0]]).unwrap();
        sound(&b);
        assert_eq!(counts(&b), (1, 4, 4));
        assert!(b.edges().all(|e| b.is_laminar(e)));
        assert_eq!(j.events().len(), 9);
        let face = Name::derive(K, Role::Sheet(SheetRole::Face), &[]);
        assert!(matches!(b.by_name(face), Some(EntityId::Face(_))));
        let side = Name::derive(K, Role::Sheet(SheetRole::SideU1), &[face]);
        assert!(matches!(b.by_name(side), Some(EntityId::Edge(_))));
        // A bigger box renames nothing.
        let (c, _) = from_surface(K, &s, [[0.0, 3.0], [-1.0, 5.0]]).unwrap();
        let names = |b: &Body| {
            let mut n: Vec<Name> = b.vertices().map(|v| b.vertices[v].name).collect();
            n.extend(b.edges().map(|e| b.edges[e].name));
            n.sort();
            n
        };
        assert_eq!(names(&b), names(&c));
    }

    #[test]
    fn a_cylinder_band_and_a_strip() {
        let s = Surface::Cylinder(Cylinder::new(frame(), 0.5).unwrap());
        let (b, _) = from_surface(K, &s, [[0.0, TAU], [0.0, 1.0]]).unwrap();
        sound(&b);
        assert_eq!(counts(&b), (1, 2, 2));
        let (c, _) = from_surface(K, &s, [[0.5, 2.0], [0.0, 1.0]]).unwrap();
        sound(&c);
        assert_eq!(counts(&c), (1, 4, 4));
    }

    #[test]
    fn poles_and_apexes() {
        let ball = Surface::Sphere(Sphere::new(frame(), 1.0).unwrap());
        // A lune from pole to pole.
        let (b, _) = from_surface(K, &ball, [[0.0, PI], [-FRAC_PI_2, FRAC_PI_2]]).unwrap();
        sound(&b);
        assert_eq!(counts(&b), (1, 2, 2));
        // The northern hemisphere: one circle, no vertex at the pole.
        let (h, _) = from_surface(K, &ball, [[0.0, TAU], [0.0, FRAC_PI_2]]).unwrap();
        sound(&h);
        assert_eq!(counts(&h), (1, 1, 1));
        // The whole ball, closed.
        let (w, _) = from_surface(K, &ball, [[0.0, TAU], [-FRAC_PI_2, FRAC_PI_2]]).unwrap();
        sound(&w);
        assert_eq!(counts(&w), (1, 0, 0));
        assert!(w.edges().next().is_none());
        // A cone from its apex: the apex a lone vertex.
        let cone = Surface::Cone(Cone::new(frame(), 0.5, 0.4).unwrap());
        let Surface::Cone(c) = &cone else {
            unreachable!()
        };
        let apex = c.apex_v();
        let (k, _) = from_surface(K, &cone, [[0.0, TAU], [apex, 1.0]]).unwrap();
        sound(&k);
        assert_eq!(counts(&k), (1, 1, 2));
        // A torus closed both ways.
        let t = Surface::Torus(Torus::new(frame(), 2.0, 0.5).unwrap());
        let (t, _) = from_surface(K, &t, [[0.0, TAU], [0.0, TAU]]).unwrap();
        sound(&t);
        assert_eq!(counts(&t), (1, 0, 0));
    }

    #[test]
    fn every_kind_meshes_and_splines_take_parameter_lines() {
        use crate::mesh::{MeshParams, check_mesh, tessellate};
        let cyl = Surface::Cylinder(Cylinder::new(frame(), 0.5).unwrap());
        let (n, _) = cyl.to_nurbs([[0.0, PI], [0.0, 1.0]]).unwrap();
        let spline = Surface::Nurbs(Arc::new(n));
        let ball = Surface::Sphere(Sphere::new(frame(), 1.0).unwrap());
        let cone = Surface::Cone(Cone::new(frame(), 0.5, 0.4).unwrap());
        let apex = match &cone {
            Surface::Cone(c) => c.apex_v(),
            _ => unreachable!(),
        };
        let cases = [
            (spline.clone(), spline.domain()),
            (cyl, [[0.0, TAU], [0.0, 1.0]]),
            (ball.clone(), [[0.0, PI], [-FRAC_PI_2, FRAC_PI_2]]),
            (ball, [[0.0, TAU], [0.0, FRAC_PI_2]]),
            (cone, [[0.0, TAU], [apex, 1.0]]),
        ];
        for (surface, rect) in cases {
            let (b, _) = from_surface(K, &surface, rect).unwrap();
            sound(&b);
            let mesh = tessellate(&b, &MeshParams::new(0.01, 0.5)).unwrap();
            assert!(mesh.faults.is_empty(), "{:?}", mesh.faults);
            assert_eq!(check_mesh(&b, &mesh), Ok(()));
        }
        // The spline's sides are its own parameter lines (it turns whole,
        // so it has the two along `u`).
        let (b, _) = from_surface(K, &spline, spline.domain()).unwrap();
        assert_eq!(spline.periods().0, Some(TAU));
        assert_eq!(counts(&b), (1, 2, 2));
        assert!(
            b.edges()
                .all(|e| matches!(b.curves[b.edges[e].curve], Curve::Nurbs(_)))
        );
    }

    #[test]
    fn bad_boxes() {
        let s = Surface::Cylinder(Cylinder::new(frame(), 0.5).unwrap());
        let bad = |r| from_surface(K, &s, r).unwrap_err();
        let want: Error = SheetError::BadRect.into();
        assert_eq!(bad([[0.0, TAU + 0.1], [0.0, 1.0]]), want);
        assert_eq!(bad([[1.0, 1.0], [0.0, 1.0]]), want);
        assert_eq!(bad([[0.0, 1.0], [0.0, f64::NAN]]), want);
        let ball = Surface::Sphere(Sphere::new(frame(), 1.0).unwrap());
        assert_eq!(
            from_surface(K, &ball, [[0.0, 1.0], [0.0, 2.0]]).unwrap_err(),
            want
        );
    }
}
