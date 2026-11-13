"""The merge round's wrapper: every call logged; the merge role refused
anything but reading and settling a merge. Run: python3 -I test-ryueda.py"""

import json
import os
import subprocess
import sys
import tempfile

HERE = os.path.dirname(os.path.abspath(__file__))


def call(role, args, log):
    env = dict(os.environ, ROUND_RYUEDA="/bin/echo", ROUND_LOG=log, ROUND_ROLE=role)
    return subprocess.run([sys.executable, "-I", os.path.join(HERE, "ryueda"), *args],
                          env=env, capture_output=True, text=True)


def main():
    with tempfile.TemporaryDirectory() as d:
        log = os.path.join(d, "calls.jsonl")
        assert call("branch", ["component", "add", "buck"], log).returncode == 0
        out = call("merge", ["component", "add", "buck"], log)
        assert out.returncode == 2, out
        assert json.loads(out.stdout)["error"]["code"] == "round_role"
        for ok in (["merge", "status"], ["merge", "resolve", "x", "--repair", "join"],
                   ["render", "--block", "buck"], ["describe"], ["diff"], ["check"], ["--help"]):
            assert call("merge", ok, log).returncode == 0, ok
        lines = [json.loads(l) for l in open(log)]
        assert len(lines) == 9 and lines[1]["refused"] is True, lines
    print("ok")


main()
