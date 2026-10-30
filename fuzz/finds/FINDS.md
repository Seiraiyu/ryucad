# ryukei 8.F fuzz campaign: finds

Campaign: origin/main a8eb9edd (8.E merged), in the devcontainer image,
on cox-worker-2's box (8 cores, 16 GB, shared with other sessions; Linux
6.17.2-1-pve), 2026-10-30. `cargo +nightly fuzz run` in release with
cargo-fuzz's defaults (AddressSanitizer and debug assertions on, `-Ccodegen-units=1`), libFuzzer
fork mode (`-fork=1 -ignore_crashes=1 -ignore_timeouts=1 -ignore_ooms=1`),
`-max_total_time=10800 -rss_limit_mb=6144`; blend, boolean, offset and sweep
also take `-max_len=128 -len_control=0`, and sweep and offset `-timeout=600`
(the others keep libFuzzer's 1,200 s). Two targets ran side by side.

Each find's folder is `fuzz/finds/<target>/`. Replay one with
`cargo +nightly fuzz run <target> fuzz/finds/<target>/<file>`.
"Found after" is the time from the target's start to the artifact being
written.

Stacks were taken under gdb at libFuzzer's first alarm, about 120 s into a
run of the input alone (`-timeout=240`). They name the ryukei frames of the
rayon workers doing the work; the main thread waits on them.

## Runs

| target | ran | execs | corpus | cov | crash / timeout / OOM |
|---|---|---|---|---|---|
| general | 3 h | 4,107 | 16 | 4,246 | 0 / 0 / 0 |
| offset | 3 h | 193 | 1 | 22,686 | 0 / 0 / 0 |

## Finds

### blend: timeout in `intersect::fit::pcurve_bound` (fillet rail)

- File: `blend/timeout-3efbe4f1c3d0bbf5a7e769eff2d8e76918c69814` (107 B)
- Minimized: `blend/minimized-ffff0a` (3 B, hex `ffff0a`). This is tmin's result
  against a 120 s timeout, made while the box was at load average ~48, so
  it is only "slower than 120 s under load". It still has to be checked
  alone, and whether it also ends in the mesh fault.
- Kind: timeout (over 1,200 s), and then a crash. Found after 92 min.
- Run alone with no timeout (two fuzzers beside it), the blend finished
  after 2,029 s, passed `check` and `check_intersections`, and then failed the
  target's mesh assertion (blend.rs:45): `assertion left == right failed,
  left: [FaceFault { face: FaceId(2), kind: RefinementStalled }], right: []`.
  So `tessellate` stalls refining face 2 of the result at chord 1e-2, angle 0.5.
  The input's first byte is odd, so this is the target's `spline_case`.
- Hex: `4f4e4f4f4f4f4f4f4f4f4f4f4f4f4f4ffc4f4f4f4f204f4f4f4f4f4f4f4f4f4f4f4f4f4f4f4f4f4f4f4f4f4f4f4f0000004f4f4f4f4f4f4f4f4f4f4f4f4f4f4f4f4f4f4f4f4f4f4f4f4f4f4f4f4f4f4f4f4f4f4f4f4f4f4f0000004f584f4f200000004f4f4f4f4f4f4f0a`
- Stack (two workers):
  - `intersect::fit::pcurve_bound::go` (fit.rs:355, recursed 11 deep) → `second` (fit.rs:282)
  - `pcurve_bound` (fit.rs:357)
  - `fit_along`'s closures (fit.rs:1215, 1229)
  - `blend::rail::rail` (rail.rs:76)
  - `blend::plan::fillet_partial` (plan.rs:579)
  - `plan::partials` (plan.rs:204)
  - `blend::once` (mod.rs:208)
  - The other worker is at `go` (fit.rs:323) → `geom::nurbs::bound::derivative_bounds` (bound.rs:678) → `patch_piece` → `patch_jets` → `part` → `sub_curve` / `split` (bound.rs:118, 126).

### boolean: timeout in `intersect::ss_trace` `clusters`

- File: `boolean/timeout-62012e0e3b836318f71aaacd91893e13589b15cc` (109 B, not minimized yet)
- Kind: timeout (over 1,200 s). Found after 96 min.
- Run alone it was still running at a 3,600 s cap (exit 70, timeout),
  with the box heavily loaded.
- Hex: `01000000000000090a00000000ffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffff0000000000000000000000000000000000000500000000`
- Stack (two workers):
  - `intersect::ss_trace` `clusters` (ss_trace.rs:655/657: the all-pairs loop over singular leaves, quadratic in their count)
  - `link` (ss_trace.rs:558)
  - `run` (284)
  - `attempt` (145)
  - `trace` (104)
  - `intersect::ss::meet` (ss.rs:133)
  - `face::face_face_within` (face.rs:140)
  - `face_face` (101)
  - `boolean::table` closure (table.rs:259)
  - The main thread waits in `boolean::table::build` (table.rs:263).

### sweep: timeout in a loft's self-intersection check (`intersect::general`)

- File: `sweep/timeout-1c7ea944cd423ea6d97ae931e51c0305782a55ed` (105 B, not minimized yet)
- Kind: timeout (over 600 s). Found after 78 min. The first ~60 min went on
  fork mode's first pass over the 717 seed inputs, with the box at load
  average 27 to 48.
- Hex: `0adfdfdfdfdfdfdfdfdfdfdfdfdfdfdfdf0000000000000008dfdfdfdfdfdfdfdfdfdfdfdfdfdfdfdfdfdfdfdfdfdfdfdfdfdfdfdfdfdfdfdfdfdfdfdfdfdfdfdfdfdfdfdfdfdfdfdfdfdfdfdfdfdfdfdfdfdfdfdfdfdfdfdfdfdfdfdfdfdfdfdfdfdfdfdfdfdfdf`
- Path: `sweep::loft` (loft.rs:98) → `self_check` (loft.rs:330) → `check::check_intersections` (check/intersect.rs:60), from `along_bytes::carried` (along_bytes.rs:165).
- Stack (workers):
  - One worker in `check::intersect` `pair` (175) → `apart_beside` (256) → `intersect::beside::face_beside` (beside.rs:140) → `run` (442) → `general::apart` (260) → `apart_within` (318) → `general.rs:158` → `sheet::jet_box` → `NurbsSurface::jet_box` (bound.rs:713) → `patch_jets` → `part` → `sub_curve` (bound.rs:115) → `interval` `sub`/`outward`.
  - Another in `intersect::general` `search` (general.rs:1482, recursed 9+ deep) → `enclose` (1412) → `jet_box` → `patch_jets` (bound.rs:513) → `split`/`lerp4`.

### sweep: timeout in a sweep-along's self-intersection check (`fit::pcurve_bound`)

- File: `sweep/timeout-a73041963ed854e33b70395d83be635a32f8982c` (78 B, not minimized yet)
- Kind: timeout (over 600 s). Found after 88 min (same conditions as above).
- Hex: `8c8c8c8c8c8c8c8c8c8c8c8c8c8c8c8c8c8c8c8c8c8caf8c8c8c8c8c8c8c8c8c8c8c8c8c8c8c8c8c8c8c8c8c8c8c8c8c8c8c8c8c8c8c8c8c8c8c8c8c8c8c8c8c8c8c8c8c8c8c8c8c8c8c8c8c0a0a`
- Path: `sweep::along` (along.rs:171) → `self_check` (along.rs:183) → `check::check_intersections` (check/intersect.rs:60), from `along_bytes::carried` (along_bytes.rs:126). So `along` couldn't judge this sweep one to one.
- Stack (workers):
  - One in `check::intersect` `pair` (159) → `face::face_face` (101) → `face_face_within` (140) → `ss::meet` (ss.rs:146) → `ss::fitted` (ss.rs:521) → `fit::fit_branch` (fit.rs:624) → `span_bound` (912) → `pcurve_bound` (357) → `go` (fit.rs:355, recursed 8+ deep). This is the same recursion as the blend find.
  - Another in `pair` (153) → `apart_beside` → `beside::face_beside` → `run` (455) → `outside` (beside.rs:166) → `inface::point_in_face` → `first_crossing` (inface.rs:255) → `bernstein::roots` → `isolate` → `monotone` → `refine` (bernstein.rs:349).
