//! Refolding: a flat, cut or not, carried back to its folded shape.
//!
//! The flat is first cut at the bend lines of the strips it folds
//! ([`super::imprint`]), so every face and edge lies in one strip or beside
//! the strips ([`super::regions`]). Faces beside them move rigidly, each
//! group as its flange does. A strip's faces fold by its map, a point at
//! `(x, y, z)` of the strip's frame going to `O + y a + (r + z)(cos(κ x)
//! e₁ + sin(κ x) e₂)` on the bend's axis frame, `κ = θ / ba`:
//!
//! - its sides become the bend's cylinders, their pcurves mapped exactly
//!   by the affine map from the flat's parameters to the cylinders';
//! - planes square to its axis stay planes (they may run on across the
//!   strip's bend lines, as a bracket's end faces do), and planes through
//!   it become planes through the axis;
//! - every other wall square to its sides (a plane at an angle, a
//!   cylinder or an extrusion along the sheet's normal) becomes a
//!   [`Bent`] surface of its own surface, keeping its parameters and so
//!   its pcurves exactly;
//! - lines along the axis stay lines, lines across it become circles,
//!   lines through the sheet become radial lines; any other edge, on a
//!   side, is fitted along its pcurve there (`intersect::fit_along`), its
//!   certified bound joining its tolerance.
//!
//! Names are kept, each `Modified`; what the cuts at the bend lines make
//! is journalled as the Euler operators name it. The refolded body must
//! pass [`crate::check::check`] and [`crate::check::check_intersections`]:
//! faces that cross or overlap are `FoldCollides`.

use super::curve2::{self, Affine2};
use super::imprint::imprint;
use super::regions::{self, Region, Regions};
use super::shape;
use super::unfold::fault_at;
use super::{DEFAULT_TOLERANCE, FlatMap, RefoldSpec, Strip};
use crate::check::{self, Fault};
use crate::geom::{
    Bent, Circle, Curve, Cylinder, Frame, Line, Plane, Point3, Rigid, Surface, UnitVector3, Vector3,
};
use crate::name::{Event, Journal, Name, OpKey};
use crate::tol::Tolerance;
use crate::topo::{Arena, Body, EdgeId, FaceId, Pcurve, Sense, VertexId};
use crate::{Error, SheetMetalError};
use core::f64::consts::PI;
use rayon::prelude::*;
use std::collections::BTreeMap;
use std::sync::Arc;

/// Refolds a flat by `map`, cut or not since it was unfolded: see the
/// module's documentation. `key` names what the cuts at the bend lines
/// make.
pub fn refold(
    key: OpKey,
    body: &Body,
    map: &FlatMap,
    spec: &RefoldSpec,
) -> Result<(Body, Journal), Error> {
    Ok(run(key, body, map, spec)?)
}

/// A strip as refolding folds it.
#[derive(Clone, Copy, Debug)]
struct Fold<'a> {
    strip: &'a Strip,
    /// The bend's axis frame where it is folded.
    axis: Frame,
    /// `θ / ba`.
    curvature: f64,
}

impl Fold<'_> {
    /// A point of the strip's coordinates folded.
    fn point(&self, l: Vector3) -> Point3 {
        let (s, c) = crate::math::sin_cos(l.x * self.curvature);
        let a = &self.axis;
        a.origin()
            + a.z().get() * l.y
            + (a.x().get() * c + a.y().get() * s) * (self.strip.radius + l.z)
    }

    /// The radial direction at `x` of the strip.
    fn radial(&self, x: f64) -> Vector3 {
        self.axis.radial(x * self.curvature)
    }

    fn local(&self, p: Point3) -> Vector3 {
        self.strip.frame.to_local(p)
    }

    /// A direction in the strip's coordinates.
    fn along(&self, v: Vector3) -> Vector3 {
        let f = &self.strip.frame;
        Vector3::new(v.dot(f.x().get()), v.dot(f.y().get()), v.dot(f.z().get()))
    }
}

/// Whether a unit vector in a strip's coordinates is along axis `k`.
fn along_axis(v: Vector3, k: usize) -> bool {
    (v.to_array()[k].abs() - 1.0).abs() <= 1e-12
}

/// An edge's image: its curve and interval, how its parameter moved
/// (`t' = m[0] + m[1] t`, where it did) and what its tolerance grows to.
#[derive(Clone, Debug)]
struct EdgeImage {
    curve: Curve,
    interval: [f64; 2],
    moved: Option<[f64; 2]>,
    tolerance: f64,
}

/// What a face's fins' pcurves become.
#[derive(Clone, Debug)]
enum Pcurves {
    /// Kept, as the face's surface keeps its parameters.
    Keep,
    /// Kept, each moved by whole periods so its middle lies in the box a
    /// bent surface is over: `(box, periods)`.
    Wrap([[f64; 2]; 2], [Option<f64>; 2]),
    /// Mapped by an affine map of the parameters.
    Map(Affine2),
    /// Found again.
    Drop,
}

#[derive(Clone, Debug)]
struct FaceImage {
    surface: Surface,
    sense: Sense,
    pcurves: Pcurves,
}

fn run(
    key: OpKey,
    body: &Body,
    map: &FlatMap,
    spec: &RefoldSpec,
) -> Result<(Body, Journal), SheetMetalError> {
    let folds: Vec<bool> = match &spec.bends {
        None => vec![true; map.strips.len()],
        Some(names) => {
            for &n in names {
                if !map.strips.iter().any(|s| s.bend == n) {
                    return Err(SheetMetalError::NoSuchBend { name: n });
                }
            }
            map.strips.iter().map(|s| names.contains(&s.bend)).collect()
        }
    };
    let eps = DEFAULT_TOLERANCE;
    let mut b = body.clone();
    let mut journal = Journal::new();
    imprint(
        &mut b,
        key,
        &map.strips,
        &folds,
        map.thickness,
        eps,
        &mut journal,
    )?;
    // The cuts' new fins take their exact pcurves, which a wall's box of
    // parameters is read from.
    crate::pcurve::fill(&mut b).map_err(|e| match e {
        Error::NoPcurve { face, .. } => SheetMetalError::Uncertain {
            at: b.faces[face].name,
        },
        _ => SheetMetalError::Uncertain {
            at: map.strips.first().map_or(Name::from_u128(0), |s| s.bend),
        },
    })?;
    let r = regions::classify(&b, map, &folds, eps)?;
    let fold_of: Vec<Option<Fold>> = map
        .strips
        .iter()
        .zip(&r.axes)
        .map(|(s, a)| {
            a.map(|axis| Fold {
                strip: s,
                axis,
                curvature: s.angle / s.length,
            })
        })
        .collect();
    let fold = |k: usize| fold_of[k].expect("a strip that folds");
    // Faces first: an edge fitted on a side needs the side's image.
    let faces: Vec<FaceId> = b.faces().collect();
    let face_images: Vec<FaceImage> = faces
        .par_iter()
        .map(|&f| face_image(&b, &r, map.thickness, eps, &fold, f))
        .collect::<Result<_, _>>()?;
    let face_image_of: BTreeMap<FaceId, &FaceImage> =
        faces.iter().copied().zip(face_images.iter()).collect();
    let edges: Vec<EdgeId> = b.edges().collect();
    let edge_images: Vec<EdgeImage> = edges
        .par_iter()
        .map(|&e| edge_image(&b, &r, &fold, &face_image_of, e))
        .collect::<Result<_, _>>()?;
    let vertices: Vec<VertexId> = b.vertices().collect();
    let points: Vec<Point3> = vertices
        .iter()
        .map(|&v| {
            let p = b.vertices[v].point;
            match r.vertices[&v] {
                Region::Strip(k) => {
                    let f = fold(k);
                    f.point(f.local(p))
                }
                Region::Group(g) => r.motions[g].apply_point(p),
                Region::Across(_) => unreachable!("only faces run across"),
            }
        })
        .collect();
    // The folded body: the same topology, new geometry.
    let mut out = b.clone();
    out.surfaces = Arena::default();
    out.curves = Arena::default();
    out.curves2 = Arena::default();
    for (&f, img) in faces.iter().zip(&face_images) {
        let s = out.surfaces.push(img.surface.clone());
        out.faces[f].surface = s;
        out.faces[f].sense = img.sense;
    }
    let edge_image_of: BTreeMap<EdgeId, &EdgeImage> =
        edges.iter().copied().zip(edge_images.iter()).collect();
    for &e in &edges {
        let img = edge_image_of[&e];
        let c = out.curves.push(img.curve.clone());
        let edge = &mut out.edges[e];
        edge.curve = c;
        edge.interval = img.interval;
        if img.tolerance > edge.tolerance.get() {
            edge.tolerance = Tolerance::new(img.tolerance)
                .map_err(|_| SheetMetalError::Uncertain { at: edge.name })?;
        }
    }
    for (&v, &p) in vertices.iter().zip(&points) {
        out.vertices[v].point = p;
    }
    let fins: Vec<_> = b.fins().collect();
    for fin in fins {
        let face = shape::face_of(&b, fin);
        let e = b.fins[fin].edge;
        let moved = edge_image_of[&e].moved;
        let new = match (&face_image_of[&face].pcurves, b.fins[fin].pcurve) {
            (Pcurves::Keep, Some(pc)) => Some((b.curves2[pc.curve].clone(), pc.map)),
            (Pcurves::Wrap(bx, periods), Some(pc)) => {
                let c = &b.curves2[pc.curve];
                let [t0, t1] = b.edges[e].interval;
                let mid = c.point(pc.param(0.5 * (t0 + t1)));
                let shift = |k: usize, x: f64| {
                    periods[k].map_or(0.0, |p| {
                        p * ((0.5 * (bx[k][0] + bx[k][1]) - x) / p + 0.5).floor()
                    })
                };
                let (du, dv) = (shift(0, mid.x), shift(1, mid.y));
                let c = if du == 0.0 && dv == 0.0 {
                    c.clone()
                } else {
                    curve2::mapped(c, &Affine2::diagonal([du, 1.0], [dv, 1.0]))
                        .map_err(|_| SheetMetalError::Uncertain {
                            at: b.edges[e].name,
                        })?
                        .0
                };
                Some((c, pc.map))
            }
            (Pcurves::Map(a), Some(pc)) => {
                let (c, [r0, r1]) = curve2::mapped(&b.curves2[pc.curve], a).map_err(|_| {
                    SheetMetalError::Uncertain {
                        at: b.edges[e].name,
                    }
                })?;
                Some((c, [r0 + r1 * pc.map[0], r1 * pc.map[1]]))
            }
            _ => None,
        };
        out.fins[fin].pcurve = new.map(|(c, m)| {
            // On the edge's new parameter.
            let m = match moved {
                Some([e0, e1]) => [m[0] - m[1] * e0 / e1, m[1] / e1],
                None => m,
            };
            Pcurve {
                curve: out.curves2.push(c),
                map: m,
            }
        });
    }
    crate::pcurve::fill(&mut out).map_err(|e| match e {
        Error::NoPcurve { face, .. } => SheetMetalError::Uncertain {
            at: out.faces[face].name,
        },
        _ => SheetMetalError::Uncertain {
            at: map.strips.first().map_or(Name::from_u128(0), |s| s.bend),
        },
    })?;
    let fallback = out
        .faces()
        .next()
        .map_or(Name::from_u128(0), |f| out.faces[f].name);
    if let Some(fault) = check::check(&out).first() {
        return Err(SheetMetalError::Uncertain {
            at: fault_at(&out, fault).unwrap_or(fallback),
        });
    }
    let faults = check::check_intersections(&out);
    let crossing = faults.iter().find_map(|f| match f {
        Fault::FacesCross { a, b: c } | Fault::FacesOverlap { a, b: c } => Some((*a, *c)),
        _ => None,
    });
    if let Some((a, c)) = crossing {
        let mut faces = [out.faces[a].name, out.faces[c].name];
        faces.sort_unstable();
        return Err(SheetMetalError::FoldCollides { faces });
    }
    if let Some(fault) = faults.first() {
        return Err(SheetMetalError::Uncertain {
            at: fault_at(&out, fault).unwrap_or(fallback),
        });
    }
    for &f in &faces {
        journal.push(Event::Modified(out.faces[f].name));
    }
    for &e in &edges {
        journal.push(Event::Modified(out.edges[e].name));
    }
    for &v in &vertices {
        journal.push(Event::Modified(out.vertices[v].name));
    }
    Ok((out, journal))
}

/// A plane square to strip `f`'s axis, its natural normal `n`, through a
/// point at `y` of the strip, folded: the plane through the axis frame's
/// origin moved `y` along it.
fn square_plane(f: &Fold, n: Vector3, y: f64) -> Result<Surface, Error> {
    let a = &f.axis;
    let z = if f.along(n).y > 0.0 { a.z() } else { -a.z() };
    let frame = Frame::from_axes(a.origin() + a.z().get() * y, z, a.x())?;
    Ok(Surface::Plane(Plane::new(frame)))
}

fn face_image<'a>(
    b: &Body,
    r: &Regions,
    t: f64,
    eps: f64,
    fold: &(impl Fn(usize) -> Fold<'a> + Sync),
    f: FaceId,
) -> Result<FaceImage, SheetMetalError> {
    let face = &b.faces[f];
    let surface = &b.surfaces[face.surface];
    let at = SheetMetalError::Uncertain { at: face.name };
    let not_radial = SheetMetalError::NotRadial { face: face.name };
    let k = match r.faces[&f] {
        Region::Group(g) => {
            return Ok(FaceImage {
                surface: surface.transformed(&r.motions[g]),
                sense: face.sense,
                pcurves: Pcurves::Keep,
            });
        }
        Region::Across(k) => {
            let fd = fold(k);
            let Surface::Plane(p) = surface else {
                return Err(not_radial);
            };
            let y = fd.local(p.frame().origin()).y;
            return Ok(FaceImage {
                surface: square_plane(&fd, p.frame().z().get(), y).map_err(|_| at)?,
                sense: face.sense,
                pcurves: Pcurves::Drop,
            });
        }
        Region::Strip(k) => k,
    };
    let fd = fold(k);
    let s = fd.strip;
    match surface {
        Surface::Plane(p) => {
            let pf = p.frame();
            let n = fd.along(pf.z().get());
            let o = fd.local(pf.origin());
            if along_axis(n, 2) {
                // A side: the bend's cylinder, its seam opposite the
                // bend's middle, so `u = κ x + π - θ / 2`.
                let z = o.z;
                if !(z.abs() <= eps || (z - t).abs() <= eps) {
                    return Err(not_radial);
                }
                let a = &fd.axis;
                let x = -fd.radial(0.5 * s.length);
                let frame =
                    Frame::from_axes(a.origin(), a.z(), UnitVector3::exact(x).ok_or(at.clone())?)
                        .map_err(|_| at.clone())?;
                let cylinder = Cylinder::new(frame, s.radius + z).map_err(|_| at.clone())?;
                let (px, py) = (fd.along(pf.x().get()), fd.along(pf.y().get()));
                let k = fd.curvature;
                let affine = Affine2 {
                    m: [[k * px.x, k * py.x], [px.y, py.y]],
                    o: crate::geom::Vector2::new(k * o.x + PI - 0.5 * s.angle, o.y),
                };
                // The cylinder's natural normal points out from the axis,
                // along the strip's `z`.
                let outward = match face.sense {
                    Sense::Forward => n.z,
                    Sense::Reversed => -n.z,
                };
                Ok(FaceImage {
                    surface: Surface::Cylinder(cylinder),
                    sense: if outward > 0.0 {
                        Sense::Forward
                    } else {
                        Sense::Reversed
                    },
                    pcurves: Pcurves::Map(affine),
                })
            } else if along_axis(n, 1) {
                Ok(FaceImage {
                    surface: square_plane(&fd, pf.z().get(), o.y).map_err(|_| at)?,
                    sense: face.sense,
                    pcurves: Pcurves::Drop,
                })
            } else if along_axis(n, 0) {
                // Through the axis at `κ x`, its normal along the turn.
                let a = &fd.axis;
                let phi = o.x * fd.curvature;
                let (sn, cs) = crate::math::sin_cos(phi);
                let turn = a.y().get() * cs - a.x().get() * sn;
                let z = if n.x > 0.0 { turn } else { -turn };
                let frame =
                    Frame::from_axes(a.origin(), UnitVector3::exact(z).ok_or(at.clone())?, a.z())
                        .map_err(|_| at.clone())?;
                Ok(FaceImage {
                    surface: Surface::Plane(Plane::new(frame)),
                    sense: face.sense,
                    pcurves: Pcurves::Drop,
                })
            } else if n.z.abs() <= 1e-9 {
                bent(b, &fd, f, at)
            } else {
                Err(not_radial)
            }
        }
        Surface::Cylinder(_) => match surface.frame() {
            Some(cf) if along_axis(fd.along(cf.z().get()), 2) => bent(b, &fd, f, at),
            _ => Err(not_radial),
        },
        Surface::Extrusion(x) if along_axis(fd.along(x.dir().get()), 2) => bent(b, &fd, f, at),
        _ => Err(not_radial),
    }
}

/// How many points a wall's box is read from along each edge.
const BOX_SAMPLES: usize = 16;

/// The box of a wall's own parameters, from points along its edges: in a
/// periodic direction the arc the points leave out the widest gap of (a
/// wall round part of a hole keeps to that part), or the whole period
/// where no gap is wider than an eighth of it (a hole inside the strip).
fn wall_box(b: &Body, f: FaceId, base: &Surface, into: &Rigid) -> Option<[[f64; 2]; 2]> {
    let mut us = Vec::new();
    let mut vs = Vec::new();
    for e in shape::edges_of(b, f) {
        let edge = &b.edges[e];
        let [t0, t1] = edge.interval;
        for i in 0..=BOX_SAMPLES {
            let p = b.curves[edge.curve].point(t0 + (t1 - t0) * i as f64 / BOX_SAMPLES as f64);
            let (u, v) = base.project(into.apply_point(p)).param;
            us.push(u);
            vs.push(v);
        }
    }
    let (pu, pv) = base.periods();
    let whole = base.domain();
    let range = |xs: &mut Vec<f64>, period: Option<f64>, k: usize| -> Option<[f64; 2]> {
        xs.sort_by(f64::total_cmp);
        let (lo, hi) = (*xs.first()?, *xs.last()?);
        let Some(p) = period else {
            return Some([lo, hi]);
        };
        // The widest gap between neighbours round the period.
        let mut gap = (lo + p - hi, hi);
        for w in xs.windows(2) {
            if w[1] - w[0] > gap.0 {
                gap = (w[1] - w[0], w[0]);
            }
        }
        if gap.0 <= p / 8.0 {
            return Some(whole[k]);
        }
        let start = gap.1 + gap.0;
        Some([start, start + (p - gap.0)])
    };
    let ru = range(&mut us, pu, 0)?;
    let rv = range(&mut vs, pv, 1)?;
    Some([ru, rv])
}

/// A wall square to the strip's sides: a [`Bent`] surface of its own
/// surface, moved into the strip's coordinates, over the box its pcurves
/// cover grown by a hundredth each way.
fn bent(b: &Body, fd: &Fold, f: FaceId, at: SheetMetalError) -> Result<FaceImage, SheetMetalError> {
    let face = &b.faces[f];
    let into = Rigid::from_frame(&fd.strip.frame).inverse();
    let base = b.surfaces[face.surface].transformed(&into);
    let [ru, rv] = wall_box(b, f, &base, &into).ok_or(at.clone())?;
    let base_periods = base.periods();
    let grow = |[a, c]: [f64; 2]| {
        let m = 1e-9 * (c - a).max(1.0);
        [a - m, c + m]
    };
    let surface = Bent::new(
        base,
        fd.axis,
        fd.strip.radius,
        fd.curvature,
        [grow(ru), grow(rv)],
    )
    .map_err(|_| at)?;
    let periods = [base_periods.0, base_periods.1];
    let domain = surface.domain();
    Ok(FaceImage {
        surface: Surface::Bent(Arc::new(surface)),
        sense: face.sense,
        pcurves: Pcurves::Wrap(domain, periods),
    })
}

fn edge_image<'a>(
    b: &Body,
    r: &Regions,
    fold: &(impl Fn(usize) -> Fold<'a> + Sync),
    faces: &BTreeMap<FaceId, &FaceImage>,
    e: EdgeId,
) -> Result<EdgeImage, SheetMetalError> {
    let edge = &b.edges[e];
    let at = SheetMetalError::Uncertain { at: edge.name };
    let curve = &b.curves[edge.curve];
    let k = match r.edges[&e] {
        Region::Group(g) => {
            return Ok(EdgeImage {
                curve: curve.transformed(&r.motions[g]),
                interval: edge.interval,
                moved: None,
                tolerance: 0.0,
            });
        }
        Region::Strip(k) => k,
        Region::Across(_) => unreachable!("only faces run across"),
    };
    let fd = fold(k);
    let exact = |curve: Curve| EdgeImage {
        curve,
        interval: edge.interval,
        moved: None,
        tolerance: 0.0,
    };
    if let Curve::Line(l) = curve {
        let o = fd.local(l.origin());
        let d = fd.along(l.dir().get());
        let unit = |v: Vector3| UnitVector3::exact(v).ok_or(at.clone());
        if along_axis(d, 2) {
            // Through the sheet: a radial line.
            let dir = fd.radial(o.x) * d.z.signum();
            let line = Line::from_unit(fd.point(o), unit(dir)?).map_err(|_| at.clone())?;
            return Ok(exact(Curve::Line(line)));
        }
        if along_axis(d, 1) {
            let dir = fd.axis.z().get() * d.y.signum();
            let line = Line::from_unit(fd.point(o), unit(dir)?).map_err(|_| at.clone())?;
            return Ok(exact(Curve::Line(line)));
        }
        if along_axis(d, 0) {
            // Across the axis: a circle about it, its angle `σ κ x`.
            let sigma = d.x.signum();
            let a = &fd.axis;
            let z = if sigma > 0.0 { a.z() } else { -a.z() };
            let frame = Frame::from_axes(a.origin() + a.z().get() * o.y, z, a.x())
                .map_err(|_| at.clone())?;
            let circle = Circle::new(frame, fd.strip.radius + o.z).map_err(|_| at.clone())?;
            let k = fd.curvature;
            let moved = [sigma * k * o.x, k];
            let [t0, t1] = edge.interval;
            return Ok(EdgeImage {
                curve: Curve::Circle(circle),
                interval: [moved[0] + k * t0, moved[0] + k * t1],
                moved: Some(moved),
                tolerance: 0.0,
            });
        }
    }
    // Any other edge lies on a side: fitted along its pcurve there.
    let (fin, image) = b
        .edge_fins(e)
        .find_map(|fin| {
            let img = faces[&shape::face_of(b, fin)];
            matches!(img.pcurves, Pcurves::Map(_)).then_some((fin, img))
        })
        .ok_or(at.clone())?;
    let Pcurves::Map(a) = &image.pcurves else {
        unreachable!("found above");
    };
    let pc = b.fins[fin].pcurve.ok_or(at.clone())?;
    let (c2, [r0, r1]) = curve2::mapped(&b.curves2[pc.curve], a).map_err(|_| at.clone())?;
    let map = [r0 + r1 * pc.map[0], r1 * pc.map[1]];
    let lifted = crate::intersect::fit_along(
        &image.surface,
        &c2,
        map,
        edge.interval,
        // unmetered: sheet metal takes no work
        &crate::work::Meter::free(),
    )
    .map_err(|_| at)?;
    Ok(EdgeImage {
        curve: Curve::Nurbs(Arc::new(lifted.curve)),
        interval: edge.interval,
        moved: None,
        tolerance: lifted.tolerance,
    })
}
