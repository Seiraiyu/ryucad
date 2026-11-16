"""The client against the fake session: framing, errors, versions."""

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
