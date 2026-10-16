//! What the caller asks of a mesh.

use super::bound::Targets;
use crate::Error;
use crate::tol::LINEAR_RESOLUTION;
use core::f64::consts::FRAC_PI_2;

/// How accurate a mesh must be, and how large it may grow.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct MeshParams {
    /// The most any point of the mesh may be from the exact geometry, and
    /// any point of the geometry from the mesh, in meters. At least
    /// `10 × LINEAR_RESOLUTION`.
    pub chord: f64,
    /// The most the surface normal may turn across a triangle, and the
    /// tangent along an edge segment, in radians: in `(0, π/2]`.
    pub angle: f64,
    /// The longest a mesh edge may be in model space, if limited: at least
    /// `chord`.
    pub max_edge: Option<f64>,
    /// The most triangles the mesh may have.
    pub max_triangles: u64,
}

impl MeshParams {
    /// The budget [`MeshParams::new`] sets.
    pub const DEFAULT_MAX_TRIANGLES: u64 = 50_000_000;

    /// A chord and angle tolerance, with no edge limit and the default
    /// budget.
    pub fn new(chord: f64, angle: f64) -> Self {
        Self {
            chord,
            angle,
            max_edge: None,
            max_triangles: Self::DEFAULT_MAX_TRIANGLES,
        }
    }

    /// Checks the ranges and gives what each piece must meet. The chord
    /// target keeps `LINEAR_RESOLUTION` back for rounding.
    pub(crate) fn targets(&self) -> Result<Targets, Error> {
        let chord_ok = self.chord.is_finite() && self.chord >= 10.0 * LINEAR_RESOLUTION;
        let angle_ok = self.angle > 0.0 && self.angle <= FRAC_PI_2;
        let edge_ok = self
            .max_edge
            .is_none_or(|e| e.is_finite() && e >= self.chord);
        if !(chord_ok && angle_ok && edge_ok) {
            return Err(Error::InvalidMeshParams);
        }
        Ok(Targets {
            chord: self.chord - LINEAR_RESOLUTION,
            angle: self.angle,
            max_edge: self.max_edge.unwrap_or(f64::INFINITY),
            waive: self.chord,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ranges() {
        let ok = MeshParams::new(1e-3, 0.5);
        assert_eq!(ok.max_triangles, 50_000_000);
        let t = ok.targets().unwrap();
        assert_eq!(
            (t.chord, t.angle, t.max_edge),
            (1e-3 - 1e-8, 0.5, f64::INFINITY)
        );
        let bad = Err(Error::InvalidMeshParams);
        assert_eq!(MeshParams::new(9e-8, 0.5).targets(), bad);
        assert_eq!(MeshParams::new(f64::NAN, 0.5).targets(), bad);
        assert_eq!(MeshParams::new(1e-3, 0.0).targets(), bad);
        assert_eq!(MeshParams::new(1e-3, 1.6).targets(), bad);
        assert!(MeshParams::new(1e-7, FRAC_PI_2).targets().is_ok());
        let edge = |e: f64| MeshParams {
            max_edge: Some(e),
            ..ok
        };
        assert_eq!(edge(1e-4).targets(), bad);
        assert_eq!(edge(f64::INFINITY).targets(), bad);
        assert_eq!(edge(0.1).targets().unwrap().max_edge, 0.1);
    }
}
