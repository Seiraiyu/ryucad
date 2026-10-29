// ryucad's 15 CAD glyphs (design 199, Fonts and icons), on Phosphor's
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
