// The mockups' window (design 199, plan 11.A): one function per part of
// it, so each page is the default layout plus what it shows. A page sets
// `window.PAGE = {...}` (see `shell`) and calls `mock()`. Rows' text is
// what the app writes today (the window tests read it); icons and badges
// come before and after it, from design 199's tables.
"use strict";

const Q = new URLSearchParams(location.search);
const VARIANT = { look: Q.get("look") || "drafting", theme: Q.get("theme") || "light", contrast: Q.get("contrast") || "normal" };

// An icon by name: a ryucad glyph if glyphs.js has it, else Phosphor.
function icon(name, px = 16, cls = "") {
  if (GLYPHS[name]) return `<span class="${cls}">${glyphSvg(name, px)}</span>`;
  return `<i class="ph ph-${name} ${cls}" style="font-size:${px}px"></i>`;
}

const TINT = { red: "danger", amber: "warn", blue: "info", green: "ok", weak: "weak", accent: "accent" };

// The sample tree: a part, an assembly with mates, a pattern and an import.
// [depth, open, icon, tint, id, rest, badge, badgeTint, flags]
const TREE = [
  [0, true, "cube", "accent", "bracket", "(B released)", "seal-check", "green"],
  [1, true, "cube-focus", "", "base", "(solid)"],
  [2, true, "pencil-simple-line", "", "s1", "(sketch, well)"],
  [3, false, "link-simple", "weak", "", "uses s1", null, null, "weakrow"],
  [2, false, "extrude", "", "e1", "(extrude, new)"],
  [2, false, "pencil-simple-line", "blue", "s2", "(sketch, under, 2 dof)"],
  [2, false, "hole", "", "h1", "(hole, cut)"],
  [2, false, "pattern-linear", "", "p1", "(pattern, cut)"],
  [2, false, "fillet", "red", "f1", "(fillet): failed (blend_failed)", "x-circle", "red", "failed"],
  [2, false, "chamfer", "", "c1", "(chamfer) [changed]", "note-pencil", "amber"],
  [1, false, "cube-focus", "weak", "tool", "(solid, consumed by u1)", "arrow-elbow-down-right", "weak", "weakrow"],
  [0, true, "assembly", "accent", "frame", "(assembly, under, 1 dof)", "arrows-clockwise", "amber"],
  [1, false, "cube", "", "plate", "(bracket @ label B, fixed)", "push-pin", ""],
  [1, false, "cube", "", "pin", "(pin @ working, 1 dof)"],
  [1, true, "paperclip", "weak", "Mates", ""],
  [2, false, "lock-simple", "", "m1", "(fastened: plate → base)"],
  [2, false, "arrow-counter-clockwise", "", "m2", "(revolute: base → pin)"],
  [2, false, "mate-cylindrical", "", "m3", "(cylindrical: pin → sleeve, redundant)", "warning-circle", "amber"],
  [1, true, "pattern-circular", "", "ring", "(instance_pattern, 6 copies)"],
  [2, false, "cube", "weak", "ring:1/pin", "", "eye-slash", "weak", "weakrow"],
  [0, true, "cube", "accent", "imported", "(C proposed)", "seal-question", "blue"],
  [1, false, "file-arrow-down", "", "body", "(import: bracket.step / BRK-100)"],
];

function treeRow(r, mark, chosen) {
  const [depth, open, ic, tint, id, rest, badge, btint, f] = r;
  const flags = (f || "") + (id === chosen ? " chosen" : "");
  const hasKids = open !== null && TREE.some((x, i, a) => a[i - 1] === r && x[0] > depth);
  const caret = hasKids ? icon(open ? "caret-down" : "caret-right", 11) : "";
  const hl = (s) => (mark ? s.replace(new RegExp(`(${mark})`, "gi"), "<mark>$1</mark>") : s);
  const b = badge ? `<span class="badge ${TINT[btint] || ""}" title="${rest}">${icon(badge, 14)}</span>` : "";
  return `<div class="row ${flags || ""}" style="padding-left:${6 + depth * 16}px">
    <span class="caret">${caret}</span>
    <span class="icon ${TINT[tint] || ""}">${icon(ic, 16)}</span>
    <span class="id">${hl(id)}</span><span class="rest">${hl(rest)}</span>${b}</div>`;
}

function treeTile(opts = {}) {
  let rows = TREE;
  if (opts.search) {
    // Matching rows keep their ancestors, as design 199's search does.
    const keep = new Set();
    rows.forEach((r, i) => {
      if ((r[4] + " " + r[5]).toLowerCase().includes(opts.search)) {
        keep.add(i);
        let d = r[0];
        for (let j = i - 1; j >= 0 && d > 0; j--) if (rows[j][0] < d) { keep.add(j); d = rows[j][0]; }
      }
    });
    rows = rows.filter((_, i) => keep.has(i));
  }
  const search = opts.search
    ? `<div class="search active">${icon("magnifying-glass", 14)}<span>${opts.search}</span><span class="weak" style="margin-left:auto">${rows.length} rows</span>${icon("x", 12)}</div>`
    : `<div class="search">${icon("magnifying-glass", 14)}<span>Find in tree</span><span style="margin-left:auto"><kbd>Ctrl+F</kbd></span></div>`;
  const f = opts.filters || [];
  const chip = (n) => `<span class="chip ${f.includes(n) ? "on" : ""}">${n}</span>`;
  return `${search}<div class="filters">${chip("Failing")}${chip("Changed")}${chip("Hidden")}</div>
    ${rows.map((r) => treeRow(r, opts.search, opts.chosen === undefined ? "e1" : opts.chosen)).join("")}`;
}

function tabs(names, active, extra = {}) {
  return `<div class="tabs">${names.map((n) => `<div class="tab ${n === active ? "active" : ""}">${extra[n] || ""}${n}${n === active && n !== "Viewport" ? ` <i class="ph ph-x x"></i>` : ""}</div>`).join("")}</div>`;
}

function menubar(open) {
  return `<div class="menubar">${["File", "Edit", "View", "Tools", "Project", "Help"].map((m) => `<span class="${m === open ? "open" : ""}">${m}</span>`).join("")}
    <span class="title">bracket-demo — ryucad</span></div>`;
}

function toolbar(opts = {}) {
  const tb = (ic, cls = "") => `<span class="tb ${cls}">${icon(ic, 20)}</span>`;
  const people = `<span class="chip"><span class="dot" style="background:#d95ecc"></span>agent${opts.agentProgress ? ` · <span class="weak">${opts.agentProgress}</span>` : ""}</span>
    <span class="chip"><span class="dot" style="background:#73b34d"></span>reviewer</span>
    <span class="chip"><span class="dot" style="background:var(--accent)"></span>david (you)</span>
    <span class="btn" style="height:22px">Follow agent</span>`;
  return `<div class="toolbar">${tb("arrow-u-up-left")}${tb("arrow-u-up-right")}<span class="sep"></span>
    ${tb("cube")}${icon("caret-down", 10)}${tb("arrows-out")}${tb("selection")}<span class="sep"></span>
    ${tb("square", "on")}${tb("line-segment", "on")}${tb("dot-outline", "on")}${tb("cube-focus", "off")}${tb("pencil-simple-line", "on")}<span class="sep"></span>
    ${tb("git-diff")}${tb("intersect-square")}<span class="sep"></span>${tb("command", opts.paletteOpen ? "on" : "")}
    ${opts.noPeople ? "" : `<span class="people">${people}</span>`}</div>`;
}

function statusLine(opts = {}) {
  const left = opts.status || `<span>${icon("check-circle", 14, "ok")} Rebuilt bracket in 38 ms</span>`;
  return `<div class="status">${left}<span class="right">${opts.statusRight || ""}<span>${icon("plugs-connected", 14)} daemon 0.6.0</span><span>${VARIANT.contrast === "high" ? "High contrast" : VARIANT.look[0].toUpperCase() + VARIANT.look.slice(1)} · ${VARIANT.theme}</span></div></div>`;
}

function viewport(opts = {}) {
  const cube = `<svg class="viewcube" width="72" height="72" viewBox="0 0 72 72"><g fill="var(--raised)" stroke="var(--border-strong)">
    <path d="M36 8 62 22 36 36 10 22Z"/><path d="M10 22 36 36 36 64 10 50Z"/><path d="M62 22 36 36 36 64 62 50Z"/></g>
    <g font-family="Inter" font-size="8" fill="var(--text-weak)" text-anchor="middle"><text x="36" y="24">Top</text><text x="22" y="46">Front</text><text x="50" y="46">Right</text></g></svg>`;
  const hl = opts.highlight ? `<div style="position:absolute;left:52.5%;top:55.5%;width:13.5%;height:12%;border:2px solid #d95ecc;border-radius:50%"></div>
    <div class="hl" style="left:59%;top:51%;background:#d95ecc">${icon("robot", 12)} agent · check this bore</div>` : "";
  const tip = opts.tooltip ? `<div class="tip" style="left:${opts.tooltip.x}px;top:${opts.tooltip.y}px">Face of h1 (counterbore wall)<br><span class="mono weak">bracket/base/6f0c2a91d4b7e3a85c19f02e7b6d4a10</span></div>` : "";
  return `<div class="viewport"><canvas id="model" data-src="model/viewport.png"></canvas>${opts.compare ? `<div class="comparebar">${icon("git-diff", 14)} Comparing with <span class="mono">HEAD</span>: 2 changed, 1 added <span class="ok">■ new</span><span class="warn">■ changed</span><span class="danger">■ removed</span><span class="link" style="margin-left:auto">Stop comparing</span></div>` : ""}${cube}${hl}${tip}${opts.viewportExtra || ""}</div>`;
}

const FEED = [
  ["#d95ecc", "agent", "robot", "running check (3/8 parts)", [
    ["14:22:08", "edited", ["bracket/f1"], "Undo"],
    ["14:22:05", "added", ["bracket/c1"], "Undo"],
    ["14:21:40", "set parameters", ["bracket: thickness"], ""],
  ]],
  ["#73b34d", "reviewer", "user", "watching", [["14:19:12", "moved", ["frame/pin"], ""]]],
  ["var(--text-weak)", "external", "terminal-window", "", [["14:17:55", "changed on disk", ["bracket/e1"], ""]]],
];

function feedTile() {
  return `<div class="feed">${FEED.map(([c, who, ic, st, es]) => `<div class="run"><div class="who"><span class="dot" style="display:inline-block;width:8px;height:8px;border-radius:4px;background:${c}"></span>${icon(ic, 14)}<b>${who}</b><span class="weak">${st}</span></div>
    ${es.map(([t, m, refs, act]) => `<div class="entry"><span class="t mono">${t}</span><span>${m}</span>${refs.map((r) => `<span class="chip mono">${r}</span>`).join("")}${act ? `<span class="link" style="margin-left:auto;margin-right:8px">${act}</span>` : ""}</div>`).join("")}</div>`).join("")}</div>`;
}

// The extrude form: `state` may mark `dirty`, `error` and `conflict`.
function extrudeForm(state = {}) {
  const banner = state.conflict ? `<div class="banner">${icon("warning", 16, "warn")}<div><b>agent changed e1 while you edited.</b><br>length: <span class="mono">12 mm → 15 mm</span>. Your edits are kept.<br><span class="link">Reload</span> · <span class="link">Keep mine</span></div></div>` : "";
  const length = state.dirty
    ? `<label class="dirty">Length</label><div class="field dirty"><span class="mono">thickness + 4 mm</span><span class="val">= 12 mm</span></div>`
    : `<label>Length</label><div class="field"><span class="mono">thickness</span><span class="val">= 8 mm</span></div>`;
  const offset = state.error
    ? `<label class="dirty">Offset</label><div class="field error"><span class="mono">2 * (wall</span></div><div class="fielderr">${icon("x-circle", 12)} expected ')' at byte 8</div>`
    : `<label>Offset</label><div class="field"><span class="mono">0 mm</span></div>`;
  const dirty = state.dirty || state.error;
  return `<div class="section" style="margin-top:0">${icon("extrude", 14)} e1 · extrude${dirty ? ` <span class="accent">• edited</span>` : ""}</div>${banner}
    <div class="form">
      <label>Sketch</label><div class="field ro"><span class="mono">s1</span>${icon("copy", 14)}</div>
      <label>Operation</label><div class="field"><span>New solid</span>${icon("caret-down", 12)}</div>
      <label>Extent</label><div class="field"><span>Blind</span>${icon("caret-down", 12)}</div>
      ${length}${offset}
      <label>Direction</label><div class="field"><span>Normal</span>${icon("caret-down", 12)}</div>
      <label>Symmetric</label><div>${icon("square", 16)}</div>
      <label>Revision</label><div class="field ro"><span class="mono">9a41c0e2</span>${icon("copy", 14)}</div>
    </div>
    <div class="formfoot"><span class="btn ${dirty ? "" : "disabled"}">Revert <kbd>Esc</kbd></span><span class="btn ${dirty ? "primary" : "disabled"}">Apply <kbd>Enter</kbd></span></div>`;
}

// PAGE: { tree, viewport, right, rightTab, bottomTab, center, centerTab,
// overlay, status, statusRight, menuOpen, agentProgress, focus }.
function shell(p = {}) {
  const right = p.right !== undefined ? p.right : extrudeForm();
  const centerTabs = p.centerTab ? ["Viewport", p.centerTab] : ["Viewport"];
  const center = p.center || viewport(p.viewport);
  return `<div class="window">${menubar(p.menuOpen)}${toolbar(p)}
    <div class="body">
      <div class="tile left ${p.focus === "tree" ? "focus" : ""}">${tabs(["Tree"], "Tree")}<div class="content">${treeTile(p.tree)}</div></div>
      <div class="tile">${tabs(centerTabs, p.centerTab || "Viewport")}${center}</div>
      <div class="tile">${tabs(["Properties", "Parameters", "Part", "Changes"], p.rightTab || "Properties")}<div class="content padded">${right}</div></div>
      <div class="tile bottom">${tabs(["Activity", "History", "Releases"], p.bottomTab || "Activity")}<div class="content" style="padding:0">${p.bottom || feedTile()}</div></div>
    </div>${statusLine(p)}${p.overlay || ""}</div>`;
}

// Draws model/viewport.png into its canvas with ryucad render's background
// (#f5f6f8) swapped for this variant's viewport colour, so a dark viewport
// is the window's (render's own background is fixed).
function paintModel() {
  for (const c of document.querySelectorAll("canvas[data-src]")) {
    const img = new Image();
    img.onload = () => {
      c.width = img.width; c.height = img.height;
      const g = c.getContext("2d");
      g.drawImage(img, 0, 0);
      const bg = [0xf5, 0xf6, 0xf8];
      const to = getComputedStyle(c.parentElement).backgroundColor.match(/\d+/g).map(Number);
      const d = g.getImageData(0, 0, c.width, c.height);
      for (let i = 0; i < d.data.length; i += 4) {
        const dist = Math.max(...[0, 1, 2].map((k) => Math.abs(d.data[i + k] - bg[k]))) / 255;
        const t = Math.min(1, dist / 0.15);
        for (let k = 0; k < 3; k++) d.data[i + k] = d.data[i + k] * t + to[k] * (1 - t);
      }
      g.putImageData(d, 0, 0);
    };
    img.src = c.dataset.src;
  }
}

function mock(html) {
  const r = document.documentElement;
  r.dataset.look = VARIANT.look; r.dataset.theme = VARIANT.theme; r.dataset.contrast = VARIANT.contrast;
  document.body.innerHTML = html;
  const crop = Q.get("crop");
  if (crop) {
    const [x, y] = crop.split(",").map(Number);
    document.body.firstElementChild.style.transform = `translate(${-x}px, ${-y}px)`;
  }
  paintModel();
}
