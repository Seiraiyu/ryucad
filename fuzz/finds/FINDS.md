# ryukei 8.F fuzz campaign: finds

Campaign: origin/main a8eb9edd (8.E merged), in the devcontainer image,
on cox-worker-2's box (8 cores, 16 GB, shared with other sessions; Linux
6.17.2-1-pve), 2026-10-30. `cargo +nightly fuzz run` in release with
cargo-fuzz's defaults (AddressSanitizer on, debug assertions off), libFuzzer
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

- File: `blend/timeout-3efbe4f1c3d0bbf5a7e769eff2d8e76918c69814` (107 B, not minimized yet)
- Kind: timeout (over 1,200 s). Found after 92 min.
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
