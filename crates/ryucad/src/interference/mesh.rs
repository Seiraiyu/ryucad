//! Interference judged on meshes, for pairs with a reference import's
//! body on a side (design 364): a reference is the file's topology, a
//! sheet the kernel's booleans don't take. Each side is meshed at normal
//! display quality; the meshes stand within each side's chord and
//! tolerances of its faces, so what they show is certified to within the
//! pair's reach, and nothing is claimed past it.

use ryukei::geom::{Box3, Rigid};
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
        Prepared::new_for(body, &ryukei::work::Work::unlimited())
    }

    pub fn new_for(body: &Body, work: &ryukei::work::Work) -> Result<Prepared, String> {
        let diagonal = match crate::inspect::loose_extent(body) {
            Some((lo, hi)) => {
                let d = [hi[0] - lo[0], hi[1] - lo[1], hi[2] - lo[2]];
                (d[0] * d[0] + d[1] * d[1] + d[2] * d[2]).sqrt()
            }
            None => return Err("an empty body".to_owned()),
        };
        let params = crate::mesh::Quality::Normal.params(diagonal);
        let m = ryukei::mesh::tessellate_with(body, &params, work).map_err(|e| format!("{e:?}"))?;
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
}
