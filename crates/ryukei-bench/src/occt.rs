//! OCCT, through a small C++ shim, for comparison: the same primitives
//! built with `BRepPrimAPI`, NURBS forms through `BRepBuilderAPI_NurbsConvert`,
//! meshes from `BRepMesh_IncrementalMesh` in absolute mode, and exact
//! volume, area and distance from the B-rep. Only built with the `occt`
//! feature, against a system OCCT 7.8 or later (`OCCT_DIR`, default
//! `/usr`).

use ryukei::geom::{Point3, Rigid};

#[cxx::bridge(namespace = "ryukei_occt")]
mod ffi {
    /// A mesh as OCCT makes it: positions (x, y, z flattened), triangles
    /// as position indices, counterclockwise seen from outside. Positions
    /// are not shared between faces.
    #[derive(Default)]
    struct OcctMesh {
        positions: Vec<f64>,
        triangles: Vec<u32>,
        faces_without_mesh: u32,
    }

    unsafe extern "C++" {
        include!("ryukei-bench/src/occt/shim.h");

        type Shape;

        fn make_box(dx: f64, dy: f64, dz: f64) -> UniquePtr<Shape>;
        fn make_cylinder(r: f64, h: f64) -> UniquePtr<Shape>;
        fn make_cone(r0: f64, r1: f64, h: f64) -> UniquePtr<Shape>;
        fn make_sphere(r: f64) -> UniquePtr<Shape>;
        fn make_torus(major: f64, minor: f64) -> UniquePtr<Shape>;
        fn placed(s: &Shape, m: &[f64]) -> UniquePtr<Shape>;
        fn nurbs(s: &Shape) -> UniquePtr<Shape>;
        fn mesh(s: &Shape, chord: f64, angle: f64, parallel: bool) -> OcctMesh;
        fn volume(s: &Shape) -> f64;
        fn area(s: &Shape) -> f64;
        fn distance(s: &Shape, x: f64, y: f64, z: f64) -> f64;
    }
}

pub use ffi::OcctMesh;

// SAFETY: a `Shape` is only read once built: the shim's functions take it
// by const reference, and `mesh` meshes a copy rather than the shape.
// OCCT's shapes are reference-counted with atomic counts.
unsafe impl Send for ffi::Shape {}
unsafe impl Sync for ffi::Shape {}

/// An OCCT solid.
pub struct Solid(cxx::UniquePtr<ffi::Shape>);

/// The primitives both kernels build, in the same local frames.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Primitive {
    Block([f64; 3]),
    Cylinder {
        r: f64,
        h: f64,
    },
    /// From radius `r0` at the base to `r1` (0 for a point) at height `h`.
    Cone {
        r0: f64,
        r1: f64,
        h: f64,
    },
    Sphere(f64),
    Torus {
        major: f64,
        minor: f64,
    },
}

impl Solid {
    /// A primitive moved by `at`.
    pub fn new(p: Primitive, at: &Rigid) -> Self {
        let s = match p {
            Primitive::Block([x, y, z]) => ffi::make_box(x, y, z),
            Primitive::Cylinder { r, h } => ffi::make_cylinder(r, h),
            Primitive::Cone { r0, r1, h } => ffi::make_cone(r0, r1, h),
            Primitive::Sphere(r) => ffi::make_sphere(r),
            Primitive::Torus { major, minor } => ffi::make_torus(major, minor),
        };
        let r = at.rotation_matrix();
        let t = at.translation_part();
        let m: Vec<f64> = r.iter().flatten().copied().chain([t.x, t.y, t.z]).collect();
        Self(ffi::placed(&s, &m))
    }

    /// The same solid with every curve and surface converted to NURBS.
    pub fn to_nurbs(&self) -> Self {
        Self(ffi::nurbs(&self.0))
    }

    /// OCCT's mesh at an absolute chord (its "linear deflection") and
    /// angle.
    pub fn mesh(&self, chord: f64, angle: f64, parallel: bool) -> OcctMesh {
        ffi::mesh(&self.0, chord, angle, parallel)
    }

    pub fn volume(&self) -> f64 {
        ffi::volume(&self.0)
    }

    pub fn area(&self) -> f64 {
        ffi::area(&self.0)
    }

    /// The distance from `p` to the solid's boundary (its shell).
    pub fn distance(&self, p: Point3) -> f64 {
        ffi::distance(&self.0, p.x, p.y, p.z)
    }
}

impl OcctMesh {
    pub fn point(&self, i: u32) -> Point3 {
        let k = 3 * i as usize;
        Point3::new(
            self.positions[k],
            self.positions[k + 1],
            self.positions[k + 2],
        )
    }

    /// The enclosed volume and the area.
    pub fn volume_and_area(&self) -> (f64, f64) {
        let (mut vol, mut area) = (0.0, 0.0);
        for t in self.triangles.chunks(3) {
            let [a, b, c] = [t[0], t[1], t[2]].map(|i| self.point(i).to_vector());
            vol += a.dot(b.cross(c)) / 6.0;
            area += (b - a).cross(c - a).length() / 2.0;
        }
        (vol, area)
    }

    /// The largest distance from sampled points of every `every`-th
    /// triangle to the solid's boundary: one way only, mesh to solid.
    pub fn deviation(&self, solid: &Solid, every: usize) -> f64 {
        let mut worst: f64 = 0.0;
        for t in self.triangles.chunks(3).step_by(every) {
            let [a, b, c] = [t[0], t[1], t[2]].map(|i| self.point(i).to_vector());
            for (wa, wb) in [(1.0 / 3.0, 1.0 / 3.0), (0.5, 0.5), (0.5, 0.0), (0.0, 0.5)] {
                let wc = 1.0 - wa - wb;
                let p = Point3::ORIGIN + a * wa + b * wb + c * wc;
                worst = worst.max(solid.distance(p));
            }
        }
        worst
    }
}
