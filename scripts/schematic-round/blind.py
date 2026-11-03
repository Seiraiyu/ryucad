"""The blind review's pairs and tally (plan 1.D, design 236's Judging).
Standard library only; never run by CI.

  python3 -I blind.py prepare OUT SEED PAIRS.json
      PAIRS.json lists {id, first, second}: two images of one circuit. For
      each pair, copies them as OUT/<id>/A.png and B.png in an order drawn
      by random.Random(SEED) (one draw per pair, in the file's order),
      writes OUT/key.json (which letter is which; never shown to a judge)
      and OUT/<id>/prompt.txt, the rubric.

  python3 -I blind.py tally OUT
      Reads each judge's OUT/<id>/judge-<n>.json ({better: A|B|same,
      scores: {A: {criterion: 1..5}, B: {...}}, why}), maps A and B back
      through key.json, and writes OUT/tally.json and OUT/tally.md: per
      pair the votes and majority (first, second or same), the mean scores
      per side and criterion, and where the judges disagreed.
"""

import json
import os
import random
import shutil
import sys

CRITERIA = ("flow", "grouping", "idioms", "crossings", "labels_vs_wires", "text", "paper")

RUBRIC = (
    "Two schematics of the same circuit, A and B, are the images A.png and "
    "B.png. Judge which a human reviewer reads more easily. Score each 1 "
    "(poor) to 5 (excellent) on: flow (signals run left to right, power up, "
    "ground down); grouping (parts that work together sit together); idioms "
    "(decoupling at its pin, pull-ups from their rail, dividers, crystals, "
    "feedback drawn as engineers draw them); crossings (few, and none where "
    "a small move would avoid them); labels_vs_wires (wires where a short "
    "wire reads better, labels where a wire would be long or crowded); text "
    "(designators and values clear, nothing overlapping); paper (paper fit: "
    "the sheet used well, nothing cramped or lost). Answer only JSON: "
    '{"better": "A"|"B"|"same", "scores": {"A": {"flow": n, "grouping": n, '
    '"idioms": n, "crossings": n, "labels_vs_wires": n, "text": n, '
    '"paper": n}, "B": {...}}, "why": "one paragraph"}.\n'
)


def prepare(out, seed, pairs_path):
    with open(pairs_path, encoding="utf-8") as f:
        pairs = json.load(f)
    rng = random.Random(int(seed))
    os.makedirs(out, exist_ok=True)
    key = {}
    for p in pairs:
        swap = rng.random() < 0.5
        a, b = ("second", "first") if swap else ("first", "second")
        d = os.path.join(out, p["id"])
        os.makedirs(d, exist_ok=True)
        shutil.copyfile(p[a], os.path.join(d, "A.png"))
        shutil.copyfile(p[b], os.path.join(d, "B.png"))
        with open(os.path.join(d, "prompt.txt"), "w", encoding="utf-8") as f:
            f.write(RUBRIC)
        key[p["id"]] = {"A": a, "B": b, "first": p["first"], "second": p["second"]}
    with open(os.path.join(out, "key.json"), "w", encoding="utf-8") as f:
        json.dump(key, f, indent=1, sort_keys=True)
        f.write("\n")
    return key


def tally(out):
    with open(os.path.join(out, "key.json"), encoding="utf-8") as f:
        key = json.load(f)
    result = {}
    for pid in sorted(key):
        k = key[pid]
        d = os.path.join(out, pid)
        votes = {"first": 0, "second": 0, "same": 0}
        sums = {"first": {c: [] for c in CRITERIA}, "second": {c: [] for c in CRITERIA}}
        whys = []
        for name in sorted(os.listdir(d)):
            if not (name.startswith("judge-") and name.endswith(".json")):
                continue
            with open(os.path.join(d, name), encoding="utf-8") as f:
                j = json.load(f)
            better = j.get("better")
            votes[k[better] if better in ("A", "B") else "same"] += 1
            for letter in ("A", "B"):
                for c in CRITERIA:
                    v = j.get("scores", {}).get(letter, {}).get(c)
                    if isinstance(v, (int, float)):
                        sums[k[letter]][c].append(v)
            whys.append({"judge": name, "better": k.get(better, "same"), "why": j.get("why", "")})
        top = max(votes.values()) if votes else 0
        leaders = [v for v, n in votes.items() if n == top]
        majority = leaders[0] if len(leaders) == 1 and top > 0 else "tie"
        means = {
            side: {c: round(sum(v) / len(v), 2) if v else None for c, v in crit.items()}
            for side, crit in sums.items()
        }
        result[pid] = {
            "votes": votes,
            "majority": majority,
            "means": means,
            "disagree": len([v for v in votes.values() if v]) > 1,
            "whys": whys,
            "first": k["first"],
            "second": k["second"],
        }
    with open(os.path.join(out, "tally.json"), "w", encoding="utf-8") as f:
        json.dump(result, f, indent=1, sort_keys=True)
        f.write("\n")
    lines = ["| pair | first | second | same | majority | " + " | ".join(CRITERIA) + " |",
             "|" + "---|" * (5 + len(CRITERIA))]
    for pid, r in result.items():
        cells = []
        for c in CRITERIA:
            a, b = r["means"]["first"][c], r["means"]["second"][c]
            cells.append(f"{a} / {b}")
        v = r["votes"]
        lines.append(
            f"| {pid} | {v['first']} | {v['second']} | {v['same']} | {r['majority']} | "
            + " | ".join(cells)
            + " |"
        )
    with open(os.path.join(out, "tally.md"), "w", encoding="utf-8") as f:
        f.write("\n".join(lines) + "\n\nScores are first / second, means of the judges.\n")
    return result


def main():
    if len(sys.argv) >= 2 and sys.argv[1] == "prepare" and len(sys.argv) == 5:
        prepare(sys.argv[2], sys.argv[3], sys.argv[4])
    elif len(sys.argv) == 3 and sys.argv[1] == "tally":
        for pid, r in tally(sys.argv[2]).items():
            print(pid, r["votes"], r["majority"])
    else:
        print(__doc__, file=sys.stderr)
        sys.exit(2)


if __name__ == "__main__":
    main()
