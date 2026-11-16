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
- **The re-trim's check.** Only pairs with a face the re-trim made (`check_pairs_in`): the faces outside the region come back as they were. One whole check of CTC 04's result spends 4,790,266 units; a fillet's delete now spends 42,584. Fewer units moved ryucad's rebuild golden (`deletes`, `topology_tweaks`, `blend_edits`), whose edit reports record what they spent; with that left out, the hashes are main's.
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
  - A face the table split whose pieces all merge back into one face takes its name again (#691): the groove's plate keeps its four sides' and its bottom's names. It moved the determinism golden's `retrim` section, nothing else.
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
  - FTC 07 and FTC 10 (#652): see Plane stops and the bars (phase 19.B).
  - A hole whose wall is two half cylinders can't have its rim filleted or chamfered (`blend` refuses `Topology(Inconsistent)`, #723).
- **Testing.** `blend_recognition.rs`' `a_ribs_full_round_is_no_fillet`; `delete_faces.rs`' `far_apart_sets_grow_as_one_after_another` and `deleting_a_drilled_plates_side_is_refused`; `retrim.rs`' `an_offset_past_the_opposite_side_collapses` and the groove's names; `drafts.rs`' `a_wall_drafted_past_its_neighbour_keeping_topology_loses_an_edge`; `fuzz_regressions.rs`' `the_edit_targets_first_find_answers`; unit tests in `tweak::retrim::{walk, working, body}`, `work` and `check`; ryustep's `defeature.rs`: `ftc_09s_split_holes_lose_their_countersinks`, `a_delete_checks_only_its_own_pairs`, `ftc_08s_pocket_floor_loses_its_fillets`, `ftc_07s_and_ftc_10s_blends_are_deleted` (ignored, #652), the committed corpus's bar with `KNOWN_REFUSED`, and `pinned_bodies_defeature` (ignored until the corpus is fetched).

## Known seams (decided in ryukei phase 18)

The design is document 551 in the Coxswain planning project `ryucad-ryukei`; this is its plan 18.C (#652, #658).

- **The case.** Two kept region faces tangent along an edge of the input have working faces that, carried past the edge, meet tangentially along its curve. No trace can certify a crossing there.
- **The rule** (`intersect::known`, `WorkBody::seams`, `table::build_seamed`).
  - **What is recorded.** The re-trim records each input edge whose two faces are kept, unchanged working faces (`still`) tangent along it (`classify::tangent` is `Ok(true)`). Each seam carries both fins' pcurves, moved by whole periods into the working faces' boxes (`retrim::body::seam_for`).
  - **The order.** The table first traces such a pair under `SEAM_TRACE_FIRST` (`1 << 16`) units, so a pair that settles there settles as before. Failing that, it meets the pair along its seams (`known_seam`). Failing that, it traces the pair whole.
  - **Running on.** A seam is run on along its own curve where both pcurves are lines in the parameter plane: a line over the faces' shared box, a circle or an ellipse a whole period.
  - **What it needs.** It is certified within `SEAM_SLACK` (10) edge tolerances of both surfaces through its pcurves (`pcurve_certified`). It is taken only where one face keeps off the other's surface outside the tangent strip `sqrt(2 × size × 100 tol)` (`beside::face_beside_rails`, the check's ladder `1 << 9`, `1 << 12`, `CELLS`).
  - **Placing.** The branch is then cut and placed as a traced one is (`face::placed`, split out of `face_face_over`). Nothing else is traced: a trace over the region less the strip would end short of the seam, and its boxes would hold points the strip doesn't certify.
  - Booleans, self-unions and global offsets don't change (`repair::resolve` passes no seams; `boolean::table::build_capped` takes `None`).
- **Withheld:**
  - pcurves other than lines past the edge's own interval;
  - a seam not certified within ten tolerances;
  - faces neither of which keeps off the other beside the seam. A pipe-like round's straight run carried past the circle where it meets its corner torus enters the tube on the bend's inner side and leaves it on the outer, so the two cross along curves from the circle. FTC 07's torus pair is that case, committed as a NIST cut.
- **What it doesn't reach: FTC 07 and FTC 10** (#652's census, Task 1). Design 551's premise, two kept region faces tangent along an edge with line pcurves, describes none of their failing pairs.
  - In FTC 07, four of seven failing pairs have one face outside the region, across a frontier edge (the torus pairs are tangent and cross past the seam).
  - Every pair against a NURBS face is tangent only partly along its edge (`classify::tangent` refuses `PartlySmooth`), with NURBS pcurves.
  - With all of them set aside, a non-tangent plane/plane pair at the frontier refuses behind them in both bodies.
  - So `KNOWN_REFUSED` keeps both, and question 820's carry-stop trial was skipped with the coxswain's agreement: settling FTC 07's torus pairs would leave its five partly smooth pairs, and FTC 10's four are all partly smooth. Frontier and partly smooth seams are ryukei 19's.
  - Plan 19.A holds frontier and partly smooth seams and stops runs at their iso seams (below).
- **The pinned editions** (#658, Tasks 11–13): `17 defeatured, 9 refused, 2 past the deadline, 5 with no blend under 2 mm`, 16.C's tally exactly. `PINNED_DEFEATURED` asserts that those 17 stay defeatured. Design 551 expected #652's AP242 twins there, but they don't build. The causes still standing:
  - **Frontier and partly smooth seams:** STC 07, STC 10, FTC 07 and FTC 10 in their AP242 editions, CTC 02 in both AP203 editions, and linkrods. CTC 02 with PMI is a single frontier seam, a closed ellipse tangent all along; with that one pair set aside, the delete builds.
  - Ryukei phase 19 holds these seams. Where each edition stands now is under Plane stops and the bars (phase 19.B).
  - **FTC 09 AP242:** the split of a cone face that runs down to its own apex fails (`split::split`, a pole).
  - **FTC 11 AP242:** the walk is ambiguous in the delete's second re-trim, then `Clipped` (`GapNotHealed`).
  - **linkrods:** every one of its eight chains runs past 150 s. A rung's cap bounds each parallel task of its table (`Meter::par_map` gives each the room left), not their sum, and chain 6's rung spent 84.4M units under `RUNG`'s 16.8M.
- **What moved.** Golden section `known_seams` was added: a block's rounded vertical edge, the top edges' tangent chain filleted, and the small fillets deleted. Its delete records two seams of sixteen pairs. The meeting's bits are pinned in the crate (`known::tests::known_seams_hash`). No other ryukei or ryucad golden line moved, and checks and deletes ran the same on main and the branch (none more than 5% apart).
- **Measured** (cox-worker-2, AMD Ryzen 7 9700X, one thread, 2026-11-14, load 0.7–3.5; branch `ea1dcb8a` against main `0d7ec782`):

  | Bar | Result |
  |---|---|
  | Check, MSOP-8-1EP (#303) | 45.1 s, 8,215,098 units. Unmet (30 s); the test holds 60 s |
  | Check, no undecided pinned pair (#264) | CTC 02 AP242 #1341's `cannot_check` pair stays. Unmet |
  | Check, the thread (#698) | ryucad's sweep 14.5 s, ryukei's twin 6.4 s, no `CannotCheck`. Met |
  | No check or delete slower | identical results; largest move −5%. Met |
  | Self-union, molex (#585) | `PiecesTouch` beside the split span in 9.2 s (main 9.1 s, the same refusal). Unmet |
  | PCBA interference (#436) | 0 unchecked of 1,214, byte for byte with main; interference 95.3 s (main 92.6 s, four threads). Met |
  | Blend deletes, committed (#652) | FTC 07 refused in 85.1 s, FTC 10 in 50.9 s (`KNOWN_REFUSED`). Unmet → ryukei 19 |
  | Blend deletes, pinned (#658) | 17 / 9 / 2 / 5 in 666 s; FTC 07 AP242 and linkrods past the deadline. Unmet |

  step-hunt over `committed` and `nist` (four jobs, `--stages ref`): 90 cases, no slow or timeout finding (the slowest stage 3.31 s), and the two known failures, as in 18.B's run. Fuzzing (`edit`, `boolean`, `self_union`, 30 min each under ASan, cox-worker-4 at load 15–19): no crash, timeout or OOM, nothing in the re-trim or `known`.
- **Testing.**
  - Unit tests: `intersect::known`'s (a plane tangent to a cylinder met along the line to the faces' ends; turned 0.3 rad, and 10 µm into the tube, withheld; a cylinder carried past its torus seam withheld), `known_seams_hash`, `beside::rails_tests`, `face::swap_tests`, `retrim::body`'s `kept_faces_tangent_along_a_round_record_their_seams`, and `flat`'s `a_middle_flat_span_is_never_split`.
  - The NIST cut: `retrim::tests::ftc_07s_seam_pair_crossing_past_its_seam_is_withheld` (`nist_ftc_07_seam_input.body` and `nist_ftc_07_seam_work.body`).
  - ryustep's `defeature.rs`: `ftc_07s_and_ftc_10s_blends_are_deleted` (ignored: frontier and partly smooth seams), `KNOWN_REFUSED`, and `pinned_bodies_defeature` with `PINNED_DEFEATURED`.

## Held seams and iso stops (decided in ryukei phase 19.A)

Design 642, plan 19.A (#652, #658).

- **Held seams.** `tweak::retrim::held` records each input edge that meets all of these:
  - it has two fins, both with pcurves;
  - its two faces both stay (outside the region, or `still`), and at least one of them is a working face;
  - `classify::tangent` calls it tangent all along (`Ok(true)`) or in places (`PartlySmooth`).

  Each held seam has a kind. It is *frontier* when one face is outside, *fixed* when every face round both ends stays (a closed edge's one vertex is its end), and *running* otherwise. An absorbed face counts as not staying. `WorkBody::seams` builds the table's `Seams` from the held seams, outside faces included, and `seam_for` no longer judges tangency.
- **Met along the edge.**
  - A frontier or fixed seam is the edge itself, never run on, whatever its curve and pcurves (`SeamKind`, `known::run_on`). A running seam keeps 18.C's rule.
  - `known_seam` certifies first by keep-off (18.C's), then by the section plane. The section plane needs the seams on one planar curve (`beside::section_plane`) and each face on its own side of that plane outside the strip (`face_beside_rails`, and `beside::opposite_sides`, which the check now shares). A face carried past the seam crosses the plane, so the second certificate holds only for faces that end at it.
- **Refitted pcurves** (a departure from design 642's "the input's own pcurves"). `pcurve_certified` takes only lines and polynomial splines. Where a seam's pcurve is a conic (CTC 02's `Ellipse2` on its NURBS face) or a rational spline (FTC 07's partly smooth ellipse), `known::refit` fits one to the seam's curve on that surface (`fit_pcurve_within`, certified) and moves it by whole periods to where the edge's own pcurve lies.
  - The refit is asked for the edge's own tolerance, not `SEAM_SLACK`, which still bounds what is accepted. The fit reports at least what it is asked for, so asked for the slack, the seam took ten times its tolerance, and a delete's next re-trim ten times that (CTC 02: `ToleranceTooLarge`, 6.9e-4 m).
  - No new tolerance rule, and no golden moved.
- **Iso stops.** `held::iso_stops` ends a working face's rect half a strip past a held seam with fixed ends. The margin is half of `known::seam_strip`, from the input faces' box and the edge's tolerance, so no more than what `known_seam` takes. A stop applies only where all of these hold:
  - the seam's pcurve on the face is iso (`held::iso`: a `Line2` along a parameter, or a `Nurbs2` whose control points share that coordinate bit for bit);
  - the pcurve lies at the end of the face's own box;
  - every fin running along that end is such a seam;
  - the direction isn't a whole period.

  The margin in parameters is the model-space margin divided by `first_bound`'s bound on `|P_u|` or `|P_v|`.
- **The retry with stops.** A rung is built as before. Where it refuses `Uncertain` at a pair with held seams (`Rung::Held`), `settle` builds it again with stops, under its own `Scope::Rung` cap. The stopped build's answer stands, except where its walk fails or nothing stops (`Rung::Walked`, `Rung::Unstopped`). Then the first build's `Uncertain` stands, since building it again would only repeat it. Stops never come from the first build, so bodies that built before keep their bits. `retrim_with(…, Stops::Never, …)` is the tests' re-trim without stops.
- **Census II** (#652, #658; on main's code with failing pairs skipped):
  - **FTC 07:** seven pairs.
    - Pairs 1 and 2: an outside cylinder against a NURBS run, frontier, `PartlySmooth`, both pcurves iso.
    - Pairs 3 and 4: an outside torus against a cylinder run, frontier, `Ok(true)`.
    - Pair 5: NURBS/plane, running, `PartlySmooth`; the plane is one-signed over the NURBS face (keep-off).
    - Pairs 6 and 7: NURBS/cylinder ellipses, running, `PartlySmooth`, rational pcurves, both faces on both sides of the plane.
  - **FTC 10:** four running `PartlySmooth` pairs: two sphere/NURBS (keep-off) and two cylinder/NURBS along splines (no section plane).
  - **The pinned editions:** CTC 02 with PMI has one frontier ellipse (keep-off). CTC 02's geometry-only edition and STC 07 have tangent `Uncertain` pairs, which aren't held. FTC 07 AP242 has 41 held pairs and 178 failing pairs that share no edge. Linkrods has 14 held pairs, 3 of them partly smooth splines with no plane.
- **What 19.A settles.** FTC 07's pairs 1 and 2 and FTC 10's sphere pairs are held and met, so the deletes refuse later and faster. FTC 07 went from 85.2 s to 42.5 s, and FTC 10 from 51.0 s to 15.5 s. FTC 07's AP242 edition, past the deadline on main, now refuses in 30 s. The bent-rod twin builds through its stop.
- **What it doesn't settle** (19.B):
  - **FTC 07's torus pairs 3 and 4.** The run is a whole turn: its own pcurves span 2π, so `delete_turn` leaves it whole, and its seam with the torus is a quarter of its end circle. Stopped past the seam, the rest of its rim crosses the plane outside the seam's strip. The cut test `ftc_07s_seam_pair_is_met_once_its_run_stops` is ignored with that cause.
  - **FTC 07's NURBS/cylinder pairs 6 and 7**, which need plane stops.
  - **FTC 10's next refusal** at `a289199e…`, a pair only reached now.
  - **CTC 02 with PMI.** Its ellipse is met, but a later re-trim of the same delete refuses at a cylinder/cylinder pair with no seam. Census II couldn't see it, since it skipped the ellipse pair as apart. So it stays out of `PINNED_DEFEATURED`, and Decision 12 didn't hold.
  - **CTC 02's geometry-only edition**, whose bends' `tangent` is `Uncertain`, so they aren't held.
- **Testing.**
  - **Unit tests** cover kinds (`run_on` keeps a frontier or fixed seam's interval), iso pcurves, the section plane, iso stops and the fallback (`settle`). The section plane's test stops its cylinder 0.05 mm past the seam, because faces that both end exactly on a closed seam leave `placed` only a contact.
  - **The bent-rod twin** (`ryukei_testkit::held_seams`, and `testing::bent_rod` in the crate, pinned together by `ROD_DUMP_HASH`). Without stops its rim's delete refuses `Uncertain`; with them it builds, check-clean, within 0.2% of Pappus. `tests/work.rs` bounds the stopped delete at 9,871,280 units, the same on one thread and on eight.
  - **NIST cuts:** CTC 02's closed frontier ellipse, FTC 07's partly smooth ellipse (a running seam), and FTC 07's torus/cylinder pair stopped (ignored, above).
  - **Goldens.** Section `held_seams` was added. No other ryukei or ryucad section moved.
  - **Measured** on cox-worker-2, one thread, load 1–2, branch `3def2acf` against main `de314a93`.
    - Committed deletes: none slower, same results.
    - Pinned: main 17 defeatured / 9 refused / 2 past the deadline / 5 with no blend in 629.6 s; the branch 17 / 10 / 1 / 5 in 421.0 s. The only slower body is CTC 02 with PMI (2.9 to 15.8 s), which refuses further along.
    - `edit_props` built 78 edits on the branch and on main, with the same refusals.
    - Fuzzing: the `edit` target ran 30 minutes on cox-worker-4 (Docker, 8 GB) with no crash, timeout or OOM: 5,217 runs, at about 2.9 a second, and a corpus of 654.

## Plane stops and the bars (decided in ryukei phase 19.B)

Design 642, plan 19.B (#652, #658).

- **Box-apart parts.** On a pair met in a stopped build (`Seam::stopped`, set by `WorkBody::seams`), the section plane leaves out each part of a face whose certified box lies apart from every one of the other face's boxes (`beside::face_beside_rails_near`). Those boxes are `beside::face_boxes`: the surface's certified box (`Sheet::jet_box_in`) over each part of a 16 × 16 grid of the face's parameter box, leaving out parts certified outside the face, each grown by the pair's tolerance. Such a part can't meet the other face.
  - FTC 07's torus/run pair needed the grid. The stopped run crosses the seam's plane along its whole end circle, while the seam is a quarter of it. The torus face's one box (`face_box`) holds the whole circle; its grid boxes don't.
  - Keep-off and unstopped builds are unchanged.
- **Plane stops.** `held::stop_plane` is the seam's plane (`section_plane`) where both input faces are certified on opposite sides of it outside the strip (twice `margin`), moved the margin towards the other face. `held::plane_cut` divides the working face's rect sheet by a plane sheet over the region box (`boolean::divide_in`) and keeps the one face behind it (`Working::cut`, used by `work_body`). Every edge of the cut sheet is clipped and scaffolding. `plane_stops` applies a face's stop planes in edge order, and only where no iso stop moved the face's rect at all: per face, where plan 19.B's Decision 3 said per seam, the more conservative of the two.
  - At the resolution the strip round a seam is narrow (3 mm on a 5 m tube for a 1e-8 seam), and a seam whose pcurve the division fitted can't be told apart from it there. So plane stops certify on seams with a file's tolerance, not on exact ones (`held.rs`' oblique pair takes 1e-6).
- **Running seams** take plane stops too (question 916, option 0: `RUNNING_PLANE_STOPS`). A stop that cuts off what the result needs leaves a kept piece on its clipped cut, the walk fails, and `settle` keeps the first answer. Iso stops stay fixed-ends only.
- **Uncertain bends are held** (question 917, option 0: `held_tangent` also takes `classify::tangent`'s `Uncertain`). Neither certificate assumes tangency. A transversal edge (`Ok(false)`) still isn't held. Pairs that settle under `SEAM_TRACE_FIRST` settle as before.
- **Census III** (main `469e857d`, one thread, with failing pairs skipped) and a trace of the real flow on the branch (nothing skipped), both on #652 and #658:
  - **FTC 07's runs are quarter cylinders, not whole turns.** Their pcurves straddle the period's seam (sides at u = 5.51 and 0.78). `working::own_span`'s plain min/max reads them as a whole period, so `delete_turn` carries them a whole turn. 19.A's section above calls them whole turns; it was this reading.
  - The plane/plane frontier pair census I saw was an artefact of its skipped pairs (Task 10).
  - FTC 07's NURBS/cylinder pairs fail against cylinders they share no edge with, so they have no ellipse seam (plan 19.B's Task 7, deferred).
- **What stopped FTC 07, FTC 10 and CTC 02 next** (recorded under question 882; see Limits below):
  - **FTC 07:** rung 0 refuses `Held` at the torus/run pair. The stopped build meets both torus seams, but its walk refuses `Ambiguous` at the other run: carried a whole turn and stopped past its quarter-arc seams, the run's strip past them joins the rest of the turn. Two experiments (not committed) got further. `own_span` as a circular hull, with iso values compared modulo the period, left the stopped walk `RegionTooSmall`: the run's tangent outside plane isn't carried at rung 0. Then a stopped build whose walk fails climbing the ladder instead of answering with the first `Uncertain` reached rung 2, which refuses at a cone and a cylinder carried about 0.5 m that cross nearly tangent with no shared edge.
  - **FTC 10:** rung 2 refuses at torus `a289199e` / cylinder `24401201`. The working cylinder, carried about 0.25 m, continues the torus's tube straight on and is tangent to it along a minor circle inside the torus face (equal radii, no shared edge).
  - **CTC 02 with PMI (#295):** rung 3 refuses at two cylinders with axes 2° apart, each carried about 0.9 m, crossing nearly tangent far from both input faces (no shared edge).
- **Bars** (cox-worker-2, AMD Ryzen 7 9700X, one thread, release, 2026-11-15 01:40–01:57 UTC, load 1.9–2.9; branch `80a5081f` against main `66d87d0c`):

  | Bar | Result |
  |---|---|
  | Blend deletes, committed (#652) | FTC 07 refused `Uncertain` in 22.7 s (main 42.5 s), FTC 10 in 15.5 s (main 15.5 s). `KNOWN_REFUSED` stays and `ftc_07s_and_ftc_10s_blends_are_deleted` stays ignored. Unmet: carried near-tangent pairs with no shared edge (question 882) |
  | Blend deletes, pinned (#658) | `18 defeatured, 8 refused, 2 past the deadline, 5 with no blend under 2 mm, in 389.9 s` (main 17 / 10 / 1 / 5 in 421.0 s). CTC 02's geometry-only edition (#20754) builds in 3.2 s (main refused it in 64.3 s) and joins `PINNED_DEFEATURED`; the 17 stay. FTC 07 AP242, STC 10, CTC 02 with PMI and linkrods are as on main. Partly met |
  | No delete slower | Every committed body within 1% of main. Of the pinned, only STC 07 #11020 moved: refused in 71.5 s on main, past the deadline (120 s) on the branch; with no deadline it refuses `Uncertain` in 165 s (below, Limits). It didn't build on main either |
  | Goldens | Every ryukei, ryucad, ryusketch, ryumate and ryustep section unmoved. Met |
  | Property suites | `edit_props`, `blend_props`, `boolean_props`, `heal_props`, `tolerant_boolean_props` and `general_props` pass. `edit_props` gives main's 78 outcomes exactly (40 built). Met |
  | Fuzzing | `edit`, 30 minutes under ASan on cox-worker-4 (Docker, 8 GB, at `80a5081f`, load 13–47 beside another fuzz run): 3,212 runs at about 1.8 a second, corpus 1 → 679 files, no crash, timeout or OOM. Met |

- **Testing.**
  - Unit tests: `beside::rails_tests::a_part_apart_from_the_other_face_has_no_side`; `held`'s `a_stop_plane_lies_the_margin_past_the_seam_towards_the_other_face`, `a_running_seam_has_a_stop_plane_and_a_face_paired_with_itself_none`, `a_plane_cut_keeps_the_working_face_behind_its_stop`, `plane_stops_cut_a_face_whose_seam_lies_on_no_iso_line`, `an_uncertain_bend_is_held_and_a_transversal_edge_is_not` and `stc_07s_uncertain_bend_is_held`; `retrim::tests`' `an_iso_stopped_run_takes_no_plane_stop` (the bent rod's delete pinned by `BENT_ROD_DELETED_HASH`) and `a_body_that_builds_is_the_same_with_stops_allowed` (its first build settles, so it takes no plane stop).
  - NIST cuts: `ftc_07s_seam_pair_is_met_once_its_run_stops` (now run in release builds) and `nist_stc_07_bend_input.body` (census III's plane/NURBS pair `f9366bce`/`c68684aa`).
  - ryustep's `defeature.rs`: `KNOWN_REFUSED` (FTC 07 and FTC 10), `ftc_07s_and_ftc_10s_blends_are_deleted` (ignored with the recorded cause) and `PINNED_DEFEATURED` (18).
- **Limits** (question 882's rule; bars unmet):
  - **Carried near-tangent pairs with no shared edge** (FTC 07 at rung 2, FTC 10, CTC 02 with PMI; #652, #658). Working faces carried far along their own straight directions (0.25 to 0.9 m) meet each other nearly tangent, away from any input edge, so no held seam and no certificate applies. A later stage needs a rule bounding how far a working face is carried, or one certifying such pairs apart near the input faces.
  - **Straddling periodic faces** (FTC 07): settled in phase 20.A (below).
  - **A stopped build's failed walk answers with the first `Uncertain`**: settled in phase 20.A (below).
  - **STC 07 #11020: a rung's cap bounds each parallel pair, not their sum**: settled in phase 20.A (below); STC 07 now refuses inside the deadline.

## Ordered sums and the narrowed build (decided in ryukei phase 20.A)

Design 669, plan 20.A (#652, #658).

- **Census IV** (on #652 and #658; cox-worker-2, one thread, near boxes by design 669's rule as drafted, never committed). On main: FTC 07 rd refuses at rung 0 at torus `297daf90` / cylinder `0b3042ff` (OVERLAP, one shared edge); FTC 10 rb at rung 2 at torus `a289199e` / cylinder `24401201` (**OVERLAP**, no shared edge); CTC 02 with PMI at rung 3 at cylinders `a9b6e499` / `8031cb1f` (**APART**); STC 10 at rung 2 at torus `e550357e` / cylinder `872b2691` (OVERLAP); FTC 07 AP242 at rung 0 at NURBS `783471c2` / cylinder `91346e59` (OVERLAP). On the branch: FTC 07 rd climbs narrowed to rung 2 and refuses at cylinder `18dae4b7` / NURBS `33b57350` (OVERLAP, no shared edge; not the cone/cylinder 19.B expected); FTC 10's and STC 10's narrowed retries refuse at a face paired with itself (a plane, a sphere); FTC 07 AP242's at cylinder `79b561f3` / cone `178d3260` (OVERLAP). Question 931's trigger fires: FTC 10's torus/cylinder near boxes overlap. Every near box there is nearly the rung's box, because it grows by the global `reach` (the diagonal of every deleted face, about 0.18 m on FTC 10). Cuts of FTC 07's pairs were kept for 20.B.
- **A rung's cap bounds its sum.** `Meter::par_map`'s ordered sum (see `kernel-work.md`) gives a rung's table, classification and split at most `RUNG` between them; every rung build of the census stays within `RUNG` plus a charge. STC 07 #11020: past the deadline on main (rung 2's first build 38.0M units, 96.5 s) and refused `Uncertain` in 105.6 s on the branch (16.78M in 41.5 s, then the narrowed retry's 16.78M in 58 s). linkrods #11: past the deadline on both (first build 26.0M in 117 s on main; 16.78M in 64.5 s on the branch, then the narrowed retry of its held NURBS pair, 11.3M in 52 s). One NURBS/torus pair costs about 4 µs a unit there, which the sum doesn't bound.
- **The narrowed build.** `Narrow` and `settle` replace 19's `Stops`: a rung whose first build refuses `Uncertain` at any table pair (`Rung::Uncertain`, held or not) is built again narrowed (`Build::Retry`) under its own `Scope::Rung` cap, unless no narrowing changes a working face (`Rung::Unchanged`). Its answer stands, except that a failed walk or a `Next` climbs (question 936), and every later rung is then built narrowed only (`Build::Only`; `climb`). The narrowings: period-aware turns and 19's iso and plane stops; 20.B adds near boxes. First builds are as before, bit for bit.
- **Period-aware turns.** `working::own_hull` reads a face's span in a periodic direction round the circle: each fin's span moved by whole periods to start within a period of the plain span's start, then the largest gap between them left out. Where that is no shorter than `own_span`'s min/max it is that, bit for bit. `delete_turn(…, circular)` uses it in narrowed builds only (question 929). FTC 07's run, whose pcurves straddle the period's seam (sides at u = 5.5065 and 0.7767), reads as [−0.7767, 0.7767], just under a quarter, not a whole turn.
- **Iso stops modulo the period.** Where a face straddles, `iso_stops` moves the seam's iso coordinate and each fin's by whole periods (none, one down or one up) to lie nearest the hull, then compares within the stop's margin as 19.A does (question 937). Where nothing straddles it is 19.A's to the bit.
- **Bars** (cox-worker-2, AMD Ryzen 7 9700X, release, 2026-11-15; branch `6abb8a5e` against main `22f60a8e`; one thread for the deletes, load 0.8 to 3.4 at their starts):

  | Bar | Result |
  |---|---|
  | Ordered sum | `work`'s tests on 1, 2 and 8 threads; `tests/work.rs`'s figures match main's |
  | STC 07 and linkrods | STC 07 refused in 105.6 s (met); linkrods past the deadline, 122 s (unmet, above) |
  | No delete slower | Committed: 7 defeatured, 2 refused on both, defeatured bodies within 5%. Pinned: main 18 defeatured, 8 refused, 2 past the deadline, 391 s; branch 18, 9, 1, 464 s. No body that defeatures moved; the refusals take about twice as long (FTC 07 rd 22.8 to 42.0 s, FTC 10 rb 15.6 to 31.8 s, STC 10 12.5 to 26.7 s) after their narrowed retries |
  | Goldens | every section unmoved |
  | Property suites | `edit_props` 40 built and 38 refused on both; `blend_props`, `boolean_props`, `general_props`, `heal_props`, `tolerant_boolean_props` pass |
  | Fuzzing | `edit` (ASan, cox-worker-4, Docker, 8 GB, 2 jobs), 30 min: 2,875 runs (about 1.6 a second), corpus 1 to 750 entries, no crash, timeout or OOM |

- **Testing.** `work`'s `tasks_share_what_is_left_of_a_cap_in_order`, `a_capped_map_answers_as_its_tasks_in_order_on_any_thread_count`, `a_rungs_pairs_spend_its_cap_between_them` and (with `work-stats`) `a_capped_map_past_its_room_is_noted`; `working`'s `a_straddling_quarter_reads_as_a_quarter_round_the_circle` and `a_face_that_doesnt_straddle_reads_as_before`; `held`'s `an_iso_stop_reads_a_straddling_face_modulo_the_period`; `retrim::tests`' `a_narrowed_rung_answers_unless_it_narrows_nothing`, `a_narrowed_ladder_climbs_and_stays_narrowed`, `a_retry_that_narrows_nothing_is_never_built` and `ftc_07s_run_is_a_quarter_round_the_circle`. `BENT_ROD_DELETED_HASH` and every golden unmoved.
- **Left for 20.B:** near boxes (tighter than the drafted rule's global `reach`), the twin and the `near_boxes` golden; what stops FTC 07 rd (rung 2, cylinder/NURBS with no shared edge), FTC 10 and STC 10 (a narrowed face paired with itself) and CTC 02 with PMI (rung 3, cylinders apart); `KNOWN_REFUSED`; linkrods' per-unit cost.

## Near boxes and the bars (decided in ryukei phase 20.B)

Design 669, plan 20.B (#652, #658).

- **Near boxes by the faces touched.** A narrowed build carries each working face over its near box only (`tweak::retrim::near`): its own box joined with the boxes of the changed faces it touches (shares a vertex with, or is), grown by those faces' reach alone (`retrim::reach_of`, `reach` over a subset: over every face it is `reach`, bit for bit), doubled at rung 1, inside the rung's box. A face touching no change keeps its own box and ten resolutions. Plane stops still divide within the rung's box. First builds are as before, bit for bit.
- **Not per gap (a departure from the plan's Decision 1, question 946).** The plan grew a near box by the reach of the gaps it touched (changed faces joined where they share a vertex). But `delete_faces_in` grows each edge-connected set of deleted faces in its own re-trim, so a delete's changed faces are always one gap and its reach is the body's: the near box was the rung's box. The coxswain chose the touched faces' reach (2026-11-15). Growing a face also by its kept neighbours' reach was tried for FTC 07 and only moved its clipped walk to the next face of the ring; it isn't in.
- **The twin** (`ryukei_testkit::near_boxes`): two holes drilled from a bar's ends, 2° apart, their axes 0.1 mm less than the radii's sum apart at the middle; both floors filleted and joined by a 3 mm channel, so the fillets and the channel are one set (reach 0.743 m). On main the delete refuses `Uncertain` (the carried walls cross at the middle; walls only touching there trace clean); on the branch each wall touches only its own fillet and the delete builds. 63,743 units, the same on one thread and eight; the `near_boxes` golden.
- **The tube contact** (see `kernel-intersections.md`): tried once a trace fails, and never inside a re-trim rung's first build (`Meter::first_build`, inherited by `par_map`'s tasks). A departure from the plan's Decision 2: tried in first builds it settled 19.A's bent rod (a cylinder continuing a torus) there, whose first build refuses today and whose narrowed retry builds it, and moved `held_seams`. Gated, nothing that builds moves; the bent rod's narrowed retry spends 4 more units on it (9,871,284), its result the same.
- **Overlapping stretches run together.** `boolean::table::together` also relates two carriers lying along each other over part of each (an end of each on the other, apart, their middle on both), so they share vertices and their pieces are one edge. Left unrelated, FTC 10's and STC 10's retries crossed a plane where two faces tangent along a line meet it, and the plane's split couldn't order the two stretches lying on each other (`Uncertain` at the plane paired with itself). Every golden is unmoved.
- **Census V** (cox-worker-2, 2026-11-15, branch `3b4b721e` with hooks never committed, one thread, load 1 to 2). Near boxes part the pairs the plan aimed at: FTC 07 rd's census-IV pair (cylinder `18dae4b7` / NURBS `33b57350`), FTC 10's and STC 10's torus/cylinder pairs at rung 2 (`a289199e`/`24401201`, `e550357e`/`872b2691`), FTC 07 AP242's rung-0 NURBS/cylinder (`783471c2`/`91346e59`) and CTC 02 with PMI's rung-3 cylinders (`a9b6e499`/`8031cb1f`); each has a committed cut that fails with the rung's box. "CTC 02 with PMI #295" is `AP203 with PMI/nist_ctc_02_asme1_ap203.stp`; the AP242 edition's #1341 builds but doesn't check clean, on main as on the branch.
- **What stops each body now** (recorded unmet; design 669's cases a plan can't settle):
  - FTC 10 rb: rungs 0 and 1 (first builds) and rung 2's narrowed retry walk `HealAmbiguous` at sphere `d0eb13c5`, a kept 1 mm ball slit by the curve where a carried 0.27 m NURBS face (`a497893e`) meets it, the curve ending inside the ball; the seeds keep a piece of each, three uses of one edge. A walk and seed rule, not a near box.
  - STC 10 #3044: rung 2's narrowed retry, past the plane, can't group two sphere faces holding a lone pole vertex (`split::group`, `f188a714`, `df1863ee`).
  - FTC 07 rd: rungs 0 and 1 narrowed walk clipped at cylinder `7ae04fcd`'s angular sides (20.A's turn, as in census IV); rung 2 clipped at faces of the grown ring touching no change (NURBS `4290b75c`), which a near box leaves at their own box; rung 3 refuses at torus `14131c92` / plane `9d1d527c`, both touching no change, overlapping.
  - FTC 07 AP242 #13541: the retry refuses at cylinder `79b561f3` / cone `178d3260`, overlapping with no shared edge, as in census IV.
  - CTC 02 with PMI #295: past its cylinders, the retry refuses at plane `74fca8b1` / NURBS `b150cb1a`, both touching no change, overlapping.
- **Bars** (cox-worker-2, AMD Ryzen 7 9700X, release, 2026-11-15; branch `86be98c8` and its successors against main `51555f3c`; one thread for the deletes, load 0.8 to 1.9 at their starts):

  | Bar | Result |
  |---|---|
  | Blend deletes, committed | 7 defeatured; FTC 07 refused `Uncertain` in 63.5 s, FTC 10 `HealAmbiguous` in 45.1 s, both inside 120 s; `KNOWN_REFUSED` and the ignored test kept with the recorded causes (unmet) |
  | Blend deletes, pinned | main and branch 18 defeatured, 9 refused, 1 past the deadline (470 s, 518 s); FTC 07 AP242, STC 10 and CTC 02 with PMI refuse at their next pairs (unmet, above); STC 07 refused in 106.6 s; linkrods past the deadline, 122 s (unmet, 20.A) |
  | No delete slower | no body that defeatures on main refused or slower: the short ones that read 1.25 to 1.42 times in the pinned run timed equal alternately, three runs each; the refusals take longer (FTC 07 rd 42.6 to 61.9 s, FTC 10 rb 32.4 to 45.0 s, FTC 07 AP242 36.4 to 50.3 s) |
  | Ordered sum | `work`'s tests on 1, 2 and 8 threads; `tests/work.rs` matches main but the bent rod's stopped delete, 4 units more (the tube contact in its narrowed retry, the result the same); the twin 63,743 units |
  | Goldens | every section of ryukei, ryucad, ryusketch, ryumate and ryustep unmoved; `near_boxes` new |
  | Property suites | `edit_props` 40 built and 38 refused, seed for seed as main; `blend_props`, `boolean_props`, `heal_props`, `tolerant_boolean_props`, `general_props`, `intersect_props`, `general_booleans`, `booleans`, `blends`, `retrim` pass |
  | Fuzzing | `edit` (cox-worker-4, Docker, 8 GB, 2 jobs), 30 min: 3,105 runs, corpus 790 entries, no crash, timeout or OOM |

- **Testing.** `near`'s `reach_of_every_face_is_the_bodys_reach`, `a_face_at_one_end_of_a_long_set_is_carried_past_that_end_only`, `a_face_beside_a_deleted_corner_is_carried_past_it_not_across_the_body`, `a_face_touching_no_gap_keeps_its_own_box` and `rung_one_doubles_a_near_box_inside_its_doubled_box`; `working`'s `a_cylinder_over_its_near_box_is_no_longer_than_the_box`; `retrim::tests`' `ftc_07s_carried_pair_is_apart_over_near_boxes`, `ftc_10s_tube_pair_is_apart_over_near_boxes`, `stc_10s_tube_pair_is_apart_over_near_boxes` and `ctc_02s_carried_cylinders_are_apart_over_near_boxes` (cuts `nist_*_near_*.body`); `ss_exact`'s `a_cylinder_continuing_a_torus_tube_has_a_closed_form` and `a_tube_contact_is_found_once_the_trace_fails`; `table`'s `stretches_overlapping_end_over_end_run_together`; `tests/retrim.rs`' `the_twin_is_drilled_and_filleted` and `the_twins_carried_pair_is_never_met`; `tests/work.rs`' `the_near_boxes_twin_is_bounded`; the `near_boxes` golden.
- **Limits** (bars unmet, #652 and #658 to a later stage): the five causes above; faces of a grown ring touching no change get no room in a near box; linkrods' per-unit cost (20.A).

## Pinned faces (decided in ryukei phase 21.A)

Design 683, plan 21.A (#652, #658).

- **Pinned faces.** In a narrowed build, a region face left as it is (`still`) that touches no change (`near::touched` empty) and stands for no other is pinned (`near::pinned`). It is built as its input face: the working body is built over the region less its pins (`Region::without`), so `work_body` copies each with the outside faces, keeping its input edges with them and with other pins, and `WorkBody::pinning` marks the copies. `WorkBody::pairs` meets each pin with every working face whose box meets its box, never with another pin or an outside face (question 982). `outside_seeds` leaves pins out; `face_seeds` takes each by rule 3, and a working face's edge with a pin doesn't count as frontier for its seeding. A pin counts as narrowing, so a retry that pins is built. First builds pin nothing (`near::pins`), though a `Retry` or `Only` build that succeeds today may now pin; that bodies keep their bits is shown by the evidence, not by the rule: every golden section unmoved (`near_boxes` and `held_seams` included) and the 18 pinned editions defeatured on both bit for bit the same. The tests force pins through `attempt_with`.
- **Pins never pinch.** A pin round a vertex where the region less the pins would pinch stays working, until none does (`region::pinches`, which `region::closed` now goes through, unchanged): the working body is a sheet, whose boundary can't touch itself at a vertex.
- **A wrong pin refuses.** A pin that must grow leaves a working face's piece with no partner, and the walk fails; the rung climbs. Never a wrong body (`a_pin_that_must_grow_fails_its_walk`).
- **Census VI** (cox-worker-2, 2026-11-16, main `843b1654` with hooks never committed, one thread, load 0.35):

  | Body | Rung | Faces | still / touched | Shared edges | Verdict |
  |---|---|---|---|---|---|
  | FTC 07 rd | 2 (Only) clip | NURBS `4290b75c` | still, none | – | pinnable |
  | FTC 07 rd | 3 (Only) `Uncertain` | torus `14131c92` / plane `9d1d527c` | both still, none | **none** (not the tangent shared edge design 683 expected) | pinnable |
  | CTC 02 with PMI #295 | 3 (Retry) `Uncertain` | plane `74fca8b1` (working) / NURBS `b150cb1a` (**outside** the 126-face region) | still, none / – | `d7f2245b`, tangent, held | pinnable: the plane alone, since a pin is never met with an outside face |

  Earlier rungs (FTC 07's `7ae04fcd` clips, CTC 02's `06252748` walks) touch the change and are not pinnable. The pairs' cuts are `tests/data/nist_{ftc_07,ctc_02}_pinned_{input,work}.body`.
- **The twin** (`ryukei_testkit::pinned_faces`): a block 0.2 × 0.1 × 0.05 with its +x top edge rounded 0.01 and a blind slot 0.02 × 0.02 along x, ending 0.02 short of the round; the slot's far end deleted (`Heal::Grow`) runs it out through the round and the +x side. Rungs 0 to 2 fail their walks, and rung 3's first build is `Uncertain` at the round (a cylinder, sharing held tangent edges). On main the narrowed retry refuses as well. On the branch it pins seven faces and builds, checking clean at the run-out volume; 112,546 units on one thread and eight; the `pinned_faces` golden.
- **What stops each body now** (recorded unmet, design 683's cases a plan can't settle; both moved to ryukei 22, "Working faces against pins and near-box limits"):
  - FTC 07 rd: rung 2's narrowed build pins 40-odd faces (`4290b75c` and `9d1d527c` among them), so the ring clip and rung 3's pair are gone. It now refuses `Uncertain` at rung 2, in 31 s (main 62 s), where a carried NURBS face touching a change (`33b57350`, change `374dbdc2`) meets a pinned thin plane (`f19ffa9e`, 0.15 mm tall) with no edge between them: a working face growing into a pin (ryukei 22). Letting a narrowed `Uncertain` at a pin's pair climb (tried, never committed) meets the same pair at rung 3.
  - CTC 02 with PMI #295: rung 3's narrowed retry pins 100-odd faces, `74fca8b1` among them, so its pair is never met. The walk then clips working plane `1294043e`, which touches the change, at its near box: `GapNotHealed` in 11 s (main `Uncertain` in 27 s). That is a near-box limit, not a pin's (ryukei 22).
- **Bars** (cox-worker-2, AMD Ryzen 7 9700X, release, 2026-11-16; branch `613cdbea` and its successor against main `843b1654`; one thread for the deletes, load 0.6 to 5 at their starts):

  | Bar | Result |
  |---|---|
  | Blend deletes, committed | 7 defeatured on both; FTC 07 rd refused `Uncertain` in 31.2 s (main 62.3 s) and FTC 10 `HealAmbiguous` in 26.4 s (main 45.7 s), both inside 120 s; FTC 07 stays in `KNOWN_REFUSED` with its next cause (unmet) |
  | Blend deletes, pinned | branch 18 defeatured, 9 refused, 1 past the deadline in 383 s; main 18, 8, 2 in 528 s. CTC 02 with PMI not in `PINNED_DEFEATURED` (unmet, above). STC 07 #11020 now refuses in 63.7 s (main past the deadline); linkrods past it on both (123 s, 122 s) |
  | No delete slower | the 18 bodies defeatured on both are bit for bit the same (FNV of `dump_unnamed`); none slower by more than 3% (the largest, CTC 05 AP203: 2.71 s against 2.64 s), most faster from a quieter start; every refusal as fast or faster |
  | Ordered sum | `work::` unit tests on 1, 2 and 8 threads pass; every figure of `tests/work.rs` is main's to the unit; the twin 112,546 units |
  | Goldens | every section of ryukei, ryucad, ryusketch, ryumate and ryustep unmoved; `pinned_faces` new |
  | Property suites | `edit_props` 40 built and 38 refused, seed for seed as main; `blend_props`, `boolean_props`, `general_props`, `heal_props`, `tolerant_boolean_props`, `intersect_props`, `retrim`, `blends` and `booleans` pass |
  | Fuzzing | `edit`, `boolean` and `tweak` through the repo's `fuzz.yml` (CI run 43902003233, 10 minutes each on GitHub's runners, as the workflow sets them; cox-worker-2 has no Docker for the plan's 30 minutes): no crash, timeout or OOM |

- **Testing.** `region`'s `a_region_without_its_pins_has_its_frontier_round_them`; `near`'s `ring_faces_touching_no_change_are_pinned` and `first_builds_pin_nothing`; `body`'s `pinned_faces_are_copied_whole_and_met_only_by_working_faces`; `retrim::tests`' `a_raised_blocks_bottom_is_pinned_and_it_builds_the_taller_box`, `a_pin_that_must_grow_fails_its_walk`, `ftc_07s_untouched_pair_is_pinned` and `ctc_02s_untouched_pair_is_pinned` (cuts `nist_*_pinned_*.body`; FTC 07's pair shares no input edge, CTC 02's keeps its one); `tests/retrim.rs`' `the_pinned_twin_is_rounded_and_slotted` and `the_pinned_twins_slot_runs_out`; `tests/work.rs`' `the_pinned_faces_twin_is_bounded`; the `pinned_faces` golden.
- **Limits** (bars unmet, #652 and #658 to ryukei 22, "Working faces against pins and near-box limits"): a working face touching a change that grows into a pin it shares no edge with (FTC 07 rd: working NURBS `33b57350` growing into pin `f19ffa9e`); a working face touching the change clipped at its near box in a pinned retry (CTC 02 with PMI: `1294043e` clipped at its near box in the pinned retry). Left for 21.B to 21.D: FTC 07 AP242's cylinder/cone, FTC 10's slit, STC 10's poles.

## Certified misses (decided in ryukei phase 21.B)

Design 683, plan 21.B (#658).

- **The certified miss.** Sometimes every trace of two faces fails `Uncertain` in `intersect::face::face_face_over`: the first trace, then `faces_apart`, `along_rims`, the carried surfaces and the region grown by a hair. If the faces share no edge and no vertex, `keeps_off` then tries `beside::face_keeps_off` for each face against the other's surface, under the check's ladder (`1 << 9`, `1 << 12`, `CELLS`). A face keeps off when it takes one sign over every part not certified outside it, or when every part is certified apart from a surface with no implicit form. Then the faces don't meet, and the pair is apart. Otherwise the pair refuses `Uncertain` as before. `face_keeps_off` is `face_beside` with no rails, no vertices and a strip of 0, and it stops once both signs turn up. Parts beyond a plane stop or a near box lie outside the face's loops, so `outside` leaves them out. No tolerance rule is involved: the certificate is an exact sign over certified enclosures.
- **Where, and a departure from design 683.** Design 683 put the certificate in `ss::meet`, which sees only surfaces. The certificate needs the faces, so it sits at `face_face_over`'s last refusal, after the tube contact has failed inside `ss::meet`. A shared vertex also rules it out (`sharing`), which narrows design 683's "no shared edge": the vertex lies on both surfaces.
- **The gate.** `may_miss` keeps the certificate out of every re-trim rung's first build (`Meter::in_first_build`, the tube contact's gate), so every first build keeps its answer. Outside re-trims, it is tried wherever a trace fails at such a pair, booleans and the check included (question 1021).
- **Census VI** (cox-worker-2, 2026-11-16, main `b17e2c98`, one thread, load 0.2). FTC 07 AP242 #13541 stops where 20.B left it: rung 0's narrowed retry refuses `Uncertain` at working cylinder `79b561f3` (r 1.524 mm, axis +z) and working cone `178d3260` (r 12.7 mm, 1°, axis −y), which share no edge or vertex. It refuses in 36.1 s. The axes are perpendicular, 11.2 mm apart, so the pin grazes the bore's wall. On 1024² samples the least gaps are 3.1e-9 m (cylinder to cone) and 3.4e-7 m, against tolerances of 1e-8, and each face takes both signs in the other's implicit form. `face_keeps_off` certifies that each face crosses the other's surface (`Ok([true, true])`) at every limit, in 12 and 51 units, so no certified miss holds. The cut is `tests/data/nist_ftc_07_ap242_miss_work.body`.
- **What stops each body now:** FTC 07 AP242 #13541 stops at the same pair in 36.2 s (unmet, below). FTC 07 rd is unchanged at 21.A's rung-2 pair (31.3 s, as on main). The certificate is tried there and on the bent rod's torus/cylinder pair (`tests/work.rs`'s stopped delete: +6 units, not certified, the same body).
- **Bars** (cox-worker-2, AMD Ryzen 7 9700X, release, 2026-11-16; branch `99cce278` against main `b17e2c98`; one thread for the deletes; load 1.2 to 2.1 at their starts):

  | Bar | Result |
  |---|---|
  | Blend deletes, committed | 7 defeatured and 2 refused (FTC 07 rd 31.3 s, FTC 10 rb 26.5 s) on both, inside 120 s; `KNOWN_REFUSED` unchanged |
  | Blend deletes, pinned | 18 defeatured, 9 refused and 1 past the deadline on both (361.8 s branch, 361.3 s main). FTC 07 AP242 isn't in `PINNED_DEFEATURED` (unmet, below) |
  | No delete slower | every outcome is the same, and the 7 and 18 bodies defeatured are bit for bit main's (FNV of `dump_unnamed`). The committed bodies' times sum to 67.8 s against 67.9 s, the pinned to 353.6 s against 353.2 s. The one body reading over 10% slower (FTC 06 AP242 #2155, 0.43 s against 0.38 s) deletes in 0.200 s on both, three times each, alternately |
  | Ordered sum | `work::` passes on 1, 2 and 8 threads. Every `tests/work.rs` figure is main's but `delete (stopped)`: 9,871,290 units against 9,871,284 (the torus/cylinder pair above) |
  | Goldens | every section of ryukei, ryucad, ryusketch, ryumate and ryustep unmoved; no new section |
  | Property suites | `edit_props` 40 built and 38 refused, seed for seed as main; `blend_props`, `boolean_props`, `general_props`, `heal_props`, `tolerant_boolean_props`, `intersect_props`, `retrim`, `blends`, `booleans` and ryustep's `checks` pass |
  | Fuzzing | `edit`, `boolean`, `tweak`, `intersect` and `general` through the repo's `fuzz.yml` (CI run 43939007606, 10 minutes each on GitHub's runners): no crash, timeout or OOM. `boolean`'s first attempt lost its runner during Build, before fuzzing, and its rerun is green |

- **Testing.** `face`'s `a_cone_kept_off_a_cylinder_keeps_off_it`, `a_cone_crossing_a_cylinder_doesnt_keep_off_it`, `a_cone_kept_off_a_cylinder_is_a_certified_miss`, `a_miss_is_tried_only_for_faces_sharing_nothing_outside_first_builds`, `ftc_07_ap242s_cylinder_and_cone_still_refuse` and `ftc_07_ap242s_pair_is_the_same_both_ways_and_inside_a_rungs_cap` (cut `nist_ftc_07_ap242_miss_work.body`, 4,311 units in a rung).
- **Limits** (unmet, moved to **ryukei 22**): FTC 07 AP242 #13541's cylinder `79b561f3` and cone `178d3260` cross nearly tangent inside both near boxes. Each is certified to cross the other's surface, so no keep-off certificate holds, and the pair needs an exact cylinder/cone intersection (#658). Left for 21.C and 21.D: FTC 10's slit and STC 10's poles.
