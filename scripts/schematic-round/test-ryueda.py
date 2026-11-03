"""Checks the round's ryueda wrapper keeps sessions to their arms.

Run: python3 -I scripts/schematic-round/test-ryueda.py
"""

import importlib.machinery
import importlib.util
import os
import sys

path = os.path.join(os.path.dirname(os.path.abspath(__file__)), "ryueda")
loader = importlib.machinery.SourceFileLoader("round_ryueda", path)
spec = importlib.util.spec_from_loader("round_ryueda", loader)
wrapper = importlib.util.module_from_spec(spec)
loader.exec_module(wrapper)
check = wrapper.check

CASES = [
    # (args, arm, see, refused)
    (["place", "b", "--dry-run", "--move", "sheet R1 S2", "--render", "dry.png"], "moves", "png", False),
    (["place", "b", "--dry-run", "--move=sheet R1 S2", "--render=dry.png"], "moves", "png", False),
    (["place", "b", "--move", "x", "--render", "d.svg", "--format", "svg", "--scale", "8"], "moves", "png", False),
    (["place", "b", "--move", "x", "--format=svg", "--scale=8", "--render", "d.svg"], "moves", "png", False),
    (["-C", "proj", "place", "b", "--move", "x"], "moves", "png", False),
    (["place", "b", "sheet R1 S2"], "moves", "png", True),
    (["place", "b", "sheet R1 S2", "--render", "d.png"], "lasting", "png", False),
    (["place", "b", "sheet R1 S2", "--move", "x"], "lasting", "png", True),
    (["place", "b", "sheet R1 S2", "--move=x"], "lasting", "png", True),
    (["place", "b", "sheet R1 S2", "--render=d.png"], "lasting", "describe", True),
    (["render", "--out", "x.png"], "lasting", "describe", True),
    (["describe", "--block", "b"], "lasting", "describe", False),
    (["place", "b", "--list"], "moves", "png", False),
    (["layout", "--block", "b", "--dry-run", "--render", "d.png"], "moves", "describe", True),
    (["layout", "--block", "b", "--dry-run", "--render", "d.png"], "moves", "png", False),
]

failed = 0
for args, arm, see, refused in CASES:
    got = check(args, arm, see) is not None
    if got != refused:
        failed += 1
        print(f"FAIL {args} ({arm}, {see}): refused {got}, expected {refused}")
print(f"{len(CASES) - failed}/{len(CASES)} passed")
sys.exit(1 if failed else 0)
