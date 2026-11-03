"""Checks blind.py's shuffle and its mapping back: three pairs prepared
under a fixed seed land in the same order every time, judges' answers by
A and B map back to which image was which, and the tally counts each
pair's majority.

Run: python3 -I scripts/schematic-round/test-blind.py
"""

import importlib.machinery
import importlib.util
import json
import os
import sys
import tempfile

path = os.path.join(os.path.dirname(os.path.abspath(__file__)), "blind.py")
loader = importlib.machinery.SourceFileLoader("blind", path)
spec = importlib.util.spec_from_loader("blind", loader)
blind = importlib.util.module_from_spec(spec)
loader.exec_module(blind)

failed = 0


def check(what, ok):
    global failed
    if not ok:
        failed += 1
        print("FAIL", what)


with tempfile.TemporaryDirectory() as d:
    imgs = os.path.join(d, "imgs")
    os.makedirs(imgs)
    pairs = []
    for i in range(3):
        for side in ("ours", "ref"):
            with open(os.path.join(imgs, f"{i}-{side}.png"), "wb") as f:
                f.write(f"{i}-{side}".encode())
        pairs.append({"id": f"p{i}", "first": os.path.join(imgs, f"{i}-ours.png"),
                      "second": os.path.join(imgs, f"{i}-ref.png")})
    spec_path = os.path.join(d, "pairs.json")
    with open(spec_path, "w") as f:
        json.dump(pairs, f)
    out1, out2 = os.path.join(d, "o1"), os.path.join(d, "o2")
    blind.prepare(out1, 236, spec_path)
    blind.prepare(out2, 236, spec_path)
    k1 = json.load(open(os.path.join(out1, "key.json")))
    k2 = json.load(open(os.path.join(out2, "key.json")))
    check("the same seed, the same order", k1 == k2)
    check("every pair keyed", sorted(k1) == ["p0", "p1", "p2"])
    for pid, key in k1.items():
        a = open(os.path.join(out1, pid, "A.png"), "rb").read().decode()
        check(f"{pid}: A is what the key says", a.endswith("-" + ("ours" if key["A"] == "first" else "ref")))
        prompt = open(os.path.join(out1, pid, "prompt.txt")).read()
        check(f"{pid}: the prompt is the rubric", "paper fit" in prompt and "ryueda" not in prompt)
    # Judges: on every pair, two prefer the first image and one the
    # second, whichever letter each landed on.
    scores = {c: 3 for c in blind.CRITERIA}
    for pid, key in k1.items():
        first = "A" if key["A"] == "first" else "B"
        second = "B" if first == "A" else "A"
        for n, better in enumerate([first, first, second]):
            with open(os.path.join(out1, pid, f"judge-{n + 1}.json"), "w") as f:
                json.dump({"better": better, "scores": {"A": scores, "B": scores}, "why": "x"}, f)
    t = blind.tally(out1)
    for pid in k1:
        check(f"{pid}: the majority is the first", t[pid]["majority"] == "first")
        check(f"{pid}: votes counted", t[pid]["votes"] == {"first": 2, "second": 1, "same": 0})
    check("tally files written", os.path.exists(os.path.join(out1, "tally.json"))
          and os.path.exists(os.path.join(out1, "tally.md")))

print("ok" if not failed else f"{failed} failed")
sys.exit(1 if failed else 0)
