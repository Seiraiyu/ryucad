//! The draft report: each drafted face's angle from its pull, certified.

use super::{DraftReport, FaceAngle};
use crate::geom::jet::JVec;
use crate::geom::{Surface, UnitVector3, Vector3, sheet::Sheet};
use crate::intersect::face_rect;
use crate::interval::Interval;
use crate::name::Name;
use crate::topo::{Body, EntityId, FaceId, Sense};
use crate::{Error, TweakError};

/// Cells each way a face's box is cut into.
const CELLS: usize = 8;

/// A cell's bounds wider than this (radians) are worked out again over
/// its quarters, at most [`DEPTH`] times.
const WIDTH: f64 = 1e-3;
const DEPTH: u32 = 3;

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

/// Bounds on the sine of the angle from `p` over a cell, from the
/// enclosure of `N = S_u × S_v` alone.
fn enclosed(n: &JVec<Interval>, p: Vector3, sign: f64) -> Option<(f64, f64)> {
    let [x, y, z] = [n[0].v, n[1].v, n[2].v];
    let dot = (x * Interval::point(p.x) + y * Interval::point(p.y) + z * Interval::point(p.z))
        * Interval::point(sign);
    sine_bounds(dot, x * x + y * y + z * z)
}

/// Bounds on the sine `g = σ N · p / |N|` over a cell by the mean value
/// form: `g` at the cell's middle (evaluated, with slack for rounding)
/// plus the enclosure of its gradient times the half-widths. Tight where
/// the angle hardly changes (a drafted surface holds one angle all over),
/// where the enclosure of `N` alone widens with the cell.
fn mean_value(
    sheet: &Sheet,
    cell: [[f64; 2]; 2],
    n: &JVec<Interval>,
    p: Vector3,
    sign: f64,
) -> Option<(f64, f64)> {
    let [[u0, u1], [v0, v1]] = cell;
    let mid = sheet.normal_jets_at((u0 + u1) / 2.0, (v0 + v1) / 2.0)?;
    let m = Vector3::new(mid[0].v, mid[1].v, mid[2].v);
    let len = m.length();
    if len <= 0.0 {
        return None;
    }
    let g0 = sign * m.dot(p) / len;
    let pi = [p.x, p.y, p.z].map(Interval::point);
    let dot3 = |a: [Interval; 3], b: [Interval; 3]| a[0] * b[0] + a[1] * b[1] + a[2] * b[2];
    let nv = [n[0].v, n[1].v, n[2].v];
    let n2 = dot3(nv, nv);
    let l = n2.sqrt()?;
    let l3 = l * n2;
    let np = dot3(nv, pi);
    let grad = |k: usize| -> Option<f64> {
        let d = [n[0].d[k], n[1].d[k], n[2].d[k]];
        let a = dot3(d, pi).checked_div(l)?;
        let b = (np * dot3(nv, d)).checked_div(l3)?;
        Some((a - b).mag())
    };
    let spread = grad(0)? * (u1 - u0) / 2.0 + grad(1)? * (v1 - v0) / 2.0;
    let slack = SLACK * (1.0 + g0.abs()) + 1e-15;
    Some((g0 - spread - slack, g0 + spread + slack))
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
    let cut = move |k: usize, i: usize| {
        let [a, b] = rect[k];
        [
            a + (b - a) * i as f64 / CELLS as f64,
            a + (b - a) * (i + 1) as f64 / CELLS as f64,
        ]
    };
    let mut stack: Vec<([[f64; 2]; 2], u32)> = (0..CELLS)
        .flat_map(|i| (0..CELLS).map(move |j| (i, j)))
        .map(|(i, j)| ([cut(0, i), cut(1, j)], 0))
        .collect();
    while let Some((cell, depth)) = stack.pop() {
        let n = sheet
            .normal_jet_box(cell)
            .ok_or(TweakError::Uncertain { at })?;
        let (mut lo, mut hi) = enclosed(&n, p, sign).ok_or(TweakError::Uncertain { at })?;
        if let Some((a, b)) = mean_value(&sheet, cell, &n, p, sign) {
            (lo, hi) = (lo.max(a), hi.min(b));
        }
        if lo > hi {
            return Err(TweakError::Uncertain { at }.into());
        }
        let (a, b) = (
            crate::math::asin(lo.clamp(-1.0, 1.0)),
            crate::math::asin(hi.clamp(-1.0, 1.0)),
        );
        if b - a > WIDTH && depth < DEPTH {
            let [[u0, u1], [v0, v1]] = cell;
            let (um, vm) = ((u0 + u1) / 2.0, (v0 + v1) / 2.0);
            for c in [
                [[u0, um], [v0, vm]],
                [[um, u1], [v0, vm]],
                [[u0, um], [vm, v1]],
                [[um, u1], [vm, v1]],
            ] {
                stack.push((c, depth + 1));
            }
            continue;
        }
        least = least.min(a);
        greatest = greatest.max(b);
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
