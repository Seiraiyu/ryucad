//! Planning edge flanges: every name, size and face resolved, and every
//! flange checked against the others, before anything is built.

use super::super::shape::{self, flat};
use super::super::{EdgeFlange, FlangePosition, Measure, Relief, SheetModel};
use crate::geom::{Curve, Point3, UnitVector3};
use crate::name::Name;
use crate::tol::{ANGULAR_RESOLUTION, LINEAR_RESOLUTION};
use crate::topo::{Body, EdgeId, FaceId};
use crate::{FlangeSize, SheetMetalError};
use core::f64::consts::PI;
use std::collections::{BTreeMap, BTreeSet};

/// The shortest length a flange makes anything of: a bend's inner arc, a
/// straight flange, the edge left between setbacks, a relief's width.
pub(crate) const SHORTEST: f64 = 10.0 * LINEAR_RESOLUTION;

/// An edge flange resolved against its body, in the edge's frame: `s` along
/// the edge from its start vertex, `x` into the base from the wall, and the
/// side's outward normal.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct Planned {
    pub(crate) edge: Name,
    /// The edge's start and end vertex.
    pub(crate) vertices: [Name; 2],
    /// The start vertex's point.
    pub(crate) start: Point3,
    /// Along the edge, from its start.
    pub(crate) along: UnitVector3,
    /// The edge's length.
    pub(crate) length: f64,
    /// The side's outward normal: the way the flange folds.
    pub(crate) normal: UnitVector3,
    /// The wall's outward normal.
    pub(crate) out: UnitVector3,
    pub(crate) thickness: f64,
    pub(crate) radius: f64,
    pub(crate) angle: f64,
    /// How far the base is cut back from the wall: the bend line's `x`.
    pub(crate) trim: f64,
    /// The flange's straight length past the bend.
    pub(crate) straight: f64,
    /// The flange's extent along the edge, `[s0, length - s1]`.
    pub(crate) span: [f64; 2],
    /// The reliefs cut at the start and end: `None` where none is needed.
    pub(crate) reliefs: [Relief; 2],
}

impl Planned {
    /// The point at `s` along the edge, `x` into the base and `h` along the
    /// side's normal.
    pub(crate) fn at(&self, s: f64, x: f64, h: f64) -> Point3 {
        self.start + self.along.get() * s - self.out.get() * x + self.normal.get() * h
    }
}

/// Resolves `specs` against `b` and its model, in edge-name order.
pub(crate) fn plan(
    b: &Body,
    model: &SheetModel,
    specs: &[EdgeFlange],
    tol: f64,
) -> Result<Vec<Planned>, SheetMetalError> {
    let by_name: BTreeMap<Name, EdgeId> = b.edges().map(|e| (b.edges[e].name, e)).collect();
    let mut sides: BTreeMap<Name, UnitVector3> = BTreeMap::new();
    for f in &model.flanges {
        for s in &f.sides {
            for &n in &s.faces {
                sides.insert(n, s.plane.frame().z());
            }
        }
    }
    let mut seen = BTreeSet::new();
    let mut out = Vec::with_capacity(specs.len());
    for spec in specs {
        let &e = by_name
            .get(&spec.edge)
            .ok_or(SheetMetalError::NoSuchEdge { name: spec.edge })?;
        if !seen.insert(spec.edge) {
            return Err(SheetMetalError::DuplicateFlange { edge: spec.edge });
        }
        out.push(one(b, e, spec, &sides, model.thickness, tol)?);
    }
    out.sort_by_key(|p| p.edge);
    meets(&out)?;
    Ok(out)
}

/// One flange resolved.
fn one(
    b: &Body,
    e: EdgeId,
    spec: &EdgeFlange,
    sides: &BTreeMap<Name, UnitVector3>,
    t: f64,
    tol: f64,
) -> Result<Planned, SheetMetalError> {
    let edge = spec.edge;
    let not = SheetMetalError::NotAFlangeEdge { edge };
    let bad = |what| SheetMetalError::BadSize { what, edge };
    let ed = &b.edges[e];
    if !matches!(b.curves[ed.curve], Curve::Line(_)) {
        return Err(not);
    }
    let faces = shape::faces_of(b, e);
    let [f, g] = faces.as_slice() else {
        return Err(not);
    };
    let fname = |x: FaceId| b.faces[x].name;
    // The side and the wall: the side named, else the one flange side.
    let (side, wall) = match spec.side {
        Some(n) if n == fname(*f) => (*f, *g),
        Some(n) if n == fname(*g) => (*g, *f),
        Some(_) => return Err(not),
        None => match (
            sides.contains_key(&fname(*f)),
            sides.contains_key(&fname(*g)),
        ) {
            (true, false) => (*f, *g),
            (false, true) => (*g, *f),
            _ => return Err(not),
        },
    };
    let normal = *sides.get(&fname(side)).ok_or(not.clone())?;
    let w = flat(b, wall).ok_or(not.clone())?.normal;
    let [p0, p1] = shape::ends(b, e);
    let length = p0.distance(p1);
    let along = (p1 - p0).normalize().ok_or(not.clone())?;
    let square = |u: UnitVector3, v: UnitVector3| u.get().dot(v.get()).abs() <= ANGULAR_RESOLUTION;
    if !(square(normal, w) && square(along, w) && square(along, normal)) {
        return Err(not);
    }
    // The edge must lie on the side's plane, within the tolerances.
    let eps = tol + shape::edge_tolerance(b, e);
    let on = |p: Point3| (p - b.vertices[ed.start].point).dot(normal.get()).abs() <= eps;
    if !(on(p0) && on(p1)) {
        return Err(not);
    }
    let (r, theta) = (spec.radius, spec.angle);
    if !(r.is_finite() && r >= SHORTEST) {
        return Err(bad(FlangeSize::Radius));
    }
    if !(theta.is_finite() && theta <= PI && r * theta >= SHORTEST) {
        return Err(bad(FlangeSize::Angle));
    }
    let mould = |reach: f64| {
        if theta >= PI - ANGULAR_RESOLUTION {
            f64::NAN
        } else {
            spec.length - reach * crate::math::tan(0.5 * theta)
        }
    };
    let straight = match spec.measured {
        Measure::Tangent => spec.length,
        Measure::OuterMould => mould(r + t),
        Measure::InnerMould => mould(r),
    };
    if !(straight.is_finite() && straight >= SHORTEST) {
        return Err(bad(FlangeSize::Length));
    }
    let [s0, s1] = spec.setbacks;
    if !(s0.is_finite() && s1.is_finite() && s0 >= 0.0 && s1 >= 0.0 && length - s0 - s1 >= SHORTEST)
    {
        return Err(bad(FlangeSize::Setback));
    }
    let trim = match spec.position {
        FlangePosition::MaterialInside => r + t,
        FlangePosition::MaterialOutside => r,
        FlangePosition::BendOutside => 0.0,
    };
    let vertices = [b.vertices[ed.start].name, b.vertices[ed.end].name];
    let mut reliefs = [Relief::None; 2];
    for k in 0..2 {
        if trim == 0.0 || spec.setbacks[k] == 0.0 {
            continue;
        }
        let ok = |w: f64, d: f64| w.is_finite() && d.is_finite() && w >= SHORTEST && d >= 0.0;
        reliefs[k] = match spec.reliefs[k] {
            Relief::None => {
                return Err(SheetMetalError::ReliefNeeded {
                    edge,
                    vertex: vertices[k],
                });
            }
            x @ Relief::Rectangular { width, depth } if ok(width, depth) => x,
            x @ Relief::Obround { width, depth } if ok(width, depth) && depth >= 0.5 * width => x,
            x @ Relief::Tear { width } if ok(width, 0.0) => x,
            _ => return Err(bad(FlangeSize::Relief)),
        };
    }
    Ok(Planned {
        edge,
        vertices,
        start: p0,
        along,
        length,
        normal,
        out: w,
        thickness: t,
        radius: r,
        angle: theta,
        trim,
        straight,
        span: [s0, length - s1],
        reliefs,
    })
}

/// A relief's width and how far past the bend line it reaches.
pub(crate) fn relief_size(r: Relief) -> Option<(f64, f64)> {
    match r {
        Relief::None => None,
        Relief::Rectangular { width, depth } | Relief::Obround { width, depth } => {
            Some((width, depth))
        }
        Relief::Tear { width } => Some((width, 0.0)),
    }
}

/// The quadrilaterals a flange covers seen along its side's normal: its
/// bend and flange from as far out as they reach to as far back over the
/// base, across its span, and each relief.
fn footprint(p: &Planned) -> Vec<[Point3; 4]> {
    let (r, t, e) = (p.radius, p.thickness, p.straight);
    let back = (-crate::math::cos(p.angle)).max(0.0) * e;
    let quad = |s: [f64; 2], x: [f64; 2]| {
        [
            p.at(s[0], x[0], 0.0),
            p.at(s[1], x[0], 0.0),
            p.at(s[1], x[1], 0.0),
            p.at(s[0], x[1], 0.0),
        ]
    };
    let mut out = vec![quad(p.span, [p.trim - (r + t + e), p.trim + back])];
    for (k, &rel) in p.reliefs.iter().enumerate() {
        if let Some((w, d)) = relief_size(rel) {
            let s = if k == 0 {
                [p.span[0] - w, p.span[0]]
            } else {
                [p.span[1], p.span[1] + w]
            };
            out.push(quad(s, [0.0, p.trim + d]));
        }
    }
    out
}

/// Refuses flanges on one side's plane whose footprints overlap.
fn meets(planned: &[Planned]) -> Result<(), SheetMetalError> {
    for (i, a) in planned.iter().enumerate() {
        for c in &planned[i + 1..] {
            let coplanar = a.normal.get().dot(c.normal.get()) >= 1.0 - ANGULAR_RESOLUTION
                && (c.start - a.start).dot(a.normal.get()).abs() <= SHORTEST;
            if !coplanar {
                continue;
            }
            let (fa, fc) = (footprint(a), footprint(c));
            let hit = fa
                .iter()
                .any(|qa| fc.iter().any(|qc| overlap(a, qa, qc, SHORTEST)));
            if hit {
                let shared = a.vertices.iter().find(|v| c.vertices.contains(v));
                let vertex = match shared {
                    Some(&v) => v,
                    None => *a
                        .vertices
                        .iter()
                        .chain(&c.vertices)
                        .min()
                        .expect("vertices"),
                };
                return Err(SheetMetalError::FlangesMeet { vertex });
            }
        }
    }
    Ok(())
}

/// Whether two convex quadrilaterals in `p`'s side plane overlap by more
/// than `eps` along every axis their sides give (separating axes).
fn overlap(p: &Planned, a: &[Point3; 4], c: &[Point3; 4], eps: f64) -> bool {
    let flat2 = |q: &[Point3; 4]| {
        q.map(|x| {
            let v = x - p.start;
            [v.dot(p.along.get()), v.dot(p.out.get())]
        })
    };
    let (a, c) = (flat2(a), flat2(c));
    let axes = a
        .iter()
        .zip(a.iter().cycle().skip(1))
        .chain(c.iter().zip(c.iter().cycle().skip(1)));
    for (u, v) in axes {
        let n = [v[1] - u[1], u[0] - v[0]];
        let len = crate::math::hypot(n[0], n[1]);
        if len == 0.0 {
            continue;
        }
        let span = |q: &[[f64; 2]; 4]| {
            q.iter()
                .fold([f64::INFINITY, f64::NEG_INFINITY], |[lo, hi], x| {
                    let d = (x[0] * n[0] + x[1] * n[1]) / len;
                    [lo.min(d), hi.max(d)]
                })
        };
        let (sa, sc) = (span(&a), span(&c));
        if sa[1].min(sc[1]) - sa[0].max(sc[0]) <= eps {
            return false;
        }
    }
    true
}
