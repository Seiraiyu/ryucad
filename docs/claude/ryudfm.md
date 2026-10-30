## Board model and shapes (decided in ryudfm phase 0)

The design is document 213 in the Coxswain planning project `ryucad-ryukei`; this is its plan 0.A.

- **Crate.** `ryu-board` is product-neutral. It depends on `ryu-math` and `rayon` only, and `tests/deps.rs` holds it to that. It knows no ODB++, project or session, and ryueda and ryudfm both build on it. Its `testing` feature holds a seeded generator, golden hashes, seeded symbols and features, and checks on shapes.
- **Numbers.**
  - `geom::Nm` is `i64` nanometres within ±2^40 (`COORD_LIMIT`). `Mdeg` is integer millidegrees in `[0, 360000)`, counterclockwise, with y up.
  - `geom::decimal` reads decimal text exactly, with no floating point (`nm`, `mdeg`, `millionths`). Finer digits round half to even, and `Exact` says by how much.
  - An odd size puts its extra nanometre on the positive side (`span`). A radius from an odd diameter rounds half to even (`half`).
- **Arcs.**
  - Arcs are centre form. The radius is the centre's distance to `start`, and the arc ends where the ray through `end` meets that circle. An end in `start`'s direction makes a full circle.
  - `Arc::through` makes an arc from three points (ryueda's form), with its centre rounded.
  - An arc's box includes the axis extremes it sweeps, decided exactly in `i128`.
- **Placement.**
  - `Place` scales (in millionths), mirrors x, turns counterclockwise and then moves, in that order. Right angles are exact. Other angles go through `ryu_math::sin_cos`, rounding each coordinate half to even.
  - ODB++ turns clockwise and then mirrors x. That is the same as mirroring and then turning counterclockwise by the same angle. So an `orient_def` angle `a` is `Mdeg::new(a)` when mirrored and `Mdeg::from_clockwise(a)` when not.
- **Symbols.**
  - `StandardSymbol` covers every ODB++ 8.1 standard symbol design 213 lists. It adds `sr_ths` and `oblong_ths`, which the specification also has, and ryueda's trapezoid.
  - ryueda's `roundrect` and `chamfered` pads are rectangles with `Corners`. Rounded and chamfered corners may mix on one pad, as KiCad's may.
  - `StandardSymbol::parse` reads mils from inch files and microns from millimetre files. Only rectangles take `xc`, because the specification defines no chamfered donut or thermal.
  - The 8.x stencil symbols (`hplate`, `rhplate`, `fhplate`, `radhplate`, `dshape`, `cross`, `dogbone`, `dpack`) aren't read. Their geometry is given only in figures, and `dpack`'s formula is printed garbled. A name that isn't read is no symbol, so the reader reports it as unknown.
  - Where the specification's text is silent, the geometry follows its Appendix A figures. Butterflies fill quadrants 2 and 4. The half oval is flat on the left. The triangle's apex points up, centred in its box. `s_thr` is the line thermal. Rounded thermals round only their outside.
- **Shapes.**
  - `shape::Shape` holds parts in drawing order, and parts are never merged. Each part has a polarity and is one of three things:
    - a `Region`;
    - a `Stroke`: a path and a round or square pen, kept where the outline isn't a simple region (a pen at least as wide as the radius, ends that reach each other, a square pen on an arc);
    - a `Swept` path: a line drawn with a symbol that isn't round or square.
  - Rings keep their width round corners: a rounded donut's inside is rounded by what is left of the outer radius.
  - The ellipse is made of arcs, each through three points of it, within 10 nm (`approximate`). These are not true biarcs; the tangent mismatch is inside the same bound.
- **Thermals.** Gaps are channels `gap` wide, centred on rays at `angle + k × 360°/spokes` from +x.
  - Squared pieces end on the channels' sides.
  - Rounded pieces are the ring's middle line drawn with a round pen as wide as the ring, ending where the pen clears the channel.
  - Open-cornered thermals (`s_tho`, `rc_tho`) take 1, 2, 4 or 8 gaps at multiples of 45°. They cut the sides square, half the gap either side of a side's middle. At a corner, the ring's corner square is left open, or half the gap from the corner if that is more.
  - Not confirmed by the specification's text, only by its figures: the rounded ends, the open corners, and the moiré's layout (the outermost ring reaches half the line length, and each next ring is `ring_width + ring_gap` further in).
- **Text.**
  - A text's shape is its font's strokes. Each character is scaled from the font's box to the character size. The `k`th character is moved along by `k × (font width + offset)`, scaled the same way.
  - Strokes are drawn with the text's line width and placed by its placement. The reader works out the line width: ODB++'s width factor is in units of 12 mils.
  - Missing characters are gaps, and they are listed.
  - How ODB++ fonts anchor their glyphs is checked against real boards in 0.B.
- **The model.**
  - `model::Board` holds the matrix's layers and the steps. Each step holds each layer's `Features`: its symbol table and its features as written, by index.
  - It also holds the `Library` of user symbols and fonts, the drills, the stackup, the capabilities, and `Notice`s: faults and warnings, coded by whoever produced the board.
  - User symbol cycles are found when the library is made. They fail the features that use them (`ShapeError::SymbolCycle`, naming the chain).
  - References are `<step>/<layer>/<index>`, `<step>/comp/<refdes>` and `<step>/net/<name>`.
- **Lazy and parallel.**
  - Each feature's shape, and each table symbol's shape, is built on first use and kept in a `OnceLock` in its table.
  - `Board::layer_shapes` builds one layer's shapes in parallel. `build_shapes` builds several layers side by side.
  - Shapes are the same bits on any number of threads.
- **Testing.**
  - Each module has unit tests.
  - `tests/props.rs` checks 2,000 seeded symbols and 3,000 seeded features: closed, simple, counterclockwise outside, inside their boxes, and alike on one thread and eight.
  - `tests/determinism.rs` checks `tests/golden/board.txt` in the `cpu-levels` job. Regenerate it with `UPDATE_GOLDEN=1 cargo test -p ryu-board --test determinism`.
  - `cargo bench -p ryu-board` times shapes of seeded symbols, text and a seeded layer.

## ODB++ reader and corpus (decided in ryudfm phase 0)

The design is document 213 in the Coxswain planning project `ryucad-ryukei`; this is its plan 0.B.

- **Crate.** `ryudfm` is a library in this plan: `ryudfm::odb` reads a job into `ryu_board::model::Board` (`read_path`, `read`, `ReadOptions { step, limits }`), and its `testing` feature holds the test writer, broken jobs, the corpus manifest and KiCad comparisons. The binary, project and daemon are 0.C's. It depends on `ryu-board`, `rayon`, `flate2` (miniz_oxide), `tar`, `zip` (flate2's deflate only) and `roxmltree`: no C, no kernel, no other product (`tests/deps.rs`).
- **Codes.** A job that can't be read is an `odb::Error` with a `Code` (`not_a_board`, `odb_unreadable`, `odb_version_unsupported`, `odb_step_ambiguous`, `odb_step_not_found`, `archive_unsafe`), a message and where in the job; 0.C adds them to `ryu_doc::error::Code` under the same names. What fails part of a board is a fault on it (`odb_layer_unreadable`, `odb_symbol_unknown`, `odb_symbol_cycle`, `odb_component_invalid`, `odb_eda_invalid`, and `odb_file_invalid` for any other file: a step header, profile, attribute list, tools file, netlist, font, stackup or user symbol). Warnings come once per file and kind, with a count (`odb_ignored`, `odb_rounded`, `odb_arc_inconsistent`, `odb_font_missing_char`, `odb_text_dynamic`, `odb_barcode_skipped`, `odb_info_missing`).
- **Jobs.**
  - A job is the folder holding `matrix/matrix`, at a container's root or under its one top folder, in a folder, tar, gzipped tar or zip (told apart by their first bytes).
  - Paths match in any case. `.Z` (Unix compress, decoded by `odb::lzw` as ncompress 5.0 writes it, groups of eight codes and all) and `.gz` files unpack when read.
  - Refused as `archive_unsafe`: absolute paths, `..`, links and devices (a folder's links too, but not the path given), more than a million entries or 8 GiB.
  - A singly compressed file past 2 GiB fails that file alone.
- **Names and numbers.**
  - Step, layer, symbol and font names are lower case in the model. Writers that put capitals in the matrix (Altium, KiCad) name folders in lower case.
  - Every number is read from its text in its own file's units. A symbol's `I` or `M` overrides them.
  - Each file's rounded numbers become one `odb_rounded`, with the largest change.
- **Attributes.**
  - A value means what its declaration says: the specification's 373 system attributes (`odb/sysattr.rs`, generated from its Appendix B list), or the job's own `misc/sysattr*` and `misc/userattr`, which win.
  - Undeclared attributes, and values a declaration doesn't allow, are left out with `odb_ignored`.
  - In attribute lists, booleans are `yes`/`no`, options are written by name, and texts lose one pair of enclosing quotes.
- **The matrix.**
  - Layers are in row order.
  - A board layer's side follows where its row lies against the copper rows: the first copper row and those above it are the top, the last copper row and those below it the bottom, and the rest inner.
  - An empty `START_NAME` or `END_NAME` is the first or last copper layer, not the first board layer the specification names, so a through drill classes as through.
- **Records.**
  - Pads, text and step repeats turn clockwise and then mirror. A mirrored angle `a` is `Mdeg::new(a)`, an unmirrored one `Mdeg::from_clockwise(a)`.
  - Polygons are turned to the model's orientation (`odb::outline`): islands counterclockwise, holes clockwise.
  - A text's width factor is in 12-mil units whatever the file's units.
  - Not taken, each with `odb_ignored`:
    - pad resize factors (`-1 sym resize`);
    - text placed by its widest character (version 1);
    - flipped step repeats;
    - negative font strokes.
  - Fonts are read as inches, since only their proportions matter.
- **The board step.** The one step nothing repeats and that repeats nothing, as design 213 says, else `--step`. So a board inside a panel must be named.
- **Nets.**
  - `eda/data`'s feature IDs give features their nets. Toeprints' nets give pins theirs.
  - The CAD netlist's points join the net of their name, else a net of their own.
  - Feature IDs into a layer that failed to read are covered by that layer's fault. IDs into features a layer lacks are one `odb_eda_invalid`.
  - A component naming a missing package is left out.
- **Stackup.**
  - From `matrix/stackup.xml` (its `EdaData` stackup) where a job has one.
  - Else from the matrix's copper and dielectric board layers:
    - a dielectric's thickness is its `.layer_dielectric`, in its attribute list's units;
    - a copper layer's thickness is its `.copper_weight`: microns in a metric list, and in an imperial one ounces per square foot at 1.37 mil each, IPC's figure.
    - Altium also writes `.layer_dielectric` on copper layers, as the core's thickness; it isn't taken.
- **Capabilities.**
  - `geometry`: some copper layer read.
  - `nets`: some feature has a net.
  - `components`: some component.
  - `stackup`: every stackup layer has a thickness. KiCad writes no thicknesses, so its boards lack it.
  - `drill`: a drill layer with tools and holes.
  - `feature_ids`: some `ID=`.
  - `flex`: never in this stage.
- **Model changes.** `Tool::finished` is an `Option` (ODB++'s `FINISH_SIZE=-1` says unset), and `Component` keeps its `attributes`.
- **KiCad's exports.**
  - KiCad 9.0.8 writes no `T` or `B` records and no surface arcs. Its default `--precision 2` rounds to 10 µm, so the corpus is exported at `--precision 6 --units mm`.
  - It writes a chamfered rectangle with an empty corner list (`rect575.0x1140.0xc115.0x`), which isn't a standard symbol. The reader reports it as `odb_symbol_unknown` rather than guess which corners it means (`osm-l-bmc-som` has three).
  - No corpus board writes text records (Altium, KiCad and Valor all stroke their text), so how ODB++ fonts anchor glyphs stays as 0.A built it, unconfirmed against a real board.
- **The corpus** (`crates/ryudfm/tests/corpus`).
  - **`manifest.txt` declares every board:**
    - `source` lines: author, origin and what was done to it (CC-BY's attribution goes here);
    - `licence`: an SPDX id whose text is `licenses/<id>.txt`, or `owner`;
    - `archive <file> <sha256>` and/or `folder <dir>`;
    - optionally `step`;
    - `pinned` for boards that are fetched, not committed;
    - the counts the tests write under the declaration.
  - **Committed boards:**
    - `minimal-clean`: the owner's own Altium design, as a folder and its `.tgz`; the owner confirmed its licence on 2026-10-28.
    - `gb-cart256k-a`: CC-BY-4.0, two layers.
    - `vna-se-probe`: Apache-2.0, four layers.
    - The two KiCad boards are each a zip and a folder, with KiCad's positions and IPC-D-356 netlist in `reference/<name>/`.
  - **Pinned boards:**
    - `osm-l-bmc-som`: 10 layers, LPDDR4X, micro and buried vias.
    - `lpddr4-test-board`: 8 layers, blind vias.
    - Both Apache-2.0, listed in `pinned.txt` by repository, commit, board file and the board file's SHA-256. GitHub's archives aren't byte-stable.
    - `scripts/fetch-dfm-corpus.sh` exports them with the devcontainer's KiCad into `target/dfm-corpus`.
  - The devcontainer installs `kicad=9.0.8+dfsg-1~bpo13+1` and `fonts-lato` from a fixed snapshot.debian.org date. Do not use `-t trixie-backports`: its libcurl blocks later installs.
- **What the tests hold.**
  - `tests/corpus.rs` finds every committed board through the manifest. It checks:
    - each archive's SHA-256 and each licence's text;
    - that nothing in the folder is undeclared;
    - that a board reads to its counts, as a folder and as its archive alike, apart from export dates;
    - that one thread and eight read alike.
  - `tests/reference.rs` checks components against KiCad's positions and pins' nets against its IPC-D-356 netlist. D-356 keeps a long net name's last 14 characters, in capitals, `#n` telling cut names apart and `?` for characters it can't write. It cuts designators to six characters, and lists padless footprints only in the positions.
  - `tests/pinned.rs` does the same for the pinned boards. It is `#[ignore]`d and run by full.yml's devcontainer job after the fetch script.
  - `tests/determinism.rs` holds each committed board's model and shapes in `tests/golden/ryudfm.txt` (cpu-levels).
  - `tests/malformed.rs` breaks minimal-clean on purpose (`testing::Breakage`).
  - `tests/round_trip.rs` round-trips seeded features through the test writer.
  - The `odb` fuzz target reads bytes as each file kind or as a whole archive; the `shapes` fuzz target checks seeded shapes on one thread and eight. Both are in fuzz.yml.
- **Adding a board to the corpus.**
  1. Check its licence at the exact commit or version you take. Permissive licences (Apache-2.0, MIT, BSD, CC-BY-4.0, CERN-OHL-P) may be committed; anything else may only be pinned.
  2. To commit it:
     - put the job in `tests/corpus/` as an archive, a folder, or both (an archive keeps empty folders and exact bytes);
     - add `licenses/<SPDX>.txt` if it's a new licence;
     - add a `board` block to `manifest.txt`: its `source` lines, `licence`, `archive <file> <sha256>` (`sha256sum`) and/or `folder`, and `step` if the rule can't choose its board step;
     - for a KiCad board, also add a `commit` line to `pinned.txt` and KiCad's outputs in `reference/<name>/` (the fetch script with `--all` makes them as `pos.csv` and `netlist.d356`).
  3. To pin it instead:
     - add a `fetch` line to `pinned.txt`;
     - add a `board` block with `licence` and `pinned` to `manifest.txt`, in the same order as `pinned.txt`;
     - run `sh scripts/fetch-dfm-corpus.sh` in the devcontainer.
  4. Run `UPDATE_MANIFEST=1 cargo test -p ryudfm --test corpus` (for a pinned board, `UPDATE_MANIFEST=1 cargo test -p ryudfm --release --test pinned -- --ignored`), then `UPDATE_GOLDEN=1 cargo test -p ryudfm --test determinism`.
  5. Review the counts and faults it recorded. A fault is a finding: fix the reader, or say why the board is right to fail there.
  6. The `.gitattributes` lines for `crates/ryudfm/tests/corpus/*/**` and its archives already keep the new files byte for byte.

## Projects, the CLI and renders (decided in ryudfm phase 0)

The design is document 213 in the Coxswain planning project `ryucad-ryukei`; this is its plan 0.C.

- **Crates.**
  - `ryudfm` is now a library and the `ryudfm` binary. It adds `ryu-doc`, `ryu-session`, `blake3` (`pure`: no C or assembly), `clap`, `getrandom`, `serde`, `serde_json`, and mimalloc on musl. Still no kernel and no other product (`tests/deps.rs`).
  - `ryu-board` gains `png` and `render`, the rasterizer and the SVG writer. Its `tests/deps.rs` allows `png`.
- **Codes.** The reader's six job-failing codes, its six fault codes and its seven warnings are `ryu_doc::error::Code`s under the same names, with `board_input_missing` and `board_input_changed`. `codes::code` maps `odb::Code`; a notice's code reads as a `Code` by its name.
- **Projects.**
  - `ryudfm.json` (kind `ryudfm.project`, format 1) holds `project` (128 random bits, as ryucad's manifest format 2 writes them) and `default_unit` (`mm`).
  - `init` refuses a folder holding a ryudfm, ryucad or ryueda manifest (`project_exists`). It writes `.gitattributes` lines `*.json text eol=lf`, `boards/*/input.* binary` and `boards/*/input/** binary`, keeping any lines already there. A folder input needs the last line: a pattern naming a folder doesn't reach the files in it.
  - `boards/<id>/board.json` (kind `ryudfm.board`, format 1) holds `name`, `input` (`{path, blake3, format: "odb++", container}`), `step` and `source` (`{version, application}`).
  - An input's `path` is `input` for a folder, or `input.tgz`, `input.tar` or `input.zip`; any other path is `invalid_document`.
- **Hashes and revisions.**
  - An archive's BLAKE3 is its bytes'. A folder's is BLAKE3 over `ryudfm.tree.1` and, for each file in byte order of its path, the path's length and bytes, then the content's length and BLAKE3, lengths as 8 bytes little-endian (`input::tree_hash`).
  - A board's revision is BLAKE3 of `ryudfm.board.1`, `board.json` and the input's hash as it is now. The project's is BLAKE3 of `ryudfm.project.1`, the manifest and each board's ID and revision. Both are 32 hex digits.
  - A board whose file doesn't read, or whose input is missing or isn't the bytes it names, fails alone (`board_input_missing`, `board_input_changed` with `expected` and `found`).
- **IDs.** A new board's ID is the readable form of the file's or folder's name, less an archive's extension (`gb-cart256k-a.zip` is `gb-cart256k-a`), with `-2`, `-3` on a collision. The board's name starts as the same.
  - The design said the job's name first, but exporters name jobs generically: KiCad always writes `JOB_NAME=job`, and Altium writes none (its top folder is `odb`).
  - IDs skip only boards in the working tree. An ID a deleted board used, committed once, may be used again until stage 2, which reads Git and takes up the design's rule of never reusing an ID Git's history at `HEAD` names (decided by the coxswain).
- **Import, update, undo.**
  - `import` reads the whole job before writing anything: a path that isn't a job, or a job the reader refuses, writes nothing.
  - It writes the input as given (an archive's bytes, or a folder's files), then `board.json`. The answer holds the file, the board's counts, and its faults and warnings.
  - `--update <board>` keeps the ID, name and step (the reader's `odb_step_not_found` when the step is gone, unless `--step` names one). It replaces the input, deleting what the old one had that the new one doesn't, so a folder can follow an archive.
  - A new board is guarded by the project's revision; an update, `board edit` and `board delete` by the board's.
  - Every write is one logged change. Undo writes a board's input back before its `board.json`, and a board folder emptied goes too. `boards/` is `Product::BULKY`.
- **The session.**
  - It tracks the manifest, `.gitattributes` and every file under `boards/`.
  - It keeps read boards by input hash and step, the eight most recently used (`session::KEPT`), with the shapes built on them.
  - A board is read from the session's own snapshot of its files: `Job::from_archive`, or `Job::from_files` for a folder, named by its ID.
  - `odb::job::folder_files` and `odb::job::stem` are public for import.
- **Path mode.** `inspect`, `render` and `check` take a board ID of the project they run in, else a path that exists, read in process with no daemon (`board: null`, and `path` in the answer). Writes refuse a path that isn't a board ID (`invalid_param`).
- **Inspect.**
  - The summary gives:
    - the source, steps (datum, profile box, repeats), drills (class, tools), stackup, counts, profile box, capabilities, warnings and faults;
    - each layer's row, context, kind, polarity, side, span and feature counts, its box from its shapes (built in parallel), its symbols (standard count, user and unknown names), and how many features have no shape.
  - `--layer`, `--components` and `--nets` answer a page (`--limit`, 100 by default) and `next`, the reference `--after` takes.
  - `--feature` gives the feature as written (`geometry`), its symbol (a standard symbol's parameters as their debug text, sizes in nanometres), attributes (flags `true`, numbers as numbers), net, and shape (every part's edges) or `shape_error`.
  - References are prefixed by the board's ID in a project, and `--feature` and `--after` take them as given. Only one of `--layer`, `--feature`, `--components` and `--nets` at a time.
- **Renders (`ryu_board::render`).**
  - A `Scene` is layers of shapes, each in an `Ink` (`Paint { colour, alpha }`, or `Cut` to the background), composited bottom up over a background, through a `View`: the window scaled to fit and centred, mirrored from below.
  - Within a layer, shapes are drawn in order, and a negative part clears only what that layer drew before it. Shapes wholly outside `View::visible` (the window grown to the image's proportions) aren't drawn.
  - PNG: 4 × 4 subsamples per pixel on integer subsample rows, by the nonzero rule.
    - Arcs are cut into chords within a quarter pixel, at least one per quarter turn, at most 65,536.
    - Round and square pens sweep each chord as the convex hull of the pen at both ends. A pen of its own sweeps as itself at each point plus the parallelograms its edges sweep.
    - Bands of 16 rows run in parallel. The arithmetic is `f64` with `ryu_math`'s functions only.
    - Bytes are 8-bit RGB with the Paeth filter on every row and balanced compression.
  - SVG: paths in integer nanometres, y negated (and x from below), with a `viewBox` of the visible area.
    - Arcs are SVG arcs: a full circle is two halves, and the radius is rounded to a nanometre. Round pens are strokes with round caps and joins.
    - Square pens and pens of their own are the rasterizer's polygons, their chords within a quarter pixel.
    - There is one group per layer at its opacity, and each run of negative parts is a mask over what the layer drew before it.
    - The background and masks' white are paths, not `rect`s: Chrome draws no `rect` at nanometre sizes.
  - ryudfm's `render`:
    - Default layers are the viewed side's copper, solder mask and silk screen in stack order (the top view starts lowest), then every drill layer as `Cut`, then the profile's outline (1.5 px). Given layers are drawn in the order given.
    - Colours are fixed per kind, copper by side, on `#101418`.
    - The default window is the profile's box grown by a fiftieth of its longer side (else what is drawn). `--window X0,Y0,X1,Y1` takes lengths exactly from their text (`quantity`), bare numbers in the default unit (millimetres outside a project).
    - `--size` is the long side (1600 by default) or `WxH`, 1 to 16,384. The format comes from `--format`, else `--out`'s extension.
    - The bytes come after the response line (`image/png`, `image/svg+xml`), and the CLI writes `--out` atomically, relative to where it runs (`-C`).
- **Check.**
  - It loads the project (or one board) from nothing, with a fresh session. Each board that can't be read is an error finding: `source: project` for its file and input, `odb` for the reader's refusal. Each fault the reader recorded is an error, each warning a warning (`source: odb`).
  - Findings are `schema: 1` (`{code, severity, source, board, message, details}`). Any error fails it with `check_failed`, the report in `details.report`. `check <path>` checks a job in process.
- **Not a request.** `init` is CLI only, as ryueda's is: there is no project to open a session on until it runs. The design's `project.init` method is not served.
- **Testing.**
  - Unit tests per module: the rasterizer and SVG (coverage, negatives, cuts, one thread and eight), the views, decimals typed as lengths, hashes, files, faults per board, the cache, inspect, render and check.
  - `tests/cli.rs` runs scenarios in process and through the daemon alike: projects, imports and updates, inspect and its pages, renders, check, undo and redo, watchers, path mode, and protocol parity.
  - Golden hashes: `crates/ryudfm/tests/golden/ryudfm.txt` gains each committed board's PNG from above and below and its SVG; `crates/ryu-board/tests/golden/board.txt` gains the seeded features' pixels and SVG both ways (cpu-levels).
  - `cargo bench -p ryu-board --bench render` and `cargo bench -p ryudfm`.
- **The bars.**
  - `tests/bar.rs` is release only. In full.yml it runs with a seeded stand-in of 340,000 features (minimal-clean with ten layers of seeded features). The devcontainer job runs it after the fetch with `RYUDFM_REQUIRE_CORPUS=1`, on `lpddr4-test-board`.
  - `RYUDFM_BARS=1` asserts the bars as they are, else ten times looser.
  - On the stand-in, on cox-worker-3 (Intel Core Ultra 5 250K Plus, 18 threads, 30 GB, Ubuntu 26.04.1, Linux 7.0.0-38), with bars of 2 s, 100 ms and 500 ms:
    - read and every shape built in 0.15 s;
    - through a warm daemon, the worst of five: inspect in 31 to 40 ms, and a 1600-pixel render usually in 73 to 230 ms, but twice 416 and 792 ms while the worker was loaded (load average 8).
  - On lpddr4-test-board (338,087 features), on cox-worker-4 (AMD Opteron 6380, 8 threads, 16 GB, Debian 12 in an LXC on Proxmox, Linux 6.17.2-1-pve), in the devcontainer, three runs with `RYUDFM_BARS=1`:
    - read and every shape built in 0.71 to 0.76 s (bar 2 s): passes;
    - through a warm daemon, the worst of five: inspect in 121 to 129 ms (bar 100 ms), failing in all three; a 1600-pixel render in 313 to 472 ms (bar 500 ms), passing narrowly.
  - The inspect miss is the CPU's. A trivial request through the same warm daemon costs 15 to 24 ms, so inspect's own work is about 100 ms there, against 31 to 40 ms on cox-worker-3's Core Ultra 5. cox-worker-3 has no Docker or KiCad to export the pinned boards, so the bar is still to be recorded on a fast machine that has them (ryucad#193). If that machine misses too, speeding up inspect is its own work.
- **Limits.**
  - Reading boards at a Git revision, and the ID rule above, are stage 2's.
  - Seeded features draw four in ten lines with swept pens, which real boards hardly have; their SVG takes seconds (`benches/render.rs`), where real boards take about a millisecond.
  - A board read at import isn't kept for the next request: a board read from a project is named by its ID, and from a path by its file.

## Releases (decided in ryudfm phase 0)

The design is document 213 in the Coxswain planning project `ryucad-ryukei`; this is its plan 0.D. The release line is design 173's.

- **Its own line.** ryudfm is released apart from ryucad and ryueda, as ryueda is (see ryueda.md's Releases):
  - Tags are `ryudfm-vX.Y.Z` (pre-releases `ryudfm-vX.Y.Z-pre.N`), taken only by `.github/workflows/ryudfm-release.yml` (`ryudfm-v[0-9]+.[0-9]+.[0-9]+*`).
  - `crates/ryueda/tests/release.rs` matches all three workflows' tag patterns as GitHub does and checks that no workflow takes another line's tags, so one matcher serves every line. It runs with the fast CI.
  - Tags follow the `ryudfm-release` roadmap: pre-releases by the stage's member, final and patch tags by the coxswain or the owner, nothing before its stage 1.
- **The workflow.** As ryueda's:
  - The version comes from the tag. `scripts/stamp-version.sh` also takes `ryudfm-vX.Y.Z`, and refuses `ryudfm-X.Y.Z`.
  - `full.yml` is the gate, the whole workspace's.
  - Per target: the golden hashes (`-p ryudfm --test determinism`), a build with `RYUDFM_COMMIT`, `ryudfm version` checked for the target, version and commit, `scripts/smoke-ryudfm.sh`, then an archive.
  - `workflow_dispatch` is a dry run: everything but publishing. The targets are ryucad's.
- **What a release holds.** `ryudfm-<version>-<target>.tar.gz` (`.zip` on Windows), each a folder holding only `ryudfm`, with `SHA256SUMS`, `install-ryudfm.sh` and `install-ryudfm.ps1`.
  - Notes are generated from the ryudfm tag before it.
  - It is never GitHub's latest release (`--latest=false`), and never carries `install.sh` or `install.ps1`; the test checks both.
  - The viewer joins the archives from 0.2.0, as design 173 says, when design 169 has decided it.
- **Installers.** `install-ryudfm.sh` and `install-ryudfm.ps1` are ryueda's made ryudfm's:
  - They install to `~/.ryudfm/bin`, and take `RYUDFM_VERSION` (`0.1.0`, `v0.1.0` or `ryudfm-v0.1.0`), `RYUDFM_HOME`, `RYUDFM_NO_MODIFY_PATH` and `RYUDFM_DOWNLOAD_BASE`.
  - With no version they take the newest release in the API's list whose tag starts `ryudfm-v` and is neither a draft nor a pre-release, passing over newer ryucad and ryueda releases (the tests list one of each first).
  - They verify checksums before installing, stop running daemons (`daemon stop --all`), and put PATH in the same profiles under their own marker. They register nothing with Git or the desktop.
- **Smoke test.** `scripts/smoke-ryudfm.sh <ryudfm>` runs an agent session over a copy of `minimal-clean`:
  - `init`; the archive imported (a dry run that writes nothing, then for real); `board list`; `inspect`; a top PNG and a bottom SVG; `check`;
  - the board updated from the folder (`import --update`), which replaces `input.tgz` with `input/`; undo and redo through the daemon;
  - the activity log, the daemon's status and stop, and `check` in process.
  - It copies the board into the project, so no path is translated on Windows.
- **Testing.** `crates/ryueda/tests/release.rs` (the three lines' tag patterns, the latest rule for ryueda and ryudfm, and `stamp-version.sh` on a copy of the manifests, on Unix). full.yml's installers job builds ryudfm and:
  - runs shellcheck over the new scripts and PSScriptAnalyzer over the new PowerShell;
  - runs the smoke test on Linux, macOS and Windows;
  - runs `scripts/test-install-ryudfm.sh` and `scripts/test-install-ryudfm.ps1` (Windows PowerShell 5.1 and PowerShell 7) against a fake release.
  - The workflow was proven by a dry run before it merged.

## Copper measurement and the index (decided in ryudfm phase 1)

The design is document 245 in the Coxswain planning project `ryucad-ryukei`; this is its plan 1.A.

- **Crate.** `ryu-copper` is product-neutral. It depends on `ryu-board`, `ryu-math` and `rayon` only, and `tests/deps.rs` holds it to that. ryudfm builds its layout from a board (plan 1.D); ryueda will build one from its own boards. No geometry crate decides one of its results.
- **The layout.** `layout::Layout<K>` is generic over the caller's key: copper and other `layers` in stack order, `items` (a layer, a `Shape`, a claimed net), `holes` (round or slot, a span, plated, back-drilled spans) and netlist `points`. Plan 1.A measures and indexes items; holes and points are used from 1.C.
- **Bodies.** `measure::Body::of_shape` makes what a shape's positive parts cover into solids. Negative parts are left out and counted (`negatives`); measuring through them is the resolved copper's (1.C).
  - A region is a `Shell`: every piece of its outlines, a packed tree of their boxes, and a probe point. Its inside is the nonzero rule's, as the rasterizer fills.
  - A round pen is a thick piece per edge, `w` its exact diameter.
  - A square pen along a segment is the hull of the pen at both ends, exact.
  - A convex symbol pen covers, along each chord of its path, the hull of itself at both ends: one small region per chord.
  - Any other pen covers itself at the start plus, for each pen edge and each run of chords crossing that edge's direction the same way, the one loop round the parallelograms the edge sweeps. That loop's nonzero winding is exactly their union, and a chord parallel to an edge sweeps a segment. These are exact for polygonal pens.
- **True arc ends.** An arc ends where the ray from its centre through its written end meets its circle (stage 0's rule), usually an irrational point `c + E√(R / |E|²)`. Measurement keeps it exact. Where a contour or a path goes on from the written end, a straight join runs from the true end to it. So a segment is `b + tU` with `t` from `0`, `1` or that root, and arc ends and joins are exact.
- **Bounded pens (decided by the owner on 2026-10-29).** Square and symbol pens along an arc, and symbol pens with arcs in their outline, sweep straight edges at irrational places: where the arc's tangent is parallel to a pen edge. They are measured on chords within `BodyOptions::bound`: 100 nm by default, 10 nm at the finest, each chord point rounded within `√2/2`, so a bound of `⌈r φ²/8⌉ + 1`. The body carries its bound, and a question the bound can't settle is `Verdict::Uncertain`, never guessed. A bounded body therefore never touches anything for certain: touching is distance zero, which no bound settles. The design said every pen became exact integer pieces; that holds for round pens on any path and every pen on segments without arcs.
- **Exact predicates.** Every question compares a distance with a doubled threshold, `dd = 2h`, so half nanometres stay whole. Candidates are where the least can be:
  - meeting points (zero);
  - point to point;
  - a point to its foot on a segment;
  - a point to an arc along the ray from its centre, or a centre to its arc;
  - a segment and an arc along the perpendicular from the centre;
  - two arcs along their line of centres.

  Each is a sign of `a + b√x` or `a + b√x + c√y + d√(xy)` (`surd::sign1`, `sign2`), found by squaring, over `wide::Int`. That is one sign-magnitude integer of up to 2,048 bits that touches only the limbs in use; the design named fixed `I256` and `I512`, but two irrational true ends need about 1,664 bits at ±2^40 nm (`exact::tests::extremes`). Thresholds are capped at `MAX_REACH` (2^42 nm), beyond any distance on a board.
- **Filters.** Boxes, then floating-point lower bounds, rule pieces out before any exact work, never in. Their margin is a nanometre plus 1e-9 of the magnitude, far beyond their rounding (about 1e-12 of it).
  - Per pair of pieces (`pair::lower`): segments by their ends unless they may cross, arcs by their whole circles.
  - Per piece and box (`pair::lower_to_box`).
  - Per region and box: zero where the region holds a corner of the box or the box its probe, else its pieces' within the walk's limit.

  Bodies and shells keep packed trees of their solids and pieces. A segment slanting across more than 50 µm on its box's narrow side enters a tree as a box per stretch (up to 64), so a long slanted trace's box doesn't cover everything it never nears. `compare` walks pairs of solids, then pieces, nearest bound first (`tree::Packed::nearest_pairs`), and stops at anything nearer, or at touching when the threshold is zero. Containment (a box test, then a winding count over the pieces meeting the ray to the right) comes before outlines.
- **Questions.** `within` (at most `d`), `closer` (under `d`), `touches` (`within` 0), `contains` (a point, its outline included) answer `Verdict::Yes`, `No` or `Uncertain`. `distance` gives the nanometres rounded to nearest, ties to even, exactly: a floating-point estimate (a branch and bound over both bodies' trees) says where to look, and exact comparisons at `n ± 1/2` settle it, with bisection if the estimate were ever far off. It also gives `exact` when the distance is a whole number, the bound, the solids, and nearest points within a nanometre of the true ones (from the estimate, rounded, not correctly rounded).
- **The index.** `index::Index<K>` keeps per layer a packed R-tree of every solid's box (16 to a node, bulk-loaded in Hilbert order on a fixed 512 nm grid) and the edits since:
  - inserted items in packed trees of doubling sizes (a binary counter's carries) in place of the design's small dynamic tree;
  - removed slots dropped as they are met.

  A layer is packed again once its edits pass an eighth of it, or 64. A removed item's slot is held until its layer is packed again (the trees may still name it), then numbered again by the next insert, so the slots grow with the items, not with the edits. Queries reach past `d` by the query body's bound and the largest bound on the layer, so a bounded pair the bounds leave open is answered `Uncertain`, never dropped. Queries are `in_box`, `near` and `near_body` (`Near { key, verdict }`), `pairs` (every pair within `d` on a layer, judged per item in parallel against the trees in place of a band sweep) and `nearest` (by the distance rounded to the nanometre, ties by key). Every answer is sorted by key, so neither the trees' shapes nor the thread count shows.
- **Testing.**
  - Unit tests per module, among them every candidate at the coordinate limits.
  - `tests/props.rs`: distances between seeded segments, points and arcs (ends up to 3 nm off their circles) against dense sampling, the comparisons consistent with each distance, and seeded regions' containment against a fine floating-point winding.
  - `tests/index_props.rs` (release only, its own step in full.yml): pairs and nearest items against brute force, random edits against a rebuild, one thread against eight.
  - `tests/determinism.rs` and `tests/golden/copper.txt` (cpu-levels): `UPDATE_GOLDEN=1 cargo test -p ryu-copper --test determinism`.
  - `cargo bench -p ryu-copper`.
- **Limits.** A concave symbol pen swept along an arc (a 5 mm thermal along a 2 mm arc) makes a body of up to hundreds of thousands of pieces, whose making and measuring take up to seconds; real boards rarely draw them. The bars are plan 1.D's.
