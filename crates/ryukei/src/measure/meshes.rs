//! A box tree over a mesh's triangles, for callers outside the kernel
//! that judge meshes against each other (ryucad's interference with
//! reference imports, design 364): the nearest pair between two meshes,
//! the nearest point of one to a point, the pairs within a distance, and
//! the winding number of a closed mesh about a point, decided by exact
//! orientation tests. Distances are plain f64: the caller keeps a margin
//! for rounding.

use super::prims::Prim;
use super::tree::{Piece, Tree};
use crate::geom::{Point3, Vector3};
use crate::predicates::{Sign, orient3d};

/// Ray directions for winding numbers, away from the axes so a ray
/// rarely runs along a mesh's edges (heal's shell judging's).
const DIRECTIONS: [[f64; 3]; 4] = [
    [0.8432, 0.3721, 0.3879],
    [-0.2914, 0.9137, 0.2833],
    [0.1871, -0.4187, 0.8886],
    [-0.7313, -0.5917, 0.3393],
];

/// A mesh's triangles in a box tree, each tagged by its index.
pub struct MeshTree {
    tree: Tree,
    triangles: Vec<[Point3; 3]>,
}

/// The nearest pair of two meshes: the distance, and each side's point
/// and triangle.
#[derive(Clone, Copy, Debug)]
pub struct MeshNear {
    pub distance: f64,
    pub a: (Point3, u32),
    pub b: (Point3, u32),
}

impl MeshTree {
    pub fn new(triangles: Vec<[Point3; 3]>) -> Self {
        let pieces = triangles
            .iter()
            .enumerate()
            .map(|(k, t)| Piece {
                prim: Prim::Triangle(*t),
                tag: k as u32,
            })
            .collect();
        Self {
            tree: Tree::new(pieces),
            triangles,
        }
    }

    pub fn triangles(&self) -> &[[Point3; 3]] {
        &self.triangles
    }

    /// The box round every triangle, `None` for none.
    pub fn bounds(&self) -> Option<(Point3, Point3)> {
        self.tree.bounds()
    }

    /// The nearest pair of triangles of `self` and `other` (zero where
    /// they cross).
    pub fn nearest(&self, other: &MeshTree) -> Option<MeshNear> {
        self.tree.nearest(&other.tree).map(|n| MeshNear {
            distance: n.distance,
            a: n.a,
            b: n.b,
        })
    }

    /// The distance from `p` to the nearest triangle, that triangle's
    /// nearest point and its index.
    pub fn nearest_to(&self, p: Point3) -> Option<(f64, Point3, u32)> {
        let one = Tree::new(vec![Piece {
            prim: Prim::Point(p),
            tag: 0,
        }]);
        one.nearest(&self.tree).map(|n| (n.distance, n.b.0, n.b.1))
    }

    /// Every pair `(i, j)` of a triangle of `self` and one of `other`
    /// within `d` of each other, sorted.
    pub fn within(&self, other: &MeshTree, d: f64) -> Vec<(u32, u32)> {
        self.tree.within(&other.tree, d)
    }

    /// The signed number of times the triangles wind round `p`: a ray's
    /// crossings, +1 where it leaves through a triangle's front (the side
    /// its corners wind counterclockwise about), by exact orientation
    /// tests against the triangles whose boxes the ray meets. `None`
    /// where `p` lies on a triangle or every ray tried passes through an
    /// edge or a corner. Meaningful only for a closed mesh.
    pub fn winding(&self, p: Point3) -> Option<i64> {
        let (lo, hi) = self.bounds()?;
        let reach = 2.0 * (hi - lo).length() + 2.0 * lo.distance(p) + 1.0;
        'rays: for d in DIRECTIONS {
            let q = p + Vector3::new(d[0], d[1], d[2]) * reach;
            let mut w = 0i64;
            for k in self.tree.along(p, q, 0.0) {
                let [a, b, c] = self.triangles[k as usize];
                let (sp, sq) = (orient3d(a, b, c, p), orient3d(a, b, c, q));
                if sq == Sign::Zero {
                    continue 'rays;
                }
                if sp == Sign::Zero || sp == sq {
                    // In the triangle's plane: on it or off it, the ray
                    // meets that plane only at `p`.
                    if sp == Sign::Zero && super::prims::point_triangle(p, &[a, b, c]) == p {
                        return None;
                    }
                    continue;
                }
                let s = [
                    orient3d(p, q, a, b),
                    orient3d(p, q, b, c),
                    orient3d(p, q, c, a),
                ];
                if s.contains(&Sign::Zero) {
                    if s.contains(&Sign::Positive) && s.contains(&Sign::Negative) {
                        continue;
                    }
                    continue 'rays;
                }
                if s[0] == s[1] && s[1] == s[2] {
                    w += if sp == Sign::Positive { 1 } else { -1 };
                }
            }
            return Some(w);
        }
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::geom::{Point3, Rigid};
    use crate::mesh::{MeshParams, tessellate};
    use crate::name::OpKey;

    fn mesh_of(body: &crate::topo::Body) -> Vec<[Point3; 3]> {
        let m = tessellate(body, &MeshParams::new(1e-4, 0.3)).unwrap();
        m.welded_triangles()
            .map(|t| t.map(|k| m.positions[k as usize]))
            .collect()
    }

    fn block(at: [f64; 3], side: f64) -> crate::topo::Body {
        let r = Rigid::translation(crate::geom::Vector3::new(at[0], at[1], at[2]));
        crate::primitive::block(OpKey::from_bytes(b"m"), side, side, side, &r)
            .unwrap()
            .0
    }

    #[test]
    fn nearest_and_within_match_brute_force() {
        let a = MeshTree::new(mesh_of(&block([0.0; 3], 0.01)));
        let b = MeshTree::new(mesh_of(&block([0.013, 0.002, 0.001], 0.01)));
        let n = a.nearest(&b).unwrap();
        assert!((n.distance - 0.003).abs() < 1e-12, "{}", n.distance);
        let mut brute = Vec::new();
        for (i, s) in a.triangles().iter().enumerate() {
            for (j, t) in b.triangles().iter().enumerate() {
                let d = super::super::prims::between(
                    &super::super::prims::Prim::Triangle(*s),
                    &super::super::prims::Prim::Triangle(*t),
                )
                .0;
                if d <= 0.0035 {
                    brute.push((i as u32, j as u32));
                }
            }
        }
        assert!(!brute.is_empty());
        assert_eq!(a.within(&b, 0.0035), brute);
        let (d, _, _) = a.nearest_to(Point3::new(0.02, 0.005, 0.005)).unwrap();
        assert!((d - 0.01).abs() < 1e-12, "{d}");
    }

    #[test]
    fn winding_is_one_inside_a_closed_mesh_and_zero_outside() {
        let a = MeshTree::new(mesh_of(&block([0.0; 3], 0.01)));
        assert_eq!(a.winding(Point3::new(0.005, 0.004, 0.003)), Some(1));
        assert_eq!(a.winding(Point3::new(0.015, 0.004, 0.003)), Some(0));
        assert_eq!(a.winding(Point3::new(-1.0, -1.0, -1.0)), Some(0));
        // A ball's mesh, from just inside its surface.
        let (ball, _) =
            crate::primitive::sphere(OpKey::from_bytes(b"s"), 0.005, &Rigid::IDENTITY).unwrap();
        let s = MeshTree::new(mesh_of(&ball));
        assert_eq!(s.winding(Point3::new(0.0, 0.0, 0.0049)), Some(1));
        assert_eq!(s.winding(Point3::new(0.0, 0.0, 0.0051)), Some(0));
    }
}
