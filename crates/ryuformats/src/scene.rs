//! What the writers take: a tree of products, each holding its solids'
//! meshes in its own frame and placing other products in it.

use ryukei::geom::Rigid;
use ryukei::mesh::BodyMesh;
use std::sync::Arc;

/// The length unit a file is written in. Model space is meters; a writer
/// multiplies by [`Unit::per_meter`] once, in f64.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Unit {
    #[default]
    Millimeter,
    Centimeter,
    Meter,
}

impl Unit {
    pub const ALL: [Unit; 3] = [Unit::Millimeter, Unit::Centimeter, Unit::Meter];

    /// `mm`, `cm` or `m`.
    pub fn parse(s: &str) -> Option<Unit> {
        Unit::ALL.into_iter().find(|u| u.as_str() == s)
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Unit::Millimeter => "mm",
            Unit::Centimeter => "cm",
            Unit::Meter => "m",
        }
    }

    /// Its full name, as 3MF's `unit` attribute spells it.
    pub fn name(self) -> &'static str {
        match self {
            Unit::Millimeter => "millimeter",
            Unit::Centimeter => "centimeter",
            Unit::Meter => "meter",
        }
    }

    /// How many of it make a meter.
    pub fn per_meter(self) -> f64 {
        match self {
            Unit::Millimeter => 1000.0,
            Unit::Centimeter => 100.0,
            Unit::Meter => 1.0,
        }
    }
}

/// The products to write, the root first. No product may place itself,
/// at any depth.
#[derive(Clone, Debug, Default)]
pub struct Scene {
    pub products: Vec<Product>,
}

/// A part at a revision: its identifier (a part number), its name, the
/// step an occurrence's path starts with when it is the root (its part's
/// ID), its solids in model order and what it places.
#[derive(Clone, Debug, Default)]
pub struct Product {
    pub id: String,
    pub name: String,
    pub part: String,
    pub solids: Vec<Solid>,
    pub children: Vec<Child>,
}

/// A solid's name (its founder's ID) and its mesh in its product's frame,
/// in meters: welded, watertight, triangles counterclockwise from outside.
#[derive(Clone, Debug)]
pub struct Solid {
    pub name: String,
    pub mesh: Arc<BodyMesh>,
}

/// One placing of a product (an index into the scene's products) in
/// another, by a rigid motion of its frame into its parent's, in meters.
/// `name` is the step it adds to an occurrence's path: the instance's ID.
#[derive(Clone, Debug)]
pub struct Child {
    pub product: usize,
    pub place: Rigid,
    pub name: String,
}

/// A solid of an occurrence, placed in the root's frame: its path
/// (`<root part>/<instance>/…/<solid>`), its mesh and where it goes.
#[derive(Clone, Debug)]
pub struct Placed<'a> {
    pub path: String,
    pub mesh: &'a BodyMesh,
    pub place: Rigid,
}

impl Scene {
    /// Every solid of every occurrence in the root's frame, depth first: a
    /// product's own solids in model order, then each child's.
    pub fn placed(&self) -> Vec<Placed<'_>> {
        let mut out = Vec::new();
        if let Some(root) = self.products.first() {
            self.visit(0, &root.part, Rigid::IDENTITY, &mut out);
        }
        out
    }

    fn visit<'a>(&'a self, k: usize, path: &str, place: Rigid, out: &mut Vec<Placed<'a>>) {
        let p = &self.products[k];
        for s in &p.solids {
            out.push(Placed {
                path: format!("{path}/{}", s.name),
                mesh: &s.mesh,
                place,
            });
        }
        for c in &p.children {
            let path = format!("{path}/{}", c.name);
            self.visit(c.product, &path, place.then_after(&c.place), out);
        }
    }

    /// How many triangles a product holds, its children's included.
    pub fn triangles(&self, k: usize) -> u64 {
        let p = &self.products[k];
        let own: u64 = p.solids.iter().map(|s| s.mesh.triangles.len() as u64).sum();
        own + p
            .children
            .iter()
            .map(|c| self.triangles(c.product))
            .sum::<u64>()
    }
}

/// `p` placed by `place` and scaled to `unit`, in f64.
pub fn placed_point(place: &Rigid, unit: Unit, p: ryukei::geom::Point3) -> [f64; 3] {
    let q = place.apply_point(p);
    let k = unit.per_meter();
    [q.x * k, q.y * k, q.z * k]
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use ryukei::geom::{Rigid, Vector3};
    use ryukei::mesh::MeshParams;
    use ryukei::name::OpKey;

    /// A block `x` by `y` by `z` meters from the origin, meshed.
    pub(crate) fn block(x: f64, y: f64, z: f64) -> Arc<BodyMesh> {
        let b = ryukei::primitive::block(OpKey(1), x, y, z, &Rigid::IDENTITY)
            .unwrap()
            .0;
        Arc::new(ryukei::mesh::tessellate(&b, &MeshParams::new(1e-5, 0.26)).unwrap())
    }

    pub(crate) fn at(x: f64, y: f64, z: f64) -> Rigid {
        Rigid::translation(Vector3::new(x, y, z))
    }

    /// A frame placing a plate and a pin twice, the pin a product of its
    /// own: the root, the pin.
    pub(crate) fn frame() -> Scene {
        let solid = |name: &str, mesh| Solid {
            name: name.into(),
            mesh,
        };
        Scene {
            products: vec![
                Product {
                    id: "F-1".into(),
                    name: "Frame".into(),
                    part: "frame".into(),
                    solids: vec![solid("plate", block(0.1, 0.1, 0.01))],
                    children: vec![
                        Child {
                            product: 1,
                            place: at(0.0, 0.0, 0.01),
                            name: "a".into(),
                        },
                        Child {
                            product: 1,
                            place: at(0.05, 0.0, 0.01),
                            name: "b".into(),
                        },
                    ],
                },
                Product {
                    id: "P-1".into(),
                    name: "Pin".into(),
                    part: "pin".into(),
                    solids: vec![solid("shank", block(0.01, 0.01, 0.02))],
                    children: vec![],
                },
            ],
        }
    }

    #[test]
    fn occurrences_are_placed_depth_first() {
        let s = frame();
        let placed = s.placed();
        let paths: Vec<&str> = placed.iter().map(|p| p.path.as_str()).collect();
        assert_eq!(paths, ["frame/plate", "frame/a/shank", "frame/b/shank"]);
        assert_eq!(
            placed[2].place.translation_part(),
            Vector3::new(0.05, 0.0, 0.01)
        );
        assert_eq!(s.triangles(1), 12);
        assert_eq!(s.triangles(0), 36);
        assert!(Scene::default().placed().is_empty());
    }

    #[test]
    fn units() {
        assert_eq!(Unit::parse("cm"), Some(Unit::Centimeter));
        assert_eq!(Unit::parse("in"), None);
        assert_eq!(Unit::Meter.name(), "meter");
        let p = placed_point(
            &at(0.001, 0.0, 0.0),
            Unit::Millimeter,
            ryukei::geom::Point3::new(0.002, 0.5, -1.0),
        );
        assert_eq!(p, [3.0, 500.0, -1000.0]);
    }
}
