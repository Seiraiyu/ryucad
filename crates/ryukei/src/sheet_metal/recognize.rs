//! Recognition: a solid read as sheet metal, or the reason it isn't.
//!
//! In order: cones met by planes along a ruling are conical bends; coaxial
//! cylinder pairs with their bend lines are bend pieces
//! ([`super::bends`]), which give the thickness (else the flat body's
//! widest pair of opposite planes); planes `t` from a partner meeting
//! another such plane at a sharp edge, or a lone cylinder rounding such a
//! corner, are sharp bends; faces are laid out into flange pieces and
//! walls ([`super::flanges`]); pieces on one cylinder joining the same two
//! flanges become one bend; each bend's walls must be square to its axis,
//! through it or certified radial ([`super::radial`]), the faces square to
//! it lying in each flange they cross; and the walk of the bend graph from
//! the fixed flange orients each bend and finds a loop ([`super::graph`]).

use super::bends::{self, Found, Piece};
use super::cells::Verdict;
use super::flanges::{self, Layout};
use super::graph::{self, Walk};
use super::radial::{self, Wall};
use super::shape::{self, Flat, across, edges_of, flat, neighbours, unwrap};
use super::slab;
use super::{Bend, BendDirection, Cycle, Flange, SheetModel, SheetOptions, Side};
use crate::geom::{Curve, Frame, Plane, Surface, UnitVector3};
use crate::name::Name;
use crate::tol::{ANGULAR_RESOLUTION, LINEAR_RESOLUTION};
use crate::topo::{Body, FaceId};
use crate::{Error, SheetMetalError};
use std::collections::{BTreeMap, BTreeSet};

/// The caller's tolerance, checked and held above resolution.
pub(super) fn tolerance(o: &SheetOptions) -> Result<f64, SheetMetalError> {
    if !o.tolerance.is_finite() {
        return Err(SheetMetalError::BadTolerance);
    }
    Ok(o.tolerance.max(LINEAR_RESOLUTION))
}

/// The body's size: the diagonal of its vertices' box, at least a metre's
/// millionth, for angles judged from lengths.
fn size(b: &Body) -> f64 {
    let pts = b.vertices().map(|v| b.vertices[v].point);
    crate::geom::Box3::from_points(pts).map_or(1e-6, |bx| (bx.max() - bx.min()).length().max(1e-6))
}

/// A bend before its flanges are named: its pieces and its flanges by
/// their parts, as the first piece's ends give them.
struct Group {
    pieces: Vec<usize>,
    parts: [usize; 2],
}

/// Recognizes `body` as sheet metal. See the module's documentation for
/// what it checks; refusals are `Error::InvalidSheetMetal`, naming faces,
/// edges and bends by name.
pub fn recognize(body: &Body, options: &SheetOptions) -> Result<SheetModel, Error> {
    Ok(read(body, options)?.model)
}

/// A recognized body: the model, and what unfolding reads beside it.
pub(super) struct Read {
    pub(super) model: SheetModel,
}

pub(super) fn read(b: &Body, options: &SheetOptions) -> Result<Read, SheetMetalError> {
    let tol = tolerance(options)?;
    let scale = size(b);
    let name = |f: FaceId| b.faces[f].name;
    conical(b)?;
    let found = bends::find(b, tol)?;
    let t = match piece_thickness(b, &found, tol)? {
        Some(t) => t,
        None => match flanges::flat_sides(b, tol, scale) {
            Some((.., d)) => d,
            None => {
                let face = b
                    .faces()
                    .map(name)
                    .min()
                    .ok_or(SheetMetalError::Disconnected)?;
                return Err(SheetMetalError::NotSheet { face });
            }
        },
    };
    sharp(b, &found, t, tol, scale)?;
    let layout = flanges::layout(b, &found, t, tol, scale)?;
    let groups = group(b, &found, &layout, tol)?;
    // Flanges by name.
    let mut order: Vec<usize> = (0..layout.parts.len()).collect();
    let part_name = |i: usize| {
        layout.parts[i].sides[0]
            .1
            .iter()
            .map(|&f| name(f))
            .min()
            .expect("a side face")
    };
    order.sort_by_key(|&i| part_name(i));
    let index: BTreeMap<usize, usize> = order.iter().enumerate().map(|(k, &i)| (i, k)).collect();
    let flange_faces: BTreeSet<FaceId> = layout.of_face.keys().copied().collect();
    // Bends by name, with their walls judged.
    let mut bends: Vec<(Name, &Group)> = groups
        .iter()
        .map(|g| {
            let n = g
                .pieces
                .iter()
                .flat_map(|&p| found.pieces[p].faces())
                .map(name)
                .min()
                .expect("a face");
            (n, g)
        })
        .collect();
    bends.sort_by_key(|(n, _)| *n);
    let mut wall_of: BTreeMap<FaceId, Vec<usize>> = BTreeMap::new();
    for (k, (_, g)) in bends.iter().enumerate() {
        for &p in &g.pieces {
            for &w in &layout.walls[p] {
                let list = wall_of.entry(w).or_default();
                if !list.contains(&k) {
                    list.push(k);
                }
            }
        }
    }
    for (&w, ks) in &wall_of {
        for &k in ks {
            let p = &found.pieces[bends[k].1.pieces[0]];
            let kind = radial::classify(b, w, &p.frame, t, tol);
            let alone = ks.len() == 1 && !neighbours(b, w).iter().any(|f| flange_faces.contains(f));
            match kind {
                Wall::Square => {}
                Wall::Through | Wall::Radial if alone => {}
                Wall::Unsure => return Err(SheetMetalError::Uncertain { at: name(w) }),
                _ => return Err(SheetMetalError::NotRadial { face: name(w) }),
            }
        }
        // A face square to a bend's axis runs on across flanges: there it
        // must lie in each one's slab.
        for e in edges_of(b, w) {
            let Some(g) = across(b, e, w) else { continue };
            let Some(&part) = layout.of_face.get(&g) else {
                continue;
            };
            let slab = layout.parts[part].slab(t);
            match slab::edge_within(b, e, &slab, tol) {
                Verdict::Yes => {}
                Verdict::No => return Err(SheetMetalError::OutOfSlab { face: name(w) }),
                Verdict::Unsure => return Err(SheetMetalError::Uncertain { at: name(w) }),
            }
        }
    }
    // The graph, walked from the flange holding the least-named side face.
    let ends: Vec<[usize; 2]> = bends
        .iter()
        .map(|(_, g)| g.parts.map(|p| index[&p]))
        .collect();
    let n = order.len();
    let w = graph::walk(n, &ends, 0);
    if w.reached.iter().any(|r| !r) {
        return Err(SheetMetalError::Disconnected);
    }
    let cycle = w.closing.map(|k| {
        let ks = graph::cycle(&w, &ends, k);
        let mut bends_in: Vec<Name> = ks.iter().map(|&j| bends[j].0).collect();
        bends_in.sort_unstable();
        let mut faces: Vec<Name> = bends[k]
            .1
            .pieces
            .iter()
            .flat_map(|&p| found.pieces[p].faces())
            .map(name)
            .collect();
        faces.sort_unstable();
        Cycle {
            bends: bends_in,
            faces,
        }
    });
    let flanges: Vec<Flange> = order
        .iter()
        .map(|&i| {
            let part = &layout.parts[i];
            let side = |(fl, fs): &(Flat, Vec<FaceId>)| {
                let mut faces: Vec<Name> = fs.iter().map(|&f| name(f)).collect();
                faces.sort_unstable();
                Side {
                    plane: plane(fl),
                    faces,
                }
            };
            let mut faces: Vec<Name> = part.faces.iter().map(|&f| name(f)).collect();
            faces.sort_unstable();
            Flange {
                name: part_name(i),
                faces,
                sides: [side(&part.sides[0]), side(&part.sides[1])],
            }
        })
        .collect();
    let mut out: Vec<Bend> = Vec::with_capacity(bends.len());
    for (k, (bend, g)) in bends.iter().enumerate() {
        let near = w.near[k].expect("every bend reached");
        out.push(oriented(b, &found, g, *bend, near, &index, &wall_of, k)?);
    }
    let mut model = SheetModel {
        thickness: t,
        tolerance: tol,
        flanges,
        bends: out,
        fixed: 0,
        cycle,
    };
    directions(&mut model, &w, 0);
    Ok(Read { model })
}

/// A planar face's plane with its outward normal as `z`.
fn plane(f: &Flat) -> Plane {
    let n = f.normal.get();
    let hint = if n.x.abs() < 0.9 {
        crate::geom::Vector3::new(1.0, 0.0, 0.0)
    } else {
        crate::geom::Vector3::new(0.0, 1.0, 0.0)
    };
    Plane::new(Frame::new(f.origin, n, hint).expect("a unit normal"))
}

/// Refuses a cone met by a plane along a ruling: a conical bend.
fn conical(b: &Body) -> Result<(), SheetMetalError> {
    for f in b.faces() {
        if !matches!(b.surfaces[b.faces[f].surface], Surface::Cone(_)) {
            continue;
        }
        for e in edges_of(b, f) {
            let line = matches!(b.curves[b.edges[e].curve], Curve::Line(_));
            if line && across(b, e, f).is_some_and(|g| flat(b, g).is_some()) {
                return Err(SheetMetalError::ConicalBend {
                    face: b.faces[f].name,
                });
            }
        }
    }
    Ok(())
}

/// The thickness every bend piece agrees on, if there are any.
fn piece_thickness(b: &Body, found: &Found, tol: f64) -> Result<Option<f64>, SheetMetalError> {
    let mut first: Option<(f64, FaceId)> = None;
    for p in &found.pieces {
        let t = p.outer_radius - p.radius;
        let eps = tol + shape::face_tolerance(b, p.inner);
        match first {
            None => first = Some((t, p.inner)),
            Some((t0, f0)) if (t - t0).abs() > eps => {
                let mut faces = [b.faces[f0].name, b.faces[p.inner].name];
                faces.sort_unstable();
                return Err(SheetMetalError::MixedThickness { faces });
            }
            Some(_) => {}
        }
    }
    Ok(first.map(|(t, _)| t))
}

/// Refuses sharp corners: an edge between two planes, each a side by
/// having a partner `t` behind it, whose planes aren't parallel; and a
/// lone cylinder rounding such a corner on one side only.
fn sharp(b: &Body, found: &Found, t: f64, tol: f64, scale: f64) -> Result<(), SheetMetalError> {
    let planar: Vec<(FaceId, Flat)> = b
        .faces()
        .filter_map(|f| flat(b, f).map(|p| (f, p)))
        .collect();
    let mut partnered = BTreeSet::new();
    for (i, (f, p)) in planar.iter().enumerate() {
        for (g, q) in &planar[i + 1..] {
            let eps = tol + shape::face_tolerance(b, *f).max(shape::face_tolerance(b, *g));
            if flanges::depth(p, q, eps, scale).is_some_and(|d| (d - t).abs() <= eps) {
                partnered.insert(*f);
                partnered.insert(*g);
            }
        }
    }
    let side = |f: FaceId| partnered.contains(&f);
    let bent = |p: &Flat, q: &Flat| !shape::parallel(p.normal, q.normal, ANGULAR_RESOLUTION);
    let mut worst: Option<Name> = None;
    let mut note = |n: Name| {
        if worst.is_none_or(|w| n < w) {
            worst = Some(n);
        }
    };
    for e in b.edges() {
        if let [f, g] = shape::faces_of(b, e).as_slice()
            && side(*f)
            && side(*g)
            && let (Some(p), Some(q)) = (flat(b, *f), flat(b, *g))
            && bent(&p, &q)
        {
            note(b.edges[e].name);
        }
    }
    for (_, ends) in &found.lone {
        let at = |k: usize| ends[k].iter().find(|l| side(l.side));
        if let (Some(l0), Some(l1)) = (at(0), at(1))
            && let (Some(p), Some(q)) = (flat(b, l0.side), flat(b, l1.side))
            && bent(&p, &q)
        {
            note(b.edges[l0.edge].name.min(b.edges[l1.edge].name));
        }
    }
    match worst {
        Some(edge) => Err(SheetMetalError::SharpBend { edge }),
        None => Ok(()),
    }
}

/// Pieces on one cylinder joining the same two flanges, as one bend each.
fn group(
    b: &Body,
    found: &Found,
    layout: &Layout,
    tol: f64,
) -> Result<Vec<Group>, SheetMetalError> {
    let part_at = |p: &Piece, end: usize| -> Result<usize, SheetMetalError> {
        let f = *p.sides(end, false).first().expect("a bend line");
        layout
            .of_face
            .get(&f)
            .copied()
            .ok_or(SheetMetalError::NotSheet {
                face: b.faces[p.inner].name,
            })
    };
    let mut out: Vec<Group> = Vec::new();
    for (i, p) in found.pieces.iter().enumerate() {
        let parts = [part_at(p, 0)?, part_at(p, 1)?];
        let eps = tol + shape::face_tolerance(b, p.inner);
        let mut joined = false;
        for g in &mut out {
            let q = &found.pieces[g.pieces[0]];
            let same_axis = shape::parallel(
                p.frame.z(),
                q.frame.z(),
                ANGULAR_RESOLUTION + eps / p.radius,
            ) && shape::axis_distance(&q.frame, p.frame.origin()) <= eps
                && (p.radius - q.radius).abs() <= eps
                && (p.angle() - q.angle()).abs() * p.radius <= eps;
            if !same_axis {
                continue;
            }
            // Where p's start lies in q's angles says which way round it is.
            let start = shape::angle_of(&q.frame, p.frame.origin() + p.frame.radial(p.range[0]));
            let aligned = (unwrap(start, q.range[0]) - q.range[0]).abs() * p.radius <= eps;
            let want = if aligned { parts } else { [parts[1], parts[0]] };
            if want == g.parts {
                g.pieces.push(i);
                joined = true;
                break;
            }
        }
        if !joined {
            out.push(Group {
                pieces: vec![i],
                parts,
            });
        }
    }
    Ok(out)
}

/// A bend oriented from its near flange: its axis frame with `x` towards
/// the near bend line and the angle running to the far one.
#[allow(clippy::too_many_arguments)]
fn oriented(
    b: &Body,
    found: &Found,
    g: &Group,
    name: Name,
    near: usize,
    index: &BTreeMap<usize, usize>,
    wall_of: &BTreeMap<FaceId, Vec<usize>>,
    k: usize,
) -> Result<Bend, SheetMetalError> {
    let n = |f: FaceId| b.faces[f].name;
    let p0 = &found.pieces[g.pieces[0]];
    let flipped = index[&g.parts[0]] != near;
    let (lo, hi) = (p0.range[0], p0.range[1]);
    let z = p0.frame.z().get();
    let (z, start) = if flipped { (-z, hi) } else { (z, lo) };
    let axis = Frame::new(p0.frame.origin(), z, p0.frame.radial(start))
        .map_err(|_| SheetMetalError::Uncertain { at: name })?;
    let mut inner = Vec::new();
    let mut outer = Vec::new();
    let mut lines = [Vec::new(), Vec::new()];
    for &i in &g.pieces {
        let p = &found.pieces[i];
        inner.push(n(p.inner));
        outer.extend(p.outer.iter().map(|&f| n(f)));
        // A piece the other way round from the first has its ends swapped.
        let s = shape::angle_of(&p0.frame, p.frame.origin() + p.frame.radial(p.range[0]));
        let aligned = (unwrap(s, lo) - lo).abs() < (unwrap(s, hi) - hi).abs();
        for l in &p.lines {
            let end = if aligned { l.end } else { 1 - l.end };
            let end = if flipped { 1 - end } else { end };
            lines[end].push(b.edges[l.edge].name);
        }
    }
    let mut walls: Vec<Name> = wall_of
        .iter()
        .filter(|(_, ks)| ks.contains(&k))
        .map(|(&f, _)| n(f))
        .collect();
    let [l0, l1] = &mut lines;
    for v in [&mut inner, &mut outer, &mut walls, l0, l1] {
        v.sort_unstable();
        v.dedup();
    }
    let parts = if flipped {
        [g.parts[1], g.parts[0]]
    } else {
        g.parts
    };
    Ok(Bend {
        name,
        inner,
        outer,
        walls,
        axis,
        radius: p0.radius,
        angle: hi - lo,
        flanges: parts.map(|p| index[&p]),
        lines,
        direction: BendDirection::Down,
    })
}

/// Each bend's direction, seen in the flat from the fixed flange's side
/// away from its first bend's centre ([`super::BendDirection`]): the
/// near flange's inner side (the one facing the axis) turned into the
/// flat faces the viewer when the bend folds up.
pub(super) fn directions(model: &mut SheetModel, w: &Walk, fixed: usize) {
    let ends: Vec<[usize; 2]> = model.bends.iter().map(|b| b.flanges).collect();
    let turns = graph::motions(model.flanges.len(), &ends, w, |k| {
        let bend = &model.bends[k];
        Some(graph::unbend(&bend.axis, bend.angle, 0.0))
    });
    let view = viewer(model, fixed);
    for bend in &mut model.bends {
        let near = bend.flanges[0];
        let inner = inner_side(&model.flanges[near], &bend.axis);
        let n = turns[near].apply_vector(inner.frame().z().get());
        bend.direction = if n.dot(view.get()) > 0.0 {
            BendDirection::Up
        } else {
            BendDirection::Down
        };
    }
}

/// The side of a flange facing a bend's axis: its inner side there.
pub(super) fn inner_side<'a>(f: &'a Flange, axis: &Frame) -> &'a Plane {
    let up = |s: &Side| (axis.origin() - s.plane.frame().origin()).dot(s.plane.frame().z().get());
    if up(&f.sides[0]) > up(&f.sides[1]) {
        &f.sides[0].plane
    } else {
        &f.sides[1].plane
    }
}

/// The normal towards the viewer of the flat: the fixed flange's outer
/// side at its first bend (the least-named one there), else its first
/// side.
pub(super) fn viewer(model: &SheetModel, fixed: usize) -> UnitVector3 {
    let f = &model.flanges[fixed];
    let first = model
        .bends
        .iter()
        .filter(|b| b.flanges.contains(&fixed))
        .min_by_key(|b| b.name);
    match first {
        Some(bend) => {
            let inner = inner_side(f, &bend.axis);
            -inner.frame().z()
        }
        None => f.sides[0].plane.frame().z(),
    }
}
