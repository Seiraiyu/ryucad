# ryucad

CAD built for AI agents to drive, with a from-scratch B-rep kernel (**ryukei**) designed for large-assembly performance. The long-term target is Parasolid-level capability.

> **"Make a flanged hub: a 60 mm flange 6 mm thick, a 28 mm hub 30 mm tall with a 12 mm bore, and four ribs between them."**

<p align="center">
  <img src="assets/hero.gif" alt="An agent builds a flanged hub in ryucad: the profile revolves round in steps, then four ribs are added" width="480">
</p>

The agent answers with `ryucad` commands. Each command prints JSON, writes plain feature files to the project's Git repo, and rebuilds deterministically. Humans can watch the same session live in `ryucad-app`. Every frame above was drawn by `ryucad render`.

<details>
<summary>What the agent ran</summary>

```sh
ryucad init --unit mm
ryucad part create hub
ryucad feature add hub sketch --id profile --plane zx
ryucad sketch edit hub profile @profile.json        # points, lines and a construction axis
ryucad feature add hub revolve --id body --sketch profile --axis l7 --angle full
ryucad feature add hub sketch --id ribs_a --plane zx
ryucad sketch edit hub ribs_a @ribs_a.json
ryucad feature add hub extrude --id ribs_x --sketch ribs_a --length 4 --extent symmetric
ryucad feature add hub sketch --id ribs_b --plane yz
ryucad sketch edit hub ribs_b @ribs_b.json
ryucad feature add hub extrude --id ribs_y --sketch ribs_b --length 4 --extent symmetric
ryucad render --out hub.png
```

`scripts/hero.sh target/release/ryucad` rebuilds the model and the animation (it needs Python with Pillow).

</details>

## Why

- **Agents model, humans watch.** Every operation is a CLI command or a JSON-RPC request with structured errors and stable codes. The desktop app is a viewer for people to watch, point and judge, and its every action is also a command.
- **A model is a folder of text files in Git.** Features are written as canonical JSON, one file each, so history, branches and review all come from Git. Bodies, meshes and renders are only a cache.
- **References survive edits.** Faces, edges and vertices have stable names derived from the operations that made them, so a reference never silently lands on the wrong edge after a rebuild. This is the "toponaming" problem that FreeCAD suffers from.
- **A kernel built for scale and agreement.** ryukei stores topology in flat arrays and runs in parallel from the start. It makes topology decisions with exact or certified predicates and produces bit-identical results on every OS and CPU, checked by golden hashes in CI.

## What's in the box

| Layer | Crate | What works today |
|---|---|---|
| Kernel | `crates/ryukei` | Box, cylinder, sphere, cone and torus; rational NURBS curves and surfaces; extrude and revolve from validated profiles; certified, watertight, parallel tessellation; certified curve and surface intersections and a self-intersection check; unite, intersect and subtract with any number of tools; pipes, constant-radius fillets and chamfers on edges and tangent chains |
| Sketch solver | `crates/ryusketch` | Points, lines, arcs and circles with geometric and dimensional constraints; degrees of freedom, and redundant and conflicting sets reported |
| Application | `crates/ryucad` | Projects of feature files; parameters and expressions with units; sketches, extrude and revolve; references to earlier features by stable name; incremental recompute; the agent CLI and per-project session daemon; activity log with undo; Git history; renders; `doctor` |
| Desktop app | `crates/ryucad-app` | A live viewer (egui and wgpu) attached to the daemon: feature tree, properties, history, picking that yields stable references, settings |
| Testing | `crates/ryukei-testkit`, `crates/ryukei-bench`, `fuzz/` | Seeded and property tests, fuzz targets, golden hashes, and head-to-head comparisons with OCCT and SMLib |

Booleans and fillets are in the kernel but not yet features in ryucad; that is ryucad phase 4. Design documents and roadmaps live in the Coxswain planning project `ryucad-ryukei`, not in this repo.

## Roadmap and releases

Three layers, each with its own phased roadmap: **ryukei** (the kernel), **ryucad** (the application) and **ryupdm** (product data management, built on ryucad projects in Git).

### Released

| Release | Date | What it brought |
|---|---|---|
| [v0.1.0](https://github.com/Seiraiyu/ryucad/releases/tag/v0.1.0) | 2026-10-16 | ryukei phases 0–3: deterministic math, tolerances and exact predicates; flat-array topology, Euler operators, stable names and the five primitives; NURBS geometry; certified tessellation. ryucad phase 0: projects of feature files, the agent CLI, the session daemon, installers |
| [v0.2.0](https://github.com/Seiraiyu/ryucad/releases/tag/v0.2.0) | 2026-10-18 | ryucad phase 1: the live session (participants, notifications, activity log and undo, settings, Git history, meshes, renders, `doctor`) and the desktop app, `ryucad-app` |
| [v0.2.1](https://github.com/Seiraiyu/ryucad/releases/tag/v0.2.1) | 2026-10-18 | ryukei phase 4: extrude and revolve from 2D profiles |
| [v0.2.2](https://github.com/Seiraiyu/ryucad/releases/tag/v0.2.2) | 2026-10-19 | ryukei phase 5.A: pcurves on every edge, and cavities |
| [v0.3.0](https://github.com/Seiraiyu/ryucad/releases/tag/v0.3.0) | 2026-10-20 | ryukei phase 5 complete (certified intersections, the self-intersection check, booleans) and 6.A (pipes). ryucad phases 2.A–2.C: parameters and expressions, the sketch solver, sketches to extrudes and revolves, references by stable name, incremental recompute. App looks (`ui.style`) |
| `main`, unreleased | | ryukei phase 6.B: constant-radius fillets and chamfers on edges and tangent chains |

### Next

| Layer | Now | Then |
|---|---|---|
| ryukei | Phase 6, blends: corners where several blends meet (6.C), then blends in booleans and the self-intersection check (6.D) | 7 STEP interop, 8 full NURBS (general intersections, NURBS booleans and blends, offset, shell, loft), 9 Parasolid-class (direct editing, sheets, draft, mass properties) |
| ryucad | Phase 2, parametric parts: sketches, labels and the parameter panel in the app, and solver stage 2 (2.D) | 3 Git-native workflow (CAD-aware diff and merge, `ryucad check`), 4 solid modeling (boolean, fillet, chamfer, hole, pattern and shell features), 5 assemblies, 6 STEP, 7 scale (10k+ part assemblies), 8 other formats and drawings |
| ryupdm | Phase 0, the data model, is decided (part identity, revisions, release records in the repo) | 1 local PDM, 2 hosted review, 3 where-used and BOM, 4 release workflow, 5 multi-repo, 6 teams and hosting |

Roadmaps are plans, not commitments; versions aren't assigned ahead of time.

## Building from source

The toolchain is pinned in `rust-toolchain.toml`; the default build has no C++ dependency.

```sh
cargo build --workspace
cargo test --workspace
cargo run -p ryucad -- --help
cargo build && cargo run -p ryucad-app -- <project folder>   # the app needs ryucad built beside it
```

## Install

Prebuilt binaries for Linux (x86_64 and aarch64), macOS (Apple silicon) and Windows (x64) are published with each release: the `ryucad` command (static on Linux) and the desktop app, `ryucad-app` (on Linux it needs glibc 2.35 or newer and a desktop). The installers put both in `~/.ryucad/bin`, add it to your PATH, and let you open a project folder in the app from your desktop: "Open in ryucad" on folders in Explorer, an application entry on Linux that also opens `ryucad.json`, and `~/Applications/ryucad.app` on macOS (drop a folder on it, or `open -a ryucad <folder>`). Running one again updates.

While the repository is private, the installers need a GitHub token: they use `GH_TOKEN` or `GITHUB_TOKEN`, or ask `gh` (`gh auth login` first).

```sh
# Linux, macOS and WSL
gh release download -R Seiraiyu/ryucad -p install.sh -O - | sh
```

```powershell
# Windows (PowerShell)
gh release download -R Seiraiyu/ryucad -p install.ps1 -O install.ps1; Get-Content -Raw install.ps1 | iex
```

Once the repository is public, no token is needed:

```sh
curl -fsSL https://github.com/Seiraiyu/ryucad/releases/latest/download/install.sh | sh
```

```powershell
irm https://github.com/Seiraiyu/ryucad/releases/latest/download/install.ps1 | iex
```

Set `RYUCAD_VERSION=v0.1.0` to install a particular version, `RYUCAD_HOME` to install somewhere other than `~/.ryucad`, `RYUCAD_NO_MODIFY_PATH=1` to leave your shell profile alone, and `RYUCAD_NO_APP=1` to install the command only (servers, CI). `ryucad open [folder]` opens a project in the app, or brings its window forward.

To uninstall, run `ryucad daemon stop --all`, delete `~/.ryucad` (`%USERPROFILE%\.ryucad` on Windows), and remove the PATH line the installer added (marked `# added by the ryucad installer`) or the entry in your Windows user PATH. Then remove what lets the desktop open projects: on Linux `~/.local/share/applications/ryucad.desktop` and `~/.local/share/mime/packages/ryucad.xml` (then `update-mime-database ~/.local/share/mime`), on macOS `~/Applications/ryucad.app`, and on Windows the keys `HKCU\Software\Classes\Directory\shell\ryucad` and `HKCU\Software\Classes\Directory\Background\shell\ryucad`.

## Releasing

Merge, then push a tag `vX.Y.Z` (or `vX.Y.Z-rc.1` for a prerelease). The version comes from the tag: the repo's workspace version stays `0.0.0`, and the workflow stamps the tag's version in before building. The `Release` workflow builds and tests every target, publishes the archives with `SHA256SUMS` and both installers, and then tests the installers against the new release.
