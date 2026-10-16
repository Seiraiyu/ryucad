# ryukei roadmap

A high-level plan, not a commitment. Each phase produces something testable and benchmarkable. The long-term target is Parasolid-level capability.

**Every phase, from day one:** a topology validity checker run after every operation, fuzzing, a growing corpus of real models, benchmarks against OCCT, comparisons against the reference kernels (see below), and checks that results are identical across platforms.

| # | Phase | Delivers |
|---|---|---|
| 0 | Foundations (done) | Vector and transform math, a deterministic math layer, the tolerance model, robust geometric predicates, and the test/bench harness |
| 1 | Topology core (done) | ID-based flat-array topology (body/shell/face/loop/edge/vertex), Euler operators, validity checker, stable face/edge IDs, primitives (box, cylinder, sphere, cone, torus) |
| 2 | Geometry (done) | NURBS curves and surfaces alongside the analytic types from phase 1, derivative evaluation, general closest-point projection |
| 3 | Tessellation | Watertight, parallel, tolerance-driven meshing; first head-to-head benchmark against OCCT |
| 4 | Sweeps | Extrude and revolve from 2D profiles. Before it starts: the SMLib spike (see below) |
| 5 | Booleans (analytic) | Exact analytic intersections, face splitting, inside/outside classification, merging; serious fuzzing starts; results compared against OCCT and SMLib |
| 6 | Blends | Constant-radius fillets and chamfers, then corners where several fillets meet; SMLib's blending is a reference |
| 7 | Interop | STEP AP242 read/write and healing of imported geometry; real-world models join the test corpus |
| 8 | Full NURBS | General surface-surface intersection, NURBS booleans and blends, offset/shell, sweep/loft; SMLib's intersection and offset are a reference |
| 9 | Parasolid-class | Direct editing, full tolerant modeling, sheet bodies, draft, mass properties |

Wire ryucad to ryukei early (around phase 1 or 4) so agents use the kernel and generate edge cases. Phases 0–5 make ryukei useful; phases 8–9 are the long road to Parasolid level.

## Reference kernels

ryukei exists because OCCT is too slow for large assemblies, not to reinvent what already works. Two open kernels serve it, and neither is ever in the default build or the long-term kernel:

- **OCCT** (LGPL). The performance baseline, since it is what FreeCAD and CadQuery run on, and a reference for results. Behind `ryukei-bench`'s `occt` feature.
- **SMLib** (Apache 2.0), released by NVIDIA as USD BRep in October 2026 (`github.com/NVIDIA-Omniverse/usd-brep`). A NURBS-native C++ kernel with booleans, fillets, shelling, surface intersection, tessellation and healing. It is used three ways:
  - **Second reference.** Results are compared against both OCCT and SMLib, behind a feature in `ryukei-bench`. Where both agree and ryukei doesn't, the bug is probably ryukei's; where they disagree, the case joins the corpus as a hard one.
  - **Algorithm source.** Its license allows porting its algorithms (intersection, booleans, blends, offsets) into ryukei, keeping its NOTICE and attribution. Algorithms are ported, numerics are not: topology decisions go through ryukei's exact and certified predicates and the `tol` model, never SMLib's float tests. Its known weaknesses (repairs that fail without reporting, misoriented triangles from tessellation) make it a map, not an oracle.
  - **Temporary fallback** in ryucad for operations ryukei can't do yet, preferred over OCCT because its license is permissive and its geometry is NURBS-native.

NVIDIA doesn't accept contributions to USD BRep, so any fix we need lives in our own fork. solidcore (`github.com/bherbruck/solidcore`, Apache 2.0) is an independent Rust port of SMLib; its C++ FFI and differential tests may be reusable for the reference harness.

**SMLib spike (before phase 4).** Time-boxed to a day or two: does SMLib's core build without OpenUSD, Hydra and NVIDIA's packman toolchain, on all four CI platforms or only in the devcontainer; can solidcore's FFI be reused; and a first reading of its speed (load, tessellate, boolean on large bodies) and of whether repeated runs give identical results. The outcome decides how the reference feature is built and is recorded here.
