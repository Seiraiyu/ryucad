# ryukei roadmap

A high-level plan, not a commitment. Each phase produces something testable and benchmarkable. The long-term target is Parasolid-level capability.

**Every phase, from day one:** a topology validity checker run after every operation, fuzzing, a growing corpus of real models, benchmarks against OCCT, and checks that results are identical across platforms.

| # | Phase | Delivers |
|---|---|---|
| 0 | Foundations (done) | Vector and transform math, a deterministic math layer, the tolerance model, robust geometric predicates, and the test/bench harness |
| 1 | Topology core (done) | ID-based flat-array topology (body/shell/face/loop/edge/vertex), Euler operators, validity checker, stable face/edge IDs, primitives (box, cylinder, sphere, cone, torus) |
| 2 | Geometry (done) | NURBS curves and surfaces alongside the analytic types from phase 1, derivative evaluation, general closest-point projection |
| 3 | Tessellation | Watertight, parallel, tolerance-driven meshing; first head-to-head benchmark against OCCT |
| 4 | Sweeps | Extrude and revolve from 2D profiles |
| 5 | Booleans (analytic) | Exact analytic intersections, face splitting, inside/outside classification, merging; serious fuzzing starts |
| 6 | Blends | Constant-radius fillets and chamfers, then corners where several fillets meet |
| 7 | Interop | STEP AP242 read/write and healing of imported geometry; real-world models join the test corpus |
| 8 | Full NURBS | General surface-surface intersection, NURBS booleans and blends, offset/shell, sweep/loft |
| 9 | Parasolid-class | Direct editing, full tolerant modeling, sheet bodies, draft, mass properties |

Wire ryucad to ryukei early (around phase 1 or 4) so agents use the kernel and generate edge cases. Phases 0–5 make ryukei useful; phases 8–9 are the long road to Parasolid level.
