# Bug burn-down notes

- 2026-10-27, lap 1: no issue carries a `burn` label (the label doesn't exist in
  Seiraiyu/ryucad), so the lap took open bug #123 as the burn-down target.
- #123: `feature.list` now reads the session's (cached) rebuild of the part, so a
  feature whose file loads but doesn't build is `ok: false` with its error, and
  features after it read `dependency_failed`. Cost: a cold `--no-daemon` list now
  rebuilds the part. The issue's `feature edit` warning and `assembly tree`
  notes were left for later.
- Lap 2: the `burn` label now exists. #151: `ryucad review` maps the service's
  `not_in_beta`, `invalid_request`, `merge_unavailable` (exit 1) and
  `github_unreachable` (exit 4) to `review_*` codes of their own. Codes were
  added beside the other review codes in `ryu-doc`'s enum (serialized by name,
  so order doesn't matter).
- Lap 3: #146 finished from lap 2's WIP: `heal::seamed` unwraps a loop's
  samples across a pole run (`across_poles`), so a cone lune through its apex
  reads as the lune, not a hole round the cone. All three corpus bodies seam;
  `tests/step.rs` now exports every part that builds.
- Lap 3 found that the loop's check fails on the first open burn issue
  (descending) with no `Fixes` commit. After #146 that is #133, a plan 8.F
  sized kernel performance issue.
- Lap 4: #133. The fuzz input's sweep folds: the profile sits beyond the
  arc leg's centre of curvature, so the arc carries it backwards against its
  normal while the line legs carry it forwards. `along` now judges the map
  carrying the profile's plane (`sweep/along/embedded.rs`) before building:
  certified one to one (boxes apart, or `N(r) · Φ_s` of one sign over a run)
  skips `check_intersections`; certified both signs at one lump's vertices
  is `PathTooCurved` at once; otherwise the old pairwise check. The input
  now refuses in ~5 ms (was 38 s on davids-m4, 150 s in the issue).
  Validated against the old path on 500 seeded `along_bytes` inputs: no
  body built that the old check found crossing; 14 of the first 200 now
  build where the old check was only `Uncertain` (extrusions it couldn't
  intersect), and those pass `check`, mesh cleanly and show only
  `CannotCheck`. Assumption: a Jacobian sign change within one lump means
  a fold, so refusing it is sound; a lump moving backwards throughout is
  not refused by this (left to the pairwise check or the builder).
