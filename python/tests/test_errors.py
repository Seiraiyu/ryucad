"""Refusals become exceptions by their stable codes."""

import unittest

import ryucad
from ryucad import EXIT, Code
from ryucad._errors import refusal


def error(code, details=None, message="refused"):
    """An `error` object as the session writes it."""
    data = {"code": code, "message": message}
    if details is not None:
        data["details"] = details
    return {"code": -32000, "message": message, "data": data}


BY_EXIT = {
    1: ryucad.FixableError,
    2: ryucad.UsageError,
    3: ryucad.ProjectError,
    4: ryucad.ExternalError,
}


class Errors(unittest.TestCase):
    def test_every_code_has_an_exit_class(self):
        self.assertEqual(set(EXIT), set(Code))
        self.assertLessEqual(set(EXIT.values()), {1, 2, 3, 4})
        self.assertIs(Code("conflict"), Code.CONFLICT)
        self.assertEqual(Code.CONFLICT, "conflict")

    def test_each_exit_class_has_its_exception(self):
        for name, cls in [
            ("invalid_param", ryucad.FixableError),
            ("usage", ryucad.UsageError),
            ("project_not_found", ryucad.ProjectError),
            ("io", ryucad.ExternalError),
        ]:
            e = refusal(error(name))
            self.assertIs(type(e), cls, name)
            self.assertIsInstance(e, ryucad.RyucadError)
            self.assertIsInstance(e, ryucad.Error)
            self.assertIs(e.code, Code(name))
            self.assertIs(BY_EXIT[EXIT[Code(name)]], cls)

    def test_named_codes_sit_under_their_exit_class(self):
        for name, cls in [
            ("conflict", ryucad.Conflict),
            ("not_found", ryucad.NotFound),
            ("rebuild_failed", ryucad.RebuildFailed),
            ("has_dependents", ryucad.HasDependents),
            ("assembly_overconstrained", ryucad.AssemblyOverconstrained),
            ("cancelled", ryucad.Cancelled),
            ("daemon", ryucad.DaemonError),
        ]:
            e = refusal(error(name))
            self.assertIs(type(e), cls, name)
            self.assertTrue(issubclass(cls, BY_EXIT[EXIT[Code(name)]]), name)

    def test_an_unknown_code_is_a_plain_refusal(self):
        e = refusal(error("from_a_newer_ryucad"))
        self.assertIs(type(e), ryucad.RyucadError)
        self.assertEqual(e.code, "from_a_newer_ryucad")
        self.assertEqual(str(e), "from_a_newer_ryucad: refused")

    def test_a_batch_refusal_names_its_op(self):
        e = refusal(error("invalid_param", {
            "op": 1,
            "method": "part.ensure",
            "pointer": "/ops/1/params/features/0/id",
            "path": "parts/p/part.json",
        }, message="no id"))
        self.assertEqual(e.op, 1)
        self.assertEqual(e.method, "part.ensure")
        self.assertEqual(e.pointer, "/ops/1/params/features/0/id")
        self.assertEqual(e.path, "parts/p/part.json")
        self.assertEqual(e.message, "no id")
        self.assertEqual(e.details["op"], 1)
        self.assertEqual(str(e), "invalid_param: no id")

    def test_absent_details_are_none(self):
        e = refusal(error("not_found"))
        self.assertEqual(e.details, {})
        self.assertEqual((e.op, e.method, e.pointer, e.path), (None, None, None, None))

    def test_an_error_without_a_code_is_a_protocol_error(self):
        self.assertIsInstance(refusal({"code": -32000, "message": "x"}), ryucad.ProtocolError)
        self.assertIsInstance(refusal("x"), ryucad.ProtocolError)

    def test_the_clients_own_errors(self):
        for cls in (ryucad.NotInstalled, ryucad.Incompatible, ryucad.SessionClosed,
                    ryucad.ProtocolError):
            self.assertTrue(issubclass(cls, ryucad.Error))
            self.assertFalse(issubclass(cls, ryucad.RyucadError))
        self.assertEqual(ryucad.SessionClosed("gone", 3).returncode, 3)
