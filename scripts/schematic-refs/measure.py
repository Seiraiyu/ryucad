#!/usr/bin/env python3
"""Measures a KiCad schematic's drawing habits: `python3 -I measure.py
FILE.kicad_sch...` prints one JSON object per file.

What it counts, per file (a KiCad sheet):
  wires        wire segments
  crossings    two wire segments crossing at a point inside both, with no
               junction there (a T at a junction is a connection, not a
               crossing; an end on another wire is a T)
  junctions    junction dots
  labels       local labels; global and hierarchical labels apart
  power        power symbols by the way they point: up (supplies drawn
               the usual way), down (grounds the usual way), sideways
  symbols      other symbols placed
  paper        the sheet's paper and its drawing area in millimetres:
               the paper less ryueda's 10 mm border on every side
               (`schem::BORDER`), so ours and the references' compare
  extent       the drawing's box over the drawing area, per mille: the
               box of every wire, junction, label, power symbol and
               symbol body (its library graphics and pins, placed)
  fill         the share of the drawing area's 10 mm cells holding ink,
               per mille: a cell a wire passes through, or a symbol's,
               label's, junction's or power symbol's box touches; ryusch's
               `lint::fill` counts ours the same way

Standard library only; never run by CI. ryueda's manifest records each
reference's crossings (`crossings <n>`), checked by eye on its render.
"""
import json
import sys


def parse(text):
    """KiCad's s-expressions as nested lists of strings."""
    out, stack, i, n = [], [], 0, len(text)
    cur = out
    while i < n:
        c = text[i]
        if c == "(":
            stack.append(cur)
            new = []
            cur.append(new)
            cur = new
            i += 1
        elif c == ")":
            cur = stack.pop()
            i += 1
        elif c.isspace():
            i += 1
        elif c == '"':
            j = i + 1
            buf = []
            while text[j] != '"':
                if text[j] == "\\":
                    j += 1
                buf.append(text[j])
                j += 1
            cur.append("".join(buf))
            i = j + 1
        else:
            j = i
            while j < n and not text[j].isspace() and text[j] not in "()":
                j += 1
            cur.append(text[i:j])
            i = j
    return out[0]


def items(node, head):
    return [x for x in node if isinstance(x, list) and x and x[0] == head]


def first(node, head):
    found = items(node, head)
    return found[0] if found else None


def key(x, y):
    # KiCad writes millimetres to four places: compare in 0.1 µm.
    return (round(float(x) * 10000), round(float(y) * 10000))


PAPERS = {
    "A4": (297, 210), "A3": (420, 297), "A2": (594, 420),
    "A1": (841, 594), "A0": (1189, 841), "A": (279.4, 215.9),
    "B": (431.8, 279.4), "C": (558.8, 431.8), "D": (863.6, 558.8),
    "E": (1117.6, 863.6),
}
BORDER = 10.0
CELL = 10.0


def lib_boxes(root):
    """Each library symbol's box in its own frame (y up), from its
    graphics and its pins' lines."""
    out = {}
    lib = first(root, "lib_symbols")
    for sym in items(lib, "symbol") if lib else []:
        pts = []

        def walk(node):
            for x in node:
                if not isinstance(x, list) or not x:
                    continue
                h = x[0]
                if h == "rectangle":
                    for k in ("start", "end"):
                        a = first(x, k)
                        pts.append((float(a[1]), float(a[2])))
                elif h in ("polyline", "bezier"):
                    for q in items(first(x, "pts"), "xy"):
                        pts.append((float(q[1]), float(q[2])))
                elif h == "circle":
                    c, r = first(x, "center"), float(first(x, "radius")[1])
                    cx, cy = float(c[1]), float(c[2])
                    pts.extend([(cx - r, cy - r), (cx + r, cy + r)])
                elif h == "arc":
                    for k in ("start", "mid", "end"):
                        a = first(x, k)
                        pts.append((float(a[1]), float(a[2])))
                elif h == "pin":
                    a = first(x, "at")
                    ln = float(first(x, "length")[1])
                    px, py, ang = float(a[1]), float(a[2]), int(float(a[3])) % 360
                    dx, dy = {0: (1, 0), 90: (0, 1), 180: (-1, 0), 270: (0, -1)}[ang]
                    pts.extend([(px, py), (px + dx * ln, py + dy * ln)])
                elif h == "symbol":
                    walk(x)
        walk(sym)
        if pts:
            xs, ys = [q[0] for q in pts], [q[1] for q in pts]
            out[sym[1]] = (min(xs), min(ys), max(xs), max(ys))
    return out


def placed_box(box, at, mirror):
    """A library box placed on the sheet (y down): mirrored, turned
    counterclockwise by the placement's angle, then moved."""
    x0, y0, x1, y1 = box
    corners = [(x0, y0), (x0, y1), (x1, y0), (x1, y1)]
    rot = int(float(at[3])) % 360 if len(at) > 3 else 0
    out = []
    for x, y in corners:
        if mirror == "x":
            y = -y
        elif mirror == "y":
            x = -x
        for _ in range(rot // 90):
            x, y = -y, x
        out.append((float(at[1]) + x, float(at[2]) - y))
    xs, ys = [q[0] for q in out], [q[1] for q in out]
    return (min(xs), min(ys), max(xs), max(ys))


def paper_fill(root, segs):
    """The paper, the drawing's extent and its fill, as the module's doc
    says."""
    paper = first(root, "paper")
    name = paper[1] if paper else "A4"
    w, h = PAPERS.get(name, PAPERS["A4"])
    if paper and "portrait" in paper:
        w, h = h, w
    area = (BORDER, BORDER, w - BORDER, h - BORDER)
    boxes = []
    libs = lib_boxes(root)
    for s in items(root, "symbol"):
        lib, at = first(s, "lib_id"), first(s, "at")
        if lib is None or at is None:
            continue
        m = first(s, "mirror")
        box = libs.get(lib[1])
        if box is None:
            x, y = float(at[1]), float(at[2])
            box, at, m = (-1, -1, 1, 1), ["at", x, y], None
        boxes.append(placed_box(box, at, m[1] if m else None))
    for head in ("label", "global_label", "hierarchical_label", "junction"):
        for x in items(root, head):
            a = first(x, "at")
            px, py = float(a[1]), float(a[2])
            boxes.append((px - 1, py - 1, px + 1, py + 1))
    for (a, b) in segs:
        ax, ay, bx, by = a[0] / 10000, a[1] / 10000, b[0] / 10000, b[1] / 10000
        boxes.append((min(ax, bx), min(ay, by), max(ax, bx), max(ay, by)))
    cols = int((area[2] - area[0]) // CELL)
    rows = int((area[3] - area[1]) // CELL)
    cells = set()
    for x0, y0, x1, y1 in boxes:
        c0 = max(0, int((x0 - area[0]) // CELL))
        c1 = min(cols - 1, int((x1 - area[0]) // CELL))
        r0 = max(0, int((y0 - area[1]) // CELL))
        r1 = min(rows - 1, int((y1 - area[1]) // CELL))
        for c in range(c0, c1 + 1):
            for r in range(r0, r1 + 1):
                cells.add((c, r))
    if boxes:
        ex = (max(b[2] for b in boxes) - min(b[0] for b in boxes)) * (
            max(b[3] for b in boxes) - min(b[1] for b in boxes))
    else:
        ex = 0
    drawn = (area[2] - area[0]) * (area[3] - area[1])
    return {
        "paper": {"size": name, "area_mm": [area[2] - area[0], area[3] - area[1]]},
        "extent": round(1000 * ex / drawn),
        "fill": round(1000 * len(cells) / (cols * rows)),
    }


def measure(path):
    root = parse(open(path, encoding="utf-8").read())
    segs = []
    for w in items(root, "wire"):
        pts = [key(p[1], p[2]) for p in items(first(w, "pts"), "xy")]
        for a, b in zip(pts, pts[1:]):
            segs.append((a, b))
    junctions = {key(*first(j, "at")[1:3]) for j in items(root, "junction")}

    def inside(p, s):
        (ax, ay), (bx, by) = s
        if ax == bx == p[0]:
            return min(ay, by) < p[1] < max(ay, by)
        if ay == by == p[1]:
            return min(ax, bx) < p[0] < max(ax, bx)
        return False

    crossings = 0
    for i, s in enumerate(segs):
        for t in segs[i + 1:]:
            (ax, ay), (bx, by) = s
            (cx, cy), (dx, dy) = t
            if ax == bx and cy == dy:
                p = (ax, cy)
            elif ay == by and cx == dx:
                p = (cx, ay)
            else:
                continue
            if inside(p, s) and inside(p, t) and p not in junctions:
                crossings += 1
    power = {"up": 0, "down": 0, "sideways": 0}
    symbols = 0
    for s in items(root, "symbol"):
        lib = first(s, "lib_id")
        if lib is None:
            continue
        if lib[1].startswith("power:"):
            at = first(s, "at")
            rot = int(float(at[3])) % 360 if len(at) > 3 else 0
            name = lib[1].split(":", 1)[1].upper()
            ground = "GND" in name or "VSS" in name or name.startswith("EARTH")
            # A KiCad power symbol at 0 degrees points up; a ground's down.
            if rot in (90, 270):
                power["sideways"] += 1
            elif (rot == 0) != ground:
                power["up"] += 1
            else:
                power["down"] += 1
        else:
            symbols += 1
    return {
        **paper_fill(root, segs),
        "file": path,
        "wires": len(segs),
        "crossings": crossings,
        "junctions": len(junctions),
        "labels": len(items(root, "label")),
        "global_labels": len(items(root, "global_label")),
        "hierarchical_labels": len(items(root, "hierarchical_label")),
        "power": power,
        "symbols": symbols,
    }


if __name__ == "__main__":
    for f in sys.argv[1:]:
        print(json.dumps(measure(f), sort_keys=True))
