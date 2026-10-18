# CLAUDE.md

This file provides guidance to Claude Code (claude.ai/code) when working with code in this repository.

## What this is

ryucad is CAD built for AI agents to drive, not primarily for humans clicking in a GUI. FreeCAD was rejected because it performs poorly on large assemblies, and its kernel, OCCT, is a main reason. This project builds its own kernel, **ryukei**, and the long-term target is Parasolid-level capability.

The project is in its early stages. Roadmap phases 0 (foundations: deterministic math, the tolerance model, exact predicates, interval arithmetic, the test harness), 1 (topology core: flat-array B-rep bodies, Euler operators, the validity checker, stable names, the five primitives), 2 (geometry: rational NURBS, the ellipse, pcurves, derivatives, global projection) and 3 (tessellation: certified, watertight, parallel meshing, and the first OCCT comparison) are done. Sweeps (phase 4) come next; see design 14 (ryukei roadmap) in the Coxswain planning project `ryucad-ryukei`. On the application side, ryucad phase 0 (projects of feature files, deterministic rebuild of the primitives, the agent CLI, the session daemon, releases and installers) is done, and so is phase 1 (design 28): the live session, the activity log and undo, settings, Git history, display meshes, renders and `doctor` in the daemon, and the desktop app, `ryucad-app`, with its installers.

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
cargo bench -p ryucad-app
cargo run -p ryucad-app -- <project folder>                 # the desktop app (cargo build first: it needs ryucad beside it)
RYUCAD_REQUIRE_GPU=1 cargo test -p ryucad-app               # fail, don't skip, the drawing tests without an adapter
UPDATE_SNAPSHOTS=1 cargo test -p ryucad-app --test snapshot # redraw the viewport snapshot (lavapipe's)
cargo test -p ryukei-bench --features occt,smlib --release --test kernels -- --nocapture   # head-to-head with OCCT and SMLib; devcontainer only
cd fuzz && cargo +nightly fuzz run predicates               # needs nightly + cargo-fuzz
sh scripts/test-install.sh target/release/ryucad          # install.sh against a fake release
sh scripts/smoke.sh target/release/ryucad target/release/ryucad-app   # an agent session, then the app's offscreen frame
```

Releases are cut by pushing a tag `vX.Y.Z` (or `vX.Y.Z-pre`, published as a prerelease). The version comes from the tag, never from a file: the workspace version in the repo stays `0.0.0`, and `.github/workflows/release.yml` stamps the tag's version in (`scripts/stamp-version.sh`), builds, tests and publishes, then tests the installers against the new release. Pushing a tag publishes: only the owner does it, or asks for it.

Development happens on a headless Linux VM using the devcontainer, driven by the `devcontainer` CLI rather than VS Code:

```sh
devcontainer up --workspace-folder .
devcontainer exec --workspace-folder . cargo test --workspace
```

The product targets Windows, macOS, and Linux natively, not just WSL. CI (`.github/workflows/ci.yml`) runs fmt, clippy, and tests on Linux x64, Linux ARM, macOS ARM, and Windows, and also checks that the devcontainer builds. The devcontainer is always Linux, so the CI matrix is the only proof that native Mac and Windows builds work.

## Layout

- `crates/ryukei`: the B-rep geometry kernel (library).
- `crates/ryucad`: the application layer and binary.
- `crates/ryucad-app`: the desktop app, a client of the daemon (`core`: link, model, people, navigation, picking, forms; `ui`: the egui window and the wgpu scene).
- `packaging/`: what the installers register: the Linux desktop entry and project MIME type, and the macOS launcher's AppleScript.
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
- **Structure.** Edges are sampled once (bisection until the curve and every adjacent face are satisfied, with an exact sample at every seam crossing) and shared, so the mesh is watertight by index. Each face is a constrained Delaunay triangulation on exact `orient2d`/`incircle` in its parameter plane scaled by powers of two (so scaling is exact), cut at seams (the domain start of a periodic direction) into one period, with collapsed rows (poles, apexes) as sides whose points weld to one position. Refinement is longest-edge bisection; a face never splits a boundary segment itself but asks its edge for a sample, and is meshed again (up to three rounds). A vertex whose neighbours meet one welded position in two runs (a "pinch" at a pole or across a seam) is repaired by flips or splits before assembly.
- **Output.** `BodyMesh`: welded `positions` (body vertices, then edge samples, then face points), per-face `corners` (position, unwrapped `(u, v)`, outward normal), triangles counterclockwise from outside, `FaceMesh` ranges by `FaceId`, `EdgePolyline`s by `EdgeId`, and `faults`. A face that can't meet the guarantees (`EdgeOffSurface`, `BadDomain`, `RefinementStalled`, `BoundaryNotResolved`) is left out and reported; `check_mesh` verifies indices, winding and that every welded edge has exactly two opposite uses.
- **Parallelism and determinism.** `ryukei` depends on `rayon`: edges, then faces, run on the current pool; each is a pure function and results are gathered in ID order, so the mesh is the same bits on any number of threads. Golden hashes pin meshes across platforms.
- **Limits.** Closed directions are periodic ones only (a clamped NURBS face whose ends meet is `BadDomain`); doubly periodic faces must be loopless. Meshes are not FEA-quality, and ryukei makes about 1.5 to 18 times OCCT's triangles at the same chord.

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

## Live session (decided in ryucad phase 1)

The design is document 28 in the Coxswain planning project `ryucad-ryukei`.

- **Use model.** Agents model; humans watch, point, judge and nudge. Parity: everything the app can do is a `proto` method with a CLI command (a test fails otherwise), and a human's edit goes through the same request, with `expected_revision`, as an agent's. State lives in five layers: the model and project knowledge in Git, org standards (later, ryupdm), user preferences in the OS config folder, and session state in the daemon. A setting goes in Git only if someone checking out a commit needs it to rebuild or understand the model.
- **Participants.** A connection says `session.hello` as a human (an app window, gone with its last connection) or an agent (reused by name across CLI calls: `--as`/`RYUCAD_PARTICIPANT`, `agent` by default, kept 10 minutes). Each has its own selection, camera, highlights (style `pending`/`changed`/`note`, replaced by style and label) and status. "The human" is the one human attached; with several, `ambiguous_participant`. Entity references are `<part>/<feature>/<32 hex>` everywhere and never resolve to a nearest match; references that stop resolving drop out after a rebuild.
- **Notifications.** `subscribe` turns a connection into a subscription; each has a queue drained by its writer thread, and past 1000 queued it gets one `resync`. Notifications carry state: `model.changed`, `project.changed`, `config.changed`, `participant.changed`, `activity.appended`, and only changes made after subscribing. While anyone is subscribed, the daemon runs its freshness check every `daemon.poll_ms` (250 ms). `ryucad watch` lets an agent wait for them.
- **Activity and undo.** The daemon logs every change it makes (who, the method, each file before and after) and every change it sees behind its back (`external`, not undoable), keeping 1000 entries or 64 MiB. `undo`/`redo` restore files only if every touched file is still as the entry left it (else `undo_conflict`), and are logged themselves. The log is memory only; Git is the history, read with `gix` (no writes, no network).
- **Settings.** A fixed schema in `config`: `default_unit` in `ryucad.json` (a logged, undoable file change guarded by the project revision), everything else in `preferences.json` under the OS config folder (`RYUCAD_CONFIG_DIR` overrides it); a file that can't be read is reported, used as defaults, and kept as `.bak` when next written.
- **Meshes and renders.** Display meshes come from `display.quality` (chord a fraction of the body's diagonal), cached by body hash and quality, missed bodies meshed in parallel. `mesh.get` answers JSON, or with `transfer: binary`, `ryucad.mesh.1` bytes after the response line (`binary: {encoding, length}`); renders are always bytes. `render` is a CPU rasterizer in `ryucad::render`: twice the size averaged down, parallel bands, f32 pixel arithmetic, normal display quality and a fixed theme whatever the user's preferences, so its pixels are golden-tested on every platform.
- **Doctor.** `doctor` reads the folder directly (it works with no manifest; a folder with parts but no manifest is `project_damaged`) and reports 14 issue codes, each with planned repairs: exact file changes, an ID with a hash of the files involved (`repair_conflict` if they changed), and parameters (`unit`, `order`). Repairs restore from Git `HEAD` where they can, and never rename (that would change stable names).

## Desktop app (decided in ryucad phase 1)

The design is document 28 in the Coxswain planning project `ryucad-ryukei`; the plan is the phase 1.B plan there.

- **Process.** `ryucad-app` is eframe (egui on winit and wgpu), all Rust. It is a client of the project's daemon and nothing else: it depends on the `ryucad` library for `proto` and `client`, never touches project files (a damaged project is repaired by running `ryucad doctor`) and never calls ryukei. It starts daemons from the `ryucad` binary beside it, else on `PATH`, which must be the same build: the same ryucad, ryukei, commit and protocol (`VersionInfo::same_build`; the target differs on Linux, where the app is glibc and the CLI static musl). `RYUCAD_APP` names the app for `ryucad open`.
- **Connections.** Two per window, as one human participant named `<OS user> (window n)`: requests on a thread, notifications on a `client::Subscription`, which wakes its own reader when dropped so a closed window leaves the session. A lost daemon greys the window, which reconnects with backoff and loads again.
- **Model.** Parts and features from `rebuild` reports (which give each feature's `type`), meshes fetched as `ryucad.mesh.1` one body per request and kept by body hash, so an unchanged body is never fetched twice. Cameras move with `view::{orbit, pan, zoom}` and are shared at most every 150 ms; matrices are made relative to a reference point near the model so f32 GPU positions stay precise.
- **Drawing.** The scene renders with wgpu into a texture of its own (MSAA when the adapter can), shown as an egui image and read back in tests; each body uploads once per hash, and selections, highlights and hover paint over faces from a per-frame buffer at equal depth. Picking is a CPU ray cast on faces and screen distance for edges and vertices (vertex over edge over face, nothing hidden). Hardware adapters first, else a software one (lavapipe, WARP) with a note in the status line.
- **Writes.** The property panel saves through `feature.edit` against the revision `feature.show` read it at (`feature.show` now returns it); a conflict reloads the form and lists what changed. Settings go through `config.set`; preference writes hold a lock file, since every project's daemon shares `preferences.json`, and the daemon (not the app) puts a project first among the recent ones when a window says hello.
- **Tests.** App tests run against real daemons, building `ryucad` themselves; drawing tests skip without an adapter unless `RYUCAD_REQUIRE_GPU` is set (CI sets it on Linux, with lavapipe, and Windows). Window tests drive the real app with `egui_kittest`, whose accessibility check means every control needs a name. The viewport snapshot is lavapipe's, compared within a tolerance.
- **Packaging.** Release archives carry `ryucad` and `ryucad-app` (plus `share/` files on Linux and macOS); installers install both unless `RYUCAD_NO_APP=1`, and register "Open in ryucad" on folders (Windows, per user), a desktop entry and `ryucad.json` MIME type (Linux), and an AppleScript launcher compiled at install time into `~/Applications/ryucad.app` (macOS, unsigned).

## Open decisions

Don't assume an answer to these. Ask the user, or record the decision here once it's made:

- When the MCP adapter over `proto` arrives.
- Which parts are open source and which are paid.
- License. Apache-2.0 is the lean (design 22), and ported SMLib code (Apache-2.0) rules out GPLv2-only.
