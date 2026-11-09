//! Interference judged on meshes, for pairs with a reference import's
//! body on a side (design 364): a reference is the file's topology, a
//! sheet the kernel's booleans don't take. Each side is meshed at normal
//! display quality; the meshes stand within each side's chord and
//! tolerances of its faces, so what they show is certified to within the
//! pair's reach, and nothing is claimed past it.

use crate::interference::Verdict;
use ryukei::geom::{Box3, Point3, Rigid};
use ryukei::measure::MeshTree;
use ryukei::name::Name;
use ryukei::topo::Body;

/// One body's mesh at normal quality in its own frame, its box tree, each
/// triangle's face, whether it closes, and its reach: the chord plus the
/// largest tolerance of its edges and vertices.
pub struct Prepared {
    pub tree: MeshTree,
    /// The face each triangle belongs to, by name.
    pub faces: Vec<Name>,
    /// No laminar edge and no face left out of the mesh: a winding
    /// number about a point means something.
    pub closed: bool,
    pub reach: f64,
}

impl Prepared {
    /// `body`'s mesh, or the mesher's refusal as text.
    pub fn new(body: &Body) -> Result<Prepared, String> {
        Prepared::new_for(body, &ryukei::work::Work::unlimited()).map_err(|e| format!("{e:?}"))
    }

    /// [`Prepared::new`] under `work`, the mesher's refusal as it gave it
    /// (a cancel among them).
    pub fn new_for(body: &Body, work: &ryukei::work::Work) -> Result<Prepared, ryukei::Error> {
        let diagonal = match crate::inspect::loose_extent(body) {
            Some((lo, hi)) => {
                let d = [hi[0] - lo[0], hi[1] - lo[1], hi[2] - lo[2]];
                (d[0] * d[0] + d[1] * d[1] + d[2] * d[2]).sqrt()
            }
            None => 1.0,
        };
        let params = crate::mesh::Quality::Normal.params(diagonal);
        let m = ryukei::mesh::tessellate_with(body, &params, work)?;
        let mut triangles = Vec::with_capacity(m.triangles.len());
        let mut faces = Vec::with_capacity(m.triangles.len());
        for fm in &m.faces {
            let name = body.face(fm.face).expect("a meshed face is live").name;
            for t in &m.triangles[fm.triangles.start as usize..fm.triangles.end as usize] {
                triangles.push(t.map(|c| m.positions[m.corners[c as usize].position as usize]));
                faces.push(name);
            }
        }
        let laminar = body.edges().any(|e| body.is_laminar(e));
        let tol = body
            .edges()
            .map(|e| body.edge(e).expect("live").tolerance.get())
            .chain(
                body.vertices()
                    .map(|v| body.vertex(v).expect("live").tolerance.get()),
            )
            .fold(0.0, f64::max);
        Ok(Prepared {
            tree: MeshTree::new(triangles),
            faces,
            closed: !laminar && m.faults.is_empty(),
            reach: params.chord + tol,
        })
    }

    /// Its triangles moved by `r`, in a tree of their own, with the same
    /// faces, closure and reach.
    pub fn placed(&self, r: &Rigid) -> Prepared {
        let moved = self
            .tree
            .triangles()
            .iter()
            .map(|t| t.map(|p| r.apply_point(p)))
            .collect();
        Prepared {
            tree: MeshTree::new(moved),
            faces: self.faces.clone(),
            closed: self.closed,
            reach: self.reach,
        }
    }

    /// Its box.
    pub fn bounds(&self) -> Option<Box3> {
        self.tree.bounds().map(|(lo, hi)| Box3::new(lo, hi))
    }
}

/// Pieces a pair's search may visit before it gives up: past it the pair
/// is unchecked.
pub const BUDGET: u64 = 1 << 20;

/// Faces of each side named at most.
const FACES: usize = 64;

/// The pair's reach: both sides' chords and tolerances, and a part in
/// 10⁹ of the coordinates' size for rounding.
fn reach(a: &Prepared, b: &Prepared, both: &Box3) -> f64 {
    let (lo, hi) = (both.min(), both.max());
    let size = [lo.x, lo.y, lo.z, hi.x, hi.y, hi.z]
        .iter()
        .fold(0.0f64, |m, x| m.max(x.abs()));
    a.reach + b.reach + 1e-9 * size.max(1e-3)
}

/// The distance from `p` to triangle `t`.
fn to_triangle(p: Point3, t: &[Point3; 3]) -> f64 {
    let one = MeshTree::new(vec![*t]);
    one.nearest_to(p).map_or(f64::INFINITY, |(d, _, _)| d)
}

/// A search for a deep point that ran past [`BUDGET`] pieces.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct OverBudget;

/// A point of a triangle of `tris` (those whose boxes come within `reach`
/// of `inside`'s box) whose winding in `inside` isn't zero and that lies
/// farther than `reach` from it, found by quartering triangles. A piece
/// is dropped:
/// - where its corners all lie within `reach` of the triangle of
///   `inside` nearest its middle (the distance to a triangle is convex,
///   so every point of it does: flush faces end at once);
/// - where a ball round its middle holding it misses `inside`'s mesh
///   and the middle winds zero (the piece is wholly outside);
/// - once it is an eighth of `reach` across (what it may still hold lies
///   within about the reach of the mesh: not deep).
///
/// A piece wholly inside (its ball misses the mesh, its middle winds) is
/// deep when its middle lies farther than `reach`. [`OverBudget`] past
/// [`BUDGET`] pieces, counted in `spent`.
pub fn deep_point(
    inside: &MeshTree,
    tris: &[[Point3; 3]],
    reach: f64,
    spent: &mut u64,
) -> Result<Option<Point3>, OverBudget> {
    let Some((lo, hi)) = inside.bounds() else {
        return Ok(None);
    };
    let near = Box3::new(lo, hi).grow(reach);
    let mut stack: Vec<[Point3; 3]> = tris
        .iter()
        .filter(|t| Box3::from_points(t.iter().copied()).is_some_and(|b| b.intersects(&near)))
        .copied()
        .collect();
    while let Some(t) = stack.pop() {
        *spent += 1;
        if *spent > BUDGET {
            return Err(OverBudget);
        }
        let c = Point3::new(
            (t[0].x + t[1].x + t[2].x) / 3.0,
            (t[0].y + t[1].y + t[2].y) / 3.0,
            (t[0].z + t[1].z + t[2].z) / 3.0,
        );
        let r = t.iter().map(|p| p.distance(c)).fold(0.0, f64::max);
        let (d, _, k) = inside.nearest_to(c).expect("a mesh");
        if d + r <= reach {
            continue;
        }
        let nearest = &inside.triangles()[k as usize];
        if t.iter().all(|&p| to_triangle(p, nearest) <= reach) {
            continue;
        }
        match inside.winding(c) {
            Some(0) if d > r => continue,
            Some(w) if w != 0 && d > reach => return Ok(Some(c)),
            _ => {}
        }
        if r < reach / 8.0 {
            continue;
        }
        let m = |p: Point3, q: Point3| {
            Point3::new((p.x + q.x) / 2.0, (p.y + q.y) / 2.0, (p.z + q.z) / 2.0)
        };
        let (ab, bc, ca) = (m(t[0], t[1]), m(t[1], t[2]), m(t[2], t[0]));
        stack.extend([[t[0], ab, ca], [ab, t[1], bc], [ca, bc, t[2]], [ab, bc, ca]]);
    }
    Ok(None)
}

/// `a` against `b` placed at `rel` in `a`'s frame, on their meshes;
/// `spent` counts the search's pieces. `None` past [`BUDGET`].
pub fn verdict(a: &Prepared, b: &Prepared, rel: &Rigid, spent: &mut u64) -> Option<Verdict> {
    let bp = b.placed(rel);
    let (Some(ba), Some(bb)) = (a.bounds(), bp.bounds()) else {
        return Some(Verdict::Clear);
    };
    let reach = reach(a, b, &ba.union(&bb));
    let nearest = a
        .tree
        .nearest(&bp.tree)
        .map_or(f64::INFINITY, |n| n.distance);
    let mut deep = None;
    if a.closed {
        deep = deep_point(&a.tree, bp.tree.triangles(), reach, spent).ok()?;
    }
    if deep.is_none() && bp.closed {
        deep = deep_point(&bp.tree, a.tree.triangles(), reach, spent).ok()?;
    }
    if deep.is_some() {
        let (mut faces_a, mut faces_b) = (
            std::collections::BTreeSet::new(),
            std::collections::BTreeSet::new(),
        );
        for (i, j) in a.tree.within(&bp.tree, reach) {
            if faces_a.len() < FACES {
                faces_a.insert(a.faces[i as usize]);
            }
            if faces_b.len() < FACES {
                faces_b.insert(b.faces[j as usize]);
            }
        }
        let (lo, hi) = (ba.min().max(bb.min()), ba.max().min(bb.max()));
        return Some(Verdict::Approximate {
            min: [lo.x, lo.y, lo.z],
            max: [hi.x, hi.y, hi.z],
            open: !a.closed || !b.closed,
            faces_a: faces_a.into_iter().collect(),
            faces_b: faces_b.into_iter().collect(),
        });
    }
    Some(if nearest <= reach {
        Verdict::Touching
    } else {
        Verdict::Clear
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use ryukei::geom::Rigid;
    use ryukei::name::OpKey;

    pub(crate) fn block(side: f64) -> Body {
        ryukei::primitive::block(OpKey::from_bytes(b"b"), side, side, side, &Rigid::IDENTITY)
            .unwrap()
            .0
    }

    #[test]
    fn a_solid_block_is_prepared_closed_with_its_reach() {
        let p = Prepared::new(&block(0.01)).unwrap();
        assert!(p.closed);
        let diagonal = (3.0f64).sqrt() * 0.01;
        let chord = crate::mesh::Quality::Normal.params(diagonal).chord;
        // The chord and the block's tolerances (the resolution).
        let tol = ryukei::tol::LINEAR_RESOLUTION;
        assert!((p.reach - chord - tol).abs() < 1e-15, "{} {chord}", p.reach);
        assert_eq!(p.tree.triangles().len(), p.faces.len());
        assert_eq!(p.tree.triangles().len(), 12);
    }

    #[test]
    fn a_sheet_is_prepared_open() {
        let b = block(0.01);
        let top = b.faces().next().unwrap();
        let name = b.face(top).unwrap().name;
        let (sheet, _) = ryukei::sheet::extract(OpKey::from_bytes(b"s"), &b, &[name]).unwrap();
        let p = Prepared::new(&sheet).unwrap();
        assert!(!p.closed);
        assert_eq!(p.tree.triangles().len(), 2);
    }
    fn at(x: f64, y: f64, z: f64) -> Rigid {
        Rigid::translation(ryukei::geom::Vector3::new(x, y, z))
    }

    fn pin(r: f64, h: f64) -> Body {
        ryukei::primitive::cylinder(OpKey::from_bytes(b"p"), r, h, &Rigid::IDENTITY)
            .unwrap()
            .0
    }

    #[test]
    fn blocks_apart_touching_and_overlapping() {
        let a = Prepared::new(&block(0.01)).unwrap();
        let b = Prepared::new(&block(0.01)).unwrap();
        assert_eq!(
            verdict(&a, &b, &at(0.011, 0.0, 0.0), &mut 0).unwrap(),
            Verdict::Clear
        );
        assert_eq!(
            verdict(&a, &b, &at(0.01, 0.0, 0.0), &mut 0).unwrap(),
            Verdict::Touching
        );
        let v = verdict(&a, &b, &at(0.005, 0.002, 0.0), &mut 0).unwrap();
        let Verdict::Approximate {
            min,
            max,
            open,
            faces_a,
            faces_b,
        } = v
        else {
            panic!("{v:?}")
        };
        assert!(!open && !faces_a.is_empty() && !faces_b.is_empty());
        assert!(
            (min[0] - 0.005).abs() < 1e-9 && (max[0] - 0.01).abs() < 1e-9,
            "{min:?} {max:?}"
        );
    }

    #[test]
    fn a_pin_through_a_plate_interferes_though_no_vertex_is_deep() {
        // The plate's top and bottom are two triangles each, the pin's
        // vertices all on its rims above and below the plate.
        let plate = ryukei::primitive::block(
            OpKey::from_bytes(b"pl"),
            0.04,
            0.04,
            0.005,
            &Rigid::IDENTITY,
        )
        .unwrap()
        .0;
        let a = Prepared::new(&plate).unwrap();
        let b = Prepared::new(&pin(0.003, 0.02)).unwrap();
        let v = verdict(&a, &b, &at(0.02, 0.02, -0.0075), &mut 0).unwrap();
        assert!(
            matches!(v, Verdict::Approximate { open: false, .. }),
            "{v:?}"
        );
    }

    #[test]
    fn one_inside_another_interferes_and_a_sheet_through_a_block_is_open() {
        let a = Prepared::new(&block(0.01)).unwrap();
        let small = Prepared::new(&block(0.002)).unwrap();
        let v = verdict(&a, &small, &at(0.004, 0.004, 0.004), &mut 0).unwrap();
        assert!(matches!(v, Verdict::Approximate { .. }), "{v:?}");
        let b = block(0.01);
        let top = b.faces().next().unwrap();
        let (sheet, _) =
            ryukei::sheet::extract(OpKey::from_bytes(b"s"), &b, &[b.face(top).unwrap().name])
                .unwrap();
        let s = Prepared::new(&sheet).unwrap();
        // The square sheet moved to lie across the block's middle.
        let box3 = s.bounds().unwrap();
        let mid = 0.005 - box3.min().z;
        let v = verdict(&a, &s, &at(0.002, 0.002, mid), &mut 0).unwrap();
        assert!(
            matches!(v, Verdict::Approximate { open: true, .. }),
            "{v:?}"
        );
        // Two sheets crossing only touch.
        let v = verdict(&s, &s, &at(0.002, 0.002, 0.0), &mut 0).unwrap();
        assert!(matches!(v, Verdict::Touching | Verdict::Clear), "{v:?}");
    }
}
