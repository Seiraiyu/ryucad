"""Checks fixture.py turns a round's call log into the committed fixture.

Run: python3 -I scripts/board-round/test-fixture.py
"""

import importlib.machinery
import importlib.util
import json
import os
import unittest

path = os.path.join(os.path.dirname(os.path.abspath(__file__)), "fixture.py")
loader = importlib.machinery.SourceFileLoader("board_fixture", path)
spec = importlib.util.spec_from_loader("board_fixture", loader)
fx = importlib.util.module_from_spec(spec)
loader.exec_module(fx)


def call(args, exit=0, result=None):
    answer = {"ok": exit == 0, "result": result or {}}
    return json.dumps({"args": args, "exit": exit, "answer": answer})


class Command(unittest.TestCase):
    def test_writes_are_found_past_global_flags(self):
        args = ["--pretty", "board", "track", "add", "b", "--net", "sda",
                "--layer", "top", "--path", "1,1", "R1/1"]
        self.assertEqual(fx.command(args), ("board", "track", "add"))
        self.assertEqual(fx.command(["-C", "p", "board", "place", "b", "R1", "--at", "1,2"]),
                         ("board", "place"))
        self.assertEqual(fx.command(["undo"]), ("undo",))

    def test_reads_are_not_writes(self):
        for args in (["board", "show", "b", "--pads"], ["board", "drc", "b"],
                     ["board", "render", "b", "--out", "x.png"],
                     ["board", "export", "b", "--out", "fab"], ["netlist"], ["--help"],
                     ["board", "create", "--help"], ["board", "track", "add", "-h"],
                     ["board", "via", "delete", "help"]):
            self.assertIsNone(fx.command(args), args)

    def test_flags_dropped(self):
        self.assertEqual(
            fx.stripped(["--pretty", "-C", "p", "board", "place", "b", "R1", "--at", "1,2",
                         "--expect", "r1", "--expect=r2", "--as=me", "--no-daemon"]),
            ["board", "place", "b", "R1", "--at", "1,2"])


class Pinned(unittest.TestCase):
    def test_an_add_takes_its_answered_id(self):
        args = ["board", "via", "add", "b", "--net", "rail:gnd", "--at", "3,4"]
        got = fx.pinned(("board", "via", "add"), args,
                        {"result": {"vias": [{"id": "v9"}]}})
        self.assertEqual(got, args + ["--id", "v9"])

    def test_a_given_id_stays(self):
        args = ["board", "hole", "add", "b", "--at", "3,4", "--drill", "3.2", "--id=h1"]
        self.assertEqual(fx.pinned(("board", "hole", "add"), args,
                                   {"result": {"holes": [{"id": "h1"}]}}), args)

    def test_ops_take_their_ids_in_order(self):
        ops = json.dumps([{"net": "sda", "layer": "top", "path": ["R1/1", "R2/1"]},
                          {"net": "scl", "layer": "top", "path": ["R1/2", "R2/2"], "id": "keep"}])
        got = fx.pinned(("board", "track", "add"), ["board", "track", "add", "b", "--ops", ops],
                        {"result": {"tracks": [{"id": "t1"}, {"id": "keep"}]}})
        asks = json.loads(got[-1])
        self.assertEqual([a["id"] for a in asks], ["t1", "keep"])

    def test_a_board_takes_its_reference(self):
        args = ["board", "create", "Main", "--design", "board", "--rect", "20,15"]
        got = fx.pinned(("board", "create"), args, {"result": {"reference": "board/k2"}})
        self.assertEqual(got, args + ["--id", "k2"])


class Log(unittest.TestCase):
    def test_failed_calls_and_undone_writes_are_left_out(self):
        lines = [
            call(["board", "create", "M", "--design", "board", "--rect", "9,9"],
                 result={"reference": "board/m"}),
            call(["board", "place", "m", "R1", "--at", "1,1"]),
            call(["board", "place", "m", "R2", "--at", "x"], exit=2),
            call(["board", "place", "m", "R2", "--at", "2,2"]),
            call(["undo"]),
            call(["board", "place", "m", "R2", "--at", "3,3"]),
            call(["board", "place", "m", "R3", "--at", "4,4"]),
            call(["undo"]),
            call(["redo"]),
            call(["board", "show", "m"]),
        ]
        applied, other = fx.fixture(lines)
        self.assertEqual([a for _, a in applied], [
            ["board", "create", "M", "--design", "board", "--rect", "9,9", "--id", "m"],
            ["board", "place", "m", "R1", "--at", "1,1"],
            ["board", "place", "m", "R2", "--at", "3,3"],
            ["board", "place", "m", "R3", "--at", "4,4"],
        ])
        self.assertEqual(other, [["board", "show", "m"]])

    def test_a_new_write_drops_what_redo_would_bring_back(self):
        lines = [
            call(["board", "create", "M", "--design", "board", "--rect", "9,9", "--id", "m"],
                 result={"reference": "board/m"}),
            call(["board", "place", "m", "R1", "--at", "1,1"]),
            call(["undo"]),
            call(["board", "place", "m", "R1", "--at", "2,2"]),
            call(["redo"]),
        ]
        applied, _ = fx.fixture(lines)
        self.assertEqual([a[-1] for _, a in applied], ["m", "2,2"])


class Render(unittest.TestCase):
    def test_rust_source(self):
        applied = [(("board", "create"), ["board", "create", "M", "--design", "board",
                                          "--rect", "9,9", "--id", "m"]),
                   (("board", "track", "add"), ["board", "track", "add", "m", "--ops",
                                                '[{"net":"a\\\\b"}]'])]
        text = fx.render(applied, "board-1")
        self.assertIn('pub const BOARD: &str = "m";', text)
        self.assertIn(r'"[{\"net\":\"a\\\\b\"}]"', text)
        self.assertIn("agent round `board-1`", text)
        self.assertTrue(text.endswith("}\n"))


if __name__ == "__main__":
    unittest.main()
