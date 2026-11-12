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

    /// [`MeshTree::nearest_to`] without building a tree for `p`: the same
    /// distance, point and index, bit for bit, and nothing allocated.
    pub fn nearest_point(&self, p: Point3) -> Option<(f64, Point3, u32)> {
        self.tree.nearest_point(p)
    }

    /// The distance from `p` to triangle `k`: what [`MeshTree::nearest_to`]
    /// gives on a tree of that triangle alone, without the tree.
    pub fn to_triangle(&self, p: Point3, k: u32) -> f64 {
        let t = &self.triangles[k as usize];
        p.distance(super::prims::point_triangle(p, t))
    }

    /// The triangles whose boxes meet the box `lo`–`hi` (touching counts),
    /// by index, sorted, into `out` (cleared first; keep it between calls
    /// and nothing is allocated once it has grown).
    pub fn in_box(&self, lo: Point3, hi: Point3, out: &mut Vec<u32>) {
        self.tree.in_box(lo, hi, out)
    }

    /// Whether some triangle of `self` and one of `other` lie within `d`
    /// of each other: what `nearest(other)`'s distance `<= d` answers,
    /// stopped at the first pair found.
    pub fn any_within(&self, other: &MeshTree, d: f64) -> bool {
        self.tree.any_within(&other.tree, d)
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
        // A triangle in an axis-aligned plane has a box of no thickness, so
        // the ray's slab test against it, rounded, could drop a crossing on
        // its side: pad the boxes by a few ulps of the coordinates' size.
        // More candidates cost only exact tests.
        let size = [lo, hi, p]
            .iter()
            .map(|c| c.x.abs() + c.y.abs() + c.z.abs())
            .sum::<f64>();
        let pad = 1e-12 * size;
        'rays: for d in DIRECTIONS {
            let q = p + Vector3::new(d[0], d[1], d[2]) * reach;
            let mut w = 0i64;
            for k in self.tree.along(p, q, pad) {
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

    #[test]
    fn rays_grazing_an_axis_aligned_face_still_count_it() {
        // Blocks far from the origin, their faces axis-aligned, so each
        // face's triangles have boxes of no thickness; points just inside
        // a face send their first ray within rounding of that face's box.
        for at in [[0.0; 3], [123.456, -77.7, 3.3], [-400.0, 400.0, -400.0]] {
            let side = 0.01;
            let a = MeshTree::new(mesh_of(&block(at, side)));
            for eps in [1e-9, 1e-7, 1e-5] {
                for (k, sign) in [
                    (0, 1.0),
                    (0, -1.0),
                    (1, 1.0),
                    (1, -1.0),
                    (2, 1.0),
                    (2, -1.0),
                ] {
                    let mut c = [at[0] + side / 2.0, at[1] + side / 2.0, at[2] + side / 2.0];
                    c[k] += sign * (side / 2.0 - eps);
                    let inside = Point3::new(c[0], c[1], c[2]);
                    assert_eq!(a.winding(inside), Some(1), "{at:?} {eps} {k} {sign}");
                    c[k] += sign * 2.0 * eps;
                    let outside = Point3::new(c[0], c[1], c[2]);
                    assert_eq!(a.winding(outside), Some(0), "{at:?} {eps} {k} {sign}");
                }
            }
        }
    }

    /// Points round a block, a ball and a sheet of slivers, seeded.
    fn probes(n: usize) -> Vec<Point3> {
        let mut s = 0x9e37_79b9_7f4a_7c15u64;
        let mut next = || {
            s = s
                .wrapping_mul(6364136223846793005)
                .wrapping_add(1442695040888963407);
            (s >> 11) as f64 / (1u64 << 53) as f64
        };
        (0..n)
            .map(|_| {
                Point3::new(
                    0.03 * next() - 0.01,
                    0.03 * next() - 0.01,
                    0.03 * next() - 0.01,
                )
            })
            .collect()
    }

    #[test]
    fn nearest_point_is_nearest_to_bit_for_bit() {
        let (ball, _) =
            crate::primitive::sphere(OpKey::from_bytes(b"s"), 0.005, &Rigid::IDENTITY).unwrap();
        let slivers: Vec<[Point3; 3]> = (0..200)
            .map(|k| {
                let x = k as f64 * 1e-4;
                [
                    Point3::new(x, 0.0, 0.0),
                    Point3::new(x + 1e-4, 0.0, 0.0),
                    Point3::new(x, 0.02, 0.0),
                ]
            })
            .collect();
        for tris in [mesh_of(&block([0.0; 3], 0.01)), mesh_of(&ball), slivers] {
            let m = MeshTree::new(tris);
            for p in probes(500)
                .into_iter()
                .chain(m.triangles().iter().map(|t| t[1]))
            {
                let (d0, q0, k0) = m.nearest_to(p).unwrap();
                let (d1, q1, k1) = m.nearest_point(p).unwrap();
                assert_eq!(d0.to_bits(), d1.to_bits(), "{p:?}");
                assert_eq!(
                    [q0.x, q0.y, q0.z].map(f64::to_bits),
                    [q1.x, q1.y, q1.z].map(f64::to_bits),
                    "{p:?}"
                );
                assert_eq!(k0, k1, "{p:?}");
            }
        }
        assert!(
            MeshTree::new(Vec::new())
                .nearest_point(Point3::ORIGIN)
                .is_none()
        );
    }

    #[test]
    fn to_triangle_is_a_one_triangle_tree_bit_for_bit() {
        let m = MeshTree::new(mesh_of(&block([0.0; 3], 0.01)));
        for p in probes(300) {
            for (k, t) in m.triangles().iter().enumerate() {
                let one = MeshTree::new(vec![*t]).nearest_to(p).unwrap().0;
                assert_eq!(
                    m.to_triangle(p, k as u32).to_bits(),
                    one.to_bits(),
                    "{p:?} {k}"
                );
            }
        }
    }

    #[test]
    fn in_box_matches_brute_force() {
        let (ball, _) =
            crate::primitive::sphere(OpKey::from_bytes(b"s"), 0.005, &Rigid::IDENTITY).unwrap();
        let m = MeshTree::new(mesh_of(&ball));
        let mut found = Vec::new();
        let ps = probes(400);
        for w in ps.chunks(2) {
            let (lo, hi) = (w[0].min(w[1]), w[0].max(w[1]));
            m.in_box(lo, hi, &mut found);
            let brute: Vec<u32> = m
                .triangles()
                .iter()
                .enumerate()
                .filter(|(_, t)| {
                    let (a, b) = (t[0].min(t[1]).min(t[2]), t[0].max(t[1]).max(t[2]));
                    a.x <= hi.x
                        && lo.x <= b.x
                        && a.y <= hi.y
                        && lo.y <= b.y
                        && a.z <= hi.z
                        && lo.z <= b.z
                })
                .map(|(k, _)| k as u32)
                .collect();
            assert_eq!(found, brute, "{lo:?} {hi:?}");
        }
        // A flat box on a face's plane still finds that face's triangles.
        let b = MeshTree::new(mesh_of(&block([0.0; 3], 0.01)));
        b.in_box(
            Point3::new(0.002, 0.002, 0.01),
            Point3::new(0.003, 0.003, 0.01),
            &mut found,
        );
        assert!(!found.is_empty());
        MeshTree::new(Vec::new()).in_box(lo_hi().0, lo_hi().1, &mut found);
        assert!(found.is_empty());
    }

    fn lo_hi() -> (Point3, Point3) {
        (Point3::ORIGIN, Point3::new(1.0, 1.0, 1.0))
    }

    #[test]
    fn any_within_answers_what_nearest_does() {
        let (ball, _) =
            crate::primitive::sphere(OpKey::from_bytes(b"s"), 0.005, &Rigid::IDENTITY).unwrap();
        let ball = mesh_of(&ball);
        let cube = mesh_of(&block([0.0; 3], 0.01));
        let moved = |t: &[[Point3; 3]], o: Point3| -> Vec<[Point3; 3]> {
            t.iter()
                .map(|t| t.map(|q| Point3::new(q.x + o.x, q.y + o.y, q.z + o.z)))
                .collect()
        };
        let a = MeshTree::new(cube.clone());
        let mut checked = 0;
        for o in probes(60) {
            for other in [&ball, &cube] {
                let b = MeshTree::new(moved(other, o));
                let m = a.nearest(&b).unwrap().distance;
                for d in [0.0, m * (1.0 - 1e-9), m, m * (1.0 + 1e-9), 1e-4, 1.0] {
                    assert_eq!(a.any_within(&b, d), m <= d, "{o:?} {d} {m}");
                    checked += 1;
                }
            }
        }
        assert_eq!(checked, 720);
        assert!(!MeshTree::new(Vec::new()).any_within(&a, 1.0));
    }
}
