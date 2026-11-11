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

  A pair's side may be a list of images (several sheets): copied as
  <letter>-1.png, <letter>-2.png … and named so in the prompt (plan 2.G).

  python3 -I blind.py target OUT
      After tally: over the pairs whose id starts with ref- (ours first,
      the reference second), whether design 348's target holds: ours the
      same or better by majority on at least two, and the reference's mean
      lead on grouping and paper at most one point. Writes OUT/target.json.
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


def place(src, d, letter):
    """Copies one side's image(s): one as <letter>.png, a list as
    <letter>-1.png, <letter>-2.png …; returns the names."""
    if isinstance(src, str):
        shutil.copyfile(src, os.path.join(d, f"{letter}.png"))
        return [f"{letter}.png"]
    names = []
    for i, s in enumerate(src, 1):
        names.append(f"{letter}-{i}.png")
        shutil.copyfile(s, os.path.join(d, names[-1]))
    return names


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
        a_names = place(p[a], d, "A")
        b_names = place(p[b], d, "B")
        rubric = RUBRIC
        if len(a_names) > 1 or len(b_names) > 1:
            rubric = RUBRIC.replace(
                "are the images A.png and B.png",
                f"are the images {' '.join(a_names)} (A) and {' '.join(b_names)} (B), a sheet an image",
            )
        with open(os.path.join(d, "prompt.txt"), "w", encoding="utf-8") as f:
            f.write(rubric)
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


def target(out):
    """Design 348's target over the pairs named ref-* (ours first, the
    reference second): ours the same or better by majority on at least two,
    and the reference's mean lead on grouping and paper at most a point."""
    with open(os.path.join(out, "tally.json"), encoding="utf-8") as f:
        result = json.load(f)
    refs = {pid: r for pid, r in result.items() if pid.startswith("ref-")}
    same_or_better = sum(1 for r in refs.values() if r["majority"] in ("first", "same", "tie"))

    def gap(c):
        d = [r["means"]["second"][c] - r["means"]["first"][c] for r in refs.values()
             if r["means"]["first"][c] is not None and r["means"]["second"][c] is not None]
        return round(sum(d) / len(d), 2) if d else None

    g, p = gap("grouping"), gap("paper")
    t = {
        "pairs": len(refs),
        "same_or_better": same_or_better,
        "gap": {"grouping": g, "paper": p},
        "held": same_or_better >= 2 and g is not None and g <= 1 and p is not None and p <= 1,
    }
    with open(os.path.join(out, "target.json"), "w", encoding="utf-8") as f:
        json.dump(t, f, indent=1, sort_keys=True)
        f.write("\n")
    return t


def main():
    if len(sys.argv) >= 2 and sys.argv[1] == "prepare" and len(sys.argv) == 5:
        prepare(sys.argv[2], sys.argv[3], sys.argv[4])
    elif len(sys.argv) == 3 and sys.argv[1] == "tally":
        for pid, r in tally(sys.argv[2]).items():
            print(pid, r["votes"], r["majority"])
    elif len(sys.argv) == 3 and sys.argv[1] == "target":
        print(json.dumps(target(sys.argv[2]), sort_keys=True))
    else:
        print(__doc__, file=sys.stderr)
        sys.exit(2)


if __name__ == "__main__":
    main()
