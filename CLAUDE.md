# CLAUDE.md

This file provides guidance to Claude Code (claude.ai/code) when working with code in this repository.

## What this is

ryucad is CAD built for AI agents to drive, not primarily for humans clicking in a GUI. FreeCAD was rejected because it performs poorly on large assemblies, and its kernel, OCCT, is a main reason. This project builds its own kernel, **ryukei**, and the long-term target is Parasolid-level capability.

The project is in its early stages. The workspace is scaffolded but has no implementation yet.

## Commands

This is a Rust Cargo workspace (edition 2024). `rust-toolchain.toml` pins the toolchain, and both native builds and the devcontainer use it.

```sh
cargo build --workspace
cargo test --workspace
cargo test -p ryukei <test_name>     # single test in one crate
cargo clippy --workspace --all-targets -- -D warnings
cargo fmt --all
cargo run -p ryucad
```

Development happens on a headless Linux VM using the devcontainer, driven by the `devcontainer` CLI rather than VS Code:

```sh
devcontainer up --workspace-folder .
devcontainer exec --workspace-folder . cargo test --workspace
```

The product targets Windows, macOS, and Linux natively, not just WSL. CI (`.github/workflows/ci.yml`) runs fmt, clippy, and tests on Linux x64, Linux ARM, macOS ARM, and Windows, and also checks that the devcontainer builds. The devcontainer is always Linux, so the CI matrix is the only proof that native Mac and Windows builds work.

## Layout

- `crates/ryukei`: the B-rep geometry kernel (library).
- `crates/ryucad`: the application layer and binary.
- `docs/`: design documents. `docs/roadmap.md` holds the phased ryukei plan.

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
- **Keep OCCT out of the default build.** Put it behind a Cargo feature or in a separate bench crate, so the default build has no C++ dependency on any platform.
- **Robustness through testing at scale.** Use a corpus of real models, randomly generated and mutated operations, checks that the topology stays valid after every operation, comparison of results against OCCT, and benchmarks against OCCT's numbers. OCCT may serve as a reference and benchmark baseline, and as a temporary fallback for operations ryukei can't do yet, but never as the long-term kernel.

## Open decisions

Don't assume an answer to these. Ask the user, or record the decision here once it's made:

- Concrete designs for the topology storage, the surface/curve representation, the precision strategy, and stable naming (planned as a design pass, written to `docs/`).
- How agents talk to ryucad (CLI, library API, RPC, MCP, etc.) and how models are persisted.
- License.
