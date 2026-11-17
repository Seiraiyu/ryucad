"""The fake speaks the session's framing, so the client's tests mean it."""

import json
import os
import subprocess
import sys
import unittest

from support import FAKE


class Fake(unittest.TestCase):
    def test_it_answers_in_the_sessions_framing(self):
        lines = (
            b'{"jsonrpc":"2.0","id":1,"method":"bytes","params":{"pattern":"ab","times":2}}\n'
            b'{"jsonrpc":"2.0","id":2,"method":"x"}\n'
        )
        run = subprocess.run([sys.executable, FAKE, "session", "--stdio"], input=lines,
                             capture_output=True, timeout=60)
        self.assertEqual(run.returncode, 0, run.stderr)
        head, rest = run.stdout.split(b"\n", 1)
        self.assertEqual(json.loads(head)["result"]["binary"]["length"], 4)
        self.assertEqual(rest[:4], b"abab")
        self.assertEqual(json.loads(rest[4:]),
                         {"jsonrpc": "2.0", "id": 2, "result": {"method": "x", "params": {}}})

    def test_it_says_its_version(self):
        run = subprocess.run([sys.executable, FAKE, "version"], capture_output=True,
                             env=dict(os.environ, FAKE_PROTOCOL="2"), timeout=60)
        self.assertEqual(json.loads(run.stdout)["result"]["protocol"], 2)
