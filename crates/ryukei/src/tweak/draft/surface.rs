//! Drafted surfaces: the plane, cone or binormal sweep through a hinge.

use crate::geom::Vector3;

/// How far from the pull, at least, a hinge's tangent and a face's
/// normal must turn (radians, as a sine) for the draft to have a
/// direction.
pub(crate) const ALONG: f64 = 0.01;

/// The draft's frame at a hinge point.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct Turned {
    /// The pull made square to the hinge's tangent, unit.
    pub(crate) across: Vector3,
    /// Square to the tangent and `across`, on the side the face's outward
    /// normal points.
    pub(crate) side: Vector3,
    /// The drafted outward normal: `cos θ side + sin θ across`.
    pub(crate) normal: Vector3,
    /// The ruling: `cos θ across − sin θ side`.
    pub(crate) ruling: Vector3,
}

/// The draft's frame where the hinge's tangent is `t` and the face's
/// outward normal `n`, for `pull` and the signed `angle`: `None` where
/// either runs within [`ALONG`] of the pull.
pub(crate) fn turned(t: Vector3, n: Vector3, pull: Vector3, angle: f64) -> Option<Turned> {
    let t = t.normalize()?.get();
    let n = n.normalize()?.get();
    let pull = pull.normalize()?.get();
    let dp = pull - t * pull.dot(t);
    if dp.length() < ALONG || n.cross(pull).length() < ALONG {
        return None;
    }
    let across = dp.normalize()?.get();
    let e1 = pull.cross(t).normalize()?.get();
    let k = n.dot(e1);
    if k.abs() < ALONG {
        return None;
    }
    let side = if k > 0.0 { e1 } else { -e1 };
    let (s, c) = crate::math::sin_cos(angle);
    Some(Turned {
        across,
        side,
        normal: side * c + across * s,
        ruling: across * c - side * s,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_walls_draft_tilts_its_normal_to_the_pull_and_rules_in_its_plane() {
        let pull = Vector3::new(0.0, 0.0, 1.0);
        let n = Vector3::new(1.0, 0.0, 0.0);
        let th = 3.0_f64.to_radians();
        let (s, c) = crate::math::sin_cos(th);
        for t in [Vector3::new(0.0, 1.0, 0.0), Vector3::new(0.0, -1.0, 0.0)] {
            let d = turned(t, n, pull, th).unwrap();
            assert!((d.normal - Vector3::new(c, 0.0, s)).length() < 1e-15);
            assert!((d.ruling - Vector3::new(-s, 0.0, c)).length() < 1e-15);
            assert!(d.normal.dot(d.ruling).abs() < 1e-15);
            assert!(d.ruling.dot(t).abs() < 1e-15);
        }
        // A hole's wall (normal -x) opens along the pull.
        let d = turned(Vector3::new(0.0, 1.0, 0.0), -n, pull, th).unwrap();
        assert!((d.ruling - Vector3::new(s, 0.0, c)).length() < 1e-15);
        // A face already at 2° drafts to 3°, not 5°.
        let (s2, c2) = crate::math::sin_cos(2.0_f64.to_radians());
        let d = turned(
            Vector3::new(0.0, 1.0, 0.0),
            Vector3::new(c2, 0.0, s2),
            pull,
            th,
        )
        .unwrap();
        assert!((d.normal.dot(pull) - s).abs() < 1e-15);
    }

    #[test]
    fn a_hinge_or_a_normal_along_the_pull_has_no_draft() {
        let pull = Vector3::new(0.0, 0.0, 1.0);
        assert!(turned(pull, Vector3::new(1.0, 0.0, 0.0), pull, 0.05).is_none());
        assert!(turned(Vector3::new(1.0, 0.0, 0.0), pull, pull, 0.05).is_none());
    }
}
