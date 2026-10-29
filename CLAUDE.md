# CLAUDE.md

This file provides guidance to Claude Code (claude.ai/code) when working with code in this repository.

## What this is

ryucad is CAD built for AI agents to drive, not primarily for humans clicking in a GUI. FreeCAD was rejected because it performs poorly on large assemblies, and its kernel, OCCT, is a main reason. This project builds its own kernel, **ryukei**, and the long-term target is Parasolid-level capability.


The project is in its early stages. What each roadmap has done and what comes next is in the Coxswain planning project `ryucad-ryukei` (`cox roadmap show`), never here.
The roadmaps are designs 14 (ryukei), 20 (ryucad) and 21 (ryupdm), and design 23 lines up all three.

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
UPDATE_GOLDEN=1 cargo test -p ryusketch --test determinism   # and for the sketch solver's
UPDATE_GOLDEN=1 cargo test -p ryumate --test determinism   # and for the mate solver's
UPDATE_GOLDEN=1 cargo test -p ryustep --test determinism   # and for STEP's bodies and bytes
ryucad cache stats                                          # what the disk cache holds (RYUCAD_CACHE_DIR moves it)
cargo bench -p ryukei-bench
cargo bench -p ryusketch
cargo bench -p ryumate
cargo bench -p ryucad
cargo bench -p ryucad-app
cargo bench -p ryueda
cargo bench -p ryu-board
cargo bench -p ryudfm
cargo run -p ryucad-app -- <project folder>                 # the desktop app (cargo build first: it needs ryucad beside it)
RYUCAD_REQUIRE_GPU=1 cargo test -p ryucad-app               # fail, don't skip, the drawing tests without an adapter
UPDATE_SNAPSHOTS=1 cargo test -p ryucad-app --test snapshot # redraw the viewport snapshot (lavapipe's)
cargo test -p ryukei-bench --features occt,smlib --release --test kernels -- --nocapture   # head-to-head with OCCT and SMLib; devcontainer only
cargo test -p ryukei-bench --features occt,smlib --release --test spike -- --nocapture     # OCCT and SMLib booleans, speed and repeatability; devcontainer only
cargo test -p ryukei-bench --features occt,smlib --release --test booleans -- --nocapture  # booleans head to head with OCCT and SMLib, and the 1000-hole race; devcontainer only
cargo test -p ryukei-bench --features occt,smlib --release --test blends -- --nocapture    # blends head to head with OCCT and SMLib, and the 1000-rim race; devcontainer only
cargo test -p ryukei --release --test boolean_props         # the boolean property tests (ignored in debug builds: minutes there)
cargo test -p ryukei --release --test booleans              # booleans at a millimetre and a centimetre (the scaled cases are ignored in debug builds)
cargo test -p ryukei --release --test pipe_props            # pipes against seeded random surfaces and lines (ignored in debug builds)
cargo test -p ryukei --release --test general_props         # intersections of splines and swept surfaces against sampling, and tangency along a curve (ignored in debug builds)
cargo test -p ryukei --release --test blends                # fillets, pipe ones included (the slow ones are ignored in debug builds)
cargo test -p ryukei --release --test chamfers              # chamfers, ruled ones included (likewise)
cargo test -p ryukei --release --test blend_props           # blends of seeded random primitives (ignored in debug builds)
cargo test -p ryukei --release --test corners               # corners: balls, trims, mitres, rolls, fans (the slow ones are ignored in debug builds)
cargo test -p ryukei --release --test corner_props          # corners of seeded random boxes and sweeps (ignored in debug builds)
cargo test -p ryukei --release --test blend_booleans        # booleans through fillets, pipes included (the pipe ones are ignored in debug builds)
cargo test -p ryukei --release --test heal_props            # seeded bodies written with seams and healed again, and given a file's faults (ignored in debug builds)
cargo test -p ryukei --release --test mirror                # mirror images of primitives, sweeps, booleans and blends (the pipe and seeded ones are ignored in debug builds)
cargo test -p ryukei --release --test along                 # sweeps along open and closed paths against Pappus (the spline and closed ones are ignored in debug builds)
cargo test -p ryukei --release --test loft                  # lofts: frustums, prisms, holes, tangents, a ring (the ring is ignored in debug builds)
cargo test -p ryustep --release --test write                # bodies written and read back, sweeps and lofts too (those ignored in debug builds)
cargo test -p ryustep --release --test round_trip_props     # seeded bodies through STEP and back (ignored in debug builds)
cargo test -p ryustep --release --test checks               # bodies healed from the OCCT files and the quick corpus files pass the self-intersection check (ignored in debug builds)
cargo test -p ryustep --release --test checks -- --ignored slow     # the three slow corpus files likewise (about half an hour)
cargo test -p ryustep --release --test checks -- --ignored ctc_05   # nist_ctc_05 checks but for its 25 pinned pairs (about twenty minutes; plan 8.F's)
sh scripts/fetch-corpus.sh                                    # the pinned STEP corpus into target/corpus, checked by SHA-256 (curl, unzip)
cargo test -p ryukei-bench --features occt --release --test step -- --nocapture   # STEP head to head with OCCT, the pinned corpus, ryucad's exports and the large-assembly race; devcontainer only
cargo test -p ryucad --release --test bar -- --nocapture    # a 1,000-hole pattern against ryukei's own boolean, one thread (ignored in debug builds)
RYUCAD_BARS=1 cargo test -p ryucad --release --test assembly_bar -- --nocapture   # design 72's bars: 1,000 mates held and jostled, a cold daemon over a warm cache (without RYUCAD_BARS, ten times looser)
RYUCAD_BARS=1 cargo test -p ryucad --release --test step_bar -- --nocapture   # design 120's bars: the race's 1,600 parts imported, rebuilt cold and warm, and exported (minutes; ten times looser without RYUCAD_BARS)
RYUCAD_BARS=1 cargo test -p ryucad-app --release --test frames -- --ignored --nocapture   # 1,001 occurrences at 60 frames a second; hardware adapters only
ryucad interference <asm> [--instance <path>]               # which instances of an assembly share a volume
ryucad export <part> --out <file> [--rev <rev>|label:<L>] [--schema ap214] [--unit m]   # a part or assembly as STEP
ryucad export <part> --out <file.stl|file.3mf> [--chord 0.01mm] [--angle 15deg] [--ascii]  # as STL or 3MF, written by the daemon
ryucad export <part> --out <file.usda> [--no-brep|--no-mesh] [--pipe-tolerance 0.0001mm]  # as USD: omniSolid BrepArrays and meshes, instanced
cargo test -p ryukei-bench --features occt,formats --release --test formats -- --nocapture   # STL read by OCCT, 3MF by lib3mf; devcontainer only
cargo test -p ryukei-bench --features usd --release --test usd -- --nocapture   # USD opened by OpenUSD, validated and imported into SMLib by usd-brep; devcontainer only
cd fuzz && cargo +nightly fuzz run predicates               # needs nightly + cargo-fuzz (targets also: interval, euler, nurbs, mesh, feature, sweep, intersect, general, boolean, blend, expr, sketch, mate, merge, pdm, step, heal, codec, eda, kicad, odb, shapes)
sh scripts/test-install.sh target/release/ryucad          # install.sh against a fake release
sh scripts/smoke.sh target/release/ryucad target/release/ryucad-app   # an agent session, then the app's offscreen frame
cargo run -p ryueda -- <command>                          # the ryueda CLI (`ryueda --help`); run in a ryueda project folder, or with -C <dir>
UPDATE_GOLDEN=1 cargo test -p ryueda --test determinism   # after a deliberate change to ryueda's netlists, ERC or imports
RYUEDA_BARS=1 cargo test -p ryueda --release --test bar -- --nocapture   # design 157's bars: 10,000 occurrences loaded, flattened and checked, and a warm daemon's write (ten times looser without RYUEDA_BARS)
sh scripts/fetch-kicad.sh                                 # KiCad 9.0.9's symbol, footprint and 3D model libraries into target/kicad, checked by SHA-256
RYUEDA_REQUIRE_KICAD=1 cargo test -p ryueda --release --test kicad_corpus -- --nocapture   # every item of those libraries imports or is a typed refusal
sh scripts/test-install-ryueda.sh target/release/ryueda   # install-ryueda.sh against a fake release
sh scripts/smoke-ryueda.sh target/release/ryueda          # an agent session: a KiCad library imported, a small design, its netlist, ERC and check
cargo run -p ryudfm -- <command>                          # the ryudfm CLI (`ryudfm --help`); run in a ryudfm project folder, or with -C <dir>, or on a job's path
UPDATE_GOLDEN=1 cargo test -p ryu-board --test determinism   # after a deliberate change to board shapes or renders
UPDATE_GOLDEN=1 cargo test -p ryudfm --test determinism   # after a deliberate change to what ryudfm reads or renders of the committed boards
UPDATE_MANIFEST=1 cargo test -p ryudfm --test corpus      # record the committed boards' counts after a change that reads more (review them)
sh scripts/fetch-dfm-corpus.sh                            # the pinned KiCad boards exported to ODB++ into target/dfm-corpus; devcontainer only (its KiCad)
cargo test -p ryudfm --release --test pinned -- --ignored   # the pinned boards to their counts and KiCad's own results, after the fetch
RYUDFM_BARS=1 cargo test -p ryudfm --release --test bar -- --nocapture   # design 213's bars: a dense board read and shaped, a warm daemon's inspect and render (ten times looser without RYUDFM_BARS)
sh scripts/test-install-ryudfm.sh target/release/ryudfm   # install-ryudfm.sh against a fake release
sh scripts/smoke-ryudfm.sh target/release/ryudfm          # an agent session: minimal-clean imported, inspected, rendered, checked, updated and undone
```

Releases are cut by pushing a tag `vX.Y.Z` (or `vX.Y.Z-pre`, published as a prerelease). The version comes from the tag, never from a file: the workspace version in the repo stays `0.0.0`, and `.github/workflows/release.yml` stamps the tag's version in (`scripts/stamp-version.sh`), builds, tests and publishes, then tests the installers against the new release. Pushing a tag publishes, so it follows the release roadmaps (`release` for this repo, `service-release` for ryupdm-service; design 116): each version is cut when its product phase is done, as a pre-release (`vX.Y.0-pre.N`, pushed by the stage's agent), e2e on Mac, Linux and Windows, a burn-down PR, then the release tag (`vX.Y.0`), which only the coxswain pushes, or the owner. ryueda has a version line of its own (design 150): `ryueda-vX.Y.Z` tags (or `ryueda-vX.Y.Z-pre.N`), which only `.github/workflows/ryueda-release.yml` takes (`crates/ryueda/tests/release.rs` checks that no release workflow takes another's), publish the `ryueda` binaries with `install-ryueda.sh` and `install-ryueda.ps1`, never as GitHub's latest release (that is ryucad's, which `install.sh` fetches), after the same `full.yml` gate; they follow the `ryueda-release` roadmap the same way, and nothing is tagged until its stage 1. ryudfm has one too (design 173), the same way: `ryudfm-vX.Y.Z` tags (or `ryudfm-vX.Y.Z-pre.N`), taken only by `.github/workflows/ryudfm-release.yml`, publishing the `ryudfm` binaries with `install-ryudfm.sh` and `install-ryudfm.ps1`, never as GitHub's latest release, following the `ryudfm-release` roadmap, nothing tagged until its stage 1. No other tags.

Development happens on a headless Linux VM using the devcontainer, driven by the `devcontainer` CLI rather than VS Code:

```sh
devcontainer up --workspace-folder .
devcontainer exec --workspace-folder . cargo test --workspace
```

The product targets Windows, macOS, and Linux natively, not just WSL. CI (`.github/workflows/ci.yml`) runs fmt, clippy, and tests on Linux x64, Linux ARM, macOS ARM, and Windows, and also checks that the devcontainer builds. The devcontainer is always Linux, so the CI matrix is the only proof that native Mac and Windows builds work.

## Layout

- `crates/ryu-math`: deterministic scalar math (library): `libm` and `sqrt`, the same bits everywhere. `ryukei` re-exports it as `ryukei::math`, which kernel code uses.
- `crates/ryukei`: the B-rep geometry kernel (library).
- `crates/ryusketch`: the 2D constraint solver for sketches (library), and `ryusketch::core`, the solver apart from sketches. It depends on `ryukei` only for `math`.
- `crates/ryumate`: the rigid-body mate solver for assemblies (library), over `ryusketch::core`. It depends on `ryukei` only for `math`, and never reads files, documents or sessions.
- `crates/ryu-doc`: what every ryu product's files are written in (library): canonical JSON, IDs and project IDs, sources, units, expressions, and the error type with every product's codes. It depends only on `ryu-math`, `serde` and `serde_json`.
- `crates/ryucad-doc`: ryucad's file format (`doc`, `units`, `expr`, `error`), split out so ryupdm can read projects; `ryucad` re-exports each under its old path. `units`, `expr`, `error` and `doc`'s `canonical`, `id` and `source` are `ryu-doc`'s, re-exported here.
- `crates/ryu-session`: what a ryu product's session daemon shares (library), generic over a `Product` marker (ryucad's is `ryucad::Cad`): JSON-RPC framing, endpoints, notifications, freshness (`fresh::Stamps`), participants (`live`), the activity log with undo, the preferences file (`prefs`; the keys stay the product's), and the daemon loop and its clients, over a `Serve` impl (the product's requests and session). ryucad names its types for itself (`ryucad::endpoint::Endpoint`), so its callers never see the parameter.
- `crates/ryupdm`: local product data management (library): the PDM settings, part properties, revision labels, release records, the release branch's history and tags, part states, the release checks, the commit guard and merge validation. It reads projects through `ryucad-doc` and Git through `gix`, and never calls the kernel, a session or the app.
- `crates/ryu-part21`: STEP's exchange structure alone (library): Part 21 parsed and written, with no kernel. `ryustep` re-exports it; `ryueda` checks 3D models with it.
- `crates/ryustep`: STEP files (library): Part 21 exchange structures, the AP203/AP214/AP242 entities, units and the product tree, and bodies read and written. `tests/data/occt` holds files OCCT wrote, with the `gen.cpp` that made them; `tests/corpus` holds public domain NIST test models, byte for byte, with `manifest.txt` and the pinned corpus's `pinned.txt`.
- `crates/ryuformats`: the formats ryucad writes besides STEP (library): STL and 3MF from ryukei's meshes, a deterministic zip writer, and USD (a layer model, usda, omniSolid BrepArrays from ryukei's bodies), pure Rust over `ryukei`, `miniz_oxide` and `rayon`. It never depends on ryucad or `ryustep`, and ryukei never depends on it.
- `crates/ryucad`: the application layer and binary.
- `crates/ryueda`: electronics CAD (library and binary): projects and library projects of canonical JSON, symbols, footprints, models and library components, connectivity, the flattened netlist, ERC, `check` and KiCad library import, behind a CLI and a daemon. It depends on `ryu-doc`, `ryu-session` and `ryu-part21`, never on the kernel, `ryustep` or `ryucad`. `tests/data/kicad` holds KiCad-format files written for its tests, and `tests/kicad/pinned.txt` the pinned KiCad libraries.
- `crates/ryu-board`: the board model every ryu product's boards are read into (library): integer nanometre geometry, exact decimals, placements, ODB++'s standard and user symbols, exact shapes of features and stroke text, layers, steps, components, nets, drills and the stackup with capabilities and notices, and the PNG and SVG renderer. It depends on `ryu-math`, `rayon` and `png` only; its `testing` feature holds seeded symbols and features and the golden hashes.
- `crates/ryudfm`: design for manufacturing of boards (library and binary): the ODB++ reader (`odb`: folders and archives into `ryu-board`'s model, faults and warnings), projects of canonical JSON holding boards as committed inputs, import and update, `inspect`, renders and `check`, behind a CLI and a daemon. It depends on `ryu-board`, `ryu-doc` and `ryu-session`, never on the kernel or another product. `tests/corpus` holds the committed ODB++ boards with `manifest.txt` and the pinned boards' `pinned.txt`.
- `crates/ryucad-app`: the desktop app, a client of the daemon (`core`: link, model, people, navigation, picking, forms; `ui`: the egui window and the wgpu scene).
- `packaging/`: what the installers register: the Linux desktop entry and project MIME type, and the macOS launcher's AppleScript.
- `crates/ryukei-testkit`: test-only helpers (exact references, seeded inputs, golden hashes, Euler operator sequences, sweeps, sweeps along paths and lofts, booleans, blends and shapes to heal from bytes, random NURBS). Never a runtime dependency.
- `crates/ryukei-bench`: divan benchmarks, and the comparisons behind its `occt` and `smlib` features: `cxx` shims over a system OCCT 7.8 and over `libsmlib`, the kernel of NVIDIA's USD BRep built from a pinned commit. The devcontainer installs both.
- `fuzz/`: cargo-fuzz targets, outside the workspace (nightly only).
- `install.sh`, `install.ps1`: the installers, also published with each release. `install-ryueda.sh`, `install-ryueda.ps1`: ryueda's, published with each of its releases, installing to `~/.ryueda/bin`. `install-ryudfm.sh`, `install-ryudfm.ps1`: ryudfm's likewise, installing to `~/.ryudfm/bin`. `scripts/` holds their tests and the release smoke tests.
- Design documents, roadmaps and plans live only in the Coxswain planning project `ryucad-ryukei` (`cox doc list`), never as markdown in the repo, so there is one copy of each.

ryupdm's local PDM (the `ryupdm` crate and its commands) lives in this repo; its hosted review service lives in the private repo `Seiraiyu/ryupdm-service` (see Hosted review below).

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

## Decisions by area

What each phase decided is recorded per area under `docs/claude/`, imported here:

- @docs/claude/kernel-numerics.md — units, tolerances, precision policy, determinism
- @docs/claude/kernel-topology.md — flat-array B-rep bodies, Euler operators, stable names, validity
- @docs/claude/kernel-geometry.md — curves, surfaces, NURBS, derivatives, projection
- @docs/claude/kernel-tessellation.md — certified, watertight, parallel meshing
- @docs/claude/kernel-sweeps.md — profiles, extrude and revolve
- @docs/claude/kernel-intersections.md — pcurves, cavities, analytic intersections, the self-intersection check
- @docs/claude/kernel-booleans.md — analytic booleans
- @docs/claude/kernel-blends.md — pipes, fillets, chamfers, corners, booleans on blends
- @docs/claude/step.md — ryustep, bodies both ways, healing, real-world files
- @docs/claude/kernel-general-intersections.md — intersections of every surface
- @docs/claude/kernel-paths-lofts.md — sweeps along paths and lofts
- @docs/claude/kernel-measure.md — measurement: properties, distances, angles, mass, with bounds
- @docs/claude/ryucad-core.md — projects, units, rebuild, the agent CLI, the daemon, releases
- @docs/claude/ryucad-session.md — participants, notifications, activity and undo, settings, meshes, renders, doctor
- @docs/claude/ryucad-app.md — the desktop app
- @docs/claude/ryucad-parameters.md — parameters and expressions
- @docs/claude/ryusketch.md — the sketch solver
- @docs/claude/ryucad-sketches.md — sketch features, sweeps, references, recompute, sketches in the app
- @docs/claude/ryucad-git.md — diff, check, merge, the compare view and conflict panel
- @docs/claude/ryucad-solids.md — solids, fillets, chamfers, holes, patterns
- @docs/claude/ryucad-assemblies.md — assemblies, mates, scale, interference
- @docs/claude/ryucad-step.md — STEP import, re-import and export
- @docs/claude/ryucad-scale.md — the benchmark project, file events, the memory budget, the window at scale
- @docs/claude/ryucad-formats.md — export formats: the scene, STL, 3MF, USD, and their references
- @docs/claude/ryupdm.md — the PDM data model and local PDM
- @docs/claude/ryupdm-review.md — hosted review
- @docs/claude/shared-crates.md — ryu-math, ryu-doc, ryu-session
- @docs/claude/ryudfm.md — ryudfm: the board model and shapes (`ryu-board`), the ODB++ reader and corpus, projects, the CLI, renders and releases
- @docs/claude/ryueda.md — ryueda projects, library projects, symbols, footprints, models, library components

When a plan records what it decided, it edits or adds only its area's file under `docs/claude/`. It doesn't touch CLAUDE.md, except to add one import line here for a new area, and it never writes project status anywhere in the repo.

## Persistence (decided 2026-10-16)

A ryucad model is a folder of canonical text feature files in a Git repo, one per feature or object, rebuilt deterministically by ryukei. Git provides history, branches and merges; ryucad supplies CAD-aware diff and merge drivers, and features reference topology by stable name. Derived data (bodies, meshes, renders) is a cache, never committed. ryupdm builds on the same repos rather than a second store. This follows the freecad-spike (github.com/Seiraiyu/freecad-spike) GO verdict, without FreeCAD.

## Open decisions

Don't assume an answer to these. Ask the user, or record the decision here once it's made:

- When the MCP adapter over `proto` arrives.
- Which parts are open source and which are paid.
- License. Apache-2.0 is the lean (design 22), and ported SMLib code (Apache-2.0) rules out GPLv2-only.
