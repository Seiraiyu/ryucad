## The window (decided in phase 3.D)

The design is document 519 (phases 4 and 5) in the Coxswain planning project `ryucad-ryukei`.

- **Its own crate.** `crates/ryueda-app` is ryueda's window, on the shared shell `ryu-app`. It depends on `ryueda` (for `proto`, `client` and `Eda`), never on `ryucad` or `ryukei`, and on `ryusch` only as a dev-dependency: the window tests share ryueda's breakout fixture, whose `tests/support/draw.rs` measures pin names with the engine's font (`tests/deps.rs` keeps it out of `[dependencies]`). `core` is the app without a window, tested headless. `ui` is the window, a thin egui view over it.
- **It draws the daemon's scene and nothing else.**
  - `core::scene` reads `scene`'s answer in millimetres (nanometres over 10⁶, the unit `ryu_app::canvas` takes).
  - `core::geom` cuts curves once per scene and half-octave zoom band, and culls by item boxes.
  - Text is fitted to the width SVG stretches it to, within a quarter of its size.
  - A scene that can't be drawn again keeps the last one, greyed, with the error over it.
  - Cut outlines are kept by design, block, revision, sheet, layer, theme and zoom band: sheet ids repeat across blocks (`s1`), so the block is part of the key.
- **Picking** is on the CPU over the scene, within 6 screen points, frontmost first: pin, label, port or rail, component body, net wire or junction (and bus), block symbol, region frame or title tab, note. Alt takes a pin's net. A filled shape is hit inside; a region's inside is not.
- **Marks.** In order of precedence: this window's selection, hover, others' selections in their colours, highlights in their style's colour edged in their participant's, then changed flashes (2 s, fading; `ui.flash_changes`). A selected net paints its labels. A selected pin is ringed.
- **Selection and view** go to the daemon at most every 150 ms, the last value winning. A window adopts its own participant's view when it changes to something it didn't send: its own Go to, always; another's `view.show`, with `ui.follow_shows` (on by default). Follow (the People strip) adopts another participant's sheet and camera as they move.
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
