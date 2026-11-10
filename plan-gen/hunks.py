"""Lists the prototype's diff hunks by file and index, for gen.py's map."""
import subprocess, sys, re
BASE = "85586e88"
ROOT = "/home/stonelyd/.coxswain/worktrees/01M72WQ5AVHEMJJK3RK468HCGN"

def files():
    out = subprocess.run(["git", "diff", "--name-status", BASE], cwd=ROOT, capture_output=True, text=True).stdout
    return [l.split("\t") for l in out.splitlines()]

def hunks(path):
    d = subprocess.run(["git", "diff", "-U3", BASE, "--", path], cwd=ROOT, capture_output=True, text=True).stdout
    parts = re.split(r"(?m)^(?=@@ )", d)
    return [p for p in parts[1:]]

if __name__ == "__main__":
    for st, f in files():
        if st == "A":
            print("NEW", f)
            continue
        for i, h in enumerate(hunks(f)):
            added = [l[1:] for l in h.splitlines()[1:] if l.startswith("+")]
            print(f, i, h.splitlines()[0][:60], "|", (added[0] if added else "")[:70].strip())
