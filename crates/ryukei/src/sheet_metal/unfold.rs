//! Unfolding: a recognized body mapped face for face onto its flat.
//!
//! The walk of the bend graph from the fixed flange orients each bend
//! (its axis frame's `x` towards the near flange) and gives each flange
//! its motion: the product of the unfolded bends between it and the fixed
//! one, each turning the far side flat about its axis and moving it on by
//! its developed length ([`super::graph::unbend`]). Every face, edge and
//! vertex then takes the map of the region it lies in: a flange's motion,
//! or an unfolded bend's strip map, `(φ, ρ, y) -> (φ ba / θ, ρ - r, y)` in
//! the bend's frame, carried on by the near flange's motion. Topology and
//! names are kept; only geometry is replaced:
//!
//! - a bend's cylinder faces become planes, the strip's sides; its walls
//!   square to the axis stay planes, those through it become planes square
//!   to the flat, and any other radial wall becomes the extrusion, along
//!   the flat's normal, of its edge on the inner cylinder;
//! - lines along the axis and radial lines stay lines with their
//!   parameters, arcs about the axis become lines (their parameters scaled
//!   by the neutral radius `ba / θ`), and any other edge on a bend face
//!   becomes its pcurve's affine image lifted into the strip
//!   ([`super::curve2`]);
//! - pcurves of faces and edges that moved rigidly together are kept; the
//!   rest are found again exactly ([`crate::pcurve::fill`]).
//!
//! The flat solid must pass [`crate::check::check`] and
//! [`crate::check::check_intersections`]; faces that cross or overlap are
//! `FlatOverlaps`.

use super::curve2::{self, Affine2};
use super::graph::{self, Walk};
use super::pattern;
use super::radial::{self, Wall};
use super::recognize::inner_side;
use super::shape::{self, angle_of, unwrap};
use super::{Allowance, FlatMap, SheetModel, Strip, UnfoldSpec, Unfolded};
use crate::check::{self, Fault};
use crate::geom::{Curve, Extrusion, Frame, Plane, Point3, Rigid, Surface};
use crate::name::{Event, Journal, Name, OpKey};
use crate::tol::{ANGULAR_RESOLUTION, Tolerance};
use crate::topo::{Arena, Body, EdgeId, EntityId, FaceId, Sense, VertexId};
use crate::{Error, SheetMetalError};
use core::f64::consts::FRAC_PI_2;
use rayon::prelude::*;
use std::collections::BTreeMap;
use std::sync::Arc;

/// A bend as this unfold sees it: oriented from its near flange.
#[derive(Clone, Debug)]
pub(super) struct Oriented {
    pub(super) name: Name,
    /// `z` along the axis, `x` towards the near flange's bend line.
    pub(super) axis: Frame,
    pub(super) radius: f64,
    pub(super) angle: f64,
    pub(super) near: usize,
    pub(super) far: usize,
    /// The developed length where it is unfolded.
    pub(super) length: Option<f64>,
    pub(super) allowance: Allowance,
}

impl Oriented {
    /// `ba / θ`, the neutral radius.
    fn neutral(&self) -> f64 {
        self.length.expect("an unfolded bend") / self.angle
    }

    /// An angle about the axis moved by whole turns into the bend's span,
    /// or as near it as a turn allows: the gap outside the span is split
    /// at its middle.
    fn spanned(&self, a: f64) -> f64 {
        unwrap(a, 0.5 * self.angle)
    }

    /// The bend's strip in its folded place: its origin on the inner
    /// cylinder at the near bend line, `x` across the strip, `y` along the
    /// axis, `z` out through the sheet.
    fn strip(&self, rho: f64) -> Frame {
        Frame::from_axes(
            self.axis.to_world(rho, 0.0, 0.0),
            self.axis.x(),
            self.axis.y(),
        )
        .expect("the axis frame's own axes")
    }
}

/// The map an entity takes: a flange's motion, or a bend's strip map.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Map {
    Rigid(usize),
    Strip(usize),
}

/// Where a face lies.
#[derive(Clone, Debug, PartialEq, Eq)]
enum Region {
    Flange(usize),
    Bend { k: usize, outer: bool },
    Wall(Vec<usize>),
}

/// Unfolds a recognized body: see the module's documentation. `key` names
/// nothing (unfolding keeps every name); it is taken so that every
/// operation that changes a body takes one.
pub fn unfold(
    key: OpKey,
    body: &Body,
    model: &SheetModel,
    spec: &UnfoldSpec,
) -> Result<Unfolded, Error> {
    let _ = key;
    Ok(run(body, model, spec)?)
}

fn face_named(b: &Body, n: Name) -> Result<FaceId, SheetMetalError> {
    match b.by_name(n) {
        Some(EntityId::Face(f)) => Ok(f),
        _ => Err(SheetMetalError::NoSuchFace { name: n }),
    }
}

/// The plan of an unfold: its bends oriented, its flanges' motions and
/// every face's region.
pub(super) struct Plan {
    pub(super) bends: Vec<Oriented>,
    pub(super) motions: Vec<Rigid>,
    pub(super) walk: Walk,
    pub(super) fixed: usize,
    region: BTreeMap<FaceId, Region>,
}

fn plan(b: &Body, model: &SheetModel, spec: &UnfoldSpec) -> Result<Plan, SheetMetalError> {
    if let Some(c) = &model.cycle {
        return Err(SheetMetalError::BendLoop {
            bends: c.bends.clone(),
            faces: c.faces.clone(),
        });
    }
    let fixed = match spec.fixed {
        None => model.fixed,
        Some(n) => model
            .flanges
            .iter()
            .position(|f| f.sides.iter().any(|s| s.faces.contains(&n)))
            .ok_or(SheetMetalError::NoSuchFace { name: n })?,
    };
    if let Some(names) = &spec.bends {
        for &n in names {
            if model.bend(n).is_none() {
                return Err(SheetMetalError::NoSuchBend { name: n });
            }
        }
    }
    for (n, _) in &spec.overrides {
        if model.bend(*n).is_none() {
            return Err(SheetMetalError::NoSuchBend { name: *n });
        }
    }
    let ends: Vec<[usize; 2]> = model.bends.iter().map(|b| b.flanges).collect();
    let walk = graph::walk(model.flanges.len(), &ends, fixed);
    let t = model.thickness;
    let mut bends = Vec::with_capacity(model.bends.len());
    for (k, bend) in model.bends.iter().enumerate() {
        let near = walk.near[k].expect("a tree reaches every bend");
        let flipped = near != bend.flanges[0];
        let axis = if flipped {
            Frame::new(
                bend.axis.origin(),
                -bend.axis.z().get(),
                bend.axis.radial(bend.angle),
            )
            .map_err(|_| SheetMetalError::Uncertain { at: bend.name })?
        } else {
            bend.axis
        };
        let far = if flipped {
            bend.flanges[0]
        } else {
            bend.flanges[1]
        };
        let allowance = spec
            .overrides
            .iter()
            .find(|(n, _)| *n == bend.name)
            .map_or(spec.allowance, |(_, a)| *a);
        let length = allowance
            .length(bend.radius, t, bend.angle)
            .ok_or(SheetMetalError::BadAllowance { bend: bend.name })?;
        let unfolded = spec.bends.as_ref().is_none_or(|ns| ns.contains(&bend.name));
        bends.push(Oriented {
            name: bend.name,
            axis,
            radius: bend.radius,
            angle: bend.angle,
            near,
            far,
            length: unfolded.then_some(length),
            allowance,
        });
    }
    let motions = graph::motions(model.flanges.len(), &ends, &walk, |k| {
        let o = &bends[k];
        o.length.map(|ba| graph::unbend(&o.axis, o.angle, ba))
    });
    let mut region = BTreeMap::new();
    for (i, f) in model.flanges.iter().enumerate() {
        for &n in &f.faces {
            region.insert(face_named(b, n)?, Region::Flange(i));
        }
    }
    for (k, bend) in model.bends.iter().enumerate() {
        for (names, outer) in [(&bend.inner, false), (&bend.outer, true)] {
            for &n in names {
                region.insert(face_named(b, n)?, Region::Bend { k, outer });
            }
        }
        for &n in &bend.walls {
            let f = face_named(b, n)?;
            match region.entry(f).or_insert_with(|| Region::Wall(Vec::new())) {
                Region::Wall(ks) => ks.push(k),
                _ => return Err(SheetMetalError::NotSheet { face: n }),
            }
        }
    }
    for f in b.faces() {
        if !region.contains_key(&f) {
            return Err(SheetMetalError::NotSheet {
                face: b.faces[f].name,
            });
        }
    }
    Ok(Plan {
        bends,
        motions,
        walk,
        fixed,
        region,
    })
}

impl Plan {
    /// The map of a bend's region: its strip unfolded, else its near
    /// flange's motion.
    fn of_bend(&self, k: usize) -> Map {
        if self.bends[k].length.is_some() {
            Map::Strip(k)
        } else {
            Map::Rigid(self.bends[k].near)
        }
    }

    fn motion(&self, m: Map) -> &Rigid {
        match m {
            Map::Rigid(i) => &self.motions[i],
            Map::Strip(k) => &self.motions[self.bends[k].near],
        }
    }

    /// The region an edge lies in, from its faces'.
    fn edge_map(&self, b: &Body, e: EdgeId) -> Result<Map, SheetMetalError> {
        let faces = shape::faces_of(b, e);
        let regions: Vec<&Region> = faces.iter().map(|f| &self.region[f]).collect();
        let at = SheetMetalError::Uncertain {
            at: b.edges[e].name,
        };
        if let Some(k) = regions.iter().find_map(|r| match r {
            Region::Bend { k, .. } => Some(*k),
            _ => None,
        }) {
            return Ok(self.of_bend(k));
        }
        let flanges: Vec<usize> = regions
            .iter()
            .filter_map(|r| match r {
                Region::Flange(i) => Some(*i),
                _ => None,
            })
            .collect();
        if let Some(&i) = flanges.first() {
            return if flanges.iter().all(|&j| j == i) {
                Ok(Map::Rigid(i))
            } else {
                Err(at)
            };
        }
        let walls: Vec<&Vec<usize>> = regions
            .iter()
            .filter_map(|r| match r {
                Region::Wall(ks) => Some(ks),
                _ => None,
            })
            .collect();
        let common = walls[0]
            .iter()
            .filter(|k| walls.iter().all(|w| w.contains(k)))
            .min()
            .copied();
        common.map(|k| self.of_bend(k)).ok_or(at)
    }

    /// A point of a bend's region in the flat.
    fn strip_point(&self, k: usize, p: Point3) -> Point3 {
        let o = &self.bends[k];
        let l = o.axis.to_local(p);
        let phi = o.spanned(crate::geom::polar_angle(l.y, l.x));
        let rho = crate::math::hypot(l.x, l.y);
        let q = o.axis.to_world(rho, phi * o.neutral(), l.z);
        self.motions[o.near].apply_point(q)
    }

    fn point(&self, m: Map, p: Point3) -> Point3 {
        match m {
            Map::Rigid(i) => self.motions[i].apply_point(p),
            Map::Strip(k) => self.strip_point(k, p),
        }
    }
}

/// An edge's new curve and interval.
#[derive(Clone, Debug)]
struct EdgeImage {
    curve: Curve,
    interval: [f64; 2],
}

/// The image of an edge in an unfolded bend's region.
fn strip_edge(b: &Body, p: &Plan, k: usize, e: EdgeId) -> Result<EdgeImage, SheetMetalError> {
    let o = &p.bends[k];
    let m = p.motion(Map::Strip(k));
    let rn = o.neutral();
    let edge = &b.edges[e];
    let [t0, t1] = edge.interval;
    let eps = shape::edge_tolerance(b, e) + crate::tol::LINEAR_RESOLUTION;
    let at = SheetMetalError::Uncertain { at: edge.name };
    let a = o.axis.z();
    let line = |origin: Point3, dir: crate::geom::UnitVector3| {
        crate::geom::Line::from_unit(m.apply_point(origin), m.apply_unit(dir))
            .map(Curve::Line)
            .map_err(|_| at.clone())
    };
    match &b.curves[edge.curve] {
        Curve::Line(l) if shape::parallel(l.dir(), a, ANGULAR_RESOLUTION) => {
            let l0 = o.axis.to_local(l.origin());
            let phi = o.spanned(angle_of(&o.axis, shape::middle(b, e)));
            let rho = crate::math::hypot(l0.x, l0.y);
            let origin = o.axis.to_world(rho, phi * rn, l0.z);
            let dir = if l.dir().dot(a) > 0.0 { a } else { -a };
            Ok(EdgeImage {
                curve: line(origin, dir)?,
                interval: edge.interval,
            })
        }
        Curve::Line(l) if l.dir().get().dot(a.get()).abs() <= ANGULAR_RESOLUTION => {
            let mid = shape::middle(b, e);
            let phi = o.spanned(angle_of(&o.axis, mid));
            let out = o.axis.radial(phi);
            let side = o.axis.radial(phi + FRAC_PI_2);
            if (l.origin() - o.axis.origin()).dot(side).abs() > eps {
                return Err(at);
            }
            let l0 = o.axis.to_local(l.origin());
            let rho0 = (l.origin() - o.axis.origin()).dot(out);
            let origin = o.axis.to_world(rho0, phi * rn, l0.z);
            let x = o.axis.x();
            let dir = if l.dir().get().dot(out) > 0.0 { x } else { -x };
            Ok(EdgeImage {
                curve: line(origin, dir)?,
                interval: edge.interval,
            })
        }
        Curve::Circle(c)
            if shape::parallel(c.frame().z(), a, ANGULAR_RESOLUTION)
                && shape::axis_distance(&o.axis, c.frame().origin()) <= eps =>
        {
            let f = c.frame();
            let sigma = if f.z().dot(a) > 0.0 { 1.0 } else { -1.0 };
            let x = f.x().get();
            let alpha = crate::geom::polar_angle(x.dot(o.axis.y().get()), x.dot(o.axis.x().get()));
            let mid = 0.5 * (t0 + t1);
            let alpha = o.spanned(alpha + sigma * mid) - sigma * mid;
            let yc = o.axis.to_local(f.origin()).z;
            let origin = o.axis.to_world(c.radius(), rn * alpha, yc);
            let d = o.axis.y();
            let dir = if sigma > 0.0 { d } else { -d };
            Ok(EdgeImage {
                curve: line(origin, dir)?,
                interval: [rn * t0, rn * t1],
            })
        }
        _ => lifted(b, p, k, e),
    }
}

/// An edge on one of a bend's cylinders, other than a line along its axis
/// or an arc about it: its pcurve there, mapped into the strip's plane and
/// lifted.
fn lifted(b: &Body, p: &Plan, k: usize, e: EdgeId) -> Result<EdgeImage, SheetMetalError> {
    let o = &p.bends[k];
    let m = p.motion(Map::Strip(k));
    let edge = &b.edges[e];
    let at = SheetMetalError::Uncertain { at: edge.name };
    let fin = b
        .edge_fins(e)
        .find(|&fin| {
            matches!(
                p.region.get(&shape::face_of(b, fin)),
                Some(Region::Bend { k: j, .. }) if *j == k
            )
        })
        .ok_or(at.clone())?;
    let face = shape::face_of(b, fin);
    let cyl = shape::round(b, face).ok_or(at.clone())?;
    let pc = b.fins[fin].pcurve.ok_or(at.clone())?;
    let c2 = &b.curves2[pc.curve];
    let fc = &cyl.frame;
    let s = if fc.z().dot(o.axis.z()) > 0.0 {
        1.0
    } else {
        -1.0
    };
    let xa = o.axis.x().get();
    let ux = crate::geom::polar_angle(xa.dot(fc.y().get()), xa.dot(fc.x().get()));
    let [t0, t1] = edge.interval;
    let um = c2.point(pc.param(0.5 * (t0 + t1))).x;
    let phi_mid = o.spanned(s * (um - ux));
    let ux = um - s * phi_mid;
    let rn = o.neutral();
    let y0 = (fc.origin() - o.axis.origin()).dot(o.axis.z().get());
    let affine = Affine2::diagonal([-rn * s * ux, rn * s], [y0, s]);
    let (image, [r0, r1]) = curve2::mapped(c2, &affine).map_err(|_| at.clone())?;
    let curve = curve2::lift(&image, &o.strip(cyl.radius)).map_err(|_| at.clone())?;
    let curve = curve.transformed(m);
    let (mut a0, mut a1) = (r0 + r1 * pc.map[0], r1 * pc.map[1]);
    let curve = if a1 < 0.0 {
        let (back, [q0, q1]) = curve2::reversed(&curve).ok_or(at.clone())?;
        (a0, a1) = (q0 + q1 * a0, q1 * a1);
        back
    } else {
        curve
    };
    Ok(EdgeImage {
        curve,
        interval: [a0 + a1 * t0, a0 + a1 * t1],
    })
}

/// A face's new surface and sense.
#[derive(Clone, Debug)]
struct FaceImage {
    surface: Surface,
    sense: Sense,
}

fn face_image(
    b: &Body,
    p: &Plan,
    model: &SheetModel,
    f: FaceId,
    edges: &BTreeMap<EdgeId, EdgeImage>,
) -> Result<FaceImage, SheetMetalError> {
    let face = &b.faces[f];
    let surface = &b.surfaces[face.surface];
    let at = SheetMetalError::Uncertain { at: face.name };
    let kept = |m: &Rigid| FaceImage {
        surface: surface.transformed(m),
        sense: face.sense,
    };
    match &p.region[&f] {
        Region::Flange(i) => Ok(kept(&p.motions[*i])),
        Region::Bend { k, .. } => match p.of_bend(*k) {
            Map::Rigid(i) => Ok(kept(&p.motions[i])),
            Map::Strip(k) => {
                let cyl = shape::round(b, f).ok_or(at.clone())?;
                let plane = Surface::Plane(Plane::new(p.bends[k].strip(cyl.radius)));
                Ok(FaceImage {
                    surface: plane.transformed(p.motion(Map::Strip(k))),
                    sense: face.sense,
                })
            }
        },
        Region::Wall(ks) => {
            let k0 = *ks.iter().min().expect("a bend");
            let o = &p.bends[k0];
            let wall = radial::classify(b, f, &o.axis, model.thickness, model.tolerance);
            match (wall, p.of_bend(k0)) {
                (Wall::Square, _) | (_, Map::Rigid(_)) => Ok(kept(&p.motions[o.near])),
                (Wall::Through, Map::Strip(k)) => {
                    let n = shape::flat(b, f).ok_or(at.clone())?;
                    let natural = match face.sense {
                        Sense::Forward => n.normal,
                        Sense::Reversed => -n.normal,
                    };
                    let mid = face_point(b, f).ok_or(at.clone())?;
                    let phi = o.spanned(angle_of(&o.axis, mid));
                    let sign = natural.get().dot(o.axis.radial(phi + FRAC_PI_2));
                    let d = o.axis.y();
                    let z = if sign > 0.0 { d } else { -d };
                    let frame = Frame::from_axes(
                        o.axis.to_world(o.radius, phi * o.neutral(), 0.0),
                        z,
                        o.axis.z(),
                    )
                    .map_err(|_| at.clone())?;
                    Ok(FaceImage {
                        surface: Surface::Plane(Plane::new(frame))
                            .transformed(p.motion(Map::Strip(k))),
                        sense: face.sense,
                    })
                }
                (Wall::Radial, Map::Strip(k)) => radial_wall(b, p, k, f, edges),
                _ => Err(SheetMetalError::NotRadial { face: face.name }),
            }
        }
    }
}

/// A point of a face off its bend's axis: the middle of its first edge.
fn face_point(b: &Body, f: FaceId) -> Option<Point3> {
    shape::edges_of(b, f).first().map(|&e| shape::middle(b, e))
}

/// The edge of a wall on its bend's inner cylinder.
fn inner_edge(b: &Body, p: &Plan, k: usize, f: FaceId) -> Option<EdgeId> {
    let on: Vec<EdgeId> = shape::edges_of(b, f)
        .into_iter()
        .filter(|&e| {
            shape::across(b, e, f)
                .is_some_and(|g| p.region.get(&g) == Some(&Region::Bend { k, outer: false }))
        })
        .collect();
    (on.len() == 1).then(|| on[0])
}

/// A curved radial wall: the extrusion along the flat's normal of its edge
/// on the inner cylinder (a plane, where that edge is a line), facing as
/// the wall did.
fn radial_wall(
    b: &Body,
    p: &Plan,
    k: usize,
    f: FaceId,
    edges: &BTreeMap<EdgeId, EdgeImage>,
) -> Result<FaceImage, SheetMetalError> {
    let face = &b.faces[f];
    let at = SheetMetalError::Uncertain { at: face.name };
    let o = &p.bends[k];
    let m = p.motion(Map::Strip(k));
    let e = inner_edge(b, p, k, f).ok_or(at.clone())?;
    let image = edges.get(&e).ok_or(at.clone())?;
    let up = m.apply_vector(o.axis.x().get());
    // A line's extrusion is a plane, which the kernel keeps as one.
    let surface = match &image.curve {
        Curve::Line(l) => {
            let frame = Frame::new(l.origin(), l.dir().get().cross(up), l.dir().get())
                .map_err(|_| at.clone())?;
            Surface::Plane(Plane::new(frame))
        }
        c => Surface::Extrusion(Arc::new(
            Extrusion::new(c.clone(), up).map_err(|_| at.clone())?,
        )),
    };
    // The wall's outward normal at its inner edge's middle, carried into
    // the strip, against the new surface's natural normal there.
    let [t0, t1] = b.edges[e].interval;
    let mid = b.curves[b.edges[e].curve].point(0.5 * (t0 + t1));
    let old = &b.surfaces[face.surface];
    let (u, v) = old.project(mid).param;
    let n = old.normal(u, v).ok_or(at.clone())?.get();
    let n = if face.sense == Sense::Forward { n } else { -n };
    let phi = o.spanned(angle_of(&o.axis, mid));
    let tangential = n.dot(o.axis.radial(phi + FRAC_PI_2));
    let along = n.dot(o.axis.z().get());
    let want = m.apply_vector(o.axis.y().get() * tangential + o.axis.z().get() * along);
    let [s0, s1] = image.interval;
    let (u, v) = surface.project(image.curve.point(0.5 * (s0 + s1))).param;
    let natural = surface.normal(u, v).ok_or(at.clone())?.get();
    let dot = natural.dot(want);
    if dot.abs() <= 1e-6 * want.length() {
        return Err(at);
    }
    Ok(FaceImage {
        surface,
        sense: if dot > 0.0 {
            Sense::Forward
        } else {
            Sense::Reversed
        },
    })
}

/// The name a check fault is about.
fn fault_at(b: &Body, fault: &Fault) -> Option<Name> {
    let face = |f: FaceId| b.faces.get(f).map(|x| x.name);
    let edge = |e: EdgeId| b.edges.get(e).map(|x| x.name);
    let vertex = |v: VertexId| b.vertices.get(v).map(|x| x.name);
    match fault {
        Fault::FacesCross { a, .. }
        | Fault::FacesOverlap { a, .. }
        | Fault::CannotCheck { a, .. } => face(*a),
        Fault::VertexOffCurve { vertex: v, .. } | Fault::VertexOffSurface { vertex: v, .. } => {
            vertex(*v)
        }
        Fault::EdgeOffSurface { edge: e, .. } | Fault::BadInterval { edge: e } => edge(*e),
        Fault::PcurveOffEdge { fin, .. } | Fault::MissingPcurve { fin } => {
            b.fins.get(*fin).and_then(|f| edge(f.edge))
        }
        Fault::FaceInsideOut { face: f } | Fault::Loopless { face: f } => face(*f),
        _ => None,
    }
}

fn run(b: &Body, model: &SheetModel, spec: &UnfoldSpec) -> Result<Unfolded, SheetMetalError> {
    let p = plan(b, model, spec)?;
    let edges: Vec<EdgeId> = b.edges().collect();
    let maps: Vec<Map> = edges
        .iter()
        .map(|&e| p.edge_map(b, e))
        .collect::<Result<_, _>>()?;
    let images: Vec<EdgeImage> = edges
        .par_iter()
        .zip(maps.par_iter())
        .map(|(&e, &m)| match m {
            Map::Rigid(i) => Ok(EdgeImage {
                curve: b.curves[b.edges[e].curve].transformed(&p.motions[i]),
                interval: b.edges[e].interval,
            }),
            Map::Strip(k) => strip_edge(b, &p, k, e),
        })
        .collect::<Result<_, _>>()?;
    let edge_maps: BTreeMap<EdgeId, Map> = edges.iter().copied().zip(maps).collect();
    let edge_images: BTreeMap<EdgeId, EdgeImage> = edges.iter().copied().zip(images).collect();
    let faces: Vec<FaceId> = b.faces().collect();
    let face_images: Vec<FaceImage> = faces
        .par_iter()
        .map(|&f| face_image(b, &p, model, f, &edge_images))
        .collect::<Result<_, _>>()?;
    let vertices: Vec<VertexId> = b.vertices().collect();
    let points: Vec<Point3> = vertices
        .iter()
        .map(|&v| {
            let mut m = None;
            for e in b.vertex_edges(v) {
                let em = edge_maps[&e];
                if matches!(em, Map::Strip(_)) || m.is_none() {
                    m = Some(em);
                }
                if matches!(em, Map::Strip(_)) {
                    break;
                }
            }
            let point = b.vertices[v].point;
            m.map_or(point, |m| p.point(m, point))
        })
        .collect();
    // Which faces moved rigidly, and by what.
    let face_map = |f: FaceId| match &p.region[&f] {
        Region::Flange(i) => Some(Map::Rigid(*i)),
        Region::Bend { k, .. } => match p.of_bend(*k) {
            Map::Rigid(i) => Some(Map::Rigid(i)),
            Map::Strip(_) => None,
        },
        Region::Wall(_) => None,
    };
    // Radial walls accepted within the tolerance widen their other edges.
    let mut widen: BTreeMap<EdgeId, f64> = BTreeMap::new();
    for &f in &faces {
        if let Region::Wall(ks) = &p.region[&f]
            && let Map::Strip(k) = p.of_bend(*ks.iter().min().expect("a bend"))
            && shape::flat(b, f).is_none()
        {
            let o = &p.bends[k];
            let grow = model.tolerance * (o.neutral() / o.radius).max(1.0);
            let inner = inner_edge(b, &p, k, f);
            for e in shape::edges_of(b, f) {
                if Some(e) != inner {
                    *widen.entry(e).or_insert(0.0) += grow;
                }
            }
        }
    }
    // The flat body: the same topology, new geometry.
    let mut flat = b.clone();
    flat.surfaces = Arena::default();
    flat.curves = Arena::default();
    flat.curves2 = Arena::default();
    for (&f, img) in faces.iter().zip(face_images) {
        let s = flat.surfaces.push(img.surface);
        flat.faces[f].surface = s;
        flat.faces[f].sense = img.sense;
    }
    for &e in &edges {
        let img = &edge_images[&e];
        let c = flat.curves.push(img.curve.clone());
        let edge = &mut flat.edges[e];
        edge.curve = c;
        edge.interval = img.interval;
        if let Some(w) = widen.get(&e) {
            edge.tolerance = Tolerance::new(edge.tolerance.get() + w)
                .map_err(|_| SheetMetalError::Uncertain { at: edge.name })?;
        }
    }
    for (&v, &pt) in vertices.iter().zip(&points) {
        flat.vertices[v].point = pt;
    }
    let fins: Vec<_> = b.fins().collect();
    for fin in fins {
        let face = shape::face_of(b, fin);
        let edge = b.fins[fin].edge;
        let keep = face_map(face).is_some_and(|m| m == edge_maps[&edge]);
        flat.fins[fin].pcurve = match (keep, b.fins[fin].pcurve) {
            (true, Some(pc)) => {
                let c = flat.curves2.push(b.curves2[pc.curve].clone());
                Some(crate::topo::Pcurve {
                    curve: c,
                    map: pc.map,
                })
            }
            _ => None,
        };
    }
    crate::pcurve::fill(&mut flat).map_err(|e| match e {
        Error::NoPcurve { face, .. } => SheetMetalError::Uncertain {
            at: flat.faces[face].name,
        },
        _ => SheetMetalError::Uncertain {
            at: model.flanges[p.fixed].name,
        },
    })?;
    let fallback = model.flanges[p.fixed].name;
    if let Some(fault) = check::check(&flat).first() {
        return Err(SheetMetalError::Uncertain {
            at: fault_at(&flat, fault).unwrap_or(fallback),
        });
    }
    // Faces that cross or overlap say more than pairs that couldn't be
    // checked, so they come first.
    let faults = check::check_intersections(&flat);
    let crossing = faults.iter().find_map(|f| match f {
        Fault::FacesCross { a, b: c } | Fault::FacesOverlap { a, b: c } => Some((*a, *c)),
        _ => None,
    });
    if let Some((a, c)) = crossing {
        let mut faces = [flat.faces[a].name, flat.faces[c].name];
        faces.sort_unstable();
        return Err(SheetMetalError::FlatOverlaps { faces });
    }
    if let Some(fault) = faults.first() {
        return Err(SheetMetalError::Uncertain {
            at: fault_at(&flat, fault).unwrap_or(fallback),
        });
    }
    let mut journal = Journal::new();
    for &f in &faces {
        journal.push(Event::Modified(b.faces[f].name));
    }
    for &e in &edges {
        journal.push(Event::Modified(b.edges[e].name));
    }
    for &v in &vertices {
        journal.push(Event::Modified(b.vertices[v].name));
    }
    let map = flat_map(b, model, &p)?;
    let pattern = pattern::pattern(&flat, model, &p)?;
    Ok(Unfolded {
        body: flat,
        journal,
        pattern,
        map,
    })
}

/// The strips of the unfolded bends in the walk's order, and every
/// flange's motion.
fn flat_map(b: &Body, model: &SheetModel, p: &Plan) -> Result<FlatMap, SheetMetalError> {
    let mut strips = Vec::new();
    for &k in &p.walk.order {
        let o = &p.bends[k];
        let Some(length) = o.length else {
            continue;
        };
        // The bend's extent along its axis, from its faces' vertices.
        let bend = &model.bends[k];
        let mut span = [f64::INFINITY, f64::NEG_INFINITY];
        for &n in bend.inner.iter().chain(&bend.outer) {
            for e in shape::edges_of(b, face_named(b, n)?) {
                for q in shape::ends(b, e) {
                    let y = o.axis.to_local(q).z;
                    span = [span[0].min(y), span[1].max(y)];
                }
            }
        }
        if span[0] >= span[1] {
            return Err(SheetMetalError::Uncertain { at: o.name });
        }
        strips.push(Strip {
            bend: o.name,
            near: model.flanges[o.near].name,
            far: model.flanges[o.far].name,
            axis: o.axis,
            frame: strip_frame(p, k),
            radius: o.radius,
            angle: o.angle,
            length,
            span,
            allowance: o.allowance,
        });
    }
    Ok(FlatMap {
        thickness: model.thickness,
        strips,
        flanges: model
            .flanges
            .iter()
            .zip(&p.motions)
            .map(|(f, m)| (f.name, *m))
            .collect(),
    })
}

/// The normal of a near flange's inner side at a bend, in the flat.
pub(super) fn inner_normal(model: &SheetModel, p: &Plan, k: usize) -> crate::geom::Vector3 {
    let o = &p.bends[k];
    let side = inner_side(&model.flanges[o.near], &o.axis);
    p.motions[o.near].apply_vector(side.frame().z().get())
}

/// An unfolded bend's strip in the flat ([`super::Strip::frame`]).
pub(super) fn strip_frame(p: &Plan, k: usize) -> Frame {
    let o = &p.bends[k];
    o.strip(o.radius).transformed(&p.motions[o.near])
}
