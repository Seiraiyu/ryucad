//! Cutting a flat at the bend lines of the strips refolding folds, so
//! every face and edge lies in one strip or in none.
//!
//! Each bend line of a folded strip is the plane through it square to the
//! flat, `x = 0` or `x = ba` in the strip's frame, over the strip's span.
//! Every edge crossing it there is split where it crosses (SEMV). Then
//! every face crossing it gets an edge along the cut between two vertices
//! on it: a side (a plane square to the strip's `z`) along the line of the
//! cut, a wall square to the sides (a plane, or a cylinder or extrusion
//! along `z`) along a line through the sheet, wherever the segment's
//! middle is inside the face. An edge joining two loops of a face merges
//! them (MEKR); one joining a loop to itself splits the face (MEF), which
//! is refused where the face still has other loops (which side of the cut
//! they belong on isn't worked out). Planes square to the strip's axis
//! are left whole: refolding takes them to one plane. Each cut is named
//! under a key from the refold's key, the strip's bend and which line it
//! is ([`crate::name::SheetMetalRole::Imprint`]).

use super::Strip;
use super::shape;
use crate::SheetMetalError;
use crate::euler::{Corner, mef, mekr, semv};
use crate::geom::{Curve, Frame, Line, Plane, Point3, Surface, UnitVector3, Vector3};
use crate::intersect::{CurveHit, Place, Side, curve_surface, point_in_face};
use crate::name::{Journal, Name, OpKey, Role, SheetMetalRole};
use crate::tol::LINEAR_RESOLUTION;
use crate::topo::{Body, EdgeId, FaceId, FinId, VertexId};

/// The key the cut at a strip's line `line` (0 near, 1 far) is made under.
fn cut_key(key: OpKey, bend: Name, line: u128) -> OpKey {
    OpKey(
        Name::derive(
            key,
            Role::SheetMetal(SheetMetalRole::Imprint),
            &[bend, Name::from_u128(line)],
        )
        .to_u128(),
    )
}

/// How a face is cut at a bend line: along the line, through the sheet,
/// or not at all.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Cut {
    Along,
    Through,
    None,
}

fn parallel(a: Vector3, b: Vector3) -> bool {
    (a.dot(b).abs() - 1.0).abs() <= 1e-12
}

fn square(a: Vector3, b: Vector3) -> bool {
    a.dot(b).abs() <= 1e-9
}

/// How a face on `surface` is cut in strip `s`.
fn cut_of(surface: &Surface, s: &Strip) -> Cut {
    let (x, y, z) = (s.frame.x().get(), s.frame.y().get(), s.frame.z().get());
    match surface {
        Surface::Plane(p) => {
            let n = p.frame().z().get();
            if parallel(n, z) {
                Cut::Along
            } else if square(n, z) && !parallel(n, y) && !parallel(n, x) {
                Cut::Through
            } else {
                Cut::None
            }
        }
        Surface::Cylinder(_) => match surface.frame() {
            Some(f) if parallel(f.z().get(), z) => Cut::Through,
            _ => Cut::None,
        },
        Surface::Extrusion(e) if parallel(e.dir().get(), z) => Cut::Through,
        _ => Cut::None,
    }
}

/// The fins of a face, each with the vertex it starts at.
fn corners(b: &Body, f: FaceId) -> Vec<(FinId, VertexId)> {
    shape::fins_of(b, f)
        .into_iter()
        .map(|fin| (fin, b.start_of(fin)))
        .collect()
}

/// Cuts the flat at both bend lines of every strip `folds` marks.
pub(super) fn imprint(
    b: &mut Body,
    key: OpKey,
    strips: &[Strip],
    folds: &[bool],
    thickness: f64,
    eps: f64,
    journal: &mut Journal,
) -> Result<(), SheetMetalError> {
    for (k, s) in strips.iter().enumerate() {
        if !folds[k] {
            continue;
        }
        for line in 0..2u128 {
            let c = if line == 0 { 0.0 } else { s.length };
            let sub = cut_key(key, s.bend, line);
            split_edges(b, sub, s, c, thickness, eps, journal)?;
            cut_faces(b, sub, s, c, thickness, eps, journal)?;
        }
    }
    Ok(())
}

/// Whether a point in the strip's coordinates is on its bend line `x =
/// c`, within its span and the sheet.
fn on_line(s: &Strip, t: f64, l: Vector3, c: f64, eps: f64) -> bool {
    (l.x - c).abs() <= eps
        && s.span[0] - eps <= l.y
        && l.y <= s.span[1] + eps
        && -eps <= l.z
        && l.z <= t + eps
}

fn split_edges(
    b: &mut Body,
    key: OpKey,
    s: &Strip,
    c: f64,
    t: f64,
    eps: f64,
    journal: &mut Journal,
) -> Result<(), SheetMetalError> {
    let origin = s.frame.to_world(c, 0.0, 0.0);
    let cut = Surface::Plane(Plane::new(
        Frame::from_axes(origin, s.frame.x(), s.frame.y())
            .map_err(|_| SheetMetalError::Uncertain { at: s.bend })?,
    ));
    let edges: Vec<EdgeId> = b.edges().collect();
    for e in edges {
        let at = SheetMetalError::Uncertain {
            at: b.edges[e].name,
        };
        // Edges whose box keeps to one side of the line, or off the
        // strip's span, can't cross it there.
        let edge = &b.edges[e];
        let curve = b.curves[edge.curve].clone();
        let interval = edge.interval;
        let bx = curve.bounds(interval);
        let (lo, hi) = (bx.min(), bx.max());
        let corners: Vec<Vector3> = (0..8)
            .map(|i| {
                s.frame.to_local(Point3::new(
                    if i & 1 == 0 { lo.x } else { hi.x },
                    if i & 2 == 0 { lo.y } else { hi.y },
                    if i & 4 == 0 { lo.z } else { hi.z },
                ))
            })
            .collect();
        if corners.iter().all(|l| l.x - c > eps) || corners.iter().all(|l| l.x - c < -eps) {
            continue;
        }
        let hits =
            curve_surface(&curve, interval, &cut, LINEAR_RESOLUTION).map_err(|_| at.clone())?;
        let mut ts: Vec<(f64, Point3)> = hits
            .iter()
            .filter_map(|h| match h {
                CurveHit::Point {
                    t: u,
                    point,
                    before,
                    after,
                } if *before != Side::Beyond
                    && *after != Side::Beyond
                    && before != after
                    && on_line(s, t, s.frame.to_local(*point), c, eps) =>
                {
                    Some((*u, *point))
                }
                _ => None,
            })
            .collect();
        ts.sort_by(|a, b| a.0.total_cmp(&b.0));
        let mut current = e;
        for (u, p) in ts {
            let [t0, t1] = b.edges[current].interval;
            if !(t0 < u && u < t1) {
                continue;
            }
            let (made, j) = semv(b, key, current, u, p).map_err(|_| at.clone())?;
            journal.append(j);
            current = made.edge;
        }
    }
    Ok(())
}

/// A segment to add across a face: its two vertices.
type Chord = (VertexId, VertexId);

fn cut_faces(
    b: &mut Body,
    key: OpKey,
    s: &Strip,
    c: f64,
    t: f64,
    eps: f64,
    journal: &mut Journal,
) -> Result<(), SheetMetalError> {
    let faces: Vec<FaceId> = b.faces().collect();
    for f in faces {
        let surface = b.surfaces[b.faces[f].surface].clone();
        let how = cut_of(&surface, s);
        if how == Cut::None {
            continue;
        }
        let at = SheetMetalError::Uncertain {
            at: b.faces[f].name,
        };
        let mut on: Vec<(VertexId, Vector3)> = Vec::new();
        for (_, v) in corners(b, f) {
            let l = s.frame.to_local(b.vertices[v].point);
            if on_line(s, t, l, c, eps) && !on.iter().any(|(w, _)| *w == v) {
                on.push((v, l));
            }
        }
        if on.len() < 2 {
            continue;
        }
        let mut pairs: Vec<Chord> = Vec::new();
        match how {
            Cut::Along => {
                on.sort_by(|a, b| a.1.y.total_cmp(&b.1.y));
                pairs.extend(on.windows(2).map(|w| (w[0].0, w[1].0)));
            }
            _ => {
                for (i, &(a, la)) in on.iter().enumerate() {
                    for &(v, lv) in &on[i + 1..] {
                        if (la.y - lv.y).abs() <= eps && (la.z - lv.z).abs() >= 0.5 * t {
                            pairs.push(if la.z < lv.z { (a, v) } else { (v, a) });
                        }
                    }
                }
            }
        }
        // Keep the segments whose middle is inside the face and that no
        // edge already joins, judged before the face is cut.
        let joined = |b: &Body, a: VertexId, v: VertexId| {
            shape::edges_of(b, f).iter().any(|&e| {
                let edge = &b.edges[e];
                (edge.start == a && edge.end == v) || (edge.start == v && edge.end == a)
            })
        };
        let mut chords = Vec::new();
        for (a, v) in pairs {
            if joined(b, a, v) {
                continue;
            }
            let m = b.vertices[a].point + (b.vertices[v].point - b.vertices[a].point) * 0.5;
            let (pu, pv) = surface.project(m).param;
            match point_in_face(b, f, crate::geom::Point2::new(pu, pv)) {
                Ok(Place::Inside) => chords.push((a, v)),
                Ok(_) => {}
                Err(_) => return Err(at),
            }
        }
        cut_face(b, key, f, chords, journal).map_err(|_| at)?;
    }
    Ok(())
}

/// Adds the chords to face `f` and the faces it splits into: those joining
/// two loops first, then those splitting one.
fn cut_face(
    b: &mut Body,
    key: OpKey,
    f: FaceId,
    mut chords: Vec<Chord>,
    journal: &mut Journal,
) -> Result<(), crate::Error> {
    let mut pieces = vec![f];
    while !chords.is_empty() {
        // The piece and corners of each chord, and whether its ends are
        // in one loop.
        let mut next = None;
        for (i, &(a, v)) in chords.iter().enumerate() {
            for &p in &pieces {
                let cs = corners(b, p);
                let fa: Vec<FinId> = cs.iter().filter(|x| x.1 == a).map(|x| x.0).collect();
                let fv: Vec<FinId> = cs.iter().filter(|x| x.1 == v).map(|x| x.0).collect();
                if fa.is_empty() || fv.is_empty() {
                    continue;
                }
                if fa.len() > 1 || fv.len() > 1 {
                    return Err(crate::Error::Degenerate);
                }
                let one = b.fins[fa[0]].loop_ == b.fins[fv[0]].loop_;
                let better = match next {
                    None => true,
                    Some((_, _, _, _, one_before)) => one_before && !one,
                };
                if better {
                    next = Some((i, p, fa[0], fv[0], one));
                }
            }
        }
        let Some((i, p, fa, fv, one)) = next else {
            return Err(crate::Error::Degenerate);
        };
        let (a, v) = chords.remove(i);
        let (pa, pv) = (b.vertices[a].point, b.vertices[v].point);
        let line = Line::from_unit(
            pa,
            UnitVector3::exact((pv - pa) * (1.0 / pa.distance(pv)))
                .or_else(|| (pv - pa).normalize())
                .ok_or(crate::Error::Degenerate)?,
        )?;
        let curve = b.add_curve(Curve::Line(line));
        let interval = [0.0, pa.distance(pv)];
        if one {
            if b.face_loops(p).count() > 1 {
                return Err(crate::Error::Degenerate);
            }
            let (made, j) = mef(
                b,
                key,
                Corner::Before(fa),
                Corner::Before(fv),
                curve,
                interval,
            )?;
            journal.append(j);
            pieces.push(if made.faces[0] == p {
                made.faces[1]
            } else {
                made.faces[0]
            });
        } else {
            let (_, j) = mekr(
                b,
                key,
                Corner::Before(fa),
                Corner::Before(fv),
                curve,
                interval,
            )?;
            journal.append(j);
        }
    }
    Ok(())
}
