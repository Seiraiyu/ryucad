//! Bend pieces: a concave cylinder face and the convex faces coaxial with
//! it `t` further out, joined at both ends of its angle tangentially to
//! planes along lines parallel to the axis (the bend lines).
//!
//! A piece is found from its inner face: the convex cylinders a box tree
//! over the cylinder faces offers as neighbours (each box grown by its
//! radius, so faces about one axis meet), coaxial with it, larger, whose
//! own bend lines lie at the inner face's two ends. Pieces on one cylinder
//! joining the same flanges become one bend later
//! ([`super::recognize`]).

use super::shape::{
    self, HALF_TURN, Round, across, axial_line, face_tolerance, fins_of, flat, parallel, round,
    unwrap,
};
use crate::SheetError;
use crate::geom::{Box3, Frame};
use crate::intersect::broad::BoxTree;
use crate::intersect::face_box;
use crate::tol::ANGULAR_RESOLUTION;
use crate::topo::{Body, EdgeId, FaceId, Sense};
use core::f64::consts::TAU;

/// A bend line: an edge where a bend cylinder meets a plane tangentially,
/// at end 0 (the start of the inner face's angle) or 1 (its end).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) struct BendLine {
    pub(super) end: usize,
    pub(super) edge: EdgeId,
    /// The plane across the line.
    pub(super) side: FaceId,
    /// Whether the line is on the outer cylinder.
    pub(super) outer: bool,
}

/// An inner face, its outer faces and their bend lines.
#[derive(Clone, Debug, PartialEq)]
pub(super) struct Piece {
    pub(super) inner: FaceId,
    pub(super) outer: Vec<FaceId>,
    /// The inner face's cylinder: angles are its `u`.
    pub(super) frame: Frame,
    pub(super) radius: f64,
    pub(super) outer_radius: f64,
    /// The inner face's angles, `range[0] < range[1]`.
    pub(super) range: [f64; 2],
    pub(super) lines: Vec<BendLine>,
}

impl Piece {
    pub(super) fn angle(&self) -> f64 {
        self.range[1] - self.range[0]
    }

    /// The planes across the bend lines on one cylinder (the outer, or the
    /// inner) at an end: their faces, each once, sorted.
    pub(super) fn sides(&self, end: usize, outer: bool) -> Vec<FaceId> {
        let mut out: Vec<FaceId> = self
            .lines
            .iter()
            .filter(|l| l.end == end && l.outer == outer)
            .map(|l| l.side)
            .collect();
        out.sort_unstable();
        out.dedup();
        out
    }

    pub(super) fn faces(&self) -> Vec<FaceId> {
        let mut out = vec![self.inner];
        out.extend(&self.outer);
        out
    }
}

/// What the cylinder faces of a body make.
#[derive(Clone, Debug, Default, PartialEq)]
pub(super) struct Found {
    pub(super) pieces: Vec<Piece>,
    /// Cylinder faces joined tangentially to planes at both ends of their
    /// angle with no partner: the round half of a corner whose other half
    /// is sharp.
    pub(super) lone: Vec<(FaceId, [Vec<BendLine>; 2])>,
}

/// A face's tangent lines: each straight edge along the axis of `frame`
/// at `radius` from it, across which a plane meets the face tangentially
/// and smoothly, with its angle about the axis moved by whole turns next to
/// `about`.
fn tangent_lines(
    b: &Body,
    f: FaceId,
    frame: &Frame,
    radius: f64,
    about: f64,
    tol: f64,
) -> Vec<(f64, EdgeId, FaceId)> {
    let mut out = Vec::new();
    let sense = b.faces[f].sense;
    for fin in fins_of(b, f) {
        let e = b.fins[fin].edge;
        let eps = tol + shape::edge_tolerance(b, e);
        let Some(at) = axial_line(b, e, frame, radius, eps) else {
            continue;
        };
        let Some(g) = across(b, e, f) else { continue };
        let Some(p) = flat(b, g) else { continue };
        let radial = frame.radial(at);
        let out_normal = match sense {
            Sense::Forward => radial,
            Sense::Reversed => -radial,
        };
        let cross = p.normal.get().cross(out_normal).length();
        let tangent = cross <= ANGULAR_RESOLUTION + eps / radius;
        if tangent && p.normal.get().dot(out_normal) > 0.0 {
            out.push((unwrap(at, about), e, g));
        }
    }
    out
}

/// The ends of a span of angles a face's tangent lines mark, `None` where
/// they mark fewer than two angles `eps` apart.
fn span(lines: &[(f64, EdgeId, FaceId)], radius: f64, eps: f64) -> Option<[f64; 2]> {
    let lo = lines.iter().map(|l| l.0).fold(f64::INFINITY, f64::min);
    let hi = lines.iter().map(|l| l.0).fold(f64::NEG_INFINITY, f64::max);
    ((hi - lo) * radius > eps).then_some([lo, hi])
}

/// Tangent lines at the ends of `range`, as bend lines; lines at other
/// angles are left out.
fn at_ends(
    lines: &[(f64, EdgeId, FaceId)],
    range: [f64; 2],
    radius: f64,
    eps: f64,
    outer: bool,
) -> Vec<BendLine> {
    lines
        .iter()
        .filter_map(|&(a, edge, side)| {
            let end = [0, 1]
                .into_iter()
                .find(|&k| (a - range[k]).abs() * radius <= eps)?;
            Some(BendLine {
                end,
                edge,
                side,
                outer,
            })
        })
        .collect()
}

/// Whether a cylinder's axis is another's within `eps` (a length).
fn coaxial(a: &Round, c: &Round, eps: f64) -> bool {
    let scale = a.radius.max(c.radius);
    if !parallel(a.frame.z(), c.frame.z(), ANGULAR_RESOLUTION + eps / scale) {
        return false;
    }
    shape::axis_distance(&a.frame, c.frame.origin()) <= eps
}

/// Every bend piece of a body, by inner face, and the lone cylinders.
/// Refuses a coaxial pair that closes on itself (a tube: `BendLoop`), and
/// an inner face with a partner whose angle passes half a turn or that
/// has no bend line at an end (`NotSheet`).
pub(super) fn find(b: &Body, tol: f64) -> Result<Found, SheetError> {
    let cylinders: Vec<(FaceId, Round)> = b
        .faces()
        .filter_map(|f| round(b, f).map(|r| (f, r)))
        .collect();
    let boxes: Vec<(Box3, usize)> = cylinders
        .iter()
        .enumerate()
        .map(|(i, (f, r))| (face_box(b, *f).grow(r.radius + tol), i))
        .collect();
    let mut partners: Vec<Vec<usize>> = vec![Vec::new(); cylinders.len()];
    for (i, j) in BoxTree::new(boxes).self_pairs() {
        let (fi, ri) = &cylinders[i];
        let (fj, rj) = &cylinders[j];
        let eps = tol + face_tolerance(b, *fi).max(face_tolerance(b, *fj));
        if !coaxial(ri, rj, eps) {
            continue;
        }
        match (ri.sense, rj.sense) {
            (Sense::Reversed, Sense::Forward) if rj.radius > ri.radius + eps => partners[i].push(j),
            (Sense::Forward, Sense::Reversed) if ri.radius > rj.radius + eps => partners[j].push(i),
            _ => {}
        }
    }
    let mut found = Found::default();
    for (i, (f, r)) in cylinders.iter().enumerate() {
        let eps = tol + face_tolerance(b, *f);
        let name = |f: FaceId| b.faces[f].name;
        let Some(u) = shape::u_range(b, *f) else {
            return Err(SheetError::Uncertain { at: name(*f) });
        };
        let whole = u[1] - u[0] >= TAU - eps / r.radius;
        let about = 0.5 * (u[0] + u[1]);
        let lines = tangent_lines(b, *f, &r.frame, r.radius, about, tol);
        let range = span(&lines, r.radius, eps);
        if r.sense == Sense::Forward || partners[i].is_empty() {
            if let Some(range) = range
                && partners_of(&partners, i).is_empty()
            {
                let ends = at_ends(&lines, range, r.radius, eps, r.sense == Sense::Forward);
                let split = [0, 1].map(|k| ends.iter().filter(|l| l.end == k).copied().collect());
                found.lone.push((*f, split));
            }
            continue;
        }
        let mut outer = Vec::new();
        let mut outer_radius = 0.0;
        let mut all = match range {
            Some(range) => at_ends(&lines, range, r.radius, eps, false),
            None => Vec::new(),
        };
        for &j in &partners[i] {
            let (g, rg) = &cylinders[j];
            let mine = tangent_lines(b, *g, &r.frame, rg.radius, about, tol);
            let matched = match range {
                Some(range) => at_ends(&mine, range, rg.radius, eps, true),
                None => Vec::new(),
            };
            let both = [0, 1].iter().all(|k| matched.iter().any(|l| l.end == *k));
            if whole || both {
                if outer.is_empty() {
                    outer_radius = rg.radius;
                }
                outer.push(*g);
                all.extend(matched);
            }
        }
        if outer.is_empty() {
            continue;
        }
        if whole {
            let mut faces: Vec<_> = core::iter::once(*f).chain(outer).map(name).collect();
            faces.sort_unstable();
            return Err(SheetError::BendLoop {
                bends: vec![faces[0]],
                faces,
            });
        }
        let Some(range) = range else {
            return Err(SheetError::NotSheet { face: name(*f) });
        };
        if range[1] - range[0] > HALF_TURN + eps / r.radius {
            return Err(SheetError::NotSheet { face: name(*f) });
        }
        found.pieces.push(Piece {
            inner: *f,
            outer,
            frame: r.frame,
            radius: r.radius,
            outer_radius,
            range,
            lines: all,
        });
    }
    Ok(found)
}

/// The inner faces a convex cylinder is a partner of.
fn partners_of(partners: &[Vec<usize>], j: usize) -> Vec<usize> {
    partners
        .iter()
        .enumerate()
        .filter(|(_, ps)| ps.contains(&j))
        .map(|(i, _)| i)
        .collect()
}
