"""The client against the fake session: framing, errors, versions."""

import concurrent.futures
import json
import os
import shutil
import sys
import tempfile
import unittest
import warnings

import ryucad
from ryucad._session import check_version
from support import FAKE, fake


class Framing(unittest.TestCase):
    def setUp(self):
        self.tmp = tempfile.mkdtemp(prefix="ryucad-client-")
        self.addCleanup(shutil.rmtree, self.tmp, True)
        self.log = os.path.join(self.tmp, "log")

    def lines(self):
        with open(self.log, "rb") as f:
            return f.read().splitlines(keepends=True)

    def test_a_call_is_one_line_out_and_its_answer_back(self):
        with fake(FAKE_LOG=self.log) as cad:
            got = cad.call("feature.show", {"from": "a", "part": "q"}, part="p", note="naïve")
        self.assertIsInstance(got, ryucad.Answer)
        self.assertEqual(got, {"method": "feature.show",
                               "params": {"from": "a", "part": "p", "note": "naïve"}})
        self.assertIsNone(got.data)
        self.assertEqual(self.lines(), [
            '{"jsonrpc":"2.0","id":1,"method":"feature.show",'
            '"params":{"from":"a","part":"p","note":"naïve"}}\n'.encode("utf-8"),
        ])

    def test_no_params_sends_none_and_ids_count_up(self):
        with fake(FAKE_LOG=self.log) as cad:
            self.assertEqual(cad.call("part.list"), {"method": "part.list", "params": {}})
            cad.call("part.list")
        self.assertEqual(self.lines(), [
            b'{"jsonrpc":"2.0","id":1,"method":"part.list"}\n',
            b'{"jsonrpc":"2.0","id":2,"method":"part.list"}\n',
        ])

    def test_the_sessions_arguments(self):
        argv = os.path.join(self.tmp, "argv")
        ryucad.connect(self.tmp, as_="builder", no_daemon=True, exe=[sys.executable, FAKE],
                       env=dict(os.environ, FAKE_ARGV=argv)).close()
        with open(argv, encoding="utf-8") as f:
            self.assertEqual(json.load(f), ["-C", os.path.abspath(self.tmp), "--as", "builder",
                                            "--no-daemon", "session", "--stdio"])

    def test_close_ends_the_session(self):
        cad = fake()
        self.assertEqual(cad.version["protocol"], 3)
        self.assertEqual(cad.close(), 0)
        with self.assertRaises(ryucad.SessionClosed):
            cad.call("part.list")

    def test_a_refusal_raises_its_class_and_the_session_goes_on(self):
        with fake() as cad:
            with self.assertRaises(ryucad.Conflict) as e:
                cad.call("fail", code="conflict", details={"part": "p"})
            self.assertEqual(e.exception.details, {"part": "p"})
            self.assertEqual(cad.call("after")["method"], "after")

    def test_a_refusal_at_the_start(self):
        with fake(FAKE_REFUSE="project_not_found") as cad:
            with self.assertRaises(ryucad.ProjectError):
                cad.call("part.list")
            with self.assertRaises(ryucad.SessionClosed):
                cad.call("part.list")

    def test_a_line_that_isnt_json_ends_the_session(self):
        for method in ("garbage", "wrong_id", "not_an_object"):
            with fake() as cad:
                with self.assertRaises(ryucad.ProtocolError, msg=method):
                    cad.call(method)
                with self.assertRaises(ryucad.SessionClosed, msg=method):
                    cad.call("after")

    def test_the_session_dying_is_session_closed(self):
        with fake() as cad:
            with self.assertRaises(ryucad.SessionClosed) as e:
                cad.call("die", code=3)
            self.assertEqual(e.exception.returncode, 3)
            with self.assertRaises(ryucad.SessionClosed):
                cad.call("after")


class Versions(unittest.TestCase):
    def test_an_old_protocol_is_incompatible(self):
        with self.assertRaises(ryucad.Incompatible):
            fake(FAKE_PROTOCOL="2")

    def test_an_older_binary_warns(self):
        with self.assertWarns(UserWarning):
            check_version({"ryucad": "0.10.0", "protocol": 3}, "0.11.0")

    def test_the_same_or_newer_or_a_dev_build_doesnt(self):
        with warnings.catch_warnings():
            warnings.simplefilter("error")
            check_version({"ryucad": "0.11.0-pre.2", "protocol": 3}, "0.11.0rc2")
            check_version({"ryucad": "0.12.0", "protocol": 3}, "0.11.0")
            check_version({"ryucad": "0.0.0", "protocol": 3}, "0.11.0")
            check_version({"ryucad": "0.10.0", "protocol": 3}, "0.0.0")

    def test_a_binary_that_isnt_ryucad(self):
        with self.assertRaises(ryucad.Incompatible):
            ryucad.connect(exe=[sys.executable, "-c", "print('hello')"])


class Binary(unittest.TestCase):
    def test_binary_answers_are_read_exactly(self):
        with fake() as cad:
            got = cad.call("bytes", pattern="a\r\n\x00\xff\n", times=3)
            self.assertEqual(got.data, b"a\r\n\x00\xff\n" * 3)
            self.assertEqual(got["binary"]["length"], 18)
            big = cad.call("bytes", pattern="0123456789abcdef", times=65536)
            self.assertEqual(len(big.data), 1 << 20)
            # The stream is still in step.
            self.assertEqual(cad.call("after")["method"], "after")
            self.assertIsNone(cad.call("after").data)


class Notifications(unittest.TestCase):
    def test_progress_goes_to_on_progress(self):
        seen = []
        with fake(on_progress=seen.append) as cad:
            self.assertEqual(cad.call("progress", lines=3), {"progressed": True})
            self.assertEqual(cad.notifications, [])
        self.assertEqual(seen, [{"done": 0}, {"done": 1}, {"done": 2}])

    def test_progress_without_a_callback_is_dropped(self):
        with fake() as cad:
            self.assertEqual(cad.call("progress"), {"progressed": True})
            self.assertEqual(cad.notifications, [])

    def test_notifications_are_kept_for_the_script(self):
        with fake() as cad:
            self.assertEqual(cad.call("notify"), {"notified": True})
            self.assertEqual(cad.notifications, [
                {"jsonrpc": "2.0", "method": "changed", "params": {"part": "p"}},
            ])

    def test_a_callback_that_raises_raises_after_the_answer(self):
        def boom(params):
            raise ValueError("boom")

        with fake(on_progress=boom) as cad:
            with self.assertRaises(ValueError):
                cad.call("progress")
            self.assertEqual(cad.call("after")["method"], "after")


class Requests(unittest.TestCase):
    def test_op_builds_an_op(self):
        self.assertEqual(ryucad.op("part.ensure", part="p", features=[]),
                         {"method": "part.ensure", "params": {"part": "p", "features": []}})
        self.assertEqual(ryucad.op("undo"), {"method": "undo", "params": {}})

    def test_batch_sends_only_the_options_given(self):
        ops = [ryucad.op("part.ensure", part="p", features=[]), ryucad.op("undo")]
        with fake() as cad:
            self.assertEqual(cad.batch(ops), {"method": "batch", "params": {"ops": ops}})
            got = cad.batch(ops, expect="r1", dry_run=True, rebuild=False, prune=True)
        self.assertEqual(got["params"], {"ops": ops, "expect": "r1", "dry_run": True,
                                         "rebuild": False, "prune": True})

    def test_ensure_is_part_ensure(self):
        features = [{"id": "b", "type": "box"}]
        with fake() as cad:
            self.assertEqual(cad.ensure("p", features)["params"],
                             {"part": "p", "features": features})
            got = cad.ensure("p", features, kind="part", params={}, props={"name": "P"},
                             expected_revision="r")
        self.assertEqual(got, {"method": "part.ensure", "params": {
            "part": "p", "features": features, "kind": "part", "params": {},
            "props": {"name": "P"}, "expected_revision": "r"}})

    def test_undo(self):
        with fake() as cad:
            self.assertEqual(cad.undo(), {"method": "undo", "params": {}})


class Threads(unittest.TestCase):
    def test_threads_sharing_a_session_take_turns(self):
        with fake() as cad:
            def worker(t):
                return [cad.call("echo", n=t * 1000 + i)["params"]["n"] for i in range(200)]

            with concurrent.futures.ThreadPoolExecutor(4) as pool:
                results = list(pool.map(worker, range(4)))
        for t, got in enumerate(results):
            self.assertEqual(got, [t * 1000 + i for i in range(200)])
