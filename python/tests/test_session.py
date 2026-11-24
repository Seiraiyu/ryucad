"""The client against a real ryucad (RYUCAD_EXE): run by
crates/ryucad/tests/python.rs with the binary the workspace built, and
skipped without one. Each test makes a new project in a temporary folder
outside every Git repository, with a private runtime, config and cache
folder, through the daemon and in process (#920: never in a checkout)."""

import os
import shutil
import struct
import subprocess
import tempfile
import unittest

import ryucad

EXE = os.environ.get("RYUCAD_EXE")
HOLES = 4
LEAVES = 7


def rectangle(w, h):
    """A w x h rectangle's sketch ops, its left side l4 on the axis."""
    ops = [
        {"op": "add", "id": "p1", "type": "point", "at": [0, 0]},
        {"op": "add", "id": "p2", "type": "point", "at": [19, 1]},
        {"op": "add", "id": "p3", "type": "point", "at": [21, 9]},
        {"op": "add", "id": "p4", "type": "point", "at": [1, 11]},
    ]
    for lid, a, b in [("l1", "p1", "p2"), ("l2", "p2", "p3"), ("l3", "p3", "p4"),
                      ("l4", "p4", "p1")]:
        ops.append({"op": "add", "id": lid, "type": "line", "start": a, "end": b})
    for kind, a in [("horizontal", "l1"), ("horizontal", "l3"), ("vertical", "l2"),
                    ("vertical", "l4")]:
        ops.append({"op": "add", "type": kind, "a": a})
    ops.append({"op": "add", "type": "fix", "a": "p1"})
    ops.append({"op": "add", "id": "w", "type": "distance_x", "a": "p1", "b": "p3", "value": w})
    ops.append({"op": "add", "id": "h", "type": "distance_y", "a": "p1", "b": "p3", "value": h})
    return ops


def leaf(i, k=None):
    """Leaf i's ensure: a plate less holes (i % 4 is 0 or 1), a sketch
    extruded (2) or revolved (3), sized by its constant k."""
    if i % 4 in (0, 1):
        tools = [f"h{h:02}" for h in range(HOLES)]
        features = [{"id": "base", "type": "box", "params": {"x": "k", "y": 30, "z": 6}}]
        for h, t in enumerate(tools):
            features.append({"id": t, "type": "cylinder",
                             "params": {"radius": 0.8, "height": 20},
                             "placement": {"origin": [2 + h * 2, 5, -5]}})
        features.append({"id": "cut", "type": "boolean", "op": "subtract", "target": "base",
                         "tools": tools})
    elif i % 4 == 2:
        features = [{"id": "s", "type": "sketch", "ops": rectangle("k", "10 mm")},
                    {"id": "e", "type": "extrude", "sketch": "s",
                     "extent": {"length": "5 mm"}}]
    else:
        features = [{"id": "s", "type": "sketch", "ops": rectangle("k", "10 mm")},
                    {"id": "r", "type": "revolve", "sketch": "s", "axis": "l4",
                     "angle": "full"}]
    return {"part": f"p{i:02}", "kind": "part", "params": {"k": k or f"{20 + i % 7} mm"},
            "features": features, "props": {"name": f"Part {i:02}", "material": "steel"}}


def project(k=None, leaves=range(LEAVES), placed=None):
    """The script's one batch: the material, each leaf's ensure, then `top`
    placing `placed` (default: every leaf). `k` maps a leaf to its constant."""
    k = k or {}
    placed = leaves if placed is None else placed
    ops = [ryucad.op("materials.set", name="steel", density="7850 kg/m^3")]
    ops += [ryucad.op("part.ensure", **leaf(i, k.get(i))) for i in leaves]
    top = [{"id": f"i{i:02}", "type": "instance", "of": f"p{i:02}",
            "placement": {"origin": [i * 40, 0, 0]}} for i in placed]
    ops.append(ryucad.op("part.ensure", part="top", kind="assembly", features=top,
                         props={"name": "Machine"}))
    return ops


def statuses(answer):
    return {p: s["status"] for p, s in answer["parts"].items()}


def in_repository(folder):
    """The repository holding folder, found as Git finds it: upward, never
    into a folder GIT_CEILING_DIRECTORIES names (#1004)."""
    ceilings = {os.path.realpath(c)
                for c in os.environ.get("GIT_CEILING_DIRECTORIES", "").split(os.pathsep) if c}
    here = os.path.abspath(folder)
    while True:
        if os.path.exists(os.path.join(here, ".git")):
            return here
        up = os.path.dirname(here)
        if up == here or os.path.realpath(up) in ceilings:
            return None
        here = up


@unittest.skipUnless(EXE, "RYUCAD_EXE names no ryucad: the integration tests need one")
class ThroughTheDaemon(unittest.TestCase):
    no_daemon = False

    def setUp(self):
        self.scratch = tempfile.mkdtemp(prefix="ryucad-py-")
        repo = in_repository(self.scratch)
        if repo:
            shutil.rmtree(self.scratch, ignore_errors=True)
            self.fail(f"{self.scratch} is inside the Git repository {repo}: "
                      "set TMPDIR to a folder outside every repository")
        self.folder = os.path.join(self.scratch, "project")
        os.makedirs(self.folder)
        # A socket's path must fit sun_path (104 bytes on macOS, 108 on
        # Linux), or the session runs in process: the daemon's runtime
        # folder goes in /tmp, not under a TMPDIR that may be long.
        self.run = (tempfile.mkdtemp(prefix="ryu-", dir="/tmp") if os.name == "posix"
                    else os.path.join(self.scratch, "run"))
        self.env = dict(os.environ,
                        RYUCAD_RUNTIME_DIR=self.run,
                        RYUCAD_CONFIG_DIR=os.path.join(self.scratch, "config"),
                        RYUCAD_CACHE_DIR=os.path.join(self.scratch, "cache"),
                        RYUCAD_IDLE_TIMEOUT="600")
        self.env.pop("RYUCAD_NO_DAEMON", None)
        self.env.pop("RYUCAD_PARTICIPANT", None)
        subprocess.run([EXE, "init", "--unit", "mm"], cwd=self.folder, env=self.env,
                       check=True, capture_output=True, timeout=120)

    def tearDown(self):
        subprocess.run([EXE, "daemon", "stop"], cwd=self.folder, env=self.env,
                       capture_output=True, timeout=120)
        shutil.rmtree(self.scratch, ignore_errors=True)
        shutil.rmtree(self.run, ignore_errors=True)

    def connect(self, **kwargs):
        return ryucad.connect(self.folder, exe=EXE, env=self.env, no_daemon=self.no_daemon,
                              **kwargs)

    def test_a_script_builds_once_and_again_changes_nothing(self):
        with self.connect() as cad:
            self.assertGreaterEqual(cad.version["protocol"], 3)
            first = cad.batch(project())
            self.assertEqual(len(first["parts"]), LEAVES + 1, first)
            self.assertEqual(set(statuses(first).values()), {"created"}, first)
            self.assertEqual(first["rebuild"]["failed"], 0, first)
            again = cad.batch(project())
            self.assertEqual(set(statuses(again).values()), {"unchanged"}, again)
            self.assertEqual(again["written"], 0)
            changed = cad.batch(project(k={2: "30 mm"}))
            moved = {p: s for p, s in statuses(changed).items() if s != "unchanged"}
            self.assertEqual(moved, {"p02": "updated"}, changed)

    def test_prune_deletes_what_the_script_no_longer_names(self):
        with self.connect() as cad:
            cad.batch(project())
            gone = cad.batch(project(leaves=range(LEAVES - 1)), prune=True)
            self.assertEqual(statuses(gone)["p06"], "deleted", gone)
            self.assertFalse(os.path.exists(os.path.join(self.folder, "parts", "p06")))
            with self.assertRaises(ryucad.HasDependents) as e:
                cad.batch(project(leaves=range(LEAVES - 2), placed=range(LEAVES - 1)),
                          prune=True)
            self.assertEqual(e.exception.pointer, "/prune")

    def test_undo_takes_a_batch_back(self):
        with self.connect() as cad:
            if self.no_daemon:
                with self.assertRaises(ryucad.FixableError) as e:
                    cad.undo()
                self.assertIs(e.exception.code, ryucad.Code.NO_LIVE_SESSION)
                return
            cad.batch(project())
            cad.batch(project(k={2: "30 mm"}))
            cad.undo()
            back = cad.batch(project())
            self.assertEqual(set(statuses(back).values()), {"unchanged"}, back)

    def test_a_refusal_names_its_op_and_writes_nothing(self):
        ops = project()
        del ops[1]["params"]["features"][0]["id"]
        with self.connect() as cad:
            with self.assertRaises(ryucad.FixableError) as e:
                cad.batch(ops)
        err = e.exception
        self.assertIs(err.code, ryucad.Code.INVALID_PARAM, err)
        self.assertEqual((err.op, err.method), (1, "part.ensure"), err.details)
        self.assertEqual(err.pointer, "/ops/1/params/features/0/id", err.details)
        self.assertFalse(os.path.exists(os.path.join(self.folder, "parts", "p00")))

    def test_exports_land_in_the_project_and_bytes_come_back(self):
        with self.connect() as cad:
            cad.batch(project())
            written = cad.call("export", part="p00", out="p00.stl")
            self.assertIsNone(written.data)
            self.assertTrue(os.path.isfile(os.path.join(self.folder, "p00.stl")))
            stl = cad.call("export", part="p00", format="stl")
            (triangles,) = struct.unpack_from("<I", stl.data, 80)
            self.assertEqual(len(stl.data), 84 + 50 * triangles)
            step = cad.call("export", part="p00")
            self.assertTrue(step.data.startswith(b"ISO-10303-21;"), step.data[:40])
            parts = [p["part"] for p in cad.call("part.list")["parts"]]
            self.assertIn("p00", parts)

    def test_progress_reaches_on_progress(self):
        seen = []
        with self.connect(on_progress=seen.append) as cad:
            cad.batch(project())
            cad.call("rebuild", progress=True)
        self.assertTrue(seen)
        self.assertTrue(all(isinstance(p, dict) for p in seen))

    def test_as_names_the_participant(self):
        if self.no_daemon:
            self.skipTest("no participants in process")
        with self.connect(as_="builder") as cad:
            cad.batch(project())
            last = cad.call("activity.log")["entries"][-1]
        self.assertEqual(last["method"], "batch", last)
        self.assertEqual(last["participant"]["name"], "builder", last)


class InProcess(ThroughTheDaemon):
    no_daemon = True
