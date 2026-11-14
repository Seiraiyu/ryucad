//! The silhouettes of a body's faces (design 326, phase 2).

use super::View;
use super::closed::{Closed, Raw, closed_form};
use super::edge_on::edge_on;
use super::trim::trim;
use crate::geom::{Curve, Curve2, Surface};
use crate::intersect::{Failure, face_box, face_rect, fit_contour, trace_contour};
use crate::name::Name;
use crate::tol::LINEAR_RESOLUTION;
use crate::topo::{Body, EdgeId, FaceId};
use crate::work::{Meter, Work};
use crate::{Error, ViewError};
use std::sync::Arc;

/// One stretch of a face's silhouette: where the face's natural normal is
/// square to the view. It is no body entity and has no stable name: it is
/// known by its face and its ordinal among the face's stretches (ordered
/// by their least point in the face's parameters), so a drawing can point
/// at it; a dimension that must survive edits names the face.
#[derive(Clone, Debug, PartialEq)]
pub struct Silhouette {
    pub face: FaceId,
    pub face_name: Name,
    pub ordinal: u32,
    pub curve: Curve,
    /// The stretch of the curve's parameter.
    pub range: [f64; 2],
    /// The stretch on the face's surface, and the affine map from the
    /// curve's parameter to the pcurve's.
    pub pcurve: (Curve2, [f64; 2]),
    /// The curve lies within this of the true silhouette, and of the
    /// surface along its pcurve.
    pub tolerance: f64,
}

/// A body's silhouettes seen in a view.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Silhouettes {
    /// By face, then ordinal.
    pub branches: Vec<Silhouette>,
    /// Edges a silhouette runs along (a half cylinder's boundary rulings):
    /// they draw it. Sorted, each once.
    pub on_edges: Vec<EdgeId>,
    /// Faces seen edge-on: no silhouette, their edges draw their image.
    /// Sorted.
    pub edge_on: Vec<FaceId>,
}

/// [`silhouettes_with`] with no cancel or budget.
pub fn silhouettes(body: &Body, view: &View) -> Result<Silhouettes, Error> {
    silhouettes_with(body, view, &Work::unlimited())
}

/// Every face's silhouette in `view`, trimmed to the face. Refused with
/// `ViewError::Uncertain` naming the first face (in face order) whose
/// silhouette couldn't be certified; `Cancelled` and `BudgetExceeded` as
/// every `_with` call.
pub fn silhouettes_with(body: &Body, view: &View, work: &Work) -> Result<Silhouettes, Error> {
    crate::work::run(work, |m| silhouettes_in(body, view, m))
}

/// [`silhouettes_with`] charging `m`, for a caller already under a meter
/// (hidden lines, plan 10.B).
pub(crate) fn silhouettes_in(body: &Body, view: &View, m: &Meter) -> Result<Silhouettes, Error> {
    let faces: Vec<FaceId> = body.faces().collect();
    let per = m.par_map(&faces, |m, &f| of_face(body, f, view, m));
    let mut out = Silhouettes::default();
    for (&f, r) in faces.iter().zip(per) {
        let face = r.map_err(|_| {
            Error::InvalidView(ViewError::Uncertain {
                at: body.faces[f].name,
            })
        })?;
        if face.edge_on {
            out.edge_on.push(f);
        }
        out.branches.extend(face.branches);
        out.on_edges.extend(face.on_edges);
    }
    out.on_edges.sort();
    out.on_edges.dedup();
    Ok(out)
}

/// One face's share.
#[derive(Default)]
struct FaceOut {
    edge_on: bool,
    branches: Vec<Silhouette>,
    on_edges: Vec<EdgeId>,
}

fn of_face(body: &Body, f: FaceId, view: &View, m: &Meter) -> Result<FaceOut, Failure> {
    // A face's silhouette.
    m.charge(1)?;
    let face = &body.faces[f];
    let surface = &body.surfaces[face.surface];
    // A face with no loops (a whole sphere or torus) is its whole surface.
    let whole = body.face_loops(f).next().is_none();
    let rect = if whole {
        surface.domain()
    } else {
        face_rect(body, f).ok_or(Failure::Uncertain)?
    };
    let t = view.towards().get();
    let __t = std::time::Instant::now();
    let __closed = closed_form(surface, t, rect, m);
    super::lap(13, __t);
    let raws = match __closed? {
        Some(Closed::EdgeOn) => {
            return Ok(FaceOut {
                edge_on: true,
                ..FaceOut::default()
            });
        }
        Some(Closed::Branches(b)) => b,
        None => {
            if edge_on(surface, t, rect, m)? {
                return Ok(FaceOut {
                    edge_on: true,
                    ..FaceOut::default()
                });
            }
            let __t = std::time::Instant::now();
            let __tr = traced(body, f, surface, t, m);
            super::lap(14, __t);
            __tr?
        }
    };
    let mut stretches: Vec<(Raw, [f64; 2])> = Vec::new();
    let mut out = FaceOut::default();
    for raw in raws {
        if whole {
            let range = raw.range;
            stretches.push((raw, range));
            continue;
        }
        let trimmed = trim(body, f, &raw, m)?;
        out.on_edges.extend(trimmed.on_edges);
        for r in trimmed.stretches {
            stretches.push((covering(surface, &raw, r, m)?, r));
        }
    }
    let least = |(raw, [a, b]): &(Raw, [f64; 2])| {
        let (c2, map) = &raw.pcurve;
        let p = c2.point(map[0] + map[1] * a);
        let q = c2.point(map[0] + map[1] * b);
        let (p, q) = ((p.x, p.y), (q.x, q.y));
        let lo = if (q.0, q.1) < (p.0, p.1) { q } else { p };
        (lo, (*a, *b))
    };
    stretches.sort_by(|x, y| {
        let (kx, ky) = (least(x), least(y));
        kx.0.0
            .total_cmp(&ky.0.0)
            .then(kx.0.1.total_cmp(&ky.0.1))
            .then(kx.1.0.total_cmp(&ky.1.0))
            .then(kx.1.1.total_cmp(&ky.1.1))
    });
    for (i, (raw, range)) in stretches.into_iter().enumerate() {
        out.branches.push(Silhouette {
            face: f,
            face_name: face.name,
            ordinal: i as u32,
            curve: raw.curve,
            range,
            pcurve: raw.pcurve,
            tolerance: raw.tolerance,
        });
    }
    Ok(out)
}

/// `raw` for its stretch `r`: as it is where its pcurve covers `r`, else
/// (a stretch of a closed curve across its start, past the range a fitted
/// pcurve was made over) with a pcurve fitted again over `r`.
fn covering(surface: &Surface, raw: &Raw, r: [f64; 2], m: &Meter) -> Result<Raw, Failure> {
    let [r0, r1] = raw.range;
    if !matches!(raw.pcurve.0, Curve2::Nurbs2(_)) || (r[0] >= r0 && r[1] <= r1) {
        return Ok(raw.clone());
    }
    let (pc, bound) =
        crate::intersect::fit_pcurve_within(surface, &raw.curve, r, LINEAR_RESOLUTION, 512, m)?;
    Ok(Raw {
        curve: raw.curve.clone(),
        range: r,
        pcurve: (Curve2::Nurbs2(Arc::new(pc)), [0.0, 1.0]),
        tolerance: raw.tolerance.max(bound),
    })
}

/// A traced face's branches, fitted. A branch under ten tolerances in
/// space is a point, and left out.
fn traced(
    body: &Body,
    f: FaceId,
    surface: &Surface,
    t: crate::geom::Vector3,
    m: &Meter,
) -> Result<Vec<Raw>, Failure> {
    let region = face_box(body, f).grow(LINEAR_RESOLUTION);
    let topo = trace_contour(surface, t, &region, m)?;
    let mut out = Vec::new();
    for b in &topo.branches {
        let span = b
            .points
            .windows(2)
            .map(|w| w[0].distance(w[1]))
            .sum::<f64>();
        if span <= 10.0 * LINEAR_RESOLUTION {
            continue;
        }
        let fit = fit_contour(surface, t, b, m)?;
        let range = fit.curve.domain();
        out.push(Raw {
            curve: Curve::Nurbs(Arc::new(fit.curve)),
            range,
            pcurve: (Curve2::Nurbs2(Arc::new(fit.pcurve)), [0.0, 1.0]),
            tolerance: fit.tolerance,
        });
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::geom::{Rigid, Vector3};
    use crate::name::OpKey;
    use crate::primitive;

    #[test]
    fn a_box_has_no_silhouette_and_four_faces_edge_on_seen_along_x() {
        let (b, _) = primitive::block(OpKey(1), 0.01, 0.02, 0.03, &Rigid::IDENTITY).unwrap();
        let iso = View::new(Vector3::new(1.0, -1.0, 1.0), Vector3::new(0.0, 0.0, 1.0)).unwrap();
        let s = silhouettes(&b, &iso).unwrap();
        assert!(s.branches.is_empty() && s.edge_on.is_empty() && s.on_edges.is_empty());
        let side = View::new(Vector3::new(1.0, 0.0, 0.0), Vector3::new(0.0, 0.0, 1.0)).unwrap();
        assert_eq!(silhouettes(&b, &side).unwrap().edge_on.len(), 4);
    }

    #[test]
    fn a_cylinder_side_on_has_two_rulings_numbered_by_their_least_point() {
        let (c, _) = primitive::cylinder(OpKey(2), 0.01, 0.03, &Rigid::IDENTITY).unwrap();
        let front = View::new(Vector3::new(0.0, -1.0, 0.0), Vector3::new(0.0, 0.0, 1.0)).unwrap();
        let s = silhouettes(&c, &front).unwrap();
        assert_eq!(s.branches.len(), 2);
        assert_eq!([s.branches[0].ordinal, s.branches[1].ordinal], [0, 1]);
        let key = |b: &Silhouette| {
            b.pcurve
                .0
                .point(b.pcurve.1[0] + b.pcurve.1[1] * b.range[0])
                .x
        };
        assert!(key(&s.branches[0]) < key(&s.branches[1]));
    }

    #[test]
    fn silhouettes_in_a_free_meter_are_the_public_ones() {
        let (c, _) = primitive::cylinder(OpKey(3), 0.01, 0.03, &Rigid::IDENTITY).unwrap();
        let v = View::new(Vector3::new(1.0, -2.0, 0.5), Vector3::new(0.0, 0.0, 1.0)).unwrap();
        // unmetered: a unit test
        let m = Meter::free();
        assert_eq!(
            silhouettes_in(&c, &v, &m).unwrap(),
            silhouettes(&c, &v).unwrap()
        );
        assert!(m.spent() > 0);
    }
}
