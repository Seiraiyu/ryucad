//! OCCT, through a small C++ shim, for comparison: the same primitives
//! built with `BRepPrimAPI`, NURBS forms through `BRepBuilderAPI_NurbsConvert`,
//! sweeps from `BRepPrimAPI_MakePrism` and `BRepPrimAPI_MakeRevol` over a
//! face built from a [`BenchProfile`], booleans from `BRepAlgoAPI_Cut`,
//! fillets and chamfers from `BRepFilletAPI`, meshes from `BRepMesh_IncrementalMesh`
//! in absolute mode, exact volume, area and distance from the B-rep, and
//! STL files read by `RWStl` and written by `StlAPI_Writer`.
//! Only built with the `occt` feature, against a system OCCT 7.8 or later
//! (`OCCT_DIR`, default `/usr`).

use crate::{BenchBlend, BenchKind, BenchProfile, Primitive, Soup};
use ryukei::boolean::BooleanOp;
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
        fn extrude(
            frame: &[f64],
            sizes: &[u32],
            segments: &[f64],
            vector: &[f64],
        ) -> Result<UniquePtr<Shape>>;
        fn revolve(
            frame: &[f64],
            sizes: &[u32],
            segments: &[f64],
            axis: &[f64],
            angle: f64,
        ) -> Result<UniquePtr<Shape>>;
        fn make_compound() -> UniquePtr<Shape>;
        fn add_to_compound(compound: Pin<&mut Shape>, s: &Shape);
        fn cut(s: &Shape, tools: &Shape, parallel: bool) -> Result<UniquePtr<Shape>>;
        fn boolean(s: &Shape, tools: &Shape, op: u32, parallel: bool) -> Result<UniquePtr<Shape>>;
        fn vertices(s: &Shape) -> Vec<f64>;
        fn faces(s: &Shape) -> u32;
        fn mesh(s: &Shape, chord: f64, angle: f64, parallel: bool) -> OcctMesh;
        fn volume(s: &Shape) -> f64;
        fn area(s: &Shape) -> f64;
        fn distance(s: &Shape, x: f64, y: f64, z: f64) -> f64;
        fn fillet(s: &Shape, edges: &[f64], radii: &[f64]) -> Result<UniquePtr<Shape>>;
        fn chamfer(
            s: &Shape,
            edges: &[f64],
            modes: &[u32],
            sizes: &[f64],
            faces: &[f64],
        ) -> Result<UniquePtr<Shape>>;
        fn step_read(path: &str, fix: bool) -> Result<UniquePtr<Shape>>;
        fn step_write(s: &Shape, path: &str, ap242: bool) -> Result<()>;
        fn stl_read(path: &str) -> Result<OcctMesh>;
        fn mesh_stl(
            s: &Shape,
            chord: f64,
            angle: f64,
            parallel: bool,
            path: &str,
        ) -> Result<Vec<f64>>;
        fn solid_props(s: &Shape) -> Vec<f64>;
        fn thru_sections(sizes: &[u32], spec: &[u32], data: &[f64]) -> Result<UniquePtr<Shape>>;
        fn pipe_shell(
            sizes: &[u32],
            spec: &[u32],
            data: &[f64],
            binormal: &[f64],
        ) -> Result<UniquePtr<Shape>>;
        fn offset_shape(s: &Shape, d: f64) -> Result<UniquePtr<Shape>>;
        fn thick_solid(s: &Shape, open: &[f64], thickness: f64) -> Result<UniquePtr<Shape>>;
        fn intersect_faces(
            a: &Shape,
            pa: &[f64],
            b: &Shape,
            pb: &[f64],
            samples: u32,
        ) -> Result<Vec<f64>>;
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

    /// This solid moved by `at`.
    pub fn placed(&self, at: &Rigid) -> Self {
        let r = at.rotation_matrix();
        let t = at.translation_part();
        let m: Vec<f64> = r.iter().flatten().copied().chain([t.x, t.y, t.z]).collect();
        Self(ffi::placed(&self.0, &m))
    }

    /// The same solid with every curve and surface converted to NURBS.
    pub fn to_nurbs(&self) -> Self {
        Self(ffi::nurbs(&self.0))
    }

    /// The profile's face swept `dist` along `dir`, with
    /// `BRepPrimAPI_MakePrism`.
    pub fn extrude(profile: &BenchProfile, dir: [f64; 3], dist: f64) -> Result<Self, String> {
        let (frame, sizes, segments) = profile.flatten();
        ffi::extrude(&frame, &sizes, &segments, &crate::sweep_vector(dir, dist))
            .map(Self)
            .map_err(|e| e.to_string())
    }

    /// The profile's face turned `angle` radians about the axis through
    /// `axis_origin` along `axis_dir`, with `BRepPrimAPI_MakeRevol`.
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

    /// This solid minus every one of `tools`, in one `BRepAlgoAPI_Cut`
    /// with the tools as its tool list. On one thread unless `parallel`.
    pub fn cut(&self, tools: &[Solid], parallel: bool) -> Self {
        let mut compound = ffi::make_compound();
        for t in tools {
            ffi::add_to_compound(compound.pin_mut(), &t.0);
        }
        Self(ffi::cut(&self.0, &compound, parallel).expect("OCCT cuts"))
    }

    /// This solid united with, intersected with or minus every one of
    /// `tools`, in one `BRepAlgoAPI_BooleanOperation` with the tools as
    /// its tool list. On one thread unless `parallel`. OCCT's failure as
    /// text.
    pub fn boolean(&self, op: BooleanOp, tools: &[Solid], parallel: bool) -> Result<Self, String> {
        let mut compound = ffi::make_compound();
        for t in tools {
            ffi::add_to_compound(compound.pin_mut(), &t.0);
        }
        let code = match op {
            BooleanOp::Unite => 0,
            BooleanOp::Intersect => 1,
            BooleanOp::Subtract => 2,
        };
        ffi::boolean(&self.0, &compound, code, parallel)
            .map(Self)
            .map_err(|e| e.to_string())
    }

    /// Every edge of `blends` filleted, with one
    /// `BRepFilletAPI_MakeFillet`, or chamfered, with one
    /// `BRepFilletAPI_MakeChamfer` (two distances with `Add(d1, d2, E,
    /// F)`, distance and angle with `AddDA`). OCCT's failure as text;
    /// fillets and chamfers together are refused.
    pub fn blend(&self, blends: &[BenchBlend]) -> Result<Self, String> {
        let edges: Vec<f64> = blends.iter().flat_map(|b| b.at).collect();
        if blends
            .iter()
            .all(|b| matches!(b.kind, BenchKind::Fillet(_)))
        {
            let radii: Vec<f64> = blends
                .iter()
                .map(|b| match b.kind {
                    BenchKind::Fillet(r) => r,
                    _ => unreachable!(),
                })
                .collect();
            return ffi::fillet(&self.0, &edges, &radii)
                .map(Self)
                .map_err(|e| e.to_string());
        }
        let (mut modes, mut sizes, mut faces) = (Vec::new(), Vec::new(), Vec::new());
        for b in blends {
            let (m, s, f) = match b.kind {
                BenchKind::Fillet(_) => return Err("fillets and chamfers together".into()),
                BenchKind::Equal(d) => (0, [d, d], [0.0; 3]),
                BenchKind::Distances {
                    first,
                    second,
                    first_face,
                } => (1, [first, second], first_face),
                BenchKind::DistanceAngle {
                    distance,
                    angle,
                    face,
                } => (2, [distance, angle], face),
            };
            modes.push(m);
            sizes.extend(s);
            faces.extend(f);
        }
        ffi::chamfer(&self.0, &edges, &modes, &sizes, &faces)
            .map(Self)
            .map_err(|e| e.to_string())
    }

    /// The B-rep's vertex positions (x, y, z flattened), in OCCT's order.
    pub fn vertices(&self) -> Vec<f64> {
        ffi::vertices(&self.0)
    }

    pub fn faces(&self) -> u32 {
        ffi::faces(&self.0)
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

    /// A copy of the shape meshed by `BRepMesh_IncrementalMesh` at an
    /// absolute chord and angle and written to `path` by `StlAPI_Writer`
    /// as binary STL: the triangles written and the seconds the meshing
    /// and writing took (the copy, which a fresh shape wouldn't need, is
    /// left out). OCCT's failure as text.
    pub fn mesh_stl(
        &self,
        chord: f64,
        angle: f64,
        parallel: bool,
        path: &std::path::Path,
    ) -> Result<(u64, f64), String> {
        let v = ffi::mesh_stl(&self.0, chord, angle, parallel, &path.to_string_lossy())
            .map_err(|e| e.to_string())?;
        Ok((v[0] as u64, v[1]))
    }

    /// Every root of the STEP file at `path` as one shape, read by
    /// `STEPControl_Reader` and, if `fix`, healed by `ShapeFix_Shape`, in
    /// the file's units as OCCT converts them (millimetres). OCCT's
    /// failure as text.
    pub fn read_step(path: &std::path::Path, fix: bool) -> Result<Self, String> {
        ffi::step_read(&path.to_string_lossy(), fix)
            .map(Self)
            .map_err(|e| e.to_string())
    }

    /// The shape written to `path` by `STEPControl_Writer`, as AP242 or
    /// else AP214, in millimetres.
    pub fn write_step(&self, path: &std::path::Path, ap242: bool) -> Result<(), String> {
        ffi::step_write(&self.0, &path.to_string_lossy(), ap242).map_err(|e| e.to_string())
    }

    /// Each solid's volume, area and face count, in OCCT's order.
    pub fn solids(&self) -> Vec<SolidProps> {
        ffi::solid_props(&self.0)
            .chunks(3)
            .map(|c| SolidProps {
                volume: c[0],
                area: c[1],
                faces: c[2] as u32,
            })
            .collect()
    }
}

/// A solid's measures, as [`Solid::solids`] gives them.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct SolidProps {
    pub volume: f64,
    pub area: f64,
    pub faces: u32,
}

/// The STL file at `path` (binary or ASCII) read by `RWStl::ReadFile`,
/// OCCT's STL reader under `StlAPI_Reader`, which welds coincident
/// vertices: its triangles by position index. OCCT's failure as text.
pub fn read_stl(path: &std::path::Path) -> Result<OcctMesh, String> {
    ffi::stl_read(&path.to_string_lossy()).map_err(|e| e.to_string())
}

impl OcctMesh {
    /// Whether the triangles close up: every edge between two positions
    /// used once each way.
    pub fn closed(&self) -> bool {
        let mut uses = std::collections::BTreeMap::<(u32, u32), i32>::new();
        for t in self.triangles.chunks(3) {
            for (a, b) in [(t[0], t[1]), (t[1], t[2]), (t[2], t[0])] {
                *uses.entry((a, b)).or_default() += 1;
            }
        }
        uses.iter()
            .all(|(&(a, b), &n)| n == 1 && uses.get(&(b, a)) == Some(&1))
    }

    pub fn soup(&self) -> Soup<'_> {
        Soup {
            positions: &self.positions,
            triangles: &self.triangles,
        }
    }

    /// The enclosed volume and the area.
    pub fn volume_and_area(&self) -> (f64, f64) {
        self.soup().volume_and_area()
    }

    /// The largest distance from sampled points of every `every`-th
    /// triangle to the solid's boundary: one way only, mesh to solid.
    pub fn deviation(&self, solid: &Solid, every: usize) -> f64 {
        self.soup().deviation(every, |p| solid.distance(p))
    }
}

impl Solid {
    /// A loft through `sections`, each a closed run of curves in order,
    /// with `BRepOffsetAPI_ThruSections` (a solid, not ruled).
    pub fn loft(sections: &[Vec<(ryukei::geom::Curve, [f64; 2])>]) -> Result<Self, String> {
        let mut curves = crate::BenchCurves::default();
        let mut sizes = vec![sections.len() as u32];
        for s in sections {
            sizes.push(s.len() as u32);
            for (c, r) in s {
                curves.push(c, *r).map_err(|e| e.to_string())?;
            }
        }
        ffi::thru_sections(&sizes, &curves.spec, &curves.data)
            .map(Self)
            .map_err(|e| e.to_string())
    }

    /// `profile` (a closed run of curves) swept along `spine` (an open
    /// run) with `BRepOffsetAPI_MakePipeShell`, made a solid: by the
    /// corrected Frenet frame, or a fixed binormal.
    pub fn pipe(
        profile: &[(ryukei::geom::Curve, [f64; 2])],
        spine: &[(ryukei::geom::Curve, [f64; 2])],
        binormal: Option<[f64; 3]>,
    ) -> Result<Self, String> {
        let mut curves = crate::BenchCurves::default();
        for (c, r) in profile.iter().chain(spine) {
            curves.push(c, *r).map_err(|e| e.to_string())?;
        }
        let sizes = [profile.len() as u32, spine.len() as u32];
        let b: Vec<f64> = binormal.map_or(Vec::new(), |b| b.to_vec());
        ffi::pipe_shell(&sizes, &curves.spec, &curves.data, &b)
            .map(Self)
            .map_err(|e| e.to_string())
    }

    /// Every face moved `d` along its outward normal, with
    /// `BRepOffsetAPI_MakeOffsetShape` (arcs at opening edges).
    pub fn offset(&self, d: f64) -> Result<Self, String> {
        ffi::offset_shape(&self.0, d)
            .map(Self)
            .map_err(|e| e.to_string())
    }

    /// Hollowed inwards to `thickness`, the faces nearest `open` removed,
    /// with `BRepOffsetAPI_MakeThickSolid`.
    pub fn shell(&self, open: &[Point3], thickness: f64) -> Result<Self, String> {
        let pts: Vec<f64> = open.iter().flat_map(|p| [p.x, p.y, p.z]).collect();
        ffi::thick_solid(&self.0, &pts, thickness)
            .map(Self)
            .map_err(|e| e.to_string())
    }
}

/// Where `a`'s face nearest `pa` and `b`'s nearest `pb` meet, by
/// `BRepAlgoAPI_Section` of the two faces: each edge sampled at `samples
/// + 1` evenly spaced parameters.
pub fn intersect_faces(
    (a, pa): (&Solid, Point3),
    (b, pb): (&Solid, Point3),
    samples: u32,
) -> Result<Vec<Vec<Point3>>, String> {
    let flat = ffi::intersect_faces(
        &a.0,
        &[pa.x, pa.y, pa.z],
        &b.0,
        &[pb.x, pb.y, pb.z],
        samples,
    )
    .map_err(|e| e.to_string())?;
    let mut at = 1;
    let mut out = Vec::new();
    for _ in 0..flat[0] as usize {
        let n = flat[at] as usize;
        at += 1;
        out.push(
            (0..n)
                .map(|k| Point3::new(flat[at + 3 * k], flat[at + 3 * k + 1], flat[at + 3 * k + 2]))
                .collect(),
        );
        at += 3 * n;
    }
    Ok(out)
}
