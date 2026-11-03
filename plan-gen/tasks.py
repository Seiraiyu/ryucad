TASKS = [
(1, 4773, "Compatible curves keep their parameter maps",
"""**Files:** modify `crates/ryukei/src/geom/nurbs/skin.rs` (`CompatMap`, `compatible_mapped`; `compatible` calls it with a floor of 1), `crates/ryukei/src/geom/nurbs/skin/tests.rs` (`compatible_maps_find_each_curves_own_parameter`).

Fill needs two things `compatible` (lofts') computes and throws away: a degree floor (Hermite blends need cubic bases) and how each curve's parameter was moved, so a tangent side's patch parameter can be traced back to its edge's in closed form. `compatible` maps the domain affinely onto `[0, 1]` and scales each Bézier piece's weights by `wᵢ ρⁱ` so its end weights are 1; that is the Möbius reparameterization `x = ρσ / (1 - σ + ρσ)` of the piece's local parameter. `CompatMap` records `[a, b]` and each piece's `[u0, u1]` and `ρ`; `to_curve(u)` inverts the chain.

**Step 1 -- failing test:** add `compatible_maps_find_each_curves_own_parameter` to `skin/tests.rs` as in the diff: a rational cubic and a quarter arc cut off its knots (so `ρ ≠ 1`) made compatible at degree 4; the rebuilt curve at `u` equals the original at `to_curve(u)` within 1e-12 at 33 points; and `compatible_mapped(_, 1)` equals `compatible` exactly.

**Step 2 -- see it fail:** `cargo test -p ryukei --lib skin::` fails to build: `compatible_mapped` doesn't exist.

**Step 3 -- implement:** apply the rest of the diff.

{{DIFF}}

**Step 4 -- see it pass:** `cargo test -p ryukei --lib skin::`: `5 passed` (0.01 s, debug, cox-worker-1). Lofts reach `compatible`: `cargo test -p ryukei --test loft` passes as on main, and Task 4's run of the golden sections leaves `lofts` unchanged.

**Step 5 -- commit:** `git commit -m "ryukei 12.G: compatible curves keep their parameter maps"`."""),
(2, 4774, "Fill errors and names",
"""**Files:** modify `crates/ryukei/src/error.rs` (`Error::InvalidFill`, `FillError` with `Display` and `From`), `crates/ryukei/src/lib.rs` (re-export `FillError`), `crates/ryukei/src/name/mod.rs` (`Role::Fill(FillRole)`, role code 17, its text, and the distinct-codes test).

`FillError`'s variants are design 295's, less `DeviationTooLarge` (decision 3), plus `BadOptions`, `NoSuchEdge` and `NotLaminar` for what a caller can get wrong; `Degenerate`, `AngleTooLarge` and `Uncertain` carry `Option<usize>`, `None` meaning the loop as a whole or the inner edges. `FillRole`'s seven roles are appended under a new role code, so no existing name moves.

**Step 1 -- the change:** apply the diff.

{{DIFF}}

**Step 2 -- see it pass:** `cargo test -p ryukei --lib name::`: `role_codes_are_distinct` passes with the seven fill roles. `cargo check --workspace --tests`: nothing else matches `ryukei::Error` without a wildcard, so no other crate changes (`ryukei-testkit`'s `sheet_bytes` lists the refusals it accepts and gains `InvalidFill` in Task 7).

**Step 3 -- commit:** `git commit -m "ryukei 12.G: fill errors and names"`."""),
(3, 4775, "ryukei::fill",
"""**Files:** create `crates/ryukei/src/fill/mod.rs` (the API, the driver, the one-patch plan), `fill/sides.rs` (reading, turning and joining the sides), `fill/planar.rs` (a boundary in one plane), `fill/coons.rs` (Coons nets, Hermite coefficients, interpolation, the correction), `fill/tangent.rs` (targets, fitting, certifying), `fill/split.rs` (the centre split), `fill/build.rs` (layout and names), `fill/tests.rs`; modify `crates/ryukei/src/lib.rs` (`pub mod fill`), `crates/ryukei/src/blend/convex.rs` (`Side`, `of`, `normal` and `normal_box` made `pub(crate)`: a tangent side's face normal is read exactly as a blend's convexity reads it).

How it works, module by module:

- `sides::read` checks the count, reads each side's exact NURBS form over its range (`Curve::to_nurbs`, then `part`), turns side 0 to end nearer side 1 and each later side to start nearer the end before it, refuses a gap over the tolerance (`NotClosed`), moves each side's end control points onto the corners' midpoints (its `deviation`), and refuses a side shorter than ten resolutions (its control polygon) or a corner turning less than `MIN_TURN` (0.01 rad) or more than a half turn less it (`Degenerate`). A tangent side keeps a `Trace`: the edge's one fin (`convex::Side`), its `Reparam`, and the flip where it was turned.
- `planar::plane_of` finds the plane (decision 2); `planar::angles` certifies each tangent face against it over 64 stretches; `planar::sheet` builds one face.
- `coons::Quad::new` makes the sides in tensor order compatible pairwise (`v0`/`v1` along `u`, `u0`/`u1` along `v`), `bilinear` gives the homogeneous net (the sides' own points on its border), `surface` adds a correction and refuses a weight at or below zero; `greville`, `cubic_coefficients` (a cubic's blossom), `interpolate` (collocation at the Greville abscissae, banded, no pivoting), `local_hermite` and `correction` build the tangency term.
- `tangent::patch` fits each targeted side's `g`, adds the correction, certifies every targeted side (`certify`: knot spans halved up to `MAX_DEPTH` (12); a face within the angle, a field within half), and refines the failing directions' knots up to `MAX_ROUNDS` (5).
- `split::split` makes the halves, the centre, the Newell plane, the inner cubics and their planes, and one patch per corner with its targets.
- `build::sheet` lays the patches out (each side's edge the patch's own parameter line, `NurbsSurface::iso`, with its `iso2` pcurve), builds through `layout::build_sheet`, and names everything (`Namer` records the recipes).

**Step 1 -- failing test:** create `fill/tests.rs` as in the diff, and add `#[cfg(test)] mod tests;` in `fill/mod.rs`.

**Step 2 -- see it fail:** before the rest of the diff, `cargo test -p ryukei --lib fill::` doesn't build: `ryukei::fill` doesn't exist.

**Step 3 -- implement:** apply the rest of the diff.

{{DIFF}}

**Step 4 -- see it pass:** on cox-worker-1, debug, `CARGO_BUILD_JOBS=4`:

- `cargo test -p ryukei --lib fill::`: `test result: ok. 9 passed; 0 failed` ({{LIB_SECS}} s; the saddle pentagon's self-intersection check is most of it).
- Every integration test the change reaches: `--test sheets` and `--test blends` pass as on main (`convex` only changed visibility).

**Step 5 -- commit:** `git commit -m "ryukei 12.G: fill"`."""),
(4, 4776, "The fill golden section",
"""**Files:** modify `crates/ryukei/tests/determinism.rs` (`fill_section`, and `fill => fill_section` in `sections!` after `sheet_blends`), `crates/ryukei/tests/golden/determinism.txt` (one new line).

A flat square (one plane face), a quad of two quarter arcs and two lines (one Coons patch on rational sides), a quad tangent to a quarter cylinder sheet's side (the correction and the certificate), and a bent triangle (three patches at a centre), each hashed with its report's deviations and angles. 13 s in a debug build (#444's lesson: the section is cheap).

**Step 1 -- the section:** add it as in the diff.

**Step 2 -- see it fail:** `cargo test -p ryukei --test determinism -- section::fill section::listed` fails: the golden file has no `fill` line.

**Step 3 -- record it:** `UPDATE_GOLDEN=1 cargo test -p ryukei --test determinism -- section::fill`, then `git diff crates/ryukei/tests/golden/` shows exactly one added line, `fill <hash>`, and nothing else. Regenerate on main as merged when you execute.

{{DIFF}}

**Step 4 -- see it pass:** `cargo test -p ryukei --test determinism -- section::fill section::listed`: `2 passed` (13.1 s, debug, cox-worker-1). The recorded line is `+fill bf6c778278568ac8`, and nothing else moved: while planning, every section was also run once before #444 merged (1,547 s, `UPDATE_GOLDEN=1`, an earlier form of the tangent case) and only the `fill` line was added.

**Step 5 -- commit:** `git commit -m "ryukei 12.G: the fill golden section"`."""),
(5, 4777, "Filling is bounded",
"""**Files:** modify `crates/ryukei/tests/work.rs` (`filling_is_bounded`).

The tangent quad on a quarter cylinder sheet (its far side bulging off the tangent plane) through `fill_with`: it charges (fitting at each Greville abscissa, each stretch certified), spends the same on one thread and eight, passes on a budget of what it spends, is `BudgetExceeded` alike one unit short, and `Cancelled` on a cancel.

{{DIFF}}

**Step 1 -- see it pass:** `cargo test -p ryukei --test work filling_is_bounded -- --nocapture`: `fill: 5 units`, `1 passed` (0.18 s, debug, cox-worker-1). The whole file's rule that every `Meter::free()` says why still holds: `fill` adds none.

**Step 2 -- commit:** `git commit -m "ryukei 12.G: filling is bounded"`."""),
(6, 4778, "fill_props",
"""**Files:** create `crates/ryukei/tests/fill_props.rs`; modify `.github/workflows/full.yml` (its own step after `sheet_blend_props`).

Design 295's `fill_props`: 48 seeded free boundaries of three to eight sides (lines, quadratic and cubic Béziers, rational quadratics, every third seed's odd sides backwards), each fill sound (checks, crosses nothing, meshes), every side within its reported deviation of the fill's laminar edges at 17 points, and across every inner edge the faces' normals within the reported inner angle at 17 points; and 16 seeded quarter cylinder sheets with three sides leaving their corners in the tangent plane, the fill's normal within the side's reported angle of the cylinder's at 17 points. The tangent cases' far side bulges off the cylinder's tangent plane, so the bilinear patch's cross derivative leaves it and the correction does the work (with a straight far side the bilinear patch is already tangent). A typed refusal is counted; at least three in four must be judged.

{{DIFF}}

**Step 1 -- see it pass:** `cargo test -p ryukei --release --test fill_props -- --nocapture`: `48 judged, 0 refused, of 48` and `16 judged, 0 refused, of 16`, `2 passed` in 367 s (release, cox-worker-1, load 3.9 to 6.2). In a debug build both tests are ignored.

**Step 2 -- commit:** `git commit -m "ryukei 12.G: fill_props"`."""),
(7, 4779, "The sheet fuzz target fills",
"""**Files:** modify `crates/ryukei-testkit/src/sheet_bytes.rs` (kind 10, `filled`; kinds read `% 11`; `InvalidFill` and `BudgetExceeded` accepted; the seeded test over eleven kinds), `fuzz/fuzz_targets/sheet.rs` (its doc only).

A sheet of the first five kinds, one of its laminar edges (the last byte) closed into a loop by two lines through an apex or three through two offset points (seeded on a coarse grid of directions and reaches), filled under `FUZZ_BUDGET`, the edge tangent or not. Most tangent cases are `AngleTooLarge` (their corners leave the face's plane), which is a typed refusal.

{{DIFF}}

**Step 1 -- see it pass:** `cargo test -p ryukei-testkit --lib sheet_bytes`: `1 passed` ({{SEEDED}}); every kind made sheets, kind 10's fills among them, each checking clean and meshing. `cd fuzz && cargo +nightly check --bins` builds.

**Step 2 -- commit:** `git commit -m "ryukei 12.G: the sheet fuzz target fills"`. **Step 3 (a fuzz box, optional):** `cd fuzz && cargo +nightly fuzz run sheet -- -max_total_time=900 -rss_limit_mb=6144` with no crash; record the runs in the work item."""),
(8, 4780, "OCCT's BRepFill_Filling as reference (devcontainer)",
"""**Files:** modify `crates/ryukei-bench/src/occt/shim.cc` and `shim.h` (`filling`, `nearest_edge`), `crates/ryukei-bench/src/occt.rs` (`ffi::filling`, `occt::fill`, `Filled`), `crates/ryukei-bench/build.rs` (link `TKBool`), `crates/ryukei-bench/Cargo.toml` (the `[[test]]`, `required-features = ["occt"]`), `.github/workflows/full.yml` (the devcontainer job's line after `sheet_blends`); create `crates/ryukei-bench/tests/fill.rs`.

`filling` adds a G1 constraint on the edge of a STEP-read sheet nearest each tangent point (its face from the ancestor map), then each free curve as a C0 constraint, builds, and returns the face's area, `G0Error`, `G1Error` and the build's seconds; every `Standard_Failure` is caught in the shim.

{{DIFF}}

**Step 1 -- build and run (devcontainer):** `cargo test -p ryukei-bench --features occt --release --test fill -- --nocapture`. While planning, in `ryucad-dev:local` on cox-worker-1 (a Docker volume for `target/`), release:

{{OCCT}}

Record each case's numbers and both kernels' times, with the machine and load, in the work item.

**Step 2 -- commit:** `git commit -m "ryukei 12.G: OCCT's BRepFill_Filling as reference"`."""),
(9, 4781, "Clippy-clean and nothing downstream breaks",
"""No commit. `cargo fmt --all --check`; `cargo clippy -p ryukei -p ryukei-testkit -p ryukei-bench --all-targets -- -D warnings` (and `--features occt` for the bench in the devcontainer); `cargo check --workspace --tests` (nothing outside `ryukei`, the testkit, the bench and the fuzz crate changes); in the devcontainer in release, the suites `skin::compatible` reaches: `cargo test -p ryukei --release --test loft --test general_booleans --test fill_props`, each passing as on main. Record the runs in the work item.

{{CLIPPY}}"""),
(10, 4782, "docs/claude: fill",
"""**Files:** modify `docs/claude/kernel-sheets.md` (a new section, Fill).

{{DIFF}}

**Step 1 -- commit:** `git commit -m "ryukei 12.G: docs/claude: fill"`. After Tasks 6 and 8 run where you execute, add their recorded numbers to the section's Testing bullet, as 12.D to 12.F did."""),
]
