"""A two-stage gearbox: a housing (a block hollowed out, its vertical
edges rounded, bearing bores through both walls and a bolt pattern round
its rim), three stepped shafts revolved from sketches, four spur gears
(a disc with its tooth gaps cut once and patterned round it, and a bore),
and an assembly holding the housing fixed, each shaft on a revolute in its
bores and each gear fastened to its shaft. Through ryucad's CLI only;
see model.py.
"""

import os
import sys

sys.dont_write_bytecode = True
sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
from model import main, polar  # noqa: E402

Z = (0, 0, 1)
X = (1, 0, 0)
Y = (0, 1, 0)

# Shaft centres along x, in the housing's frame (mm), and their radii.
SHAFTS = {"input": (-55, 10), "middle": (0, 12), "output": (60, 15)}
# Gears: (shaft, teeth, pitch radius, width, where along the shaft).
GEARS = {
    "pinion-1": ("input", 14, 21, 18, 40),
    "wheel-1": ("middle", 22, 34, 18, 40),
    "pinion-2": ("middle", 16, 24, 22, 85),
    "wheel-2": ("output", 24, 36, 22, 85),
}
LENGTH = 150  # each shaft, along the housing's y


def polygon(points, prefix=""):
    """Sketch ops for a closed polygon: points p1.., lines l1.."""
    ops = []
    for i, (x, y) in enumerate(points):
        ops.append({"op": "add", "type": "point", "id": f"{prefix}p{i + 1}",
                    "at": [f"{x} mm", f"{y} mm"]})
    n = len(points)
    for i in range(n):
        ops.append({"op": "add", "type": "line", "id": f"{prefix}l{i + 1}",
                    "start": f"{prefix}p{i + 1}", "end": f"{prefix}p{(i + 1) % n + 1}"})
    return ops


def housing(p):
    h = p.part("housing")
    p.block(h, "shell", (224, 110, 120), (0, 0, 60), Z, X)
    p.block(h, "cavity", (212, 98, 120), (0, 0, 66), Z, X, operation="cut")
    corners = p.edges(h, "shell", lambda e: e["curve"]["type"] == "line"
                      and abs(e["curve"]["dir"][2]) > 0.99
                      and abs(abs(e["curve"]["origin"][0]) - 0.112) < 1e-6
                      and abs(abs(e["curve"]["origin"][1]) - 0.055) < 1e-6)
    p.add(h, "fillet", "corners", radius="8 mm", edge=corners)
    for name, (x, r) in SHAFTS.items():
        p.add(h, "cylinder", f"bore-{name}", {"radius": r + 6, "height": 120},
              ((x, -60, 60), Y, X), operation="cut")
    rim = p.sketch(h, [{"op": "add", "type": "point", "id": "p1",
                        "at": ["-100 mm", "-52 mm"]}],
                   fid="rim", place=((0, 0, 120), Z, X))
    p.add(h, "hole", "bolt", sketch=rim, point="p1", diameter="5 mm", depth="14 mm",
          bottom="118")
    p.add(h, "pattern", "bolts", seed="bolt", form="linear", direction="x",
          spacing="40 mm", count="6", direction2="y", spacing2="104 mm", count2="2")


def shaft(p, name, r):
    s = p.part(f"shaft-{name}")
    # A stepped profile in the zx plane (sketch x along the part's z, y
    # along its x), turned about the line along its axis, the last.
    j = r - 3
    ops = polygon([(0, 0), (0, j), (12, j), (12, r), (LENGTH - 12, r), (LENGTH - 12, j),
                   (LENGTH, j), (LENGTH, 0)])
    sk = p.sketch(s, ops, fid="profile", plane="zx")
    p.add(s, "revolve", "body", sketch=sk, axis="l8", angle="full")
    ends = p.edges(s, "body", lambda e: e["curve"]["type"] == "circle"
                   and abs(e["curve"]["radius"] - j / 1000) < 1e-9
                   and e["curve"]["frame"]["origin"][2] in (0.0, LENGTH / 1000))
    p.add(s, "chamfer", "ends", d="1 mm", edge=ends)


def gear(p, name, teeth, pitch, width, bore):
    g = p.part(name)
    disc = p.sketch(g, [{"op": "add", "type": "point", "id": "c", "at": ["0", "0"]},
                        {"op": "add", "type": "circle", "id": "rim", "center": "c",
                         "r": f"{pitch + 3} mm"}],
                    fid="blank")
    p.add(g, "extrude", "disc", sketch=disc, length=f"{width} mm")
    # One tooth gap, a trapezoid cut through the rim at +x.
    gap = 3.1416 * pitch / teeth
    root = pitch - 4
    tip = pitch + 4
    gaps = p.sketch(g, polygon([(root, -gap * 0.35), (tip, -gap * 0.6),
                                (tip, gap * 0.6), (root, gap * 0.35)]),
                    fid="gap-profile")
    p.add(g, "extrude", "gap", sketch=gaps, operation="cut", extent="through_all",
          both=True)
    p.add(g, "pattern", "teeth", seed="gap", form="circular", axis="z", count=str(teeth),
          angle="360")
    hub = p.sketch(g, [{"op": "add", "type": "point", "id": "o", "at": ["0", "0"]}],
                   fid="axis", place=((0, 0, width), Z, X))
    p.add(g, "hole", "bore", sketch=hub, point="o", diameter=f"{2 * bore} mm",
          depth="through_all")
    p.add(g, "chamfer", "edges", d="0.5 mm", edge=p.edges(
        g, "disc", lambda e: e["curve"]["type"] == "circle"
        and abs(e["curve"]["radius"] - bore / 1000) < 1e-9))


def assembly(p):
    a = p.part("gearbox", assembly=True)
    p.add(a, "instance", "housing", of="housing", fixed=True)
    for name, (x, r) in SHAFTS.items():
        p.add(a, "instance", name, of=f"shaft-{name}",
              origin=f"{x}mm,-75mm,60mm", z="0,1,0", x="1,0,0")
        bore = p.faces("housing", f"shell", lambda f: f["surface"]["type"] == "cylinder"
                       and abs(f["surface"]["radius"] - (r + 6) / 1000) < 1e-9)
        journal = p.faces(f"shaft-{name}", "body",
                          lambda f: f["surface"]["type"] == "cylinder"
                          and abs(f["surface"]["radius"] - r / 1000) < 1e-9)
        p.add(a, "mate", f"turn-{name}", form="revolute",
              a="gearbox/housing/" + bore[0].split("/", 1)[1],
              b=f"gearbox/{name}/" + journal[0].split("/", 1)[1],
              b_offset='{"origin": ["0", "0", "-75 mm"]}')
    for name, (on, _teeth, _pitch, width, along) in GEARS.items():
        x = SHAFTS[on][0]
        p.add(a, "instance", name, of=name, origin=f"{x}mm,{along - 75}mm,60mm",
              z="0,1,0", x="1,0,0")
        p.add(a, "mate", f"key-{name}", form="fastened", a=f"gearbox/{on}",
              a_offset=f'{{"origin": ["0", "0", "{along - width / 2} mm"]}}',
              b=f"gearbox/{name}", b_offset="{}")


def build(p):
    p.run("init", "--unit", "mm")
    housing(p)
    for name, (_x, r) in SHAFTS.items():
        shaft(p, name, r)
    for name, (on, teeth, pitch, width, _along) in GEARS.items():
        gear(p, name, teeth, pitch, width, SHAFTS[on][1])
    assembly(p)


main(build)
