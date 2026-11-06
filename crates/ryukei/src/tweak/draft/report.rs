//! The draft report: each drafted face's angle from its pull, certified.

use super::{DraftReport, FaceAngle};
use crate::geom::{Surface, UnitVector3, sheet::Sheet};
use crate::intersect::face_rect;
use crate::interval::Interval;
use crate::name::Name;
use crate::topo::{Body, EntityId, FaceId, Sense};
use crate::{Error, TweakError};

/// Cells each way a face's box is cut into.
const CELLS: usize = 8;

/// Relative slack for rounding in the bounds' last divisions.
const SLACK: f64 = 1e-12;

/// Bounds on `dot / sqrt(n2)` from enclosures of both, `None` where
/// `n2` may vanish.
pub(crate) fn sine_bounds(dot: Interval, n2: Interval) -> Option<(f64, f64)> {
    if n2.lo() <= 0.0 {
        return None;
    }
    let (small, large) = (crate::math::sqrt(n2.lo()), crate::math::sqrt(n2.hi()));
    let lo = dot.lo() / if dot.lo() >= 0.0 { large } else { small };
    let hi = dot.hi() / if dot.hi() >= 0.0 { small } else { large };
    Some((lo - SLACK * lo.abs() - SLACK, hi + SLACK * hi.abs() + SLACK))
}

/// Face `f` of `out`'s least and greatest angle from `pull`.
fn face_angle(out: &Body, f: FaceId, pull: UnitVector3) -> Result<FaceAngle, Error> {
    let face = &out.faces[f];
    let at = face.name;
    let s = &out.surfaces[face.surface];
    let sign = if face.sense == Sense::Forward {
        1.0
    } else {
        -1.0
    };
    let rect = face_rect(out, f).unwrap_or(s.domain());
    if matches!(s, Surface::Plane(_) | Surface::Cone(_)) {
        let (u, v) = (
            (rect[0][0] + rect[0][1]) / 2.0,
            (rect[1][0] + rect[1][1]) / 2.0,
        );
        let n = s.normal(u, v).ok_or(TweakError::Uncertain { at })?.get() * sign;
        let a = crate::math::asin(n.dot(pull.get()).clamp(-1.0, 1.0));
        return Ok(FaceAngle {
            face: at,
            least: a,
            greatest: a,
        });
    }
    let sheet = Sheet::of(s);
    let p = pull.get();
    let (mut least, mut greatest) = (f64::INFINITY, f64::NEG_INFINITY);
    let cut = |k: usize, i: usize| {
        let [a, b] = rect[k];
        [
            a + (b - a) * i as f64 / CELLS as f64,
            a + (b - a) * (i + 1) as f64 / CELLS as f64,
        ]
    };
    for i in 0..CELLS {
        for j in 0..CELLS {
            let n = sheet
                .normal_jet_box([cut(0, i), cut(1, j)])
                .ok_or(TweakError::Uncertain { at })?;
            let [x, y, z] = [n[0].v, n[1].v, n[2].v];
            let dot =
                (x * Interval::point(p.x) + y * Interval::point(p.y) + z * Interval::point(p.z))
                    * Interval::point(sign);
            let n2 = x * x + y * y + z * z;
            let (lo, hi) = sine_bounds(dot, n2).ok_or(TweakError::Uncertain { at })?;
            least = least.min(crate::math::asin(lo.clamp(-1.0, 1.0)));
            greatest = greatest.max(crate::math::asin(hi.clamp(-1.0, 1.0)));
        }
    }
    Ok(FaceAngle {
        face: at,
        least,
        greatest,
    })
}

/// The report for `drafted` (each face's name in `out` and its pull), in
/// the order of the names.
pub(crate) fn report(out: &Body, drafted: &[(Name, UnitVector3)]) -> Result<DraftReport, Error> {
    let mut faces = Vec::new();
    for &(name, pull) in drafted {
        let Some(EntityId::Face(f)) = out.by_name(name) else {
            return Err(TweakError::NoSuchFace { face: name }.into());
        };
        faces.push(face_angle(out, f, pull)?);
    }
    faces.sort_by_key(|a| a.face);
    Ok(DraftReport { faces })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sines_are_bounded_outward_from_their_enclosures() {
        let iv = |a, b| Interval::new(a, b).unwrap();
        // N · pull in [0.05, 0.06], |N|² in [0.99, 1.01].
        let (lo, hi) = sine_bounds(iv(0.05, 0.06), iv(0.99, 1.01)).unwrap();
        assert!(lo <= 0.05 / crate::math::sqrt(1.01) && hi >= 0.06 / crate::math::sqrt(0.99));
        assert!(lo > 0.049 && hi < 0.061);
        // Negative dots: the larger norm bounds the upper end.
        let (lo, hi) = sine_bounds(iv(-0.06, -0.05), iv(0.99, 1.01)).unwrap();
        assert!(lo <= -0.06 / crate::math::sqrt(0.99) && hi >= -0.05 / crate::math::sqrt(1.01));
        // A norm that may vanish says nothing.
        assert!(sine_bounds(iv(0.0, 0.1), iv(0.0, 1.0)).is_none());
    }
}
