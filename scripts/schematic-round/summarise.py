"""Sums up an agent round: `python3 -I summarise.py <round dir> <ryueda>`.

Each session is a folder `<round>/<circuit>-<arm>-<see>/` holding the
project it worked in (`project/`, with `baseline.json` from
`round_projects`), the wrapper's log (`calls.jsonl`) and the session's own
`notes.md`. Prints a Markdown table, one row a session, and writes
`<round>/summary.json` with every number in it.
"""

import json
import os
import subprocess
import sys


def describe(ryueda, project):
    env = dict(os.environ, RYUEDA_NO_DAEMON="1")
    p = subprocess.run(
        [ryueda, "-C", project, "describe"], capture_output=True, env=env, check=False
    )
    env_out = json.loads(p.stdout.split(b"\n", 1)[0])
    if not env_out.get("ok"):
        return None
    return {
        b["block_id"]: {s["sheet"]: (s["score"], len(s["findings"])) for s in b["sheets"]}
        for b in env_out["result"]["blocks"]
    }


def baseline(project):
    with open(os.path.join(project, "baseline.json"), encoding="utf-8") as f:
        d = json.load(f)
    return {
        block: {
            s["sheet"]: (
                s["score"],
                sum(1 for x in v["findings"] if x["sheet"] == s["sheet"]),
            )
            for s in v["scores"]
        }
        for block, v in d.items()
    }


def calls(path):
    out = []
    if os.path.exists(path):
        with open(path, encoding="utf-8") as f:
            out = [json.loads(line) for line in f if line.strip()]
    return out


def command(c):
    words = [a for a in c["args"] if not a.startswith("-")]
    return words[0] if words else ""


def fixes_taken(log):
    """Refusals carrying fixes, and how many the next `place` sent back."""
    offered = taken = 0
    for i, c in enumerate(log):
        if command(c) != "place" or not c["fixes"]:
            continue
        offered += 1
        sent = next((d for d in log[i + 1:] if command(d) == "place"), None)
        if sent is None:
            continue
        text = " ".join(sent["args"])
        if any(all(ins in text for ins in fix) for fix in c["fixes"] if fix):
            taken += 1
    return offered, taken


def total(scores):
    return sum(score for sheets in scores.values() for score, _ in sheets.values())


def main():
    root, ryueda = sys.argv[1], sys.argv[2]
    rows = []
    for name in sorted(os.listdir(root)):
        folder = os.path.join(root, name)
        project = os.path.join(folder, "project")
        if not os.path.isdir(project):
            continue
        circuit, arm, see = name.rsplit("-", 2)
        log = calls(os.path.join(folder, "calls.jsonl"))
        offered, taken = fixes_taken(log)
        before = baseline(project)
        after = describe(ryueda, project) or {}
        seconds = round(log[-1]["t"] - log[0]["t"]) if len(log) > 1 else 0
        rows.append(
            {
                "session": name,
                "circuit": circuit,
                "arm": arm,
                "see": see,
                "calls": len(log),
                "place": sum(1 for c in log if command(c) == "place"),
                "refused": sum(1 for c in log if c["exit"] != 0),
                "arm_refused": sum(1 for c in log if c["code"] == "round_arm"),
                "fixes_offered": offered,
                "fixes_taken": taken,
                "renders": sum(1 for c in log if command(c) == "render"),
                "describes": sum(1 for c in log if command(c) == "describe"),
                "seconds": seconds,
                "before": before,
                "after": after,
                "score_before": total(before),
                "score_after": total(after),
            }
        )
    head = "| session | calls | place | refused (arm) | fixes taken | renders | describes | s | score before → after |"
    print(head)
    print("|" + "---|" * 9)
    for r in rows:
        print(
            f"| {r['session']} | {r['calls']} | {r['place']} | {r['refused']} ({r['arm_refused']}) "
            f"| {r['fixes_taken']}/{r['fixes_offered']} | {r['renders']} | {r['describes']} "
            f"| {r['seconds']} | {r['score_before']} → {r['score_after']} |"
        )
    with open(os.path.join(root, "summary.json"), "w", encoding="utf-8") as f:
        json.dump(rows, f, indent=1, sort_keys=True)
        f.write("\n")


if __name__ == "__main__":
    main()
