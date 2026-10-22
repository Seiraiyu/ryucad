//! SMLib, the B-rep kernel in NVIDIA's USD BRep (Apache-2.0), through a
//! small C++ shim, for comparison: the same primitives built with
//! `SmPrimitiveCreation`, NURBS forms by replacing every surface and
//! curve with a plain B-spline copy, sweeps of a [`BenchProfile`] from
//! `SmPrimitiveCreation`'s linear and rotational sweeps, booleans from
//! `SmMerge`, fillets from `SmFilletExecutive`, and meshes from `SmBrep::ConvertToPolyBrep`, SMLib's
//! one-call mesher. Only built with the `smlib` feature, against a `libsmlib.so` built from usd-brep's
//! `source/SMLib` (`SMLIB_DIR`, default `/usr/local`; the devcontainer
//! builds it).
//!
//! SMLib's analytic surfaces are tagged subclasses of its B-spline
//! surface, so the NURBS forms differ only in losing the tag, and
//! `ConvertToPolyBrep` meshes a copy that recognizes analytic surfaces
//! again: SMLib's NURBS meshes come out the same as the analytic ones.

use crate::{BenchBlend, BenchKind, BenchProfile, Primitive, Soup};
use ryukei::boolean::BooleanOp;
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
        fn extrude(
            frame: &[f64],
            sizes: &[u32],
            segments: &[f64],
            vector: &[f64],
        ) -> Result<UniquePtr<Body>>;
        fn revolve(
            frame: &[f64],
            sizes: &[u32],
            segments: &[f64],
            axis: &[f64],
            angle: f64,
        ) -> Result<UniquePtr<Body>>;
        fn add_tool(
            body: Pin<&mut Body>,
            kind: u32,
            a: f64,
            b: f64,
            c: f64,
            at: &[f64],
        ) -> Result<()>;
        fn subtract_tools(body: Pin<&mut Body>, parallel: bool) -> Result<()>;
        fn combine_tools(body: Pin<&mut Body>, op: u32, parallel: bool) -> Result<()>;
        fn vertices(body: &Body) -> Vec<f64>;
        fn faces(body: &Body) -> u32;
        fn mesh(b: &Body, chord: f64, angle: f64, parallel: bool) -> Result<SmlibMesh>;
        fn fillet(body: Pin<&mut Body>, edges: &[f64], radii: &[f64], parallel: bool)
        -> Result<()>;
    }
}

pub use ffi::SmlibMesh;

// SAFETY: a `Body` owns its context and every object in it, and nothing
// else points into them, so it may move between threads. It is not
// `Sync`: meshing copies the body inside its own context.
unsafe impl Send for ffi::Body {}

/// An SMLib solid.
pub struct Solid(cxx::UniquePtr<ffi::Body>);

/// SMLib's arguments for a primitive moved by `at`: the kind, three
/// sizes, and the frame.
fn args(p: Primitive, at: &Rigid) -> (u32, f64, f64, f64, [f64; 9]) {
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
    (kind, a, b, c, m)
}

impl Solid {
    /// A primitive moved by `at`, with every surface and curve a plain
    /// B-spline if `nurbs`.
    pub fn new(p: Primitive, at: &Rigid, nurbs: bool) -> Self {
        let (kind, a, b, c, m) = args(p, at);
        Self(ffi::make(kind, a, b, c, &m, nurbs).expect("SMLib builds the primitive"))
    }

    /// The profile swept `dist` along `dir`, with end caps, by
    /// `SmPrimitiveCreation::CreateLinearSweep`.
    pub fn extrude(profile: &BenchProfile, dir: [f64; 3], dist: f64) -> Result<Self, String> {
        let (frame, sizes, segments) = profile.flatten();
        ffi::extrude(&frame, &sizes, &segments, &crate::sweep_vector(dir, dist))
            .map(Self)
            .map_err(|e| e.to_string())
    }

    /// The profile turned `angle` radians about the axis through
    /// `axis_origin` along `axis_dir`, by
    /// `SmPrimitiveCreation::CreateRotationalSweep`, with end caps unless
    /// the turn is full.
    pub fn revolve(
        profile: &BenchProfile,
        axis_origin: [f64; 3],
        axis_dir: [f64; 3],
        angle: f64,
    ) -> Result<Self, String> {
        let (frame, sizes, segments) = profile.flatten();
        let axis: Vec<f64> = axis_origin.into_iter().chain(axis_dir).collect();
        ffi::revolve(&frame, &sizes, &segments, &axis, angle)
            .map(Self)
            .map_err(|e| e.to_string())
    }

    /// Adds a primitive moved by `at` to the tools that
    /// [`subtract_tools`](Self::subtract_tools) takes away. Tools must
    /// not touch each other.
    pub fn add_tool(&mut self, p: Primitive, at: &Rigid) {
        let (kind, a, b, c, m) = args(p, at);
        ffi::add_tool(self.0.pin_mut(), kind, a, b, c, &m).expect("SMLib builds the tool");
    }

    /// This solid minus every tool added, in one `SmMerge` difference.
    /// On one thread unless `parallel`.
    pub fn subtract_tools(&mut self, parallel: bool) {
        ffi::subtract_tools(self.0.pin_mut(), parallel).expect("SMLib subtracts");
    }

    /// This solid united with, intersected with or minus every tool
    /// added, in one `SmMerge` boolean. On one thread unless `parallel`.
    /// SMLib's failure as text.
    pub fn combine_tools(&mut self, op: BooleanOp, parallel: bool) -> Result<(), String> {
        let code = match op {
            BooleanOp::Unite => 0,
            BooleanOp::Intersect => 1,
            BooleanOp::Subtract => 2,
        };
        ffi::combine_tools(self.0.pin_mut(), code, parallel).map_err(|e| e.to_string())
    }

    /// Every edge of `blends` rounded, with one `SmFilletExecutive`
    /// (circular sections, constant radius). SMLib measures chamfers as
    /// a chord across the edge, not as setbacks, so only fillets are
    /// compared. SMLib's failure as text.
    pub fn fillet(&mut self, blends: &[BenchBlend], parallel: bool) -> Result<(), String> {
        let mut radii = Vec::new();
        for b in blends {
            match b.kind {
                BenchKind::Fillet(r) => radii.push(r),
                _ => return Err("only fillets".into()),
            }
        }
        let edges: Vec<f64> = blends.iter().flat_map(|b| b.at).collect();
        ffi::fillet(self.0.pin_mut(), &edges, &radii, parallel).map_err(|e| e.to_string())
    }

    /// The B-rep's vertex positions (x, y, z flattened), in SMLib's order.
    pub fn vertices(&self) -> Vec<f64> {
        ffi::vertices(&self.0)
    }

    pub fn faces(&self) -> u32 {
        ffi::faces(&self.0)
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
