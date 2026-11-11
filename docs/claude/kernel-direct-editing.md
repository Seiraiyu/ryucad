## The local re-trim (decided in ryukei phase 13)

The design is document 399 in the Coxswain planning project `ryucad-ryukei`; this is its plan 13.A (phase 1).

- **API.** `tweak::Topology { Keep, Change }` on `MoveSpec`, `OffsetFacesSpec`, `ReplaceSpec` and `DraftSpec`. `Keep` is phase 9's engine, bit for bit. `Change` runs it first, and its result stands wherever it builds; where it refuses `EdgeVanishes`, `VertexSplits`, `FaceCollapses`, `SelfIntersecting`, `Unsupported { LoneVertex }`, or `Uncertain` at one of the input's vertices (the fixed engine's three-face form of a vertex splitting, `tweak::vertices`: a block's wall drafted past 1.05 rad carried its closing line to 1.6 along itself and found the vertex at 1.604), the re-trim (`tweak::retrim`) builds the result, and its refusal is the answer. Every other refusal (`TangencyBroken`, any other `Uncertain`, a spec's faults, a stop) is the fixed engine's. New refusals: `GapNotHealed { face }`, `HealAmbiguous { face }`, `RegionTooSmall { face }` (`TweakError` stays `Copy`).
- **The region.** The changed faces and every face sharing a vertex with one, grown at pinches until the frontier (edges between it and the faces outside) is rings.
- **The working body.** The faces outside as they are, and each region face as a sheet face of its own (`sheet::from_surface`) on its surface over a parameter box covering the region box: the region's faces' box grown by how far a changed face's vertex moves (an offset's distance, a rigid motion's or a foot's displacement), periodic directions whole, NURBS, extrusions and revolutions carried past their domains (`carried_by`), pipes, sweeps, offsets and bent surfaces not. One sheet through `layout::build_sheet`, every face its original's name.
- **The table.** `repair::resolve` over the pairs of working faces whose boxes meet, and each working face with the outside faces it shares a frontier edge with (which imprints the edge). Outside faces are never cut: one a working face crosses comes back whole.
- **Seeds.** Every piece of an outside face. Then design 399's rule 3: each region face with no frontier edge is seeded by its one piece whose inner point, taken back to the old surface, lies inside the old face (none where it has no such piece, or several). A moved face's seed says it survives, which a face moved out of the body doesn't (a groove's end wall offset past the plate's side): so the walk runs with every rule-3 seed, then, where that closes nothing, with the unchanged faces' alone. A region with no outside face (it is the whole body) takes one seed instead: its least-named face's one such piece, as plan 13.A wrote it for any shell no outside face seeds.
- **The frontier walk.** From the seeds, each edge a kept piece uses must be used by exactly one other kept piece the other way (edges the input had first, an edge whose partner may yet be reached deferred while anything else is forced). Where a partner isn't forced the walk searches its choices: the face's own piece across the edge first (a working surface cuts its neighbours along lines where, in the result, the neighbour runs on: a plate's side across a groove floor's plane), then pieces of other faces the other way, then, only where no closure keeps every piece as it is, the same way kept reversed (a boss slid over its plate's edge stands on the plate's top plane reversed under its overhang). At most `CHOICES` (4,096) tries: one closure is the answer, two or more `Ambiguous`, none the first failure (`Open`; `Clipped`: a kept piece on a working face's side).
- **The ladder.** A walk that fails or a result that self-intersects retries: the box doubled, then the region grown by a ring, twice, stopping once the region is every face (a departure from design 399's single retry: a wall drafted past its neighbour and a groove run through need the neighbour in the region). The last refusal stands; a stop answers at once.
- **Names.** Booleans' names for pieces, cuts and merges under the tweak's key; a new edge between faces that met in exactly one edge before and one now takes that edge's name, a new vertex where exactly one old vertex's faces meet its name; every other new edge is `EditRole::Edge` (role code 18) from its faces' names sorted (an ordinal where they collide), every other new vertex `EditRole::Vertex` from its edges'. The journal is `Modified` only for what changed, the fixed engine's rule: a face moved or whose boundary edges differ, an edge whose curve or ends moved, a vertex that moved; so a box's top raised through the re-trim journals what the fixed engine does. Scaffolding (working faces' sides) never reaches the journal; the input's entities that don't come back are `Deleted`.
- **Not built.** Sheets and region faces with laminar edges are refused (`NotASolid`, `LaminarFace`; 13.B's). Golden sections are 13.D's.
- **Testing.** Unit tests per module (`tweak::retrim::{region, working, body, walk, seeds, names}`, hand-built walks: a face running on across a cut, a closure without reversed pieces preferred, the search bounded), `tweak::retrim::tests` (a raised top's names and journal against the fixed engine's, an offset's reach, only an `Uncertain` at a vertex re-trims); `tests/retrim.rs` (a wall drafted past its neighbour as a wedge, a groove run through splitting its top, a pyramid's apex split against booleans, a boss slid over its plate's edge, phase 9's cases the same bits under `Change`, a broken tangency still refused, one thread and eight); `tests/work.rs`' `re_trims_are_bounded` (a 10 × 6 × 4 mm box's wall drafted 1.3 rad: 9,961 units, the same on every machine; on cox-worker-2, an AMD Ryzen 7 9700X shared with another session, a debug build). No golden moved.

## Delete faces (decided in ryukei phase 13)

The design is document 399 in the Coxswain planning project `ryucad-ryukei`; this is its plan 13.B (phase 2).

- **API.** `tweak::delete_faces(key, &body, &DeleteSpec { faces, heal })` (and `delete_faces_with` under `Work`) takes faces out and heals each gap: `Heal::Auto` (a cap where one closes the set, else growth), `Cap` or `Grow`. `tweak::feature_faces(&body, seed)` gives a hole's, boss's or pocket's faces. Refusals are `TweakError`'s: `CapNotOnFace`, `GrowWouldCap`, `NoFeature` (new), `GapNotHealed` (a set nothing surrounds, every face, or growth reaching the region's side at its largest: a slab's side), and the re-trim's; a sheet set that needs growth is `NotASolid`.
- **Sets.** Deleted faces in sets joined across shared edges (`tweak::delete::sets`), each with its boundary edges and its host loops: loops of surviving faces the set uses all round.
- **Holes.** A host loop is a hole when its fins' pcurves, 16 samples a fin unwrapped by periods, come back to their start, its signed area says it runs clockwise about the face's normal (an area within a part in 10¹² of its box's decides nothing: not a hole), it doesn't reach a cone's apex row, and its face keeps a loop or is a sphere or torus (`delete::hole`). A set is capped when every boundary edge lies on such a loop. A rim across two coplanar faces, a loop round a pole and a boss's top go to growth.
- **The cap** (`delete::cap`). The body laid out again (`sheet::copy_where`, then the layout as a solid, each piece in the region its faces were in, or `build_sheet`) without the sets' faces and their hosts' holes: nothing is constructed, and every surviving face, edge and vertex keeps its geometry bit for bit; checked by `check` and by `check_pairs_in` over pairs with a capped face near the gap. A cavity's region that comes apart is `Uncertain`. It doesn't go through the re-trim, which would give the same body but re-imprint every other loop of the host (decided on plan 13.B, question 327: a departure from design 399, which made the cap the re-trim's own answer).
- **Growth** (`delete::grow`). The re-trim with `Changes::deleted`: deleted faces are in the region but have no working face, and the deleted faces' box diagonal joins the reach. With faces deleted, a face whose pieces include one alone that uses no clipped side seeds the walk with it (a growing face's inner points lie in the strip it grew by): a rounded box's 20 blend faces, which leave no face outside the region, grow back to the box. Caps run first; the sets left grow one at a time, in order of each set's least name, each on what the sets before it left (ryukei phase 16; 13.B grew them in one re-trim).
- **Journal.** A cap: hosts `Modified`, deleted faces and the edges and vertices that went `Deleted`. Growth: the re-trim's, each deleted face and each of its edges that went `Replaced` by the new edges between faces round its set (a fillet and its rails by the sharp edge), else `Deleted`.
- **Measured.** On bc2 (cox-worker-3: Intel Core Ultra 5 250K Plus, debug build, 4 rayon threads): a fillet grown back in about 10 ms, a through hole capped in about 5 ms, a rounded box's 20 blends in about 11 ms; `tests/delete_faces.rs` in 1.3 s on cox-worker-4 (AMD Opteron 6380, debug, 4 rayon threads, shared at a load of about 7); `tests/work.rs`' `deleting_faces_is_bounded`: cap 4 units, grow 705 units (the same on every machine).
- **Testing.** Unit tests in `tweak::delete::tests` (holes and hosts, the cap bit for bit, the fillet's journal, the boss's feature, refusals) and `tweak::retrim::tests` (growth, the seed rule, the slab); `tests/delete_faces.rs` (holes, a counterbore with its fillet, a boss with its foot fillet, a pocket, an edge's fillet and chamfer, a rounded box, a step, refusals, names, one thread and eight); `tests/work.rs`' `deleting_faces_is_bounded`. No golden moved; golden sections, the property test and fuzzing are 13.D's.
- **Limits.** Growth on a sheet; a cap of a rim over split coplanar faces (it grows instead, through the re-trim's coincident working faces); a cap of a region with cavities that comes apart.

## Blend recognition and edits (decided in ryukei phase 13)

The design is document 399 in the Coxswain planning project `ryucad-ryukei`; this is its plan 13.C (phases 3 and 4).

- **API.** `tweak::recognise_blends(&body, &RecogniseOptions { tolerance })` (and `_with` under `Work`) gives a `Recognition`: `blends`, each `Recognised { faces, kind, supports, chain }` by least face name, and `corners`, each `Corner { face, blends }` (a departure from design 399's `Vec<Recognised>`: corners join blends, so they are listed apart). `delete_blends(key, &body, &DeleteBlends { max_size, faces, options })`, `resize_blends(key, &body, &[(face, kind)])` (and `_with`), and `blends: Blends` (`Keep`, phase 9's, or `Rebuild`) on `MoveSpec`, `OffsetFacesSpec`, `ReplaceSpec` and `DraftSpec`. Refusals: `TweakError::NotABlend { face }`, `BlendLost { face, reason: BlendLoss }` (the blend's own refusal in a word: its edge names belong to a body the caller never had), `BadTolerance` (outside `[LINEAR_RESOLUTION, TOLERANCE_CAP]`). The default tolerance is 10 µm.
- **Fillets.** A face on a cylinder, a torus, a pipe, or a NURBS certified within the tolerance of a cylinder or torus (`tweak::blends::fit`: the centres of its larger curvature at a 3 × 3 grid give the candidate, `coincide::within_implicit` certifies every patch), whose rails, the edges not running across its spine, *roll on* exactly two faces: the stretch of spine under each rail lies on its support's offset by the radius, one way or the other, as one stretch `intersect::curve_surface` certifies whole. That is the design's rule (the spine at distance `r` from both supports along their normals), certified exactly; enclosing both faces' normals along an edge and asking their cross product to vanish could not certify a torus's or a sphere's rails (each normal's enclosure is as wide as the stretch turns, so it never narrows enough), and stayed for chamfers' chains. A face turning a half turn or more between its supports (a slot's end) is no fillet.
- **Chamfers.** A plane or cone face (a flat spline as its plane, `blend::spine::twin`) whose edges to two faces are each set back a constant distance from where those faces' surfaces meet, in closed form: two planes in a line, a plane square to a cylinder's or cone's axis in a circle; a spline edge counts when every control point lies within the reach of one line parallel to the meeting (the hull holds the curve). Of the pairs that read so the one setting back the least share of its supports' width wins (a mitred chamfer's end neighbours read as supports too, set back farther), under `CHAMFER_SHARE` (0.4); two pairs alike read nothing. A two-distance chamfer is `Distances`; `DistanceAngle` comes back as its distances. A chamfer standing on a chamfer is left out, the narrower kept. Ruled-spline chamfers between curved rails, and chamfers on spline cones, aren't recognised.
- **Corners and chains.** A sphere face every edge of which runs into a fillet of its radius whose spine passes through its centre is a ball corner. Faces on one surface with the same supports and size are one blend; fillets of one size across an edge that is a section of both their canals, and chamfers of one size across a tangent edge, are one chain.
- **Deleting.** The blends picked (by size and by face), closed over their chains and the corners they reach (so a rounded box's fillets go together), deleted by 13.B's growth.
- **Making again.** The blends deleted by growth, the step (a resize's nothing, a tweak's tweak) made on the sharp body, and each blend made again by one `blend::blend` call on the edges between its supports. Where it refuses `Overflow` at one of its supports, a cap-closed feature on that support whose loop lies nearest the overflowing edge (a hole) is capped, the blends made on the capped body, and the feature cut back: its plug is the lump of the capped body the sheet of its faces divides off (`boolean::divide`), subtracted; an addition (a boss) is the lump of the body the sheet of the capped host divides off, united. Up to 8 features.
- **Names.** A blend made again takes its old faces' names from its own journal (the faces `Replaced` the edges it was made on: one new for one old, else paired by coincident surfaces); a feature recut takes its faces' names through the booleans' `Placed` and `Split` events where one live face remains; then every other new face takes the name of the one old face with the same named neighbours, round by round, and edges and vertices the names of the old ones with the same faces round them. The journal is the net of the steps: their splits, merges and replacements whose names either body holds, `Modified` for what a step touched, `Deleted` and `Created` for what went and came. A rounded box resized whole, a fillet or a chamfer resized, and a top raised past its fillet keep every name.
- **Which blends a tweak rebuilds.** Only where the tweak refuses one of `TangencyBroken`, `EdgeVanishes`, `VertexSplits`, `FaceCollapses`, `SelfIntersecting`, `GapNotHealed`, `HealAmbiguous` or `RegionTooSmall`: the blends standing on a changed face or on a face next to one, or holding or standing on the faces round the entity the refusal names; never when the tweak names a blend's face itself. Otherwise the tweak's own result or refusal stands, bit for bit.
- **Work.** Recognition charges a unit a face and its curve searches; deleting and making again charge as growth, blends and booleans do.
- **Testing.** Unit tests in `tweak::blends::tests`; `tests/blend_recognition.rs` (a rounded box's 12 fillets and 8 corners, a chamfered box's 12 mitred chamfers, a boss's concave foot, a hole's two-distance rim, spline forms, near-blends and plain bodies left out, refusals, one thread and eight); `tests/blend_edits.rs` (against exact volumes: blends under a size deleted, a named blend deleted, a fillet, a chamfer and a whole rounded box resized with every name kept, a top raised and a side offset past their fillet rebuilt, a hole moved into a fillet's strip, a tweak naming a blend face left alone); `tests/work.rs`' `recognising_and_editing_blends_is_bounded`. No golden moved; the `blend_recognition` golden section, the property test, the corpus and fuzzing are 13.D's.
- **Limits.** Resizing one edge of a rounded box with ball corners makes rolled corners the blend refuses (`BlendLost`, `Uncertain`). A hole moved through the support's side as well is the re-trim's own refusal. Ruled-spline chamfers aren't blends here.

## Hardening and references (decided in ryukei phase 13)

The design is document 399 in the Coxswain planning project `ryucad-ryukei`; this is its plan 13.D (phase 5).

- **Cases from bytes.** `ryukei_testkit::edit_bytes` makes direct edits from bytes:
  - a body as `tweak_bytes` makes it;
  - faces or a feature deleted (`Heal::Auto`, `Cap` or `Grow`);
  - a tweak with `Topology::Change`, half the time with `Blends::Rebuild`;
  - blends deleted (those under a fraction of the body's size, or one by face);
  - one blend resized: to its own size, half or one and a half.

  Blend edits take blended bodies. `run` recognises blends and finds features under the case's `Work`.
- **Property test.** `tests/edit_props.rs` (release only, its own full.yml step) runs 128 seeds. Every result must:
  - check clean;
  - mesh;
  - hold mass's bound;
  - account in its journal for every face;
  - give the same bits on one thread and eight.

  A resize to its own size must give back every name, and every refusal must be typed. On cox-worker-4 (AMD Opteron 6380, release, load 5 to 7), 2026-11-10:
  - 39 built, 38 refused, and seed 76 past the 120 s deadline, in about 317 s;
  - by kind:

    | Kind | Built | Refused |
    |---|---|---|
    | delete | 4 | 9 |
    | delete feature | 0 | 6 |
    | delete blends | 9 | 9 |
    | resize | 9 | 5 |
    | move | 5 | 1 |
    | offset | 6 | 2 |
    | replace | 5 | 3 |
    | draft | 1 | 3 |

  `KNOWN_SLOW` was seed 76 (#659); ryukei phase 16 emptied it. Seeded blends are recognised back: 55 judged and 5 ruled chamfers counted, in 66 s. Not read back: 32 and 92, by design (`CHAMFER_SHARE`); 39, 100 and 102 (#660) are read back since phase 16.
- **The fixes found.**
  - **A capped rim's chamfer** (`recognise::chamfer`): a support every edge of which the chamfer bounds (a disc capping a rim) has share 0. Its width had been read from its one vertex, which lies on the rail, so the chamfer was never read. No golden moved.
  - **A cancel that didn't land:**
    - What was unmetered: the local re-trim's ladder and its working sheets, whose sides were fitted on a free meter.
    - What changed: the re-trim polls before each rung and working face, and fits its working sheets' sides on its own meter (`sheet::from_surface_in`; `from_surface` is unchanged).
    - What it cured: NIST FTC 10's blends, cancelled at 20 s, answered after 1,037 s while planning. The `edit` fuzz target's first find, cancelled at 30 s, answered at 40 to 43 s on cox-worker-4 (release, one rayon thread), and under instrumentation ran past libFuzzer's 600 s. Both now answer at the cancel.
    - Its tests: `fuzz_regressions.rs`' `a_cancel_stops_an_edits_working_sheets` and ryustep's `a_cancel_stops_deleting_ftc_10s_blends`.
    - No golden or work unit moved in the existing suites.
- **Fuzzing.** The `edit` target is in fuzz.yml (6 GiB, a 600 s timeout). On cox-worker-4 (load 8 to 27, other sessions):
  - before the cancel fix: a three-minute smoke made 787 runs with no find, and the first hour stopped at 43 minutes on the timeout above;
  - after it: 2,097 runs over the hour, a corpus of 824, peak RSS 2,951 MB, and no crash, timeout or running out of memory. The old timeout's input became a slow unit, cancelled at the deadline.

  Uncancelled, that input still refuses `Uncertain` after about 200 s in release (#647).
- **Goldens.** `retrim`, `delete_faces` and `blend_recognition` are new; every other section is unmoved.
- **The committed corpus** (ryustep's `tests/defeature.rs`, release only; cox-worker-4, load 9 to 18, 440 s): 0 defeatured, 8 refused, 3 past the 120 s deadline, 16 with no blend under 2 mm. The refusals:
  - CTC 01, 03 and 05 `Uncertain` (#650);
  - FTC 06 and 09 `HealAmbiguous` (#651);
  - FTC 07 and 10 past the deadline (#652);
  - FTC 08 `Uncertain` (#653);
  - CTC 04 past the deadline (#654).

  A rounded box written to STEP in millimetres and healed back is defeatured to the box.
- **The pinned corpus** (`pinned_bodies_defeature`, then in `ryukei-bench`, since phase 16 in ryustep's `defeature.rs`; by hand in the devcontainer; cox-worker-4, load 5 to 11): `pinned corpus: 0 defeatured, 19 refused, 9 past the deadline, 5 with no blend under 2 mm, in 1360.4 s`. The refusals are the committed corpus's kinds in other editions (comments on #650 to #654), and six fitting none of them (#658):
  - FTC 06's and STC 06's AP242 editions `Uncertain`;
  - FTC 11's and STC 09's `HealAmbiguous`;
  - STC 10 `Uncertain`;
  - OCCT's `linkrods.step` past the deadline.

  Two departures from the plan's test:
  - Each delete is cancelled at 120 s, as `defeature.rs`'s are. Uncancelled, FTC 07, FTC 10 and STC 07 took 17 to 24 minutes each to refuse `ExtensionUnsupported`.
  - Only blends under 2 mm count. Bodies whose blends were all larger had been counted as defeatured.

  One narrowing of "checks clean": a `CannotCheck` pair the body already had before the delete, by its faces' names, may stay. That uncertainty is the import's (CTC 02's AP242 edition, #264). A crossing, an overlap, or a pair that could be checked before still fails.
- **References.** `ryukei-bench`'s `defeaturing` test (devcontainer, `--features occt`) runs `BRepAlgoAPI_Defeaturing` through the shim (`Solid::defeature`, on one thread, without history). Five cases are held to their closed forms: a through hole, a boss, an edge's fillet, its chamfer and a rounded box's every blend. ryukei and OCCT agree on each; OCCT's volumes and times are printed.
- **The bar.** OCCT ran past an hour on the 1,000-hole plate, so it is timed on 100 holes, a lower bound for 1,000; that is a departure from design 399's like-for-like race. Three runs on cox-worker-4 (AMD Opteron 6380, devcontainer, OCCT 7.8.1, release, one test thread, `RYUKEI_BARS=1`), 2026-11-10:

  | Run | Load | ryukei, 1,000 holes, one thread | OCCT, 100 holes | Ratio |
  |---|---|---|---|---|
  | 1 | 9.0 → 6.9 | 0.226 s | 73.2 s | 0.0031 |
  | 2 | 6.9 → 6.4 | 0.218 s | 67.0 s | 0.0033 |
  | 3 | 6.4 → 9.2 | 0.224 s | 68.9 s | 0.0033 |

  OCCT's volume agrees in every run.

## Blend deletes on the corpus (decided in ryukei phase 16)

The design is document 444 in the Coxswain planning project `ryucad-ryukei`; this is its plan 16.C. Question 425's bar: every committed body with blends under 2 mm defeatured and checked clean within 120 s on one thread.

- **Full rounds.** A face between supports whose own normals, at the middles of a rail each side, turn more than π − `FULL_ROUND` is no fillet (`recognise::fillet`). The 1.41 mm round top of a rib 0.635 mm thick in NIST CTC 03 to FTC 09 is one: one wall lies 4.7 µm inside the round's tangent, so its arc runs 0.08 rad past a half turn, which `acos` of the face's own normals folds back below it. Its walls face apart and never meet, so growth couldn't close it.
- **One working face for a surface's faces** (`retrim::absorbed`). Region faces left as they are that share an edge with another such face on the same surface, the same way round, take the least-named one's working face; the journal merges them into it (`Merged`, the child that least name). FTC 09's hole walls, each two half cylinders carried a whole turn, gave coinciding working faces and closed two ways.
- **Growth one set at a time** (above, under Delete faces). Separate gaps' choices multiplied in one walk: CTC 01's three fillets each grew alone in 0.7 s and together passed `CHOICES`. Grown one after another, a delete of two far-apart sets gives the bits deleting one and then the other does.
- **The re-trim's check.** Only pairs with a face the re-trim made (`check_pairs_in`): the faces outside the region come back as they were. One whole check of CTC 04's result spends 4,790,266 units; a fillet's delete now spends 42,584.
- **Ambiguous walks** name the first piece two closures differ on, or the seed past `CHOICES`; they had named the first seed, an unrelated outside face. `CHOICES` stays 4,096.
- **The carry cap** (`retrim::working`). A carried bounded surface goes at most its domain's own length past each end: FTC 07's NURBS was carried from a few units to v ∈ [−37, 20], its working sheets fitted for 102 s. #647's case answers in 3.5 s (26.7 s before).
- **Pairs met with slack** (`WorkBody::pairs`). Working faces' boxes are grown by each face's tolerance before they are overlapped, as `face_face` grows them: FTC 08's pocket wall is tangent to its corner cylinder and lies 3e-17 m outside the cylinder's box, so the pair was never met and the cylinder ran on to its clipped side.
- **A delete's periodic faces carried less than a turn** (`working::delete_turn`). In a delete (no face moved or given a new surface), a kept periodic face is carried over its own pcurves' span and a sixteenth each way, not a whole period, when every edge it shares with a deleted face runs that whole span (so it never has to grow round its axis across one); a face standing for others on its surface keeps the whole turn. Carried whole, FTC 08's eight quarter-turn corner cylinders put their back three quarters across the pocket and the walk passed `CHOICES`.
- **`Scope::Rung`.** Each rung of the ladder runs under its own cap, `1 << 24`: 16 times the largest rung that builds (1,009,883 units, FTC 08's pocket floor, a 33-face region at rung 0, in a pinned edition), rounded up to a power of two, measured with `work-stats` over `retrim`, `delete_faces`, `blend_edits`, `tweaks`, `drafts`, `edit_props`, `work`, `fuzz_regressions`, `tweak_props` and ryustep's `defeature` with the pinned bodies (cox-worker-2, AMD Ryzen 7 9700X, 2026-11-11). The cap binds each task of a parallel map, not a rung's sum: a rung of many uncertain pairs still spends up to the cap on each.
- **The direct-edit fixes.**
  - An inside-out re-trim is `FaceCollapses`, never a panic (#704): `repair::resolve` leaves an inside-out result to its caller in debug builds too, the re-trim refuses it, a self-union refuses it `Topology(Inconsistent)`, and a global offset keeps its debug assertion.
  - A delete whose region is the whole body seeds every face it keeps by rule 3, not one face alone (#692: a drilled plate's side deleted closed from the hole's wall alone, its plug). `measure::mass` already reads each face's sense: #692's "+282.7 mm³" was that plug, the right way out.
  - `check_intersections` decides a curved solid's orientation (#692): once no faces cross, overlap or go unchecked, one `winding_off` ray just outside a face of each solid region with a face `check` can't sum; not zero is `NotOutward`. It lives there, not in `check`, since an offset's envelope, which `check` must pass, self-intersects and winds −1 round most of its faces. No unit is charged; CTC 04's check took 3.97 to 3.99 s before and after (one thread). The re-trim refuses a curved result inside out `FaceCollapses`.
  - A kept straight edge whose vertex would move past its other end is `EdgeVanishes`, not `Uncertain` (#687), so `Blends::Rebuild` rebuilds there.
  - A face the table split whose pieces all merge back into one face takes its name again (#691): the groove's plate keeps its four sides' and its bottom's names. It moved the determinism golden's `retrim` section and ryucad's rebuild golden's `blend_edits` section, nothing else.
  - A chamfer's support is as wide as points along its edges reach (#660): seeds 39, 100 and 102 are read back.
- **Measured** (cox-worker-2, AMD Ryzen 7 9700X, 8 cores, shared with other sessions; one thread a delete, release).
  - The committed corpus (`defeature.rs`' `imported_bodies_lose_their_small_blends`, `RYUKEI_BARS=1`, load 1.9 before and 1.7 after, 200 s for the file):

    | Body | Result |
    |---|---|
    | CTC 01 #0 | 3 blends, 139 → 136 faces, 0.43 s |
    | CTC 03 #0 | 3 blends, 139 → 136, 0.26 s |
    | CTC 04 #0 | 3 blends, 517 → 514, 4.35 s |
    | CTC 05 #0 and #2 | 3 blends, 208 → 205, 13.3 s |
    | FTC 06 #0 | 3 blends, 144 → 141, 0.48 s |
    | FTC 07 #0 | 87 blends, refused `Uncertain` in 85 s (`KNOWN_REFUSED`, #652) |
    | FTC 08 #0 | 33 blends, 270 → 237, 2.98 s |
    | FTC 09 #0 and #1 | 11 blends, 158 → 143, 0.49 s |
    | FTC 10 #0 | 86 blends, refused `Uncertain` in 65 s (`KNOWN_REFUSED`, #652) |

  - The pinned corpus (`pinned_bodies_defeature`, tightening on): `pinned corpus: 17 defeatured, 9 refused, 2 past the deadline, 5 with no blend under 2 mm, in 642.3 s` (load 3.2 before, 4.6 after, beside other builds). Defeatured: CTC 01, 03, 04 and 05, FTC 06, 08 and 09 in their AP203 editions, CTC 01 and 03 to 05 with PMI, and the AP242 CTC 05, FTC 06, FTC 08, STC 06, STC 08 and STC 09 (0.14 to 5.4 s). Refused: FTC 07 and 10, and STC 07 and 10, their AP242 twins (`Uncertain`, 30 to 97 s; FTC 07's AP242 edition past the deadline: #652's tangent seams); CTC 02 in two editions (`Uncertain`, 2.9 and 67 s); FTC 09's AP242 edition (`Uncertain`, 1.1 s); FTC 11's AP242 edition (`GapNotHealed`, 0.8 s); OCCT's `linkrods.step` past the deadline (#658 holds each).
  - #647's bytes refuse `Uncertain` in 3.5 s (3,489,400 units); `edit_props`' seed 76 refuses `RegionTooSmall` in 24.0 s; `edit_props` builds 40 of 78 edits, none past the deadline.
- **Limits.**
  - FTC 07 and FTC 10 (#652): every refusal is a pair of kept region faces tangent along a shared edge (a pipe-like round's straight run against its corner torus; cylinders and spheres against a NURBS), whose working faces, carried past that seam, meet tangentially along it; the trace can't certify that within its cap. It needs a rule for kept faces tangent along a seam.
  - A hole whose wall is two half cylinders can't have its rim filleted or chamfered (`blend` refuses `Topology(Inconsistent)`).
- **Testing.** `blend_recognition.rs`' `a_ribs_full_round_is_no_fillet`; `delete_faces.rs`' `far_apart_sets_grow_as_one_after_another` and `deleting_a_drilled_plates_side_is_refused`; `retrim.rs`' `an_offset_past_the_opposite_side_collapses` and the groove's names; `drafts.rs`' `a_wall_drafted_past_its_neighbour_keeping_topology_loses_an_edge`; `fuzz_regressions.rs`' `the_edit_targets_first_find_answers`; unit tests in `tweak::retrim::{walk, working, body}`, `work` and `check`; ryustep's `defeature.rs`: `ftc_09s_split_holes_lose_their_countersinks`, `a_delete_checks_only_its_own_pairs`, `ftc_08s_pocket_floor_loses_its_fillets`, `ftc_07s_and_ftc_10s_blends_are_deleted` (ignored, #652), the committed corpus's bar with `KNOWN_REFUSED`, and `pinned_bodies_defeature` (ignored until the corpus is fetched).
