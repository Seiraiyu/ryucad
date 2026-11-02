"""A nine-cylinder radial engine, as issue #25's agent built one: 13
parts of primitives joined and cut (crankcase 53 features, cyl-1 to cyl-9
48 each, exhaust 3, mount 29, propeller 62; 579 in all), each part
modelled in place about the engine's axis (z), and an assembly placing
all 13 at the origin. Through ryucad's CLI only; see model.py.
"""

import math
import os
import sys

sys.dont_write_bytecode = True
sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
from model import main, polar  # noqa: E402

CYLINDERS = 9
Z = (0, 0, 1)
X = (1, 0, 0)


def radial(deg):
    """The frame of a cylinder at `deg`: z outwards, x along the axis."""
    d = polar(1, deg)
    return d, (0, 0, 1)


def crankcase(p):
    c = p.part("crankcase")
    p.add(c, "cylinder", "case", {"radius": 60, "height": 80}, ((0, 0, -40), Z, X))
    p.add(c, "cylinder", "flange", {"radius": 72, "height": 8}, ((0, 0, -48), Z, X),
          operation="join")
    for k in range(CYLINDERS):
        out, x = radial(k * 360 / CYLINDERS)
        p.add(c, "cylinder", f"boss-{k + 1}", {"radius": 22, "height": 20},
              (tuple(50 * a for a in out), out, x), operation="join")
    for k in range(CYLINDERS):
        out, x = radial(k * 360 / CYLINDERS)
        p.add(c, "cylinder", f"bore-{k + 1}", {"radius": 15, "height": 40},
              (tuple(40 * a for a in out), out, x), operation="cut")
    p.add(c, "cone", "nose", {"radius": 40, "height": 30, "top_radius": 18},
          ((0, 0, 40), Z, X), operation="join")
    for k in range(24):
        p.add(c, "cylinder", f"flange-hole-{k + 1}", {"radius": 2.5, "height": 12},
              (polar(66, k * 15, -50), Z, X), operation="cut")
    for k in range(8):
        p.block(c, f"pad-{k + 1}", (16, 16, 6), polar(64, 22.5 + k * 45, -51), Z,
                polar(1, 22.5 + k * 45), operation="join")


def cylinder(p, k):
    c = p.part(f"cyl-{k + 1}")
    deg = k * 360 / CYLINDERS
    out, x = radial(deg)

    def at(r, side=0.0, up=0.0):
        a = polar(1, deg + 90)
        return tuple(r * o + side * s + up * z for o, s, z in zip(out, a, (0, 0, 1)))

    p.add(c, "cylinder", "barrel", {"radius": 20, "height": 90}, (at(70), out, x))
    for f in range(38):
        p.add(c, "cylinder", f"fin-{f + 1}", {"radius": 26, "height": 1.2},
              (at(80 + 2 * f), out, x), operation="join")
    p.add(c, "cylinder", "head", {"radius": 24, "height": 20}, (at(160), out, x),
          operation="join")
    p.add(c, "cylinder", "bore", {"radius": 16, "height": 80}, (at(70), out, x),
          operation="cut")
    for s, name in ((-1, "intake"), (1, "exhaust")):
        p.block(c, f"rocker-box-{name}", (14, 14, 14), at(184, 0, 10 * s), out, x,
                operation="join")
        p.block(c, f"rocker-cover-{name}", (16, 16, 3), at(192, 0, 10 * s), out, x,
                operation="join")
        p.add(c, "cylinder", f"plug-{name}", {"radius": 4, "height": 16},
              (at(170, 12 * s, 0), polar(1, deg + 90 * s), out), operation="cut")
    p.add(c, "cylinder", "port", {"radius": 7, "height": 22},
          (at(170, 0, -10), (0, 0, -1), out), operation="join")


def exhaust(p):
    c = p.part("exhaust")
    p.add(c, "torus", "ring", {"major_radius": 150, "minor_radius": 8}, ((0, 0, -45), Z, X))
    p.add(c, "torus", "pipe", {"major_radius": 150, "minor_radius": 6}, ((0, 0, -45), Z, X),
          operation="cut")
    p.add(c, "cylinder", "outlet", {"radius": 8, "height": 60},
          ((150, 0, -45), (0, 0, -1), X), operation="join")


def mount(p):
    c = p.part("mount")
    p.add(c, "cylinder", "plate", {"radius": 100, "height": 6}, ((0, 0, -70), Z, X))
    p.add(c, "cylinder", "opening", {"radius": 62, "height": 10}, ((0, 0, -72), Z, X),
          operation="cut")
    for k in range(CYLINDERS):
        d = polar(1, 20 + k * 40)
        p.block(c, f"arm-{k + 1}", (70, 10, 6), polar(130, 20 + k * 40, -67), Z, d,
                operation="join")
    for k in range(18):
        p.add(c, "cylinder", f"hole-{k + 1}", {"radius": 3, "height": 10},
              (polar(84, 10 + k * 20, -72), Z, X), operation="cut")


def propeller(p):
    c = p.part("propeller")
    p.add(c, "cylinder", "hub", {"radius": 18, "height": 40}, ((0, 0, 75), Z, X))
    p.add(c, "cone", "spinner", {"radius": 18, "height": 30, "top_radius": 2},
          ((0, 0, 115), Z, X), operation="join")
    for b, side in enumerate((1, -1)):
        for s in range(30):
            t = math.radians(35 - s)
            p.block(c, f"blade-{b + 1}-{s + 1}", (13, 40 - s, 4),
                    (side * (22 + 12 * s), 0, 95), (0, -math.sin(t), math.cos(t)),
                    (side, 0, 0), operation="join")


def engine(p):
    a = p.part("engine", assembly=True)
    parts = ["crankcase"] + [f"cyl-{k + 1}" for k in range(CYLINDERS)]
    parts += ["exhaust", "mount", "propeller"]
    for part in parts:
        p.add(a, "instance", part, of=part, fixed=True)


def build(p):
    p.run("init", "--unit", "mm")
    crankcase(p)
    for k in range(CYLINDERS):
        cylinder(p, k)
    exhaust(p)
    mount(p)
    propeller(p)
    engine(p)


main(build)
