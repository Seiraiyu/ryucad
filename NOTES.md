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
- Lap 6: #128. The general trace's simple test now also tries, near a
  spline row collapsed exactly (all control points the same bits), the
  normal divided by the distance to the row: `S_uv × S_v` over the cell
  stretched to the row. Fitting needed two more: a foot at the pole uses
  `Surface::normal`'s limit (the partials' cross product is zero there),
  and `local_foot` returns `None` when 20 steps don't settle (its doc
  already said so) so projection decides. The pinned pair now meets in
  ~1 s (was `Uncertain` after ~7 s); `general_props` no longer cuts
  spheres short of their poles, and 240 random spline pairs passed.
  Determinism golden hashes unchanged. Rows collapsed only within
  resolution (not bit for bit) are still the old path.
- Lap 6: #125. A daemon's `doctor` now rebuilds through its own session
  (`doctor::Builds::live`), which `answer_live` has refreshed, so the
  assembly check reuses the cached rebuild instead of a cold
  `Session::open` per call. Without a daemon, one session is opened on
  first need and shared by every check and repair plan (it was opened
  twice for a stale assembly). `doctor.repair` still plans cold, since
  each repair changes the files. `check` still diagnoses cold on purpose.
  The app already coalesces doctor requests while one is waiting
  (`doctor_again`), so no debounce was added; doctor still runs on the
  request path, now at the cost of the checks alone. Not timed on the
  issue's gearbox (not in the repo).
- Lap 7: #122. Two mesher stalls, both reproducers in the issue now pinned
  by value (`boolean_props::necks_and_poles_mesh`, under a second in
  release; the macOS case took ~15 min before). (1) Cone and cylinder:
  the intersect is a lens whose two faces both draw a chord across a
  neck ~0.6 mm wide; the forced split of a shared chord hit the
  refinement floor (`waive / 16`) and stalled. A shared chord is now
  split whatever its length (its midpoint is free, so the split is
  finite). (2) Torus and sphere: the sphere face is bounded by one closed
  edge whose one vertex lies near the pole, and the face's metric was the
  speed at that vertex (`a = 2^-15`), so near the equator 0.2 m needles
  looked tiny in the scaled plane and never met `duu / a²`. Where the
  vertex box is flat in a direction, `FaceGeom::new` now takes points at
  1/4, 1/2 and 3/4 along each edge into it. Golden mesh hashes moved
  (`booleans`, `lofts`, `mirror`, `sweeps` in ryukei's determinism
  file); ryucad and ryustep goldens unchanged. The third reproducer in
  the issue (plan 8.C's spline revolution and torus, an hour per run) is
  on 8.C's branch, not this one, and wasn't tried.
- Lap 7: #117. Two tori's unite leaves a torus face with one hole; the
  seam `uv::gap` put lands exactly on one of its edge's samples (snapped
  onto it). Unwrapped a period round, that sample sat an ulp past the
  computed seam line, so `seam_crossing` asked for a crossing and the
  bisection added a sample within an ulp of it: one point, two anchors
  (`Classes::mark`, `BadDomain`). An end already exactly on the seam now
  crosses nothing in that direction. Pinned by value
  (`a_seam_through_a_sample_meshes`, full identities) and by seed.
  Golden hashes unchanged.
- Lap 8: #91. Not reproduced (no access to the issue's tray part, and a
  79-face tray built here meshes coarse/normal/fine in 0.06/0.09/0.14 s
  on davids-mac-mini). What blocked: every daemon request runs under one
  core lock, and after each change the app sends doctor and a mesh.get
  per level of detail. v0.5.0's doctor rebuilt a part whose sketch reads
  other features cold on every call (fixed by #125 on this branch: it now
  reuses the session). Meshing (minutes where the mesher stalls, as #122
  did) now runs outside the lock: `Serve::prepare` finds the meshes the
  cache lacks under it, the loop makes them with it released and puts
  them back, and `handle` finds them cached. Assumption: those two were
  the issue's two minutes; if the owner still sees it, the next suspect
  is whatever else the app asks after `model.changed` (history.log, a
  compare's diff --model, release.check), all still under the lock.
- Lap 8: #83 was already fixed on main by plan 8.A (`d5c6d3fb`, PR #124:
  "a plane–cone section reaches the region's edge near its asymptotes"),
  which committed the seed but wrote "(#83)", not "Fixes", so the issue
  stayed open. Bisected by value: the issue's shrunk pair fails at
  `462ebcb5` (the same point as the issue) and passes from `d5c6d3fb`.
  Pinned by value (`a_plane_cone_section_misses_nothing`, exact frames),
  since a proptest seed only replays while the strategy is unchanged.
- Lap 9: #77. Profiled the flush case (macOS `sample`, davids-m4's Apple
  M4, release, one thread): 80% was `classify::coincident`. The table
  pairs the target's top face with each of the n tool caps, so each of
  the ~n pieces of the top face ran `point_in_face` on all n caps: n²
  ray casts. Each coincident face is now boxed once (grown by twice its
  boundary's tolerance), and a partner whose box doesn't hold the
  piece's point is skipped (it could only be `Outside`). 1,000 holes:
  ryukei 0.53 s flush against 0.50 s out of both faces (was 5.0 s
  against 0.55 s); ryucad's bar 0.56 s. The bar's bound went from 12× to
  2×. Golden hashes unchanged; boolean tests and boolean_props pass.
