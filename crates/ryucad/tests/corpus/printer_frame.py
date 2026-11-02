"""A printer's frame: aluminium extrusions (a slotted profile swept to a
length each part sets as a parameter), corner brackets (an L swept, its
inner corner rounded, drilled through both legs) and T-slot bolts (a
head, a shank and a hex socket), assembled as twelve rails held where
they are, eight brackets fastened to the uprights and four rows of bolts
along the top rails, each row an instance pattern of one bolt fastened to
its rail. Through ryucad's CLI only; see model.py.
"""

import os
import sys

sys.dont_write_bytecode = True
sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
from model import main  # noqa: E402

Z = (0, 0, 1)
X = (1, 0, 0)
# Outer size of the frame (mm): x, y, z; the profile is 20 mm square.
SIZE = (400, 360, 420)
RAILS = {"rail-x": SIZE[0] - 40, "rail-y": SIZE[1] - 40, "rail-z": SIZE[2]}
PROFILE = [(0, 0), (7, 0), (7, 6), (13, 6), (13, 0), (20, 0), (20, 7), (14, 7), (14, 13),
           (20, 13), (20, 20), (13, 20), (13, 14), (7, 14), (7, 20), (0, 20), (0, 13),
           (6, 13), (6, 7), (0, 7)]


def polygon(points):
    ops = [{"op": "add", "type": "point", "id": f"p{i + 1}", "at": [f"{x} mm", f"{y} mm"]}
           for i, (x, y) in enumerate(points)]
    n = len(points)
    ops += [{"op": "add", "type": "line", "id": f"l{i + 1}", "start": f"p{i + 1}",
             "end": f"p{(i + 1) % n + 1}"} for i in range(n)]
    return ops


def rail(p, pid, length):
    r = p.part(pid)
    p.run("param", "set", pid, f"length={length} mm")
    ops = polygon(PROFILE)
    ops += [{"op": "add", "type": "point", "id": "c", "at": ["10 mm", "10 mm"]},
            {"op": "add", "type": "circle", "id": "core", "center": "c", "r": "2.1 mm"}]
    sk = p.sketch(r, ops, fid="profile")
    p.add(r, "extrude", "body", sketch=sk, length="length")


def bracket(p):
    b = p.part("corner-bracket")
    # An L in the zx plane (sketch x along z, y along x), swept along y.
    sk = p.sketch(b, polygon([(0, 0), (20, 0), (20, 3), (3, 3), (3, 20), (0, 20)]),
                  fid="profile", plane="zx")
    p.add(b, "extrude", "body", sketch=sk, length="18 mm")
    inner = p.edges(b, "body", lambda e: e["curve"]["type"] == "line"
                    and abs(abs(e["curve"]["dir"][1]) - 1) < 1e-12
                    and abs(e["curve"]["origin"][0] - 0.003) < 1e-9
                    and abs(e["curve"]["origin"][2] - 0.003) < 1e-9)
    p.add(b, "fillet", "inner", radius="3 mm", edge=inner)
    flat = p.sketch(b, [{"op": "add", "type": "point", "id": "p1", "at": ["12 mm", "9 mm"]}],
                    fid="flat", place=((0, 0, 3), Z, X))
    p.add(b, "hole", "flat-hole", sketch=flat, point="p1", diameter="5.5 mm",
          depth="through_all")
    up = p.sketch(b, [{"op": "add", "type": "point", "id": "p1", "at": ["9 mm", "12 mm"]}],
                  fid="upright", place=((3, 0, 0), (1, 0, 0), (0, 1, 0)))
    p.add(b, "hole", "upright-hole", sketch=up, point="p1", diameter="5.5 mm",
          depth="through_all")


def bolt(p):
    b = p.part("bolt")
    p.add(b, "cylinder", "head", {"radius": 4, "height": 4}, ((0, 0, 0), Z, X))
    p.add(b, "cylinder", "shank", {"radius": 2.5, "height": 5}, ((0, 0, -5), Z, X),
          operation="join")
    hexagon = [(2 * c, 2 * s) for c, s in ((1, 0), (0.5, 0.8660254), (-0.5, 0.8660254),
                                         (-1, 0), (-0.5, -0.8660254), (0.5, -0.8660254))]
    sk = p.sketch(b, polygon(hexagon), fid="socket-profile", place=((0, 0, 4), Z, X))
    p.add(b, "extrude", "socket", sketch=sk, length="2.5 mm", reverse=True, operation="cut")


def frame(p):
    a = p.part("frame", assembly=True)
    sx, sy, sz = SIZE
    uprights = {"z-fl": (0, 0), "z-fr": (sx - 20, 0), "z-bl": (0, sy - 20),
                "z-br": (sx - 20, sy - 20)}
    for iid, (x, y) in uprights.items():
        p.add(a, "instance", iid, of="rail-z", origin=f"{x}mm,{y}mm,0mm", fixed=True)
    for level, z in (("bottom", 0), ("top", sz - 20)):
        for side, y in (("front", 0), ("back", sy - 20)):
            p.add(a, "instance", f"x-{level}-{side}", of="rail-x",
                  origin=f"20mm,{y}mm,{z}mm", z="1,0,0", x="0,1,0", fixed=True)
        for side, x in (("left", 0), ("right", sx - 20)):
            p.add(a, "instance", f"y-{level}-{side}", of="rail-y",
                  origin=f"{x}mm,20mm,{z}mm", z="0,1,0", x="0,0,1", fixed=True)
    # Brackets in the corners where the front and back rails meet the
    # uprights: placed by fastened mates to the upright, turned about z for
    # the right-hand side and over for the top.
    n = 0
    for up in uprights:
        right = up.endswith("r")
        for top in (False, True):
            n += 1
            iid = f"bracket-{n}"
            p.add(a, "instance", iid, of="corner-bracket")
            x = 0 if right else 20
            if not top:
                y, rotate, flip = (19 if right else 1), 180 if right else 0, False
            else:
                y, rotate, flip = (1 if right else 19), 180 if right else 0, True
            z = sz - 20 if top else 20
            offset = (f'{{"origin": ["{x} mm", "{y} mm", "{z} mm"], "rotate": "{rotate} deg"'
                      + (', "flip": true}' if flip else "}"))
            p.add(a, "mate", f"hold-{n}", form="fastened", a=f"frame/{up}",
                  a_offset=offset, b=f"frame/{iid}", b_offset="{}")
    # A row of T-slot bolts along each top rail: one bolt fastened in the
    # rail's top slot, then an instance pattern along the rail.
    rows = (("x-top-front", "rail-x", "x"), ("x-top-back", "rail-x", "x"),
            ("y-top-left", "rail-y", "y"), ("y-top-right", "rail-y", "y"))
    for k, (on, part, direction) in enumerate(rows):
        top = p.faces(part, "body", lambda f: f["surface"]["type"] == "plane"
                      and abs(f["surface"]["frame"]["z"][1]) > 0.99
                      and abs(f["surface"]["frame"]["origin"][1] - 0.02) < 1e-9
                      and not f["reversed"])
        iid = f"bolt-{k + 1}"
        p.add(a, "instance", iid, of="bolt")
        p.add(a, "mate", f"seat-{k + 1}", form="fastened",
              a=f"frame/{on}/" + top[0].split("/", 1)[1],
              a_offset='{"origin": ["10 mm", "-30 mm", "0"]}', b=f"frame/{iid}",
              b_offset="{}")
        p.add(a, "instance_pattern", f"row-{k + 1}", instance=iid, form="linear",
              direction=direction, spacing="40 mm", count="8")


def build(p):
    p.run("init", "--unit", "mm")
    for pid, length in RAILS.items():
        rail(p, pid, length)
    bracket(p)
    bolt(p)
    frame(p)


main(build)
