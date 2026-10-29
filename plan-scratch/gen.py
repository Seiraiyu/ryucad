import pathlib
R = pathlib.Path("repo")
A = R / "design/app"
T = dict(l.split() for l in open("tasks.txt"))
def f(p, lang=""):
    return f"```{lang}\n" + (R / p).read_text().rstrip("\n") + "\n```\n"
CO = "Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>"
def commit(msg, paths):
    return f"**Step 5 -- commit:**\n\n```sh\ngit add {paths}\ngit commit -m \"{msg}\" -m \"{CO}\"\n```\n"
PAGE_HEAD = """Each page starts with the same head, which loads Phosphor's two style sheets, the stylesheet, the glyph table and the window script, then calls `mock()` from an inline script."""
out = []
w = out.append
w("""# ryucad phase 11.A: Mockups -- Implementation Plan

**Goal:** static mockups of every surface design 199 changes, in every look, theme and high contrast, plus the tree icon sheet, approved by the owner as cox visuals before any drawing code changes.
**Architecture:** one HTML page per surface under `design/app/`, all drawn by one script (`mock.js`) over one stylesheet (`app.css`, every palette) and one glyph table (`glyphs.js`, ryucad's 15 CAD glyphs, the source 11.B ports); a stdlib Python script renders each page in eight variants with a headless Chromium; the viewport shows a bracket the CLI builds and `ryucad render` draws. The PNGs and pages are committed; ten of them go up as cox visuals and are revised until approved.
**Tech stack:** HTML, CSS and plain JavaScript; Inter 4.1, JetBrains Mono 2.304 and Phosphor 2.1.2 (embedded by 11.B from `crates/ryucad-app/fonts/`); Python 3 (stdlib) and Playwright's chrome-headless-shell; `ryucad` itself for the model; `cox visual`.
**Epic:** work item 2396 (ryucad 11.A)

This plan changes no Rust and no pixels of the app: design 199 gates everything that draws on these mockups' approval. It writes `design/app/`, `crates/ryucad-app/fonts/` (files only; 11.B embeds them), three `.gitattributes` lines and a section of `docs/claude/ryucad-app.md`.

Every file below was written and run while planning, on cox-worker-1 (Debian 12): `contrast.py` prints `contrast: 8 palettes pass`, the glyph grid check is empty, `make.sh` builds the bracket with every feature clean, and `render.py` writes 72 PNGs, 9 crops and the looks sheet in about a minute.

**Settled while planning (2026-10-29):** the mockups go up as **ten visuals**: each surface in Drafting light (eight), the looks sheet tiling every surface's eight variants, and the icon sheet, which revises the draft visual under review, `01M63ZRW0XNTNDBH74MTX70NT4`, rather than adding a second. This was the recommended option of question 172435d2, answered by the coxswain under the owner's rule for questions, not by the owner. The full set is committed in `design/app/png/`.

**Stories:** 3115 (the mockup kit: Tasks 1 to 6), 3116 (the surfaces: Tasks 7 to 10), 3117 (review and record: Tasks 11 to 14).

| Task | Work item | Description |
|------|-----------|-------------|
""")
titles = ["Bundled fonts and their licences","The render harness","Palettes and the contrast check","ryucad glyphs on Phosphor grid","The shared window script","The bracket the viewport shows","The default layout and the icon sheet","Palette, feedback and progress pages","Form, tree and shortcuts pages","The start screen","Render and commit every PNG","Upload the ten visuals","Revise until the owner approves","Record the decisions in docs/claude"]
for i, t in enumerate(titles, 1):
    w(f"| {i} | #{T[str(i)]} | {t} |\n")
w("""
Commands run from the worktree's root. Use a private target folder (`export CARGO_TARGET_DIR=/tmp/ryucad-11a/target`) if the devcontainer has left `target/` owned by root.

""")
# Task 1
w(f"""## Task 1: Bundled fonts and their licences (#{T['1']})

**Files:** create `design/app/fetch-fonts.py`, `crates/ryucad-app/fonts/LICENSES.md`, `crates/ryucad-app/fonts/{{InterVariable.ttf,JetBrainsMono-Regular.ttf,Phosphor.ttf,Phosphor-Fill.ttf,Inter-LICENSE.txt,JetBrainsMono-OFL.txt,Phosphor-LICENSE.txt}}` (written by the script), `design/app/vendor/phosphor.css`, `design/app/vendor/phosphor-fill.css` (written by the script); modify `.gitattributes`.

**Step 1 -- failing test:** the fonts aren't there yet:

```sh
test -f crates/ryucad-app/fonts/Phosphor.ttf && echo present || echo missing
```

**Step 2 -- see it fail:** it prints `missing`.

**Step 3 -- implement:** `design/app/fetch-fonts.py`:

{f('design/app/fetch-fonts.py','python')}
`crates/ryucad-app/fonts/LICENSES.md`:

{f('crates/ryucad-app/fonts/LICENSES.md','markdown')}
Append to `.gitattributes`:

```
# Fonts and the mockups' images are kept byte for byte.
crates/ryucad-app/fonts/*.ttf binary
design/app/png/** binary
design/app/model/*.png binary
```

**Step 4 -- see it pass:**

```sh
python3 design/app/fetch-fonts.py
grep -c 'ph-.*:before' design/app/vendor/phosphor.css
```

It prints seven `wrote crates/ryucad-app/fonts/...` lines (InterVariable.ttf 879708 bytes, JetBrainsMono-Regular.ttf 273900, Phosphor.ttf 488636, Phosphor-Fill.ttf 449252, and the three licences), two `wrote design/app/vendor/...` lines, then `1530`. Every glyph name design 199's tables use is among them.

{commit("ryucad 11.A: bundle Inter, JetBrains Mono and Phosphor with their licences", ".gitattributes design/app/fetch-fonts.py design/app/vendor crates/ryucad-app/fonts")}
""")
# Task 2
w(f"""## Task 2: The render harness (#{T['2']})

**Files:** create `design/app/render.py`.

It renders every page in every variant, the 2× crops and the looks sheet, with Playwright's chrome-headless-shell; `setup` installs that and, where the host lacks them (Debian 12 without a desktop lacks libnspr4, libnss3 and libasound2), unpacks the libraries into `target/mockup-libs` with `apt-get download` (no root). The pages it names arrive in Tasks 7 to 10.

**Step 1 -- failing test:**

```sh
python3 design/app/render.py setup
```

**Step 2 -- see it fail:** `python3: can't open file '.../design/app/render.py': [Errno 2] No such file or directory`.

**Step 3 -- implement:** `design/app/render.py`:

{f('design/app/render.py','python')}
**Step 4 -- see it pass:**

```sh
python3 design/app/render.py setup
```

It ends `ready: /home/<you>/.cache/ms-playwright/chromium_headless_shell-<n>/chrome-headless-shell-linux64/chrome-headless-shell` (on cox-worker-1 after `Get:… libnspr4`, the three packages it downloads). On a Mac or a desktop Linux nothing is downloaded. `python3 design/app/render.py nonsense` prints the usage and exits 1, and `python3 design/app/render.py page nope` prints `no page nope: layout, palette, …` and exits 1.

{commit("ryucad 11.A: render the mockups with a headless Chromium", "design/app/render.py")}
""")
# Task 3
w(f"""## Task 3: Palettes and the contrast check (#{T['3']})

**Files:** create `design/app/contrast.py`, `design/app/app.css`.

The palettes are `ui::style`'s (`crates/ryucad-app/src/ui/style.rs`) with what WCAG AA needs: weak text and the light themes' warning colour moved until they reach 4.5:1 on the app, raised and frame surfaces (today's fail at 2.6 to 3.4:1), `ok`, `info` and `focus` added per look, the dark viewport the window's shared grey (`#383b42`, `viewport_background`), and one high-contrast palette per theme. The rest of the stylesheet is the window's parts at `ui.scale` 1.0.

**Step 1 -- failing test:** write `design/app/contrast.py`:

{f('design/app/contrast.py','python')}
**Step 2 -- see it fail:** `python3 design/app/contrast.py` raises `FileNotFoundError: ... design/app/app.css`.

**Step 3 -- implement:** `design/app/app.css`:

{f('design/app/app.css','css')}
**Step 4 -- see it pass:** `python3 design/app/contrast.py` prints `contrast: 8 palettes pass`. (Put today's `text-weak: #8d897f` back in Drafting light and it prints three `FAIL` lines and exits 1; restore it.)

{commit("ryucad 11.A: the mockups' palettes, checked against WCAG AA", "design/app/contrast.py design/app/app.css")}
""")
# Task 4
w(f"""## Task 4: ryucad glyphs on Phosphor's grid (#{T['4']})

**Files:** create `design/app/glyphs.js`.

The 15 CAD glyphs design 199 names, as the draft sheet under review drew them (visual `01M63ZRW0XNTNDBH74MTX70NT4`), now as stroke tables: the source 11.B ports to `ui::icon::glyphs`. The chamfer's dimension line moves 4 units in so its ticks stay inside the grid.

**Step 1 -- failing test:**

```sh
node -e "$(cat design/app/glyphs.js); const bad = checkGrid(); console.log(Object.keys(GLYPHS).length, bad); process.exit(bad.length || Object.keys(GLYPHS).length !== 15 ? 1 : 0)"
```

**Step 2 -- see it fail:** `cat: design/app/glyphs.js: No such file or directory`, then `ReferenceError: checkGrid is not defined`.

**Step 3 -- implement:** `design/app/glyphs.js`:

{f('design/app/glyphs.js','javascript')}
**Step 4 -- see it pass:** the command prints `15 []` and exits 0.

{commit("ryucad 11.A: ryucad's 15 CAD glyphs on Phosphor's grid", "design/app/glyphs.js")}
""")
# Task 5
w(f"""## Task 5: The shared window script (#{T['5']})

**Files:** create `design/app/mock.js`.

One function per part of the window: menu bar, toolbar (undo and redo, view orientation, zoom to fit and to selection, the selection filter, compare, interference, the palette, the People strip), the tree with every icon and state, tab bars, the viewport (the model with its background swapped per variant, the cube, an agent's highlight, a tooltip), the activity feed, the extrude form with its dirty, error and conflict states, and the status line. Row text is today's (`(sketch, under, 2 dof)`, `failed (blend_failed)`, `[changed]`, `B released`), and `→` replaces `->`.

**Step 1 -- failing test:**

```sh
node -e "global.location={{search:''}}; $(cat design/app/glyphs.js design/app/mock.js); const h = shell({{}}); console.log(h.includes('e1') && h.includes('blend_failed') && h.includes('Activity'))"
```

**Step 2 -- see it fail:** `cat: design/app/mock.js: No such file or directory`, then `ReferenceError: shell is not defined`.

**Step 3 -- implement:** `design/app/mock.js`:

{f('design/app/mock.js','javascript')}
**Step 4 -- see it pass:** the command prints `true`.

{commit("ryucad 11.A: the mockups' window script", "design/app/mock.js")}
""")
# Task 6
w(f"""## Task 6: The bracket the viewport shows (#{T['6']})

**Files:** create `design/app/model/make.sh`; it writes `design/app/model/viewport.png` (2080×1456, the viewport tile at 2×) and `design/app/model/thumb.png` (512×300, the start screen's card).

An 80 × 50 mm plate, `thickness` 8 mm and `hole` 6.6 mm as parameters, four counterbored holes through it and its four corners filleted at 6 mm, made through the CLI as an agent would and drawn by `ryucad render` (whose background `mock.js` swaps for each variant's viewport colour). It needs `jq`.

**Step 1 -- failing test:** `test -f design/app/model/viewport.png && echo present || echo missing`

**Step 2 -- see it fail:** it prints `missing`.

**Step 3 -- implement:** `design/app/model/make.sh`:

{f('design/app/model/make.sh','sh')}
**Step 4 -- see it pass:**

```sh
cargo build --release -p ryucad
sh design/app/model/make.sh "${{CARGO_TARGET_DIR:-target}}/release/ryucad"
```

It prints `wrote .../design/app/model/viewport.png and .../thumb.png`. Open `viewport.png`: the plate iso, four counterbores, rounded corners, no sketch lines.

{commit("ryucad 11.A: the bracket the mockups' viewport shows", "design/app/model")}
""")
# pages
def pg(name): return f(f"design/app/{name}.html", "html")
w(f"""## Task 7: The default layout and the icon sheet (#{T['7']})

**Files:** create `design/app/layout.html`, `design/app/icons.html`.

The layout is design 199's default: the tree left, the viewport, Properties | Parameters | Part | Changes on the right, Activity | History | Releases below, with an agent's highlight in its colour on a bore. The icon sheet redraws the draft under review: every row type and state at 32 and 16 px in the variant's colours, the sample tree tile and the toolbar at 20 px.

**Step 1 -- failing test:**

```sh
python3 design/app/render.py page layout icons
```

**Step 2 -- see it fail:** a traceback ending `RuntimeError: file:///…/design/app/layout.html?look=drafting&theme=light&contrast=normal: …` with Chromium's `Page load failed: net::ERR_FILE_NOT_FOUND`.

**Step 3 -- implement:** `design/app/layout.html`:

{pg('layout')}
`design/app/icons.html`:

{pg('icons')}
**Step 4 -- see it pass:** Step 1 prints `wrote design/app/png/layout/drafting-light.png` and `wrote design/app/png/icons/drafting-light.png`. Open both: the window as the layout visual will show it (bracket in the viewport, the tree with its icons, e1's form, the feed), and the sheet's three columns (structure, primitives and features; assembly and states; the tree tile and toolbar).

{commit("ryucad 11.A: the default layout and the tree icon sheet", "design/app/layout.html design/app/icons.html")}
""")
w(f"""## Task 8: Palette, feedback and progress pages (#{T['8']})

**Files:** create `design/app/palette.html`, `design/app/feedback.html`, `design/app/progress.html`.

The palette open on `zoom` (a disabled command greyed with its reason); an error card opened from the status line (`blend_failed`: its entities as chips with Show, `removed_by` in words, Copy details) with three toasts (an agent's batched edits with Show and Undo, a reviewer's move, an error that stays); the window's own rebuild with its bar and Cancel, the agent's check on its chip from the `progress` topic, and a request waiting for it.

**Step 1 -- failing test:** `python3 design/app/render.py page palette feedback progress`

**Step 2 -- see it fail:** as Task 7's: `RuntimeError` for `palette.html`, `Page load failed: net::ERR_FILE_NOT_FOUND`.

**Step 3 -- implement:** `design/app/palette.html`:

{pg('palette')}
`design/app/feedback.html`:

{pg('feedback')}
`design/app/progress.html`:

{pg('progress')}
**Step 4 -- see it pass:** Step 1 prints a `wrote design/app/png/<page>/drafting-light.png` line per page; open each.

{commit("ryucad 11.A: the palette, an error card and toasts, and progress", "design/app/palette.html design/app/feedback.html design/app/progress.html")}
""")
w(f"""## Task 9: Form, tree and shortcuts pages (#{T['9']})

**Files:** create `design/app/form.html`, `design/app/tree.html`, `design/app/shortcuts.html`.

The extrude form with a dirty field (`thickness + 4 mm`, `= 12 mm` beside it), an inline syntax error at its byte, the conflict banner, Apply and Revert; the tree searched for `pin` (ancestors kept, the match marked, the tile focused as F6 leaves it, a mate's form beside it); the Shortcuts tile beside the viewport with a chord being set and a conflict refused before anything is written.

**Step 1 -- failing test:** `python3 design/app/render.py page form tree shortcuts`

**Step 2 -- see it fail:** as Task 7's: `RuntimeError` for `form.html`, `Page load failed: net::ERR_FILE_NOT_FOUND`.

**Step 3 -- implement:** `design/app/form.html`:

{pg('form')}
`design/app/tree.html`:

{pg('tree')}
`design/app/shortcuts.html`:

{pg('shortcuts')}
**Step 4 -- see it pass:** Step 1 prints a `wrote design/app/png/<page>/drafting-light.png` line per page; open each.

{commit("ryucad 11.A: the form, the tree's search and the shortcuts", "design/app/form.html design/app/tree.html design/app/shortcuts.html")}
""")
w(f"""## Task 10: The start screen (#{T['10']})

**Files:** create `design/app/start.html`.

No project open: Open…, New project… and Open sample…, recent projects as cards with the bracket's thumbnail (a missing one shows the project's initial), and the agent card with the lines to copy.

**Step 1 -- failing test:** `python3 design/app/render.py page start`

**Step 2 -- see it fail:** as Task 7's: `RuntimeError` for `start.html`, `Page load failed: net::ERR_FILE_NOT_FOUND`.

**Step 3 -- implement:** `design/app/start.html`:

{pg('start')}
**Step 4 -- see it pass:** Step 1 prints `wrote design/app/png/start/drafting-light.png`; open it.

{commit("ryucad 11.A: the start screen", "design/app/start.html")}
""")
w(f"""## Task 11: Render and commit every PNG (#{T['11']})

**Files:** create `design/app/png/` (written by `render.py`).

**Step 1 -- failing test:** `ls design/app/png/looks.png`

**Step 2 -- see it fail:** `ls: cannot access 'design/app/png/looks.png': No such file or directory`.

**Step 3 -- implement:** render everything:

```sh
python3 design/app/contrast.py
python3 design/app/render.py
```

**Step 4 -- see it pass:** `contrast: 8 palettes pass`, then 81 `wrote design/app/png/...` lines (72 page variants, 9 crops) and `wrote design/app/png/looks.png`; `find design/app/png -name '*.png' | wc -l` prints `82`, about 14 MB. Look through `png/looks.png`: every surface in every variant, nothing blank, no page of Chromium's errors. Look at each Drafting light page and each crop at full size: no text wrapping inside a field, no row clipped unexpectedly (the tree's long rows end in `…` at 280 px, as egui's would), the viewport's background the variant's own. Fix a page and render again until each is right.

{commit("ryucad 11.A: the mockups rendered in every look, theme and high contrast", "design/app/png")}
""")
w(f"""## Task 12: Upload the ten visuals (#{T['12']})

**Files:** none in the repository.

**Step 1 -- failing test:**

```sh
cox --json visual list | python3 -c "import json,sys; print(sum('11.A' in v['title'] for v in json.load(sys.stdin)['visuals']))"
```

**Step 2 -- see it fail:** it prints `0`.

**Step 3 -- implement:** nine new visuals, and the draft icon sheet revised (a revision is `revise`, never a second `add`, so its comments carry forward):

```sh
cd design/app/png
for p in layout palette feedback progress start form tree shortcuts; do
  cox visual add "$p/drafting-light.png" --title "Design 199 · 11.A: $p (Drafting light)"
done
cox visual add looks.png --title "Design 199 · 11.A: every surface in every look, theme and high contrast"
cox visual revise 01M63ZRW0XNTNDBH74MTX70NT4 icons/drafting-light.png \\
  --note "11.A's sheet: drawn from design/app/glyphs.js (the chamfer's ticks moved inside the grid), the sample tree with today's row text, the toolbar at 20 px. Every variant: png/icons/ in the repo, and the looks visual."
cd -
```

If `revise` is refused because the draft is closed, add the sheet as a new visual titled `Design 199 · 11.A: tree icons` and say so in the epic's notes. Record each visual's id against its surface on the epic:

```sh
cox doc work-item --id=2396 --notes "11.A visuals: layout <id>, palette <id>, feedback <id>, progress <id>, start <id>, form <id>, tree <id>, shortcuts <id>, looks <id>, icons 01M63ZRW0XNTNDBH74MTX70NT4 (round 2)"
```

and send the list, with each visual's page, to the coxswain, who asked to pass them on (`mcp__cox__send {{to: "parent", ...}}` from a Coxswain session).

**Step 4 -- see it pass:** Step 1's command prints `9`, `cox visual show 01M63ZRW0XNTNDBH74MTX70NT4` shows `round 2: drafting-light.png`, and `cox visual show <id>` of each new one shows `round 1` with its PNG.

**Step 5 -- commit:** nothing to commit.
""")
w(f"""## Task 13: Revise until the owner approves (#{T['13']})

**Files:** whatever page, `app.css`, `glyphs.js` or `mock.js` a comment names, and `design/app/png/`.

Each `[coxswain notice]` of a submitted review starts a round.

**Step 1 -- failing test:** `cox visual show <id>` for each of the ten: a visual whose latest round has no `approve` verdict, or has open comments, is work.

**Step 2 -- see it fail:** every comment's anchor (a place in the image) and text, as `cox visual show` prints them.

**Step 3 -- implement:** for each comment, change the page or the shared file it points at (a colour is a palette in `app.css`, a glyph `glyphs.js`, a row's text or a part of the window `mock.js`), then:

```sh
python3 design/app/contrast.py
node -e "$(cat design/app/glyphs.js); process.exit(checkGrid().length)"
python3 design/app/render.py
cox visual revise <id> design/app/png/<page>/drafting-light.png --note "<what changed, by comment>"
cox visual revise <looks id> design/app/png/looks.png --note "<the same>"
cox visual reply <id> <comment id> --body "<what changed, or why not>"
```

A change to a shared file revises every visual it shows in. Commit each round: `git commit -m "ryucad 11.A: round <n>: <what changed>" -m "{CO}"`.

**Step 4 -- see it pass:** every one of the ten visuals' latest round carries the owner's `approve`, with no open comment. The verdicts are the owner's alone (an agent can't submit, close or approve a visual), and approval of this plan's work is what lets 11.B to 11.E draw.

**Step 5 -- commit:** each round as above.
""")
docs = (pathlib.Path("docs-section.md").read_text())
w(f"""## Task 14: Record the decisions in docs/claude (#{T['14']})

**Files:** modify `docs/claude/ryucad-app.md` (append at its end).

**Step 1 -- failing test:** `grep -c 'Mockups (decided in ryucad phase 11)' docs/claude/ryucad-app.md`

**Step 2 -- see it fail:** it prints `0` and exits 1.

**Step 3 -- implement:** append this section, updating any value Task 13's rounds changed (a palette, a width):

````markdown{docs.rstrip()}
````

**Step 4 -- see it pass:** Step 1 prints `1`. `git diff --stat` touches only `docs/claude/ryucad-app.md`; CLAUDE.md is unchanged (the area file exists, so no import line).

{commit("ryucad 11.A: record the mockups' decisions", "docs/claude/ryucad-app.md")}
Then push the branch and open the pull request (`cox-execute` does this), its body naming the ten visuals and saying the app's code is unchanged.
""")
pathlib.Path("plan.md").write_text("".join(out))
print(len("".join(out)))
