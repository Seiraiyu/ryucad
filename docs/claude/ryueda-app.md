## The window (decided in phase 3.D)

The design is document 519 (phases 4 and 5) in the Coxswain planning project `ryucad-ryukei`.

- **Its own crate.** `crates/ryueda-app` is ryueda's window, on the shared shell `ryu-app`. It depends on `ryueda` (for `proto`, `client` and `Eda`), never on `ryucad` or `ryukei`, and on `ryusch` only as a dev-dependency: the window tests share ryueda's breakout fixture, whose `tests/support/draw.rs` measures pin names with the engine's font (`tests/deps.rs` keeps it out of `[dependencies]`). `core` is the app without a window, tested headless. `ui` is the window, a thin egui view over it.
- **It draws the daemon's scene and nothing else.**
  - `core::scene` reads `scene`'s answer in millimetres (nanometres over 10⁶, the unit `ryu_app::canvas` takes).
  - `core::geom` cuts curves once per scene and half-octave zoom band, and culls by item boxes.
  - Text is fitted to the width SVG stretches it to, within a quarter of its size.
  - A scene that can't be drawn again keeps the last one, greyed, with the error over it.
  - Cut outlines are kept by design, block, revision, sheet, layer, theme and zoom band: sheet ids repeat across blocks (`s1`), so the block is part of the key.
- **Picking** is on the CPU over the scene, within 6 screen points, frontmost first: pin, label, port or rail, component body, net wire or junction (and bus), block symbol, region frame or title tab, note. A pin is in front only within a third of the reach of its line (2 points); farther off it competes with the part's body by distance, so a small part's body can be clicked between its pins (the visual review). Alt takes a pin's net. A filled shape is hit inside; a region's inside is not.
- **Marks.** In order of precedence: this window's selection, hover, others' selections in their colours, highlights in their style's colour edged in their participant's, then changed flashes (2 s, fading; `ui.flash_changes`; another sheet, layer or theme starts afresh, since a theme recolours every owner). A mark recolours strokes and text; a body's fill takes a third of the mark's colour, so pin names stay readable. The hover tooltip is drawn at the pointer. A selected net paints its labels. A selected pin is ringed.
- **Selection and view** go to the daemon at most every 150 ms, the last value winning. A window adopts its own participant's view when it changes to something it didn't send: its own Go to, always; another's `view.show`, with `ui.follow_shows` (on by default). It adopts only a notification that moved its view, and remembers its last eight `view.set`s (cleared when it sends its own Go to): a view marked its own that is one of them is an echo, never adopted, even arriving after a newer send went (adopting one took the session's selection before the window's latest pick reached it, and emptied Properties). So the same show again (Go to R1, pan, Go to R1; R1, R2, R1) is adopted each time, and a Go to's own answer selects its reference, so Go to R1, pick another, Go to R1 (nothing moves) selects R1 again. An agent's repeated show that moves nothing leaves the window's selection alone. Follow (the People strip) adopts another participant's sheet and camera as they move.
- **Rescenes.** A scene is asked again only when `project.changed` names one of its `revisions`. The shown block's other sheets are asked on the link's aside connection, for the navigator's regions and a net's reach.
- **Lists.** Each is read from answers that already exist:
  - the navigator: `design.list`, `netlist`, `libcomp.list`, `block.show`;
  - properties: `component.show`, `libcomp.show`, `net.show`;
  - findings: the scene's lint, `lint.get`'s rules, `erc`, `check {offline}`;
  - instructions: `layout.instructions`, whose entries name their `refs` (plan 3.D): a net instruction's net, else the parts of every subject (`Instruction::subjects`: its parts and what it is placed relative to), sorted, each once;
  - activity: `activity.log`.
  A list's rows each have a Show button, disabled when the row names nothing, so the buttons line up with the rows. ERC's references (`board:s1/u1`) map to the block's (`sensor/u1`) through the netlist's occurrences. Undo and Redo name the newest entry that can be, whoever made it.
- **Nets by name.** The navigator lists nets by name (and rail). `block.show` has no class, and adding one would move its goldens, so the class is in Properties.
- **Tiles.** Navigator (left), Sheet (centre), Properties (right), and Findings, Instructions and Activity as tabs below. Every tile has its tab (`all_panes_must_have_tabs`, as ryucad-app's), which names it for screen readers and the tests.
- **Preferences.** ryueda's schema takes the window's keys: `ui.theme`, `ui.style`, `ui.contrast`, `ui.layout`, `ui.keys`, `ui.notify`, `ui.flash_changes`, `ui.follow_shows`, `app.window`. They live in the user's preferences file, never the project's.
- **`ryueda open [DIR]`** starts the app: `RYUEDA_APP`, else `ryueda-app` beside `ryueda` or on `PATH`; `app_not_installed` otherwise. It doesn't bring an open window to the front (ryucad's `app.open`); a second `open` is a second participant.
- **Shipping.** ryueda's archives carry `ryueda-app` (a glibc build on Linux), the fonts' licences, and `share/` with `ryueda.desktop`, `ryueda.xml` and `ryueda.applescript`. `install-ryueda.sh` and `.ps1` install it unless `RYUEDA_NO_APP=1`, and register a desktop entry and MIME type (Linux), `~/Applications/ryueda.app` (macOS) or "Open in ryueda" (Windows). `ryueda-app --version` prints the CLI's envelope. `--smoke [DIR]` draws one frame offscreen through egui's wgpu renderer.
- **Testing.**
  - Unit tests in `core`.
  - `tests/window.rs` against real daemons: navigator, pick read by the agent, an agent's show, a finding's Show, Undo.
  - `tests/snapshot.rs`: the canvas in both themes within 24 a channel on 1% of pixels, lavapipe's (`UPDATE_SNAPSHOTS=1`), and every control named.
  - `tests/cli.rs`.
  - `tests/frames.rs`: the bars, ignored.
  - The tests run `target/<profile>/ryueda`, built if stale, or as given (`RYUEDA_TEST_PREBUILT`). They skip without an adapter unless `RYUEDA_REQUIRE_GPU` or `RYUCAD_REQUIRE_GPU` is set. They draw one at a time (lavapipe crashes on parallel draws): a lock in one process, and nextest's `window` group (`.config/nextest.toml`) across processes. CI runs the window tests in a job of their own beside ryucad-app's.
- **Bars.**
  - On cox-worker-4's devcontainer (AMD Opteron 6380, 8 threads, 16 GB, Linux, load about 19 from other members' work), release, `RYUEDA_BARS=1`: the RP2040's largest sheet's scene through a warm daemon in 130.7 ms, kept in 42.0 ms (bar 50 ms; 3.B's 30.8 ms was cox-worker-3), and a 500-part block's sheet drawn again 29.1 s after a value edit (bar 150 ms, unmet until #802; 3.B's 3.67 s was cox-worker-3).
  - On cox-worker-3 (Intel Core Ultra 5 250K Plus, 18 threads, 30 GB, Linux, GNOME, NVIDIA GeForce RTX 5060 on Vulkan; load about 2, nothing else running), release, `RYUEDA_BARS=1`:
    - the RP2040's largest sheet's scene (`s1`, 178 owners) through a warm daemon in 31.4 ms, kept in 10.1 ms (bar 50 ms);
    - a 500-part block's sheet (177 owners) drawn again 3.37 s after a value edit (bar 150 ms): unmet until #802, the test stays ignored; the 500 parts lay out in 3.72 s;
    - a 500-part sheet panned and zoomed at a median 8.9 ms a frame (113 frames a second), worst 38.1 ms (bar 60 frames a second);
    - a pick seen by another window in 6.5 ms and by the agent in 4.8 ms (bar 200 ms);
    - ryueda-app 35,725,960 bytes against ryucad-app's 50,877,176 (bar: no larger);
    - a drawn sheet 118 ms after start, with the daemon already warm from the tests before it, as when a person opens a second window (bar 2 s).
  - The W4 scene time is that older, busier machine's: the same test on cox-worker-3 meets its bar.
- **Commands.**
  - `cargo run -p ryueda-app -- <project folder>` (`cargo build -p ryueda` first; it needs `ryueda` beside it).
  - `RYUEDA_REQUIRE_GPU=1 cargo test -p ryueda-app -- --test-threads=1`.
  - `UPDATE_SNAPSHOTS=1 cargo test -p ryueda-app --test snapshot`.
  - `RYUEDA_BARS=1 cargo test -p ryueda-app --release --test frames -- --ignored --nocapture` (hardware adapters).
  - `sh scripts/smoke-ryueda.sh target/release/ryueda "" target/release/ryueda-app`.

## The compare view (decided in ryueda 13.A)

The design is document 688 in the Coxswain planning project `ryucad-ryukei`; these are its phases 2 and 3, plan 13.A (document 690).

- **Whose compare.** The window's compare is its participant's: `compare.set` from an agent (`ryueda compare HEAD`) or from the window's own View menu and History tab. The window follows it from `participant.changed` (`SheetView::compare`). Which side is drawn (`core::compare::Blink`) is the window's own view state, never sent: Both (the compare scene), Working (the plain sheet) or the revision's sheet alone (`scene --rev`, read only). A new compare, or none, starts from Both; `B` flips between the two lone sides. While comparing, the window reads `compare.get` every 5 s and when it regains focus, since a symbolic revision moves with no file changing; a moved commit draws again. After a reconnect the window tells the new session its compare again.
- **What is drawn.** Everything comes from the daemon's scene:
  - Compare colours are the scene's `paper.diff`, so a style recolours them.
  - Ghosts are scene items drawn first, owned by no one, each naming its ghost (`Item::ghost`).
  - Leaders are their part's, drawn last, in `diff_changed`.
  - Marks in order, lowest first: the changed halo, compare colours, others' highlights, others' selections, hover, the window's selection.

  The canvas's cut is kept by the compare's and the revision's commits too.
- **Picks.** Where nothing working is nearer, a ghost is picked (`pick::pick_ghost`). It and any pick on a revision's sheet are described in Properties ("removed in working", "at <rev>, read only") and copied by Copy reference, never sent as a selection: the reference may name nothing in the working files.
- **The navigator.** It marks each changed sheet and block with a dot in `diff_changed` and the counts on hover. It lists the sheets only the revision has, struck through, opening as the revision drew them. A revision's scene is a blink: it moves neither the navigator's sheets nor ends the compare; a plain scene of a block does.
- **Changes and History tabs.**
  - Changes reads `diff --layout` and `diff --netlist` from the compared revision, one read at a time, again when the compare or the project moves. It lists rows by block (verb, reference, change lines) and an Electrical section. Show selects a working entry, or frames a removed one's ghost on the sheet its layout entry names (as the revision drew it, when only the revision has the sheet). ERC rows have no Show: `diff --netlist`'s findings carry no references.
  - History lists `history.log`'s commits (short id, author, UTC date, subject) each with Compare, and how many working files differ. Outside a repository it says the history is Git's.
  - Properties lists a selected reference's changes ("Changes since <rev>").
- **Commands.** View > Compare with HEAD, Compare with… (a revision typed, checked by the daemon, a refusal toasting), Stop comparing, Flip compared sides (`B`), all in the palette. The compare bar over the sheet shows `Against <rev> (<short>)`, the counts in their colours, Both / Working / `<rev>`, "read only" on the revision's side, Stop, and why the revision's block doesn't lay out when it doesn't.
- **The changed flash is a halo (#879).** A changed owner's lines are widened by 3 points a side under it, in `diff_changed` (or ryu-session's changed colour without `paper.diff`), fading over 2 s. It never recolours, so it never reads as a selection, and it shows on white paper.
- **Marks held to the paper (#879).** Every mark colour from `ryu-app` or `ryu-session` goes through `core::paint::on_paper`: under WCAG's 3:1 against the scene's paper, it is mixed toward black (light paper) or white (dark) in 5 % steps until it passes. The selection takes 4.5:1 and the hover 7.5:1 (`on_paper_at`), so they stay apart from each other and from the participants' colours for colour-blind eyes. `ryu-app`'s constants never change, so no ryucad picture moves.
- **Colour isn't the only cue.** The vision test (`ryu_app::style::vision`, CIEDE2000 ≥ 10 under each vision, white and dark paper) holds:
  - the selection and the hover against everything, but one pair: the selection vs ryu-session's participant 0 under protanopia on white (3.8);
  - the diff roles among themselves under normal vision.

  Red against green and amber under colour-blind vision, and the diff roles against ryu-session's shared participant palette, fall under 10. Every pair is listed with its ΔE beside the test and in #921, a design call for later (a colour-blind-safe diff palette moves every scene, SVG and PDF golden; the participant palette is ryucad's too). So diff marks are told apart by shape as well as colour: ghosts (removed) are dashed and unfilled, leaders (moved) dashed lines from where a part stood, added and changed owners solid. Colour alone isn't the cue for colour-blind readers.
- **The tests' folder is a repository of its own (#920).** `ryueda init` sets up the repository a project is made in. The window tests' folder, under the target directory, sat inside the clone, so every run wrote the clone's shared hooks and drivers naming the test build's binary. `common::shared` runs `git init` in it first.
- **Testing.**
  - Unit tests in `core`: `compare`, `scene`'s compare fixture, `paint`, `pick`, `nav`, `changes`, `history`, `window`'s requests.
  - `tests/compare.rs` against real daemons over the breakout committed and edited (`common::breakout_in_git`): an agent's compare, History's Compare, `B`, a removed component's Show, a reconnect, Properties' changes.
  - `tests/snapshot.rs` adds `compare-light`, `compare-dark` and `halo-light`, and the compare bar's names.
  - The 13.A pictures were made with cox-worker-3's own lavapipe (Mesa 26.0.8), which reproduces the devcontainer's `canvas-*` within the tolerance.
- **Bars.** On cox-worker-3 (as 3.D's; load under 4, nothing else running), release, `RYUEDA_BARS=1`:
  - the RP2040's largest sheet compared with `HEAD` through a warm daemon in 31.1 ms with the revision's lay-out kept (bar 100 ms), 11.8 ms kept whole (bar 50 ms);
  - `diff --layout` 18.3 ms and `diff --netlist` 173.6 ms over 10,000 occurrences (bar 2 s each);
  - the 500-part sheet with every owner marked and 100 ghosts at a median 8.5 ms a frame (118 frames a second; bar 60);
  - a 500-part block's compare sheet drawn again 3.44 s after a value edit (bar 150 ms): unmet until #802, ignored as 3.D's.
