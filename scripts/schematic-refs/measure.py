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
