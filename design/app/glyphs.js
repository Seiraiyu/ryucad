// ryucad's 42 CAD glyphs (design 199, Fonts and icons; design 297 adds
// the datum and path glyphs, design 323 the sweep and loft glyphs, design
// 327 the shell, offset, mirror and split glyphs, design 346 the base
// flange, edge flange, unfold and refold glyphs, design 388 the sheet,
// planar, fill, extract, sew and thicken glyphs, design 400 the draft,
// move-face and replace-face glyphs, design 417 the drawing glyph, design
// 445 the delete-face, delete-blend and resize-blend glyphs), on Phosphor's
// 256-unit grid with its 16-unit stroke, round caps and joins. Each glyph
// is a list of strokes: `l` a polyline, `e` an ellipse (cx, cy, rx, ry),
// `a` an arc of one (cx, cy, rx, ry, from, to: degrees clockwise from +x,
// y down, as the screen), `arrow` a line with Phosphor's open head, `box`
// a block drawn in cabinet projection (x, y, w, h, dx, dy). `w` overrides
// the stroke width; `fill` fills the shape instead. Plan 11.B ports this
// table to `ui::icon::glyphs` stroke for stroke, so the window draws
// exactly what the mockups show. Every point plus half its stroke lies
// inside 0..256 (`checkGrid`).
"use strict";

const STROKE = 16;

function box(x, y, w, h, dx = 36, dy = -28) {
  return [
    { l: [[x, y], [x + w, y], [x + w, y + h], [x, y + h], [x, y]] },
    { l: [[x, y], [x + dx, y + dy], [x + w + dx, y + dy], [x + w, y]] },
    { l: [[x + w + dx, y + dy], [x + w + dx, y + h + dy], [x + w, y + h]] },
  ];
}

function arrow(x0, y0, x1, y1) {
  const a = Math.atan2(y1 - y0, x1 - x0);
  const L = 40;
  const s = 0.55;
  return [
    { l: [[x0, y0], [x1, y1]] },
    { l: [[x1 - L * Math.cos(a + s), y1 - L * Math.sin(a + s)], [x1, y1],
          [x1 - L * Math.cos(a - s), y1 - L * Math.sin(a - s)]] },
  ];
}

function dashes(x, from, to, step, len, w) {
  const out = [];
  for (let y = from; y < to; y += step) out.push({ l: [[x, y], [x, y + len]], w });
  return out;
}

const GLYPHS = {
  cone: [
    { e: [128, 198, 88, 26] },
    { l: [[44, 196], [128, 32], [212, 196]] },
  ],
  torus: [
    { e: [128, 132, 108, 60] },
    { a: [128, 136, 44, 24, 20, 160] },
    { a: [128, 135, 32, 15, 200, 340] },
  ],
  extrude: [...box(48, 140, 128, 72), ...arrow(128, 128, 128, 24)],
  "extrude-cut": [
    ...box(40, 112, 152, 96),
    { e: [116, 116, 36, 12] },
    ...arrow(116, 20, 116, 92),
  ],
  revolve: [
    ...dashes(128, 24, 236, 40, 20, 12),
    { l: [[128, 64], [176, 64], [176, 176], [128, 176]] },
    { a: [128, 164, 88, 68, 200, 380] },
    { l: [[178, 168], [206, 176], [222, 150]] },
  ],
  "revolve-cut": [
    ...dashes(128, 24, 236, 40, 20, 12),
    { l: [[128, 48], [192, 48], [192, 96], [160, 112], [192, 128], [192, 192], [128, 192]] },
    { a: [128, 180, 96, 60, 200, 330] },
  ],
  fillet: [
    { l: [[136, 208], [24, 208], [24, 64], [136, 64]] },
    { a: [136, 160, 96, 96, 270, 360] },
    { l: [[232, 160], [232, 208], [136, 208]] },
    { l: [[136, 160], [204, 92]], w: 10 },
    { l: [[126, 150], [146, 170]], w: 10 },
  ],
  chamfer: [
    { l: [[24, 208], [24, 64], [152, 64], [232, 144], [232, 208], [24, 208]] },
    { l: [[172, 52], [240, 120]], w: 10 },
    { l: [[164, 44], [180, 60]], w: 10 },
    { l: [[232, 112], [248, 128]], w: 10 },
  ],
  hole: [
    ...box(32, 112, 160, 96),
    { e: [116, 94, 32, 12] },
    ...dashes(88, 112, 200, 28, 12, 10),
    ...dashes(144, 112, 200, 28, 12, 10),
  ],
  "pattern-linear": [
    ...[32, 104, 176].map((x) => ({ l: [[x, 88], [x + 48, 88], [x + 48, 136], [x, 136], [x, 88]] })),
    ...arrow(32, 196, 224, 196),
  ],
  "pattern-circular": [
    ...[0, 1, 2, 3, 4, 5].map((i) => {
      const a = (i * Math.PI) / 3 - Math.PI / 2;
      const c = [128 + 84 * Math.cos(a), 128 + 84 * Math.sin(a), 22, 22];
      return i === 0 ? { e: c, fill: true } : { e: c, w: 12 };
    }),
    { e: [128, 128, 12, 12], w: 10 },
  ],
  assembly: [...box(24, 124, 88, 88, 28, -22), ...box(124, 64, 88, 88, 28, -22)],
  "mate-cylindrical": [
    { e: [128, 40, 32, 16] },
    { l: [[96, 40], [96, 216]] },
    { l: [[160, 40], [160, 216]] },
    { a: [128, 216, 32, 16, 0, 180] },
    { a: [128, 128, 88, 32, 20, 160] },
    { l: [[48, 140], [60, 116]] },
    ...arrow(224, 200, 224, 64),
  ],
  "mate-planar": [
    { l: [[24, 184], [88, 120], [232, 120], [168, 184], [24, 184]] },
    ...arrow(128, 152, 128, 40),
    ...arrow(128, 152, 204, 152),
  ],
  "mate-ball": [
    { e: [128, 88, 48, 48] },
    { a: [128, 152, 88, 80, 20, 160] },
    { l: [[128, 136], [128, 176]] },
  ],
  "datum-plane": [
    { l: [[16, 192], [80, 64], [240, 64], [176, 192], [16, 192]] },
    { l: [[104, 128], [152, 128]] },
    { l: [[128, 104], [128, 152]] },
  ],
  "datum-axis": [
    ...dashes(128, 64, 240, 40, 20, 16),
    ...arrow(128, 64, 128, 16),
    { e: [128, 144, 72, 24] },
  ],
  "datum-point": [
    { l: [[40, 128], [216, 128]] },
    { l: [[128, 40], [128, 216]] },
    { e: [128, 128, 28, 28], fill: true },
  ],
  path: [
    { a: [80, 128, 48, 48, 180, 360] },
    { a: [176, 128, 48, 48, 0, 180] },
    { e: [32, 128, 16, 16], fill: true },
    { e: [224, 128, 16, 16], fill: true },
  ],
  sweep: [
    { e: [64, 176, 32, 48] },
    { a: [192, 176, 128, 104, 180, 270] },
    { e: [192, 72, 48, 32] },
  ],
  loft: [
    { l: [[40, 208], [136, 208], [216, 176], [120, 176], [40, 208]] },
    { e: [128, 64, 64, 20] },
    { l: [[40, 208], [64, 64]] },
    { l: [[216, 176], [192, 64]] },
  ],
  shell: [
    ...box(40, 128, 144, 88, 56, -56),
    { l: [[72, 120], [108, 84], [208, 84], [172, 120], [72, 120]] },
  ],
  offset: [
    { l: [[96, 104], [160, 104], [160, 168], [96, 168], [96, 104]], w: 10 },
    { l: [[96, 56], [160, 56]] },
    { a: [160, 104, 48, 48, 270, 360] },
    { l: [[208, 104], [208, 168]] },
    { a: [160, 168, 48, 48, 0, 90] },
    { l: [[160, 216], [96, 216]] },
    { a: [96, 168, 48, 48, 90, 180] },
    { l: [[48, 168], [48, 104]] },
    { a: [96, 104, 48, 48, 180, 270] },
  ],
  "mirror-body": [
    ...dashes(128, 24, 236, 40, 20, 12),
    { l: [[104, 48], [32, 48], [32, 120], [72, 152], [72, 208], [104, 208]] },
    { l: [[152, 48], [224, 48], [224, 120], [184, 152], [184, 208], [152, 208]] },
  ],
  split: [
    ...box(56, 80, 136, 48, 40, -32),
    ...box(24, 184, 136, 48, 40, -32),
    ...[16, 64, 112, 160, 208].map((x) => ({ l: [[x, 140], [x + 28, 140]], w: 10 })),
  ],
  "base-flange": [
    ...box(24, 144, 160, 40, 48, -48),
  ],
  "edge-flange": [
    { l: [[32, 176], [176, 176]] },
    { a: [176, 168, 8, 8, 0, 90] },
    { l: [[184, 168], [184, 40], [216, 40], [216, 168]] },
    { a: [176, 168, 40, 40, 0, 90] },
    { l: [[176, 208], [32, 208], [32, 176]] },
  ],
  unfold: [
    { l: [[24, 160], [232, 160], [232, 200], [24, 200], [24, 160]] },
    ...[148, 172, 196].map((y) => ({ l: [[136, y], [136, y + 12]], w: 10 })),
    { a: [136, 160, 80, 80, 270, 360] },
    { l: [[200, 140], [216, 160], [232, 140]] },
  ],
  refold: [
    { l: [[24, 160], [232, 160], [232, 200], [24, 200], [24, 160]] },
    ...[148, 172, 196].map((y) => ({ l: [[136, y], [136, y + 12]], w: 10 })),
    { a: [136, 160, 80, 80, 270, 360] },
    { l: [[156, 64], [136, 80], [156, 96]] },
  ],
  sheet: [
    { l: [[40, 96], [88, 72], [136, 88], [184, 64]] },
    { l: [[184, 64], [216, 176]] },
    { l: [[216, 176], [168, 200], [120, 184], [72, 208]] },
    { l: [[72, 208], [40, 96]] },
  ],
  "planar-face": [
    { e: [128, 128, 96, 64] },
    { l: [[72, 160], [120, 96]], w: 10 },
    { l: [[112, 176], [168, 96]], w: 10 },
    { l: [[160, 168], [192, 120]], w: 10 },
  ],
  fill: [
    { l: [[40, 64], [128, 40], [216, 48]] },
    { l: [[216, 48], [232, 128], [200, 208]] },
    { l: [[200, 208], [128, 224], [56, 200]] },
    { l: [[56, 200], [24, 128], [40, 64]] },
    { l: [[128, 40], [136, 128], [128, 224]], w: 10 },
    { l: [[24, 128], [128, 120], [232, 128]], w: 10 },
  ],
  extract: [
    ...box(40, 160, 128, 56, 48, -40),
    { l: [[40, 96], [168, 96], [216, 56], [88, 56], [40, 96]] },
  ],
  sew: [
    { l: [[24, 72], [112, 72], [112, 200], [24, 200], [24, 72]] },
    { l: [[144, 72], [232, 72], [232, 200], [144, 200], [144, 72]] },
    ...[96, 136, 176].map((y) => ({ l: [[96, y - 12], [160, y + 12]], w: 10 })),
  ],
  thicken: [
    { l: [[24, 184], [80, 160], [176, 160], [232, 184]] },
    { l: [[24, 136], [80, 112], [176, 112], [232, 136]] },
    { l: [[24, 136], [24, 184]] },
    { l: [[232, 136], [232, 184]] },
    ...arrow(128, 96, 128, 24),
  ],
  draft: [
    { l: [[48, 216], [88, 56], [168, 56], [208, 216], [48, 216]] },
    ...dashes(88, 76, 216, 40, 20, 10),
    { a: [88, 56, 104, 104, 90, 104] },
  ],
  "move-face": [
    ...box(32, 144, 136, 72, 48, -40),
    { l: [[32, 72], [168, 72], [216, 32], [80, 32], [32, 72]], w: 10 },
    ...arrow(120, 136, 120, 84),
  ],
  "replace-face": [
    ...box(40, 128, 144, 88, 48, -40),
    { a: [136, 108, 96, 40, 200, 340], w: 10 },
  ],
  drawing: [
    { l: [[40, 24], [216, 24], [216, 232], [40, 232], [40, 24]] },
    { l: [[128, 232], [128, 192], [216, 192]] },
    { l: [[72, 64], [176, 64], [176, 152], [72, 152], [72, 64]], w: 10 },
    { e: [124, 108, 20, 20], w: 10 },
  ],
  "delete-face": [
    ...box(32, 136, 144, 88, 48, -40),
    { e: [128, 116, 32, 10], w: 10 },
    { l: [[176, 24], [224, 72]], w: 12 },
    { l: [[224, 24], [176, 72]], w: 12 },
  ],
  "delete-blend": [
    { l: [[24, 208], [24, 64], [232, 64], [232, 208], [24, 208]] },
    { a: [136, 160, 96, 96, 270, 360], w: 10 },
    { l: [[56, 120], [104, 168]], w: 12 },
    { l: [[104, 120], [56, 168]], w: 12 },
  ],
  "resize-blend": [
    { l: [[136, 208], [24, 208], [24, 64], [136, 64]] },
    { a: [136, 160, 96, 96, 270, 360] },
    { l: [[232, 160], [232, 208], [136, 208]] },
    { l: [[136, 64], [176, 64]], w: 10 },
    { l: [[232, 120], [232, 160]], w: 10 },
    { a: [176, 120, 56, 56, 270, 360], w: 10 },
    { l: [[136, 160], [204, 92]], w: 10 },
  ],
};

function arcPoint(cx, cy, rx, ry, deg) {
  const t = (deg * Math.PI) / 180;
  return [cx + rx * Math.cos(t), cy + ry * Math.sin(t)];
}

function strokePath(s) {
  const f = (n) => Math.round(n * 100) / 100;
  if (s.l) return "M" + s.l.map(([x, y]) => `${f(x)} ${f(y)}`).join("L");
  if (s.e) {
    const [cx, cy, rx, ry] = s.e;
    return `M${f(cx - rx)} ${f(cy)}a${f(rx)} ${f(ry)} 0 1 0 ${f(2 * rx)} 0a${f(rx)} ${f(ry)} 0 1 0 ${f(-2 * rx)} 0Z`;
  }
  if (s.a) {
    const [cx, cy, rx, ry, a0, a1] = s.a;
    const [x0, y0] = arcPoint(cx, cy, rx, ry, a0);
    const [x1, y1] = arcPoint(cx, cy, rx, ry, a1);
    const large = a1 - a0 > 180 ? 1 : 0;
    return `M${f(x0)} ${f(y0)}A${f(rx)} ${f(ry)} 0 ${large} 1 ${f(x1)} ${f(y1)}`;
  }
  throw new Error("unknown stroke " + JSON.stringify(s));
}

// An inline SVG of a glyph, `px` square, in currentColor.
function glyphSvg(name, px) {
  const g = GLYPHS[name];
  if (!g) throw new Error("no ryucad glyph " + name);
  const paths = g
    .map((s) => s.fill
      ? `<path d="${strokePath(s)}" fill="currentColor"/>`
      : `<path d="${strokePath(s)}" fill="none" stroke="currentColor" stroke-width="${s.w || STROKE}" stroke-linecap="round" stroke-linejoin="round"/>`)
    .join("");
  return `<svg class="rg" width="${px}" height="${px}" viewBox="0 0 256 256" aria-hidden="true">${paths}</svg>`;
}

// The points of every stroke that are more than 0..256 once half the
// stroke is added: none, or the glyph leaves Phosphor's grid.
function checkGrid() {
  const out = [];
  for (const [name, g] of Object.entries(GLYPHS)) {
    for (const s of g) {
      const half = (s.w || STROKE) / 2;
      let pts = s.l || [];
      if (s.e) { const [cx, cy, rx, ry] = s.e; pts = [[cx - rx, cy - ry], [cx + rx, cy + ry]]; }
      if (s.a) { const [cx, cy, rx, ry, a0, a1] = s.a; pts = []; for (let d = a0; d <= a1; d += 5) pts.push(arcPoint(cx, cy, rx, ry, d)); }
      for (const [x, y] of pts) {
        if (x - half < 0 || y - half < 0 || x + half > 256 || y + half > 256) out.push(`${name} ${Math.round(x)},${Math.round(y)}`);
      }
    }
  }
  return out;
}
