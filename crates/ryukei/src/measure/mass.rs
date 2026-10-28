//! Areas, volumes and their moments, by the divergence theorem.
//!
//! Every face integrates `x · n dA` and its higher moments over its own
//! parameter domain on the exact surface: Gauss rules over the triangles
//! of the face's certified mesh in its parameter plane, plus, along each
//! boundary segment, the sliver between the segment and the fin's pcurve,
//! so the domain is the face's own and not the mesh's polygon. Rules of 4,
//! 8 and 16 points a side are tried in turn until two agree within the
//! tolerance on every moment.
//!
//! The quadrature runs over a coarse mesh's triangles (its orders, not
//! the triangles' size, make it accurate); the bound comes from a fine
//! certified mesh: the true surface lies within the chord
//! (plus the edge tolerances) of the triangles, so the volume enclosed by
//! the mesh, worked out exactly from its tetrahedra, is within
//! `2 × chord × curved area` of the body's; first moments within that
//! times the reach from the reference point, second moments times its
//! square. Areas are bounded by the mesh's normal turn: a patch whose
//! normal stays within `angle` of its triangle's has at most `1 / cos`
//! of its area, and boundary segments of curved edges leave strips at
//! most a chord wide. The reported bound is the distance from the value
//! to the mesh's figure plus that.

use super::Value;
use super::gauss;
use super::props::{FaceShape, Properties};
use crate::Error;
use crate::MeasureError;
use crate::geom::{Curve, Point2, Point3, Surface, UnitVector3, Vector2, Vector3};
use crate::intersect::face_box;
use crate::math;
use crate::mesh::{BodyMesh, FaceMesh, MeshParams, tessellate};
use crate::tol::LINEAR_RESOLUTION;
use crate::topo::{Body, EdgeId, FaceId, Sense};
use rayon::prelude::*;
use std::collections::HashMap;

/// How finely [`mass`] works.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct MassOptions {
    /// The relative agreement two quadrature orders must reach.
    pub tolerance: f64,
    /// The mesh chord the bound is certified at, in meters: by default a
    /// ten-thousandth of the body's size (at least
    /// `10 × LINEAR_RESOLUTION`). A finer chord tightens the bound, not
    /// the value, at the cost of a finer mesh.
    pub chord: Option<f64>,
}

impl Default for MassOptions {
    fn default() -> Self {
        Self {
            tolerance: 1e-9,
            chord: None,
        }
    }
}

/// A solid body's mass properties. Inertia is about the centre of mass,
/// in the body's axes, in kg m² (with `density` in kg/m³).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct MassProperties {
    pub volume: Value,
    pub area: Value,
    pub mass: Value,
    pub centre: Point3,
    /// The most the centre may be from the true centre of mass.
    pub centre_bound: f64,
    /// Symmetric: `[[Ixx, Ixy, Ixz], [Ixy, Iyy, Iyz], [Ixz, Iyz, Izz]]`,
    /// products of inertia with their minus sign.
    pub inertia: [[f64; 3]; 3],
    /// The most any element of `inertia` may be off.
    pub inertia_bound: f64,
    /// The eigenvalues of `inertia`, smallest first.
    pub principal_moments: [f64; 3],
    /// Their axes, right-handed.
    pub principal_axes: [UnitVector3; 3],
}

/// The most the normal may turn across a triangle of the mesh the bound
/// uses, in radians: areas are bounded to `1 / cos` of it.
const ANGLE: f64 = 0.05;

/// The normal turn of the coarse mesh the quadrature runs over: the rules'
/// orders, not the triangles' size, make it accurate.
const COARSE_ANGLE: f64 = 0.3;

/// What one face (or the whole body) adds up to, about a reference point:
/// area, volume, first moments `∫x`, and second moments `∫x²`, `∫y²`,
/// `∫z²`, `∫xy`, `∫yz`, `∫zx` over the volume.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
struct Moments {
    area: f64,
    volume: f64,
    first: [f64; 3],
    second: [f64; 6],
}

impl Moments {
    fn add(&mut self, o: &Self, w: f64) {
        self.area += w * o.area;
        self.volume += w * o.volume;
        for k in 0..3 {
            self.first[k] += w * o.first[k];
        }
        for k in 0..6 {
            self.second[k] += w * o.second[k];
        }
    }

    fn abs(&self) -> Self {
        Self {
            area: self.area.abs(),
            volume: self.volume.abs(),
            first: self.first.map(f64::abs),
            second: self.second.map(f64::abs),
        }
    }

    fn values(&self) -> [f64; 11] {
        let [a, b, c] = self.first;
        let [d, e, f, g, h, i] = self.second;
        [self.area, self.volume, a, b, c, d, e, f, g, h, i]
    }

    /// The integrands at a point `x` (from the reference point) with
    /// outward area vector `n` and area element `da`.
    fn at(x: Vector3, n: Vector3, da: f64) -> Self {
        let (px, py, pz) = (x.x, x.y, x.z);
        Self {
            area: da,
            volume: x.dot(n) / 3.0,
            first: [
                px * px / 2.0 * n.x,
                py * py / 2.0 * n.y,
                pz * pz / 2.0 * n.z,
            ],
            second: [
                px * px * px / 3.0 * n.x,
                py * py * py / 3.0 * n.y,
                pz * pz * pz / 3.0 * n.z,
                px * px * py / 2.0 * n.x,
                py * py * pz / 2.0 * n.y,
                pz * pz * px / 2.0 * n.z,
            ],
        }
    }
}

/// Consecutive samples of an edge, by their positions: the edge and the
/// parameters at both.
type Sides = HashMap<(u32, u32), Vec<(EdgeId, f64, f64)>>;

/// A body's meshes, ready for integration: a coarse one whose parameter
/// triangles the quadrature runs over, and the fine certified one the
/// bound comes from.
struct Meshed<'a> {
    body: &'a Body,
    mesh: BodyMesh,
    coarse: BodyMesh,
    chord: f64,
    origin: Point3,
    reach: f64,
    /// Consecutive samples of an edge: `(from, to)` positions to the edge
    /// and the parameters there.
    sides: Sides,
}

impl<'a> Meshed<'a> {
    fn new(body: &'a Body, chord: Option<f64>) -> Result<Self, Error> {
        let bbox = body
            .faces()
            .map(|f| face_box(body, f))
            .reduce(|a, b| a.union(&b))
            .ok_or(MeasureError::NotAnEntity)?;
        let size = (bbox.max() - bbox.min()).length();
        let chord = chord.unwrap_or(size * 1e-5).max(10.0 * LINEAR_RESOLUTION);
        let meshed = |params: MeshParams| -> Result<BodyMesh, Error> {
            let mesh = tessellate(body, &params).map_err(|e| match e {
                Error::MeshTooLarge { .. } => Error::from(MeasureError::Unsupported),
                e => e,
            })?;
            if !mesh.faults.is_empty() {
                return Err(MeasureError::Unsupported.into());
            }
            Ok(mesh)
        };
        let mesh = meshed(MeshParams::new(chord, ANGLE))?;
        let coarse = meshed(MeshParams::new(
            (size * 1e-3).max(10.0 * LINEAR_RESOLUTION),
            COARSE_ANGLE,
        ))?;
        let mid = bbox.min().lerp(bbox.max(), 0.5);
        let origin = Point3::new(mid.x, mid.y, mid.z);
        let reach = mesh
            .positions
            .iter()
            .map(|p| (*p - origin).length())
            .fold(0.0, f64::max)
            + chord;
        let mut sides = Sides::new();
        for e in &coarse.edges {
            for k in 1..e.positions.len() {
                let (a, b) = (e.positions[k - 1], e.positions[k]);
                let (ta, tb) = (e.params[k - 1], e.params[k]);
                sides.entry((a, b)).or_default().push((e.edge, ta, tb));
                sides.entry((b, a)).or_default().push((e.edge, tb, ta));
            }
        }
        Ok(Self {
            body,
            mesh,
            coarse,
            chord,
            origin,
            reach,
            sides,
        })
    }

    /// The most a mesh point is from the exact geometry: the chord and
    /// the largest edge or vertex tolerance.
    fn slack(&self) -> f64 {
        let b = self.body;
        let edges = b.edges().map(|e| b.edges[e].tolerance.get());
        let vertices = b.vertices().map(|v| b.vertices[v].tolerance.get());
        self.chord + edges.chain(vertices).fold(0.0, f64::max)
    }

    /// One face's moments at a rule of `n` points a side over the coarse
    /// mesh's triangles (`fm` is the coarse mesh's), and the same of
    /// the integrands' absolute values.
    fn face(&self, fm: &FaceMesh, n: usize) -> (Moments, Moments) {
        let b = self.body;
        let face = &b.faces[fm.face];
        let surface = &b.surfaces[face.surface];
        let sign = match face.sense {
            Sense::Forward => 1.0,
            Sense::Reversed => -1.0,
        };
        let h = |uv: Point2| {
            let e = surface.eval(uv.x, uv.y);
            let n = e.du.cross(e.dv);
            Moments::at(e.point - self.origin, n * sign, n.length())
        };
        let corners = &self.coarse.corners;
        let tris = &self.coarse.triangles[fm.triangles.start as usize..fm.triangles.end as usize];
        let (mut sum, mut abs) = (Moments::default(), Moments::default());
        let mut count: HashMap<(u32, u32), u32> = HashMap::new();
        for t in tris {
            for k in 0..3 {
                let (i, j) = (t[k], t[(k + 1) % 3]);
                *count.entry((i.min(j), i.max(j))).or_default() += 1;
            }
            let [p0, p1, p2] = t.map(|c| corners[c as usize].uv);
            let (d1, d2) = (p1 - p0, p2 - p0);
            let jac = d1.cross(d2).abs();
            for &(s, r, w) in gauss::triangle(n) {
                let m = h(p0 + d1 * s + d2 * r);
                sum.add(&m, w * jac);
                abs.add(&m.abs(), w * jac);
            }
        }
        for t in tris {
            let [p0, p1, p2] = t.map(|c| corners[c as usize].uv);
            let ccw = (p1 - p0).cross(p2 - p0) > 0.0;
            for k in 0..3 {
                let (mut i, mut j) = (t[k], t[(k + 1) % 3]);
                if count[&(i.min(j), i.max(j))] != 1 {
                    continue;
                }
                if !ccw {
                    core::mem::swap(&mut i, &mut j);
                }
                if let Some(m) = self.sliver(fm.face, surface, i, j, n, &h) {
                    sum.add(&m.0, 1.0);
                    abs.add(&m.1, 1.0);
                }
            }
        }
        let (p, pa) = self.poles(surface, tris, &count, n, &h);
        sum.add(&p, 1.0);
        abs.add(&pa, 1.0);
        (sum, abs)
    }

    /// The wedges a face's triangles leave at a pole or an apex. There
    /// every triangle has a corner of its own, all at one position but at
    /// different parameters along the collapsed row, so between two
    /// triangles sharing a corner `A` off the row lies the parameter
    /// triangle `(P, A, P')` they don't cover: no area on the surface, but
    /// part of the domain the integrands' Jacobians weigh. Boundary sides
    /// `A → P` and `P' → A` (the face on their left) bound it.
    fn poles(
        &self,
        surface: &Surface,
        tris: &[[u32; 3]],
        count: &HashMap<(u32, u32), u32>,
        n: usize,
        h: &impl Fn(Point2) -> Moments,
    ) -> (Moments, Moments) {
        let (mut sum, mut abs) = (Moments::default(), Moments::default());
        let rows = crate::mesh::uv::collapsed_rows(surface);
        if !rows.iter().flatten().any(|&r| r) {
            return (sum, abs);
        }
        let domain = surface.domain();
        let corners = &self.coarse.corners;
        // Which collapsed row a corner lies on, if any.
        let row = |c: u32| {
            let uv = corners[c as usize].uv;
            (0..2)
                .flat_map(|d| (0..2).map(move |e| (d, e)))
                .find(|&(d, e)| {
                    let x = if d == 0 { uv.x } else { uv.y };
                    let end = domain[d][e];
                    rows[d][e] && (x - end).abs() <= 1e-9 * (1.0 + end.abs())
                })
        };
        let mut to_pole: HashMap<u32, u32> = HashMap::new();
        let mut from_pole: HashMap<u32, u32> = HashMap::new();
        for t in tris {
            let [p0, p1, p2] = t.map(|c| corners[c as usize].uv);
            let ccw = (p1 - p0).cross(p2 - p0) > 0.0;
            for k in 0..3 {
                let (mut i, mut j) = (t[k], t[(k + 1) % 3]);
                if count[&(i.min(j), i.max(j))] != 1 {
                    continue;
                }
                if !ccw {
                    core::mem::swap(&mut i, &mut j);
                }
                match (row(i), row(j)) {
                    (None, Some(_)) => {
                        to_pole.insert(i, j);
                    }
                    (Some(_), None) => {
                        from_pole.insert(j, i);
                    }
                    _ => {}
                }
            }
        }
        let mut wedges: Vec<(u32, u32, u32)> = to_pole
            .iter()
            .filter_map(|(&a, &p)| from_pole.get(&a).map(|&q| (p, a, q)))
            .filter(|&(p, _, q)| row(p) == row(q))
            .collect();
        wedges.sort_unstable();
        for (p, a, q) in wedges {
            let [p0, p1, p2] = [p, a, q].map(|c| corners[c as usize].uv);
            let (d1, d2) = (p1 - p0, p2 - p0);
            let det = d1.cross(d2);
            for &(s, r, w) in gauss::triangle(n) {
                let m = h(p0 + d1 * s + d2 * r);
                sum.add(&m, w * det);
                abs.add(&m.abs(), w * det.abs());
            }
        }
        (sum, abs)
    }

    /// The integral over the sliver between the boundary segment from
    /// corner `i` to corner `j` (the face on its left in the parameter
    /// plane) and the pcurve of the edge it samples: positive where the
    /// pcurve bulges out of the face's triangles. `None` for a segment
    /// that samples no edge of the face (a seam's or a pole's side, which
    /// is straight in the parameter plane).
    fn sliver(
        &self,
        face: FaceId,
        surface: &Surface,
        i: u32,
        j: u32,
        n: usize,
        h: &impl Fn(Point2) -> Moments,
    ) -> Option<(Moments, Moments)> {
        let b = self.body;
        let (ci, cj) = (
            self.coarse.corners[i as usize],
            self.coarse.corners[j as usize],
        );
        let candidates = self.sides.get(&(ci.position, cj.position))?;
        let (pc, ta, tb) = candidates.iter().find_map(|&(e, ta, tb)| {
            b.edge_fins(e)
                .find(|&f| b.loops[b.fins[f].loop_].face == face)
                .and_then(|f| b.fins[f].pcurve)
                .map(|pc| (pc, ta, tb))
        })?;
        let c2 = &b.curves2[pc.curve];
        let at = |t: f64| c2.eval(pc.param(t));
        // Move the pcurve by whole periods onto the corners' unwrapped
        // parameters.
        let (pu, pv) = surface.periods();
        let start = at(ta).point;
        let shift = |d: f64, p: Option<f64>| p.map_or(0.0, |p| (d / p).round() * p);
        let off = Vector2::new(shift(ci.uv.x - start.x, pu), shift(ci.uv.y - start.y, pv));
        let (a, bb) = (ci.uv, cj.uv);
        let chord = bb - a;
        let dt = tb - ta;
        let (mut sum, mut abs) = (Moments::default(), Moments::default());
        for &(s, ws) in gauss::rule(n) {
            let e = at(ta + s * dt);
            let c = e.point + off;
            let dc = e.d1 * (pc.map[1] * dt);
            let l = a + chord * s;
            let gap = c - l;
            for &(lam, wl) in gauss::rule(n) {
                let x = l + gap * lam;
                let xs = chord + (dc - chord) * lam;
                let det = xs.cross(gap);
                let m = h(x);
                sum.add(&m, -ws * wl * det);
                abs.add(&m.abs(), ws * wl * det.abs());
            }
        }
        Some((sum, abs))
    }

    /// A face's moments, the order raised until two agree within `rel`.
    fn converged(&self, fm: &FaceMesh, rel: f64) -> Moments {
        let (mut last, _) = self.face(fm, gauss::ORDERS[0]);
        for &n in &gauss::ORDERS[1..] {
            let (now, abs) = self.face(fm, n);
            let agree = now
                .values()
                .iter()
                .zip(last.values())
                .zip(abs.values())
                .all(|((a, b), s)| (a - b).abs() <= rel * s);
            last = now;
            if agree {
                break;
            }
        }
        last
    }

    /// The moments of the polyhedron the face's triangles bound with the
    /// reference point, and the face's mesh area.
    fn mesh_face(&self, fm: &FaceMesh) -> Moments {
        let mut out = Moments::default();
        let tris = &self.mesh.triangles[fm.triangles.start as usize..fm.triangles.end as usize];
        for t in tris {
            let [a, b, c] = t.map(|k| {
                self.mesh.positions[self.mesh.corners[k as usize].position as usize] - self.origin
            });
            let det = a.dot(b.cross(c));
            let s = a + b + c;
            out.area += (b - a).cross(c - a).length() / 2.0;
            out.volume += det / 6.0;
            for k in 0..3 {
                out.first[k] += det / 24.0 * comp(s, k);
            }
            let pts = [a, b, c];
            let pair = |i: usize, j: usize| {
                pts.iter().map(|p| comp(*p, i) * comp(*p, j)).sum::<f64>() + comp(s, i) * comp(s, j)
            };
            let second = [
                pair(0, 0),
                pair(1, 1),
                pair(2, 2),
                pair(0, 1),
                pair(1, 2),
                pair(2, 0),
            ];
            for (o, x) in out.second.iter_mut().zip(second) {
                *o += det / 120.0 * x;
            }
        }
        out
    }

    /// Whether the face's surface is a plane (its mesh then lies on it).
    fn planar(&self, f: FaceId) -> bool {
        matches!(
            self.body.surfaces[self.body.faces[f].surface],
            Surface::Plane(_)
        )
    }

    /// The total length of the face's edges that aren't lines, by their
    /// mesh polylines (which are shorter than the edges by at most what
    /// the strips' bound already allows).
    fn curved_boundary(&self, f: FaceId) -> f64 {
        let b = self.body;
        let mut total = 0.0;
        let mut seen: Vec<EdgeId> = Vec::new();
        for l in b.face_loops(f) {
            for fin in b.loop_fins(l) {
                let e = b.fins[fin].edge;
                if seen.contains(&e) || matches!(b.curves[b.edges[e].curve], Curve::Line(_)) {
                    continue;
                }
                seen.push(e);
                if let Some(p) = self.mesh.edges.iter().find(|p| p.edge == e) {
                    total += p
                        .positions
                        .windows(2)
                        .map(|w| {
                            self.mesh.positions[w[0] as usize]
                                .distance(self.mesh.positions[w[1] as usize])
                        })
                        .sum::<f64>();
                }
            }
        }
        total
    }
}

fn comp(v: Vector3, k: usize) -> f64 {
    match k {
        0 => v.x,
        1 => v.y,
        _ => v.z,
    }
}

/// The properties of face `f`: its area, bounded, and its shape.
pub(crate) fn face_properties(body: &Body, f: FaceId, rel: f64) -> Result<Properties, Error> {
    let face = body.face(f).ok_or(MeasureError::NotAnEntity)?;
    let m = Meshed::new(body, None)?;
    let find = |mesh: &BodyMesh| {
        mesh.faces
            .iter()
            .find(|fm| fm.face == f)
            .cloned()
            .ok_or(MeasureError::Unsupported)
    };
    let q = m.converged(&find(&m.coarse)?, rel);
    let mesh = m.mesh_face(&find(&m.mesh)?);
    let bound = area_bound(&m, f, q.area, mesh.area);
    let sign = |n: UnitVector3| match face.sense {
        Sense::Forward => n,
        Sense::Reversed => -n,
    };
    let shape = match &body.surfaces[face.surface] {
        Surface::Plane(p) => FaceShape::Plane {
            origin: p.frame().origin(),
            normal: sign(p.frame().z()),
        },
        Surface::Cylinder(c) => {
            let fr = c_frame(&body.surfaces[face.surface]);
            FaceShape::Cylinder {
                origin: fr.origin(),
                axis: fr.z(),
                radius: c.radius(),
            }
        }
        Surface::Cone(c) => {
            let fr = c_frame(&body.surfaces[face.surface]);
            FaceShape::Cone {
                apex: fr.origin() + fr.z().get() * c.apex_v(),
                axis: fr.z(),
                half_angle: c.half_angle(),
            }
        }
        Surface::Sphere(s) => FaceShape::Sphere {
            centre: c_frame(&body.surfaces[face.surface]).origin(),
            radius: s.radius(),
        },
        Surface::Torus(t) => {
            let fr = c_frame(&body.surfaces[face.surface]);
            FaceShape::Torus {
                centre: fr.origin(),
                axis: fr.z(),
                major: t.major(),
                minor: t.minor(),
            }
        }
        _ => FaceShape::Other,
    };
    Ok(Properties::Face {
        area: Value::Bounded {
            value: q.area,
            bound,
        },
        shape,
    })
}

fn c_frame(s: &Surface) -> &crate::geom::Frame {
    s.frame().expect("an analytic surface has a frame")
}

/// The bound on face `f`'s area: the quadrature's distance from the
/// mesh's area, the normal turn's allowance on a curved face, and strips a
/// chord wide (twice, for safety) along its curved edges.
fn area_bound(m: &Meshed<'_>, f: FaceId, q: f64, mesh: f64) -> f64 {
    let turn = if m.planar(f) {
        0.0
    } else {
        mesh * (1.0 / math::cos(ANGLE) - 1.0)
    };
    (q - mesh).abs() + turn + 2.0 * m.slack() * m.curved_boundary(f)
}

/// The mass properties of `body`, a solid that passes
/// [`crate::check::check`], at `density` (kg/m³). Fails with
/// `NotAnEntity` for a body with no faces and `Unsupported` for one whose
/// faces don't all mesh.
pub fn mass(body: &Body, density: f64, opts: &MassOptions) -> Result<MassProperties, Error> {
    let m = Meshed::new(body, opts.chord)?;
    // Both meshes hold every face (neither has faults), in face order.
    let per_face: Vec<(FaceId, Moments, Moments)> = m
        .coarse
        .faces
        .par_iter()
        .zip(m.mesh.faces.par_iter())
        .map(|(c, f)| (f.face, m.converged(c, opts.tolerance), m.mesh_face(f)))
        .collect();
    let (mut q, mut mesh) = (Moments::default(), Moments::default());
    let (mut curved_area, mut area_bound_sum) = (0.0, 0.0);
    for (f, qf, mf) in &per_face {
        q.add(qf, 1.0);
        mesh.add(mf, 1.0);
        if !m.planar(*f) {
            curved_area += mf.area;
        }
        area_bound_sum += area_bound(&m, *f, qf.area, mf.area);
    }
    let slab = 2.0 * m.slack() * curved_area;
    let r = m.reach;
    let b_volume = (q.volume - mesh.volume).abs() + slab;
    let b_first = (0..3)
        .map(|k| (q.first[k] - mesh.first[k]).abs())
        .fold(0.0, f64::max)
        + slab * r;
    let b_second = (0..6)
        .map(|k| (q.second[k] - mesh.second[k]).abs())
        .fold(0.0, f64::max)
        + slab * r * r;
    let v = q.volume;
    let c = Vector3::new(q.first[0] / v, q.first[1] / v, q.first[2] / v);
    let centre = m.origin + c;
    let centre_bound = if v > b_volume {
        3f64.sqrt() * (b_first + c.length() * b_volume) / (v - b_volume)
    } else {
        f64::INFINITY
    };
    // Second moments about the centre of mass.
    let [sxx, syy, szz, sxy, syz, szx] = q.second;
    let s = |a: f64, i: f64, j: f64| a - v * i * j;
    let (xx, yy, zz) = (s(sxx, c.x, c.x), s(syy, c.y, c.y), s(szz, c.z, c.z));
    let (xy, yz, zx) = (s(sxy, c.x, c.y), s(syz, c.y, c.z), s(szx, c.z, c.x));
    let inertia = [
        [yy + zz, -xy, -zx],
        [-xy, xx + zz, -yz],
        [-zx, -yz, xx + yy],
    ]
    .map(|row| row.map(|x| x * density));
    let inertia_bound = density * (2.0 * b_second + 2.0 * (2.0 * r * b_first + r * r * b_volume));
    let (principal_moments, principal_axes) = principal(&inertia);
    Ok(MassProperties {
        volume: Value::Bounded {
            value: v,
            bound: b_volume,
        },
        area: Value::Bounded {
            value: q.area,
            bound: area_bound_sum,
        },
        mass: Value::Bounded {
            value: v * density,
            bound: b_volume * density,
        },
        centre,
        centre_bound,
        inertia,
        inertia_bound,
        principal_moments,
        principal_axes,
    })
}

/// The eigenvalues of a symmetric matrix (an inertia tensor), smallest
/// first, and their axes (right-handed), by cyclic Jacobi rotations: a
/// fixed sequence of operations, so the same bits everywhere.
pub fn principal(a: &[[f64; 3]; 3]) -> ([f64; 3], [UnitVector3; 3]) {
    let mut a = *a;
    let mut v = [[1.0, 0.0, 0.0], [0.0, 1.0, 0.0], [0.0, 0.0, 1.0]];
    for _ in 0..50 {
        let off = a[0][1].abs() + a[1][2].abs() + a[0][2].abs();
        let scale = a[0][0].abs() + a[1][1].abs() + a[2][2].abs();
        if off <= 1e-18 * scale || off == 0.0 {
            break;
        }
        for (p, q) in [(0, 1), (1, 2), (0, 2)] {
            if a[p][q] == 0.0 {
                continue;
            }
            let theta = (a[q][q] - a[p][p]) / (2.0 * a[p][q]);
            let t = theta.signum() / (theta.abs() + math::sqrt(theta * theta + 1.0));
            let t = if theta == 0.0 { 1.0 } else { t };
            let c = 1.0 / math::sqrt(t * t + 1.0);
            let s = t * c;
            for row in &mut a {
                let (akp, akq) = (row[p], row[q]);
                row[p] = c * akp - s * akq;
                row[q] = s * akp + c * akq;
            }
            let (rp, rq) = (a[p], a[q]);
            for k in 0..3 {
                a[p][k] = c * rp[k] - s * rq[k];
                a[q][k] = s * rp[k] + c * rq[k];
            }
            for row in &mut v {
                let (vp, vq) = (row[p], row[q]);
                row[p] = c * vp - s * vq;
                row[q] = s * vp + c * vq;
            }
        }
    }
    let mut order = [0, 1, 2];
    order.sort_by(|&i, &j| a[i][i].total_cmp(&a[j][j]));
    let col = |k: usize| Vector3::new(v[0][k], v[1][k], v[2][k]);
    let e0 = col(order[0]).normalize().unwrap_or(UnitVector3::X);
    let e1 = col(order[1]).normalize().unwrap_or(UnitVector3::Y);
    let e2 = e0
        .get()
        .cross(e1.get())
        .normalize()
        .unwrap_or(UnitVector3::Z);
    (order.map(|k| a[k][k]), [e0, e1, e2])
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::geom::Rigid;
    use crate::name::OpKey;
    use crate::primitive;
    use core::f64::consts::PI;

    fn near(v: Value, want: f64, rel: f64) {
        assert!(v.holds(want, 1e-15), "{v:?} doesn't hold {want}");
        assert!(
            (v.value() - want).abs() <= rel * want.abs(),
            "{v:?} against {want}"
        );
    }

    #[test]
    fn a_box_is_exact_but_for_rounding() {
        let (b, _) = primitive::block(OpKey(1), 2.0, 3.0, 4.0, &Rigid::IDENTITY).unwrap();
        let m = mass(&b, 2.0, &MassOptions::default()).unwrap();
        near(m.volume, 24.0, 1e-13);
        near(m.area, 52.0, 1e-13);
        near(m.mass, 48.0, 1e-13);
        assert!(m.volume.bound() < 1e-12, "{:?}", m.volume);
        assert!(m.centre.distance(Point3::new(1.0, 1.5, 2.0)) < 1e-12);
        // A box's moments about its centre: m (b² + c²) / 12, ...
        let want = [
            48.0 * (9.0 + 16.0) / 12.0,
            48.0 * (4.0 + 16.0) / 12.0,
            48.0 * (4.0 + 9.0) / 12.0,
        ];
        for (k, w) in want.iter().enumerate() {
            assert!((m.inertia[k][k] - w).abs() < 1e-11, "{:?}", m.inertia);
        }
        assert!(m.inertia[0][1].abs() < 1e-11);
        assert_eq!(
            m.principal_moments.map(|x| (x * 1e6).round()),
            [52.0e6, 80.0e6, 100.0e6]
        );
    }

    #[test]
    fn a_sphere_converges_on_its_closed_forms() {
        let (b, _) = primitive::sphere(OpKey(2), 0.5, &Rigid::IDENTITY).unwrap();
        let m = mass(&b, 1.0, &MassOptions::default()).unwrap();
        let (r, v) = (0.5f64, 4.0 / 3.0 * PI * 0.125);
        near(m.volume, v, 1e-9);
        near(m.area, 4.0 * PI * r * r, 1e-9);
        let i = 0.4 * v * r * r;
        for k in 0..3 {
            assert!((m.inertia[k][k] - i).abs() <= 1e-8 * i + m.inertia_bound);
            assert!((m.inertia[k][k] - i).abs() <= 1e-8 * i, "{:?}", m.inertia);
        }
        assert!(m.centre.distance(Point3::ORIGIN) <= 1e-12 + m.centre_bound);
    }

    #[test]
    fn a_cylinder_and_a_cone() {
        let (c, _) = primitive::cylinder(OpKey(3), 0.2, 1.0, &Rigid::IDENTITY).unwrap();
        let m = mass(&c, 1.0, &MassOptions::default()).unwrap();
        near(m.volume, PI * 0.04, 1e-9);
        near(m.area, 2.0 * PI * 0.2 * 1.0 + 2.0 * PI * 0.04, 1e-9);
        let (k, _) = primitive::cone(OpKey(4), 0.3, 0.6, &Rigid::IDENTITY).unwrap();
        let m = mass(&k, 1.0, &MassOptions::default()).unwrap();
        near(m.volume, PI * 0.09 * 0.6 / 3.0, 1e-9);
    }

    #[test]
    fn principal_axes_of_a_rotated_tensor() {
        let a = [[2.0, 1.0, 0.0], [1.0, 2.0, 0.0], [0.0, 0.0, 5.0]];
        let (m, axes) = principal(&a);
        assert!(
            (m[0] - 1.0).abs() < 1e-14 && (m[1] - 3.0).abs() < 1e-14 && (m[2] - 5.0).abs() < 1e-14
        );
        assert!((axes[0].get().x.abs() - 0.5f64.sqrt()).abs() < 1e-14);
        assert!((axes[2].get().z.abs() - 1.0).abs() < 1e-14);
    }
}
