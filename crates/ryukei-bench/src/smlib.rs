//! SMLib, the B-rep kernel in NVIDIA's USD BRep (Apache-2.0), through a
//! small C++ shim, for comparison: the same primitives built with
//! `SmPrimitiveCreation`, NURBS forms by replacing every surface and
//! curve with a plain B-spline copy, and meshes from
//! `SmBrep::ConvertToPolyBrep`, SMLib's one-call mesher. Only built with
//! the `smlib` feature, against a `libsmlib.so` built from usd-brep's
//! `source/SMLib` (`SMLIB_DIR`, default `/usr/local`; the devcontainer
//! builds it).
//!
//! SMLib's analytic surfaces are tagged subclasses of its B-spline
//! surface, so the NURBS forms differ only in losing the tag, and
//! `ConvertToPolyBrep` meshes a copy that recognizes analytic surfaces
//! again: SMLib's NURBS meshes come out the same as the analytic ones.

use crate::{Primitive, Soup};
use ryukei::geom::Rigid;

#[cxx::bridge(namespace = "ryukei_smlib")]
mod ffi {
    /// A mesh as SMLib makes it: positions (x, y, z flattened), shared
    /// between faces, and triangles as position indices, counterclockwise
    /// seen from outside.
    #[derive(Default)]
    struct SmlibMesh {
        positions: Vec<f64>,
        triangles: Vec<u32>,
        /// Whether any face failed to tessellate.
        failed_faces: bool,
        /// Edges used by only one polygon: 0 for a closed mesh.
        lamina_edges: u32,
        /// Polygons that were not triangles (and were fanned).
        polygons: u32,
    }

    unsafe extern "C++" {
        include!("ryukei-bench/src/smlib/shim.h");

        type Body;

        /// `kind`: 0 block (`a` x `b` x `c`), 1 cone or cylinder (base
        /// radius `a`, top radius `b`, height `c`), 2 sphere (radius
        /// `a`), 3 torus (radii `a` and `b`). `at`: the origin, then the
        /// images of the x and y axes.
        fn make(
            kind: u32,
            a: f64,
            b: f64,
            c: f64,
            at: &[f64],
            nurbs: bool,
        ) -> Result<UniquePtr<Body>>;
        fn mesh(b: &Body, chord: f64, angle: f64, parallel: bool) -> Result<SmlibMesh>;
    }
}

pub use ffi::SmlibMesh;

// SAFETY: a `Body` owns its context and every object in it, and nothing
// else points into them, so it may move between threads. It is not
// `Sync`: meshing copies the body inside its own context.
unsafe impl Send for ffi::Body {}

/// An SMLib solid.
pub struct Solid(cxx::UniquePtr<ffi::Body>);

impl Solid {
    /// A primitive moved by `at`, with every surface and curve a plain
    /// B-spline if `nurbs`.
    pub fn new(p: Primitive, at: &Rigid, nurbs: bool) -> Self {
        let r = at.rotation_matrix();
        let t = at.translation_part();
        let m = [
            t.x, t.y, t.z, r[0][0], r[1][0], r[2][0], r[0][1], r[1][1], r[2][1],
        ];
        let (kind, a, b, c) = match p {
            Primitive::Block([x, y, z]) => (0, x, y, z),
            Primitive::Cylinder { r, h } => (1, r, r, h),
            Primitive::Cone { r0, r1, h } => (1, r0, r1, h),
            Primitive::Sphere(r) => (2, r, 0.0, 0.0),
            Primitive::Torus { major, minor } => (3, major, minor, 0.0),
        };
        Self(ffi::make(kind, a, b, c, &m, nurbs).expect("SMLib builds the primitive"))
    }

    /// SMLib's mesh at an absolute chord and angle (radians, used for
    /// both curves and surfaces). On one thread unless `parallel`.
    pub fn mesh(&self, chord: f64, angle: f64, parallel: bool) -> SmlibMesh {
        ffi::mesh(&self.0, chord, angle, parallel).expect("SMLib meshes")
    }
}

impl SmlibMesh {
    pub fn soup(&self) -> Soup<'_> {
        Soup {
            positions: &self.positions,
            triangles: &self.triangles,
        }
    }
}
