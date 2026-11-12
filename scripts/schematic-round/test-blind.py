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
    # Several sheets a side: a list of images, copied as A-1.png, A-2.png …
    multi = os.path.join(d, "multi.json")
    with open(multi, "w") as f:
        json.dump([{"id": "m", "first": [os.path.join(imgs, "0-ours.png"), os.path.join(imgs, "1-ours.png")],
                    "second": os.path.join(imgs, "0-ref.png")}], f)
    out3 = os.path.join(d, "o3")
    k3 = blind.prepare(out3, 348, multi)["m"]
    many = "A" if k3["A"] == "first" else "B"
    one = "B" if many == "A" else "A"
    check("a list's images numbered", os.path.exists(os.path.join(out3, "m", f"{many}-1.png"))
          and os.path.exists(os.path.join(out3, "m", f"{many}-2.png")))
    check("one image stays plain", os.path.exists(os.path.join(out3, "m", f"{one}.png")))
    check("the prompt names the files", f"{many}-1.png" in open(os.path.join(out3, "m", "prompt.txt")).read())
    # The target: ours (first) against the reference (second) on pairs
    # named ref-*: the same or better on two of four, and grouping and
    # paper within a point.
    out4 = os.path.join(d, "o4")
    pairs4 = [{"id": f"ref-{i}", "first": os.path.join(imgs, f"{i % 3}-ours.png"),
               "second": os.path.join(imgs, f"{i % 3}-ref.png")} for i in range(4)]
    with open(os.path.join(d, "p4.json"), "w") as f:
        json.dump(pairs4, f)
    k4 = blind.prepare(out4, 348, os.path.join(d, "p4.json"))
    for i, (pid, key) in enumerate(sorted(k4.items())):
        first = "A" if key["A"] == "first" else "B"
        second = "B" if first == "A" else "A"
        better = first if i < 2 else second
        s_first = {c: 4 for c in blind.CRITERIA}
        s_second = {c: 4 for c in blind.CRITERIA}
        for n in range(3):
            with open(os.path.join(out4, pid, f"judge-{n + 1}.json"), "w") as f:
                json.dump({"better": better, "scores": {first: s_first, second: s_second}, "why": "x"}, f)
    blind.tally(out4)
    t = blind.target(out4)
    check("the target holds on two of four", t["held"] and t["same_or_better"] == 2)

print("ok" if not failed else f"{failed} failed")
sys.exit(1 if failed else 0)
