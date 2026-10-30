
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
