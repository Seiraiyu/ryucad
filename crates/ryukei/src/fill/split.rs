//! Fills of five or more sides, or three with a tangent side: one
//! four-sided patch per corner, meeting at a centre.
//!
//! Each side is split at the middle of its parameter. The centre is the
//! mean of the middles, and the centre plane's normal the loop's Newell
//! normal over its corners and middles. Inner curve `k` is a cubic from
//! middle `k` to the centre: it leaves the middle square to the side (in
//! the face's tangent plane where the side is tangent) and arrives in the
//! centre plane, so every patch's corner at the centre lies in that
//! plane. Patch `k` holds corner `k`: `v0` the first half of side `k`,
//! `u1` inner curve `k`, `v1` inner curve `k - 1`, `u0` the second half
//! of side `k - 1` reversed. Across inner curve `k` both patches take the
//! plane field from the plane at middle `k` (the side's tangent and the
//! curve's) to the centre plane, so they meet tangentially.

use super::build::{EdgeSpec, Namer, Piece};
use super::coons::{Quad, U0, U1, V0, V1};
use super::sides::Side;
use super::tangent::{self, Target};
use super::{FillOptions, Plan, refused};
use crate::error::{Error, FillError};
use crate::geom::{KnotVector, NurbsCurve, Point3, Vector3};
use crate::name::{FillRole, Name};
use crate::work::Meter;

/// The plan of a split fill.
pub(crate) fn split(
    sides: &[Side<'_>],
    stand_ins: &[Name],
    namer: &mut Namer,
    options: &FillOptions,
    meter: &Meter,
) -> Result<Plan, Error> {
    let n = sides.len();
    let degenerate = |side: Option<usize>| Error::from(FillError::Degenerate { side });
    // Halves and middles.
    let mut halves: Vec<(NurbsCurve, NurbsCurve)> = Vec::with_capacity(n);
    let mut mids: Vec<(f64, Point3)> = Vec::with_capacity(n);
    for s in sides {
        let [a, b] = s.curve.domain();
        let t = 0.5 * (a + b);
        halves.push(s.curve.split(t)?);
        mids.push((t, s.curve.point(t)));
    }
    let corner = |k: usize| {
        let c = &sides[k].curve;
        c.point(c.domain()[0])
    };
    let centre = Point3::ORIGIN
        + mids
            .iter()
            .fold(Vector3::ZERO, |acc, (_, m)| acc + m.to_vector())
            / n as f64;
    // Newell's normal over corners and middles in loop order.
    let mut ring = Vec::with_capacity(2 * n);
    for k in 0..n {
        ring.push(corner(k));
        ring.push(mids[k].1);
    }
    let mut newell = Vector3::ZERO;
    for k in 0..ring.len() {
        let (p, q) = (ring[k] - centre, ring[(k + 1) % ring.len()] - centre);
        newell += p.cross(q);
    }
    let nc: Vector3 = newell.normalize().ok_or_else(|| degenerate(None))?.into();
    // Inner curves and the plane at each middle.
    let mut inner: Vec<NurbsCurve> = Vec::with_capacity(n);
    let mut planes: Vec<Vector3> = Vec::with_capacity(n);
    let cubic = KnotVector::clamped(3, &[0.0, 1.0], &[4, 4])?;
    for (k, s) in sides.iter().enumerate() {
        let (t, m) = mids[k];
        let tan: Vector3 = s
            .curve
            .tangent(t)
            .ok_or_else(|| degenerate(Some(k)))?
            .into();
        let mut d = centre - m;
        d -= tan * d.dot(tan);
        if let Some(trace) = &s.trace {
            let nf = trace.normal(t);
            d -= nf * d.dot(nf);
        }
        let d: Vector3 = d.normalize().ok_or_else(|| degenerate(Some(k)))?.into();
        let mut plane: Vector3 = tan.cross(d).normalize().ok_or_else(|| degenerate(Some(k)))?.into();
        if plane.dot(nc) < 0.0 {
            plane = -plane;
        }
        if plane.dot(nc) <= 0.0 {
            return Err(degenerate(Some(k)));
        }
        let len = (centre - m).length();
        let mut e = m - centre;
        e -= nc * e.dot(nc);
        if e.length() <= len * 1e-6 {
            return Err(degenerate(Some(k)));
        }
        let pts = vec![m, m + d * (len / 3.0), centre + e / 3.0, centre];
        inner.push(NurbsCurve::new(cubic.clone(), pts, None)?);
        planes.push(plane);
    }
    // Vertices: corners, middles, the centre.
    let mut vertices = Vec::with_capacity(2 * n + 1);
    for k in 0..n {
        let prev = (k + n - 1) % n;
        let name = namer.name(FillRole::Corner, &[stand_ins[prev], stand_ins[k]]);
        vertices.push((corner(k), name));
    }
    for k in 0..n {
        let name = namer.name(FillRole::Middle, &[stand_ins[k]]);
        vertices.push((mids[k].1, name));
    }
    let centre_name = namer.name(FillRole::Centre, &[]);
    vertices.push((centre, centre_name));
    let (vc, vm, vz) = (|k: usize| k, |k: usize| n + k, 2 * n);
    // Patches.
    let mut surfaces = Vec::with_capacity(n);
    let mut side_angles: Vec<Option<f64>> = vec![None; n];
    let mut inner_angle = vec![[0.0f64; 2]; n];
    for k in 0..n {
        let prev = (k + n - 1) % n;
        let v0 = halves[k].0.clone();
        let [a, b] = halves[prev].1.domain();
        let u0 = halves[prev].1.reversed();
        let quad = Quad::new([v0, inner[k].clone(), inner[prev].clone(), u0], 3)?;
        let face = |i: usize, place: usize, flip: Option<f64>| {
            sides[i].trace.as_ref().map(|trace| Target::Face {
                trace,
                map: quad.maps[place].clone(),
                flip,
            })
        };
        let targets: [Option<Target<'_, '_>>; 4] = [
            face(k, V0, None),
            Some(Target::Field {
                a: planes[k],
                b: nc,
                curve: &inner[k],
                back: false,
            }),
            Some(Target::Field {
                a: planes[prev],
                b: nc,
                curve: &inner[prev],
                back: false,
            }),
            face(prev, U0, Some(a + b)),
        ];
        let side_of = |place: usize| match place {
            V0 => Some(k),
            U0 => Some(prev),
            _ => None,
        };
        let (s, _, angles) = tangent::patch(quad, &targets, options.angle, meter)
            .map_err(|e| refused(e, side_of))?;
        for (place, i) in [(V0, k), (U0, prev)] {
            if let Some(a) = angles[place] {
                side_angles[i] = Some(side_angles[i].map_or(a, |x: f64| x.max(a)));
            }
        }
        inner_angle[k][0] = angles[U1].unwrap_or(0.0);
        inner_angle[prev][1] = angles[V1].unwrap_or(0.0);
        surfaces.push(s);
    }
    let inner_bound = inner_angle
        .iter()
        .map(|[a, b]| a + b)
        .fold(0.0, f64::max);
    if inner_bound > options.angle {
        let angle = inner_bound;
        return Err(FillError::AngleTooLarge { side: None, angle }.into());
    }
    // Edges: each side's halves (laminar), then the inner curves.
    let mut edges: Vec<EdgeSpec> = Vec::with_capacity(3 * n);
    let mut piece_edges = vec![[None; 4]; n];
    for k in 0..n {
        let next = (k + 1) % n;
        let first = namer.name(FillRole::Half, &[stand_ins[k], Name::from_u128(0)]);
        piece_edges[k][V0] = Some(edges.len());
        edges.push(EdgeSpec {
            piece: k,
            place: V0,
            start: vc(k),
            end: vm(k),
            name: first,
        });
        // Side k's second half is patch k + 1's u0, which runs from its
        // corner (corner k + 1) to the middle.
        let second = namer.name(FillRole::Half, &[stand_ins[k], Name::from_u128(1)]);
        piece_edges[next][U0] = Some(edges.len());
        edges.push(EdgeSpec {
            piece: next,
            place: U0,
            start: vc(next),
            end: vm(k),
            name: second,
        });
    }
    for k in 0..n {
        let next = (k + 1) % n;
        let name = namer.name(FillRole::Inner, &[stand_ins[k]]);
        piece_edges[k][U1] = Some(edges.len());
        piece_edges[next][V1] = piece_edges[k][U1];
        edges.push(EdgeSpec {
            piece: k,
            place: U1,
            start: vm(k),
            end: vz,
            name,
        });
    }
    let pieces = surfaces
        .into_iter()
        .enumerate()
        .map(|(k, surface)| Piece {
            surface,
            edges: piece_edges[k],
            name: namer.name(FillRole::Patch, &[Name::from_u128(k as u128)]),
        })
        .collect();
    Ok((pieces, edges, vertices, side_angles, Some(inner_bound)))
}
