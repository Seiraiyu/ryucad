# CLAUDE.md

This file provides guidance to Claude Code (claude.ai/code) when working with code in this repository.

## What this is

ryucad is CAD built for AI agents to drive, not primarily for humans clicking in a GUI. FreeCAD was rejected because it performs poorly on large assemblies, and its kernel, OCCT, is a main reason. This project builds its own kernel, **ryukei**, and the long-term target is Parasolid-level capability.

The project is in its early stages. Roadmap phases 0 (foundations: deterministic math, the tolerance model, exact predicates, interval arithmetic, the test harness), 1 (topology core: flat-array B-rep bodies, Euler operators, the validity checker, stable names, the five primitives), 2 (geometry: rational NURBS, the ellipse, pcurves, derivatives, global projection), 3 (tessellation: certified, watertight, parallel meshing, and the first OCCT comparison) and 4 (sweeps: validated profiles, extrude, revolve, swept and spun surfaces) are done. Analytic booleans (phase 5) come next; see design 14 (ryukei roadmap) in the Coxswain planning project `ryucad-ryukei`. On the application side, ryucad phase 0 (projects of feature files, deterministic rebuild of the primitives, the agent CLI, the session daemon, releases and installers) is done.

Above the kernel sit ryucad, the application (feature files in Git, the agent CLI, a live viewer humans watch but don't model in, assemblies at scale), and ryupdm, product data management (revisions, releases, where-used, BOM, hosted review; PDM only, not PLM). ryupdm is the likely business; ryukei and ryucad may be open sourced. Their roadmaps are designs 20 (ryucad) and 21 (ryupdm), design 23 lines up all three, and design 22 is the business plan.

## Commands

This is a Rust Cargo workspace (edition 2024). `rust-toolchain.toml` pins the toolchain, and both native builds and the devcontainer use it.

```sh
cargo build --workspace
cargo test --workspace
cargo test -p ryukei <test_name>     # single test in one crate
cargo clippy --workspace --all-targets -- -D warnings
cargo fmt --all
cargo run -p ryucad
UPDATE_GOLDEN=1 cargo test -p ryukei --test determinism   # after a deliberate change to results
UPDATE_GOLDEN=1 cargo test -p ryucad --test determinism   # the same for ryucad's rebuild hashes
cargo bench -p ryukei-bench
cargo bench -p ryucad
cargo test -p ryukei-bench --features occt,smlib --release --test kernels -- --nocapture   # head-to-head with OCCT and SMLib; devcontainer only
cargo test -p ryukei-bench --features occt,smlib --release --test spike -- --nocapture     # OCCT and SMLib booleans, speed and repeatability; devcontainer only
cd fuzz && cargo +nightly fuzz run predicates               # needs nightly + cargo-fuzz
sh scripts/test-install.sh target/release/ryucad          # install.sh against a fake release
sh scripts/smoke.sh target/release/ryucad                 # a short agent session on a built binary
```

Releases are cut by pushing a tag `vX.Y.Z` that matches `version` in the root `Cargo.toml`; `.github/workflows/release.yml` builds, tests and publishes, then tests the installers against the new release. Pushing a tag publishes: only the owner does it, or asks for it.

Development happens on a headless Linux VM using the devcontainer, driven by the `devcontainer` CLI rather than VS Code:

```sh
devcontainer up --workspace-folder .
devcontainer exec --workspace-folder . cargo test --workspace
```

The product targets Windows, macOS, and Linux natively, not just WSL. CI (`.github/workflows/ci.yml`) runs fmt, clippy, and tests on Linux x64, Linux ARM, macOS ARM, and Windows, and also checks that the devcontainer builds. The devcontainer is always Linux, so the CI matrix is the only proof that native Mac and Windows builds work.

## Layout

- `crates/ryukei`: the B-rep geometry kernel (library).
- `crates/ryucad`: the application layer and binary.
- `crates/ryukei-testkit`: test-only helpers (exact references, seeded inputs, golden hashes, Euler operator sequences from bytes, random NURBS). Never a runtime dependency.
- `crates/ryukei-bench`: divan benchmarks, and the comparisons behind its `occt` and `smlib` features: `cxx` shims over a system OCCT 7.8 and over `libsmlib`, the kernel of NVIDIA's USD BRep built from a pinned commit. The devcontainer installs both.
- `fuzz/`: cargo-fuzz targets, outside the workspace (nightly only).
- `install.sh`, `install.ps1`: the installers, also published with each release. `scripts/` holds their tests and the release smoke test.
- Design documents, roadmaps and plans live only in the Coxswain planning project `ryucad-ryukei` (`cox doc list`), never as markdown in the repo, so there is one copy of each.

ryupdm code will live in this monorepo too; the repo may be split later if parts are open sourced.

The boundary between the two crates is deliberate. ryukei works on single bodies (geometry, topology, booleans, blends, tessellation) and must not depend on documents, assemblies, or agents. Large-assembly performance (instancing, lazy loading, caching, incremental recompute) and the agent interface live in ryucad. Keep both in this repo until the kernel API settles.

## Kernel principles

- **Parasolid-level is the goal.** Supporting analytic surfaces first (planes, cylinders, cones, spheres, tori, and fillets between them) only sets the order of work, not the scope. Never hard-code analytic-only assumptions into the core types. Analytic surfaces are fast special cases of a general surface/curve representation that NURBS will slot into.
- **Tolerant modeling from day one.** Edges and vertices carry their own tolerances.
- **Data-oriented topology.** Store topology in flat arrays referenced by IDs, not reference-counted pointer graphs like OCCT's. This makes models cheap to copy, cache-friendly, and shareable across instances.
- **Parallel from the start.** Design operations so independent work (tessellation, separate bodies, recomputing independent parts) can run concurrently.
- **Stable names for faces and edges.** Agents need references to faces and edges that survive a rebuild. FreeCAD's "toponaming" problem is the failure to avoid.
- **Deliberate numerical precision.** Decide explicitly which calculations must be exact and which can use floating point with tolerances.
- **Identical results on every platform.** The same model must produce the same result on every OS and CPU. Don't use the platform's own math functions (`f64::sin` and similar) where results must match; use a portable implementation such as the `libm` crate. Use fused multiply-add only explicitly, never fast-math, and only portable or runtime-selected SIMD. Otherwise booleans can succeed on one OS and fail on another, and saved test results won't match across machines.
- **CPU-only.** The kernel needs 64-bit floats and branch-heavy code, which GPUs handle poorly, so performance comes from the memory layout and running on many cores. GPU work (viewer, rendering for agents, clash detection) belongs in ryucad. CI uses software renderers for it.
- **Keep C++ kernels out of the default build.** Put OCCT and SMLib behind a Cargo feature or in a separate bench crate, so the default build has no C++ dependency on any platform.
- **Robustness through testing at scale.** Use a corpus of real models, randomly generated and mutated operations, checks that the topology stays valid after every operation, comparison of results against OCCT and SMLib, and benchmarks against OCCT's numbers. OCCT and SMLib may serve as references and benchmark baselines, and as a temporary fallback (in ryucad, preferring SMLib) for operations ryukei can't do yet, but never as the long-term kernel. See "Reference kernels" in design 14 (ryukei roadmap).
- **Port SMLib's algorithms, not its numerics.** SMLib (NVIDIA's USD BRep, Apache 2.0) may be ported into ryukei. Keep its NOTICE and credit it in the ported file. Topology decisions still go through `ryukei::predicates`, `ryukei::interval` and `tol`, never SMLib's float tests.

## Numerics (decided in phase 0)

The design is document 12 in the Coxswain planning project `ryucad-ryukei`.

- **Units and tolerances.** Model space is SI meters. `ryukei::tol` fixes `LINEAR_RESOLUTION` = 1e-8 m, `ANGULAR_RESOLUTION` = 1e-11 rad and a size box of ±500 m for every body. Per-entity `Tolerance` values are never below the resolution. Unit conversion and larger models are ryucad's job.
- **Precision policy.** Decisions that change topology or classification (side of a plane, inside a circle or sphere, ordering) use exact or certified predicates (`ryukei::predicates`, `ryukei::interval`), never a raw float sign. Geometric values (coordinates, parameters, distances, constructed points) are f64 compared through `tol`, never with `==`. Predicates take their f64 inputs as exact. An uncertain interval sign must be escalated or resolved by an explicit tolerance rule, never guessed.
- **Determinism.** Kernel code gets transcendental functions only from `ryukei::math`; `clippy.toml` bans the `f64` versions and `mul_add`. `tests/golden/determinism.txt` holds the bit-exact hashes that every CI platform and CPU level must reproduce. When you change results on purpose, regenerate it and review the diff.

## Topology (decided in phase 1)

The design is document 15 in the Coxswain planning project `ryucad-ryukei`.

- **Storage.** A `ryukei::topo::Body` holds flat arrays of regions, shells, faces, loops, fins, edges and vertices, plus its curves and surfaces, all referenced by typed `u32` IDs. Each edge has a ring of fins (Parasolid style), so sheets and non-manifold bodies fit later; phase 1 builds and accepts manifold solids only. Deleting leaves a tombstone; IDs are never reused until `Body::compact()`, which returns a `Remap`. Closed edges have exactly one vertex; periodic faces have no seam edges (a sphere or torus is one face with no loops).
- **Editing.** Topology changes only through `ryukei::euler`: MVFS/KVFS, MEV/KEV, MEF/KEF, KEMR/MEKR, KFMRH/MFKRH, SEMV/JEKV, MVR/KVR, plus `set_point`/`set_curve`/`set_surface`. Operators check every precondition before writing (on `Err` the body is unchanged) and decide no geometry; the caller passes it.
- **Geometry.** Curves and surfaces live in the body's own arrays and may be shared by several edges or faces; see Geometry below.
- **Names.** Faces, edges and vertices carry a 128-bit `name::Name`: SipHash-2-4 of the caller's `OpKey`, the entity's `Role` and its parents' names. A split retires the parent's name and names both children; every operation returns a `Journal` of the changes. The kernel never invents keys; resolving an agent's reference through the journal is ryucad's job.
- **Validity.** `check::check_topology` holds after every Euler operator (debug builds assert it); `check::check` adds geometry and orientation and holds for finished bodies (primitives assert it). Self-intersection is not checked until booleans exist (phase 5).

## Geometry (decided in phase 2)

The design is document 17 in the Coxswain planning project `ryucad-ryukei`.

- **Types.** `geom::Curve` (line, circle, ellipse, NURBS), `geom::Surface` (plane, cylinder, cone, sphere, torus, NURBS) and the pcurve type `geom::Curve2` (line, circle, ellipse, NURBS in a surface's `(u, v)` plane) are `#[non_exhaustive]` enums, `Clone` but not `Copy`. Analytic variants are fast paths with closed forms. A NURBS sits behind an `Arc` and never changes once built, so cloning a body shares it; its Bézier pieces are built on first use in a `OnceLock` that takes no part in equality or the dump. Fins don't carry pcurves yet.
- **NURBS.** Rational, degree 1 to 25, with a `KnotVector` that is clamped or periodic (distinct knots plus multiplicities). Constructors reject bad input with `Error::InvalidNurbs(NurbsError)`, `NonFinite` or `OutsideSizeBox`; collapsed control rows (poles) are allowed. `refine`, `split` and `to_clamped` return clamped splines; `transformed` takes a `Rigid`. Every analytic type converts exactly (`to_nurbs`), with a `Reparam` relating the parameters.
- **Derivatives.** `eval` gives derivatives up to second order. Tangents and normals are `Option`: a NURBS judges vanishing on derivatives scaled to its domain, and gives the limit normal at a pole. Principal curvatures are measured against the natural normal, positive where the surface bends towards it.
- **Projection.** `project` returns a point within `LINEAR_RESOLUTION` of the true global minimum, and among minima that close, the lowest parameter. The search works on the Bernstein coefficients of `(S - p)·S'` per Bézier piece: one sign means no stationary point, one sign change means exactly one (curves); surfaces add an interval Newton step and a depth limit, then Newton's method. Domain boundaries, kinks and creases (knots of multiplicity `degree`) are searched as curves, and every candidate must be a genuine local minimum. Degenerate splines (folds, collapsed patches, poles) are where this breaks: the property tests and the `nurbs` fuzz target pin the cases found so far.
- **Bodies.** Euler operators, the checker, `compact` and the dump accept every variant. The checker measures distances through `project`, bounds NURBS by their hulls, requires edge intervals inside a clamped curve's domain, and lets a closed edge lie on a clamped curve whose ends meet. A face with no loops is still only allowed on an analytic sphere or torus.

## Tessellation (decided in phase 3)

The design is document 19 in the Coxswain planning project `ryucad-ryukei`.

- **API.** `mesh::tessellate(&Body, &MeshParams) -> Result<BodyMesh, Error>`. `MeshParams` has an absolute `chord` (at least `10 × LINEAR_RESOLUTION`), an `angle` in `(0, π/2]`, an optional `max_edge`, and a `max_triangles` budget (50M by default). Bad parameters are `InvalidMeshParams`; a runaway request is `MeshTooLarge`, refused from an estimate before faces are meshed, and again if the real count passes the budget.
- **Guarantee.** Every face is within `chord` of its triangles both ways (two-sided Hausdorff), plus the local edge tolerance near tolerant edges. The bounds are certified, never sampled: `geom::{CurveBound, SurfaceBound}` bound second derivatives and turning rates over a parameter range (closed forms for analytic types, interval arithmetic on Bézier pieces for NURBS, differencing the whole piece before cutting it down), and a triangle passes when `M R² / 2 <= chord` (Waldron's bound, `R` the smallest enclosing circle in the face's scaled parameter plane). The normal turns at most `angle` across a triangle, except that the angle test is waived for pieces smaller than `chord` (features under the tolerance, and poles or apexes where the normal has no limit).
- **Structure.** Edges are sampled once (bisection until the curve and every adjacent face are satisfied, with an exact sample at every seam crossing) and shared, so the mesh is watertight by index. Each face is a constrained Delaunay triangulation on exact `orient2d`/`incircle` in its parameter plane scaled by powers of two (so scaling is exact), cut at seams (the domain start of a periodic direction) into one period, with collapsed rows (poles, apexes) as sides whose points weld to one position. Refinement is longest-edge bisection; a face never splits a boundary segment itself but asks its edge for a sample, and is meshed again (up to twelve rounds; after three, an edge still asked about is halved all along, so the asking stops). An edge's first segments are checked at their middle, so an arc of half a turn or more round a periodic face can't be mistaken for the short way round. A vertex whose neighbours meet one welded position in two runs (a "pinch" at a pole or across a seam) is repaired by flips or splits before assembly.
- **Output.** `BodyMesh`: welded `positions` (body vertices, then edge samples, then face points), per-face `corners` (position, unwrapped `(u, v)`, outward normal), triangles counterclockwise from outside, `FaceMesh` ranges by `FaceId`, `EdgePolyline`s by `EdgeId`, and `faults`. A face that can't meet the guarantees (`EdgeOffSurface`, `BadDomain`, `RefinementStalled`, `BoundaryNotResolved`) is left out and reported; `check_mesh` verifies indices, winding and that every welded edge has exactly two opposite uses.
- **Parallelism and determinism.** `ryukei` depends on `rayon`: edges, then faces, run on the current pool; each is a pure function and results are gathered in ID order, so the mesh is the same bits on any number of threads. Golden hashes pin meshes across platforms.
- **Limits.** Closed directions are periodic ones only (a clamped NURBS face whose ends meet is `BadDomain`). On a doubly periodic surface a face either has no loops or has loops that leave one direction unwound (a torus band between two circles, a pipe's elbow): that direction is meshed as open, wrapped from just below the face by its loops' orientation, and loops winding both ways are `BadDomain`. A ring through two poles (a sphere's wedge) takes the stretch between them by the face's orientation. A feature thinner than the chord (a torus tube) can stall refinement. Meshes are not FEA-quality, and ryukei makes about 1.5 to 18 times OCCT's triangles at the same chord.

## Sweeps (decided in phase 4)

The design is document 27 in the Coxswain planning project `ryucad-ryukei`.

- **Profiles.** `sweep::Profile` is a `Frame` and loops of trimmed `Curve2` segments (`t1 < t0` runs a segment backwards), each segment and loop carrying a caller key (`SegmentKey`, `LoopKey`, 128-bit, unique in the profile). `ValidProfile::new` checks structure, ranges and closure (gaps up to `LINEAR_RESOLUTION`), then certifies that segments are more than `LINEAR_RESOLUTION` apart except at their shared ends (rational Bézier pieces with a `1e-10` m rounding slack, cone tests at joints; uncertain counts as touching), signs each loop's area by an explicit rule, and nests loops by winding number into lumps with holes, outer loops counterclockwise. Failures are `Error::InvalidProfile(ProfileError)`, naming the keys. A whole period of a periodic curve must be its loop's only segment.
- **Operations.** `sweep::extrude(key, &profile, dir, start, end)` and `sweep::revolve(key, &profile, Axis, start, end)` make one body with a solid region per lump, built through Euler operators (`sweep/build.rs`) and named from the caller's keys. Extrude takes any direction more than `ANGULAR_RESOLUTION` from the profile's plane (snapped to the normal within it). Revolve's axis lies in the profile's plane; a span within `ANGULAR_RESOLUTION` of 2π is a full turn with no caps. The profile may touch the axis at segment ends or lie along it, never cross it or touch it inside a segment; a full turn refuses holes and several stretches along the axis (`EnclosesVoid`: void shells wait for booleans) and lone touches (`PinchedOnAxis`). Parameter faults are `Error::InvalidSweep(SweepError)`.
- **Surfaces.** Sides are analytic wherever exact (plane, cylinder, cone, sphere, ring torus), otherwise `Surface::Extrusion` (a planar curve along a direction) or `Surface::Revolution` (a curve in the frame's xz half-plane turned about z; poles only at a clamped curve's ends, closed genus 0 with poles at both ends, 1 for a periodic curve). Both keep their generating curve, project exactly through it, and convert exactly to NURBS. A revolve's sides measure their angle from opposite the middle of a partial turn, so no face meets a surface's seam.
- **Topology and names.** Periodic sides have no seams; a full turn's profile vertices sweep closed circles; a vertex on the axis is a body vertex only at an apex (a side meeting the axis at an angle), not at a pole. Names (`name::SweepRole`) come only from the sweep's key and the caller's keys: a side from its segment, a cap from its lump's outer loop and its end, a cap edge from its cap and side, a lateral edge and its vertices from the segments either side of the profile vertex. Editing dimensions or rotating a loop's start renames nothing; adding a segment renames only what sat where its new vertex is.
- **Limits.** Pcurves on fins wait for booleans (phase 5). Extrude and revolve cost grows with the square of a loop's segment count (MEF walks the loop): about 70 ms at 1,000 segments.

## Persistence (decided 2026-10-16)

A ryucad model is a folder of canonical text feature files in a Git repo, one per feature or object, rebuilt deterministically by ryukei. Git provides history, branches and merges; ryucad supplies CAD-aware diff and merge drivers, and features reference topology by stable name. Derived data (bodies, meshes, renders) is a cache, never committed. ryupdm builds on the same repos rather than a second store. This follows the freecad-spike (github.com/Seiraiyu/freecad-spike) GO verdict, without FreeCAD.

## ryucad (decided in ryucad phase 0)

The design is document 24 in the Coxswain planning project `ryucad-ryukei`.

- **Projects.** A project is a folder: `ryucad.json` (format, default unit), then `parts/<part>/part.json` (the ordered feature list) and `parts/<part>/features/<feature>.json`. Every file is canonical JSON (`doc::canonical`): sorted keys, one value per line, shortest round-trip numbers, LF, a trailing newline, written atomically. `format` is versioned per file kind; newer is refused with `format_too_new`. Part and feature IDs are `^[a-z0-9][a-z0-9_-]{0,62}$`, not a Windows device name, and never change. Derived data never goes into the project.
- **Units.** Quantities are strings with units (`"10 mm"`), kept as written; bare numbers take the project's default unit and are written explicitly. ryucad converts to meters only when it calls ryukei, by one f64 multiplication. Output reports `{"value", "m"}`.
- **Rebuild.** Each phase 0 feature is one primitive with its own body. Its `OpKey` is `OpKey::from_bytes("ryucad.feature\0<part>\0<feature>")`, so editing parameters or placement never renames anything. Placement is `origin` plus `z` and `x` axes (`Rigid::from_frame`). Features rebuild in parallel, are collected in order, and every body passes `check::check`. `tests/golden/rebuild.txt` holds ryucad's bit-exact hashes.
- **Agent interface.** The `ryucad` CLI prints one JSON document (`{"ok", "result" | "error", "warnings"}`) and exits 0, 1 (the agent can fix it), 2 (usage), 3 (no usable project) or 4 (not the agent's fault). Error codes (`error::Code`) are stable and never reused. A part's `revision` (a hash of the files that define it) guards writes through `--expect`.
- **Daemon.** One per project (`ryucad daemon run`), started by the CLI on demand, serving newline-delimited JSON-RPC 2.0 (`proto`) on a Unix socket in a private runtime folder or an owner-only named pipe. It checks file freshness before every request (size and time, then content hash) and caches rebuilds by revision. A daemon of another version is replaced; a request that was never delivered runs in process with a warning; `--no-daemon` or `RYUCAD_NO_DAEMON=1` skips it. CLI tests run every scenario both ways.
- **Persistence and releases.** Models are the feature files in Git. Releases are static binaries for x86_64/aarch64 Linux (musl, with mimalloc), aarch64 macOS and x86_64 Windows, with `SHA256SUMS`; the installers verify them and install to `~/.ryucad/bin`.

## Open decisions

Don't assume an answer to these. Ask the user, or record the decision here once it's made:

- When the MCP adapter over `proto` arrives.
- Which parts are open source and which are paid.
- License. Apache-2.0 is the lean (design 22), and ported SMLib code (Apache-2.0) rules out GPLv2-only.
