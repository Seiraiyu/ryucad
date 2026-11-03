# ryukei 12.G Fill Implementation Plan

**Goal:** Make a sheet spanning a closed loop of three or more sides (body edges or free curves) with `fill::fill`: position always, tangency to a laminar edge's face on request, every deviation and angle certified and reported; with names, unit and integration tests, a golden section, a work bound, a property test against dense sampling, fuzzing, OCCT's `BRepFill_Filling` as reference, and the area's docs.

**Architecture:** Each side becomes its exact NURBS form, turned to run round the loop, its ends joined at each corner's midpoint. A boundary in one plane is one plane face bounded by its sides. Four sides are one Coons patch taken in homogeneous coordinates on the sides' own knots (a B-spline basis reproduces `u` and `v` through its Greville abscissae, so the bilinearly blended Coons formula is a control-net formula, exact on rational sides). Three, or five or more, sides are split at a centre into one four-sided patch per corner. Tangency is a weightless correction added to the patch: each targeted side's cross-boundary derivative is projected into its target plane (a face's tangent plane, or a plane field both patches of an inner edge share), interpolated in the side's basis and blended in with Hermite's cubics; the angle across each targeted side is certified by interval enclosures of both normals (the face's through `blend::convex`'s existing enclosure), refining the knots where it can't be.

**Tech stack:** Rust (edition 2024): `ryukei` (`fill` (new), `geom::nurbs::skin`, `blend::convex`, `name`, `error`), `ryukei-testkit` (`sheet_bytes`), the `fuzz` crate (`sheet`), `ryukei-bench` (C++ shim over OCCT 7.8, `--features occt`, devcontainer only), `.github/workflows/full.yml`.

**Epic:** work item 4001 (ryukei 12.G). Design 295 (approved 2026-11-01); plans 12.A (document 300), 12.B (306), 12.C (311), 12.D (330, PR #404), 12.E (335, PR #423) and 12.F (345, PR #440) merged. This is the stage's last plan.

**Proven while planning.** The branch `scratch/ryukei-12g-tasks-2` (on main at `{{BASE}}`, tip `{{TIP}}`) holds one commit per task (Task 9 is a check, with no commit); each task below quotes its commit's diff in full. Cherry-pick them in order (`git cherry-pick {{BASE_SHORT}}..{{TIP_SHORT}}`), running each task's steps once its commit is in. (`scratch/ryukei-12g-proof` is the working branch the task commits were cut from, and `scratch/ryukei-12g-tasks` an earlier cut with a weaker tangent test; ignore both.) On cox-worker-1 (Intel Xeon E5-2650, 8 threads, 16 GB, shared; `CARGO_BUILD_JOBS=4`, one test binary at a time) every task was built and run as its steps say, Task 8's OCCT test in the `ryucad-dev:local` image there, and the outputs are quoted.

## Decisions this plan makes

Design 295's interview options are taken as it records them. Where its words don't fit the geometry or the code, this plan takes the option recommended in the questions on this section (each is a question below):

1. **Three sides split at a centre, not a pole.** The design makes three sides "a four-sided patch with one side collapsed to a corner point". Built that way, `check::check` on the pole patch did not finish in 40 s of a debug build (projection onto a patch with a collapsed row is the documented weak spot of `project`), and the tangency certificate can't enclose a normal over a stretch reaching a pole (`S_u × S_v` vanishes there). Three sides are split at a centre into three patches, as five or more are: no pole, and one code path.
2. **A boundary in one plane is one plane face.** Spline patches all lying on one plane (a flat pentagon) gave `CannotCheck` between neighbours in `check_intersections` (flat splines meeting at edges and at the centre) and 29 s of debug time on a pentagon. A boundary whose sides' control points lie within half of resolution of one plane is filled with one plane face bounded by the sides themselves, as `sweep::planar` would make it, provided every tangent side's face is certified within the angle of the plane.
3. **No `DeviationTooLarge`.** The design lists it, but the fill's laminar edges are the sides exactly (Coons boundaries are exact, rational ones included), so the only deviation is joining the sides' ends at each corner's midpoint, at most half the corner's gap: never more than the tolerance `NotClosed` already enforces. Each side's deviation is reported (`FillReport::deviations`), certified (an end control point's move bounds the curve's), and nothing refuses on it.
4. **Inner edges are certified, half the angle a side.** Patches meet tangentially across their inner edges within `FillOptions::angle`: each patch is certified within half of it to the shared plane field, so the two add to at most the angle (the report's `inner` is that sum).
5. **The plane field holds its curve.** An inner edge's field normal is interpolated from the plane at the side's middle to the centre plane and then made square to the inner curve's tangent, so the plane holds the curve; otherwise two patches whose cross derivatives both lie in the plane still meet at an angle (seen: 0.02 rad on a saddle pentagon, twice the limit, before this).
6. **Twists.** Where two targeted sides meet, the twist is the mean of theirs, as the design says ("Gregory's correction isn't NURBS"); two more terms move each side's resulting residual onto blends local to the corner's end spans (`coons::local_hermite`), so refining the knots shrinks it and the certificate converges. With the cubic blends alone, the residual spread along the whole side and a bent triangle's inner edges stayed 0.005 rad off at every refinement.
7. **What a side may be.** A side is an edge of a body by name (`tangent` only for a laminar edge, else `NotLaminar`), or a curve over a range; either end may come first (the loop's order is found from side 0 and 1's nearest ends). A loop of fewer than three sides (a closed edge alone) is `TooFewSides`: split it first.
8. **OCCT is the one reference.** `BRepFill_Filling` gives a plate surface within its own G0 and G1 errors, not an exact one, so only the flat pentagon's area (a plane either way) is judged, to a millionth; every other case prints both kernels' areas, ryukei's certified deviations and angles beside OCCT's errors, and the times. SMLib's shim has no filling.
9. **Fuzzing extends the `sheet` target** with kind 10 (a sheet of the first five kinds, one of its laminar edges closed into a loop by two or three lines through seeded points, filled, the edge tangent or not), as 12.D to 12.F did; the call runs under `FUZZ_BUDGET` (`fill_with`). No new target, no `fuzz.yml` change.
10. **CI.** `full.yml` runs only at release (the owner's rule, 2026-11-01): this plan adds `fill_props` and `ryukei-bench`'s `fill` to it but never dispatches it. The gate is `ci.yml` on the pull request plus the named runs below.

## Constraints for whoever executes this

- Build where the stage's coxswain places you. Each task runs only its own tests; the gate is at the end. Release runs and the OCCT task run in the devcontainer (`ryucad-dev:local`).
- No golden hash but the new `fill` line may change. If `UPDATE_GOLDEN=1` rewrites any other line, stop: a loft's bits moved through `skin::compatible`, which is a bug (`compatible` is `compatible_mapped` with a floor of 1, bit for bit, which Task 1's test holds). Regenerate on main as merged at the time (12.E's lesson). The determinism test is one test per section since #444: run `section::fill` and `section::listed` only.
- Run every integration test a change reaches, not only the lib (12.D's lesson): Task 1 reaches `--test loft` and the `lofts` section; Task 2 every crate that matches `ryukei::Error`; Task 3 `--test sheets` and `--test blends` (convexity's `Side` is now `pub(crate)`).
- A new `Meter::free()` needs a `// unmetered:` comment; this plan adds none (every charge in `fill` is on the call's meter).
- `cd fuzz && cargo +nightly check --bins` after Task 7.
- Build test projects only in `mktemp -d` folders. Never force-push. At the end remove `/tmp` scratch, target dirs and the Docker volume you made.

| Task | Work item | Commit on `scratch/ryukei-12g-tasks-2` | Description |
|------|-----------|------|-------------|
{{TABLE}}

{{TASKS}}

## The gate

Before the pull request: Tasks 1 to 10 done; `ci.yml` green on the pull request; in the devcontainer, Task 6's release run and Task 8's OCCT test pass, recorded in the work items. `full.yml` is not dispatched (the owner's rule: release only). Then remove the `/tmp` scratch, target directories and Docker volumes the run made.
