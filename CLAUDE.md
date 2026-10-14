# CLAUDE.md

This file provides guidance to Claude Code (claude.ai/code) when working with code in this repository.

## What this is

ryucad is CAD built for AI agents to drive, not primarily for humans clicking in a GUI. FreeCAD was rejected because it performs poorly on large assemblies, and its kernel, OCCT, is a main reason. This project builds its own kernel, **ryukei**, and the long-term target is Parasolid-level capability.

The project is in its early stages. The workspace is scaffolded but has no implementation yet.

## Commands

This is a Rust Cargo workspace (edition 2024).

```sh
cargo build --workspace
cargo test --workspace
cargo test -p ryukei <test_name>     # single test in one crate
cargo clippy --workspace --all-targets -- -D warnings
cargo fmt --all
cargo run -p ryucad
```

## Layout

- `crates/ryukei`: the B-rep geometry kernel (library).
- `crates/ryucad`: the application layer and binary.
- `docs/`: design documents.

The boundary between the two crates is deliberate. ryukei works on single bodies (geometry, topology, booleans, blends, tessellation) and must not depend on documents, assemblies, or agents. Large-assembly performance (instancing, lazy loading, caching, incremental recompute) and the agent interface live in ryucad. Keep both in this repo until the kernel API settles.

## Kernel principles

- **Parasolid-level is the goal.** Supporting analytic surfaces first (planes, cylinders, cones, spheres, tori, and fillets between them) only sets the order of work, not the scope. Never hard-code analytic-only assumptions into the core types. Analytic surfaces are fast special cases of a general surface/curve representation that NURBS will slot into.
- **Tolerant modeling from day one.** Edges and vertices carry their own tolerances.
- **Data-oriented topology.** Store topology in flat arrays referenced by IDs, not reference-counted pointer graphs like OCCT's. This makes models cheap to copy, cache-friendly, and shareable across instances.
- **Parallel from the start.** Design operations so independent work (tessellation, separate bodies, recomputing independent parts) can run concurrently.
- **Stable names for faces and edges.** Agents need references to faces and edges that survive a rebuild. FreeCAD's "toponaming" problem is the failure to avoid.
- **Deliberate numerical precision.** Decide explicitly which calculations must be exact and which can use floating point with tolerances.
- **Robustness through testing at scale.** Use a corpus of real models, randomly generated and mutated operations, checks that the topology stays valid after every operation, comparison of results against OCCT, and benchmarks against OCCT's numbers. OCCT may serve as a reference and benchmark baseline, and as a temporary fallback for operations ryukei can't do yet, but never as the long-term kernel.

## Open decisions

Don't assume an answer to these. Ask the user, or record the decision here once it's made:

- Concrete designs for the topology storage, the surface/curve representation, the precision strategy, and stable naming (planned as a design pass, written to `docs/`).
- How agents talk to ryucad (CLI, library API, RPC, MCP, etc.) and how models are persisted.
- License.
