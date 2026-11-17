"""The binary: exe, then RYUCAD_EXE, then PATH, then the installers' bin."""

import os
import shutil
import sys
import tempfile
import unittest

import ryucad

NAME = "ryucad.exe" if sys.platform == "win32" else "ryucad"


class Find(unittest.TestCase):
    def setUp(self):
        self.tmp = tempfile.mkdtemp(prefix="ryucad-find-")
        self.addCleanup(shutil.rmtree, self.tmp, True)

    def binary(self, *parts):
        folder = os.path.join(self.tmp, *parts)
        os.makedirs(folder, exist_ok=True)
        path = os.path.join(folder, NAME)
        with open(path, "w", encoding="utf-8"):
            pass
        os.chmod(path, 0o755)
        return path

    def env(self, **more):
        home = os.path.join(self.tmp, "home")
        env = {"PATH": os.path.join(self.tmp, "nothing"), "HOME": home, "USERPROFILE": home}
        env.update(more)
        return env

    def test_exe_comes_first(self):
        named = self.binary("named")
        env = self.env(RYUCAD_EXE=self.binary("var"), PATH=os.path.dirname(self.binary("path")))
        self.assertEqual(ryucad.find(named, env=env), [os.path.abspath(named)])
        self.assertEqual(ryucad.find(["wsl", "ryucad"], env=env), ["wsl", "ryucad"])

    def test_then_ryucad_exe(self):
        var = self.binary("var")
        env = self.env(RYUCAD_EXE=var, PATH=os.path.dirname(self.binary("path")))
        self.assertEqual(ryucad.find(env=env), [os.path.abspath(var)])

    def test_then_path(self):
        on_path = self.binary("path")
        self.binary("home", ".ryucad", "bin")
        found = ryucad.find(env=self.env(PATH=os.path.dirname(on_path)))
        self.assertEqual(os.path.normcase(found[0]), os.path.normcase(on_path))

    def test_then_the_installers_bin(self):
        home = self.binary("home", ".ryucad", "bin")
        self.assertEqual(ryucad.find(env=self.env()), [home])
        moved = self.binary("elsewhere", "bin")
        env = self.env(RYUCAD_HOME=os.path.join(self.tmp, "elsewhere"))
        self.assertEqual(ryucad.find(env=env), [moved])

    def test_none_found_says_how_to_install(self):
        with self.assertRaises(ryucad.NotInstalled) as e:
            ryucad.find(env=self.env())
        self.assertIn("install.sh", str(e.exception))
        self.assertIn("install.ps1", str(e.exception))

    def test_a_named_binary_that_isnt_there(self):
        missing = os.path.join(self.tmp, "no", NAME)
        with self.assertRaises(ryucad.NotInstalled) as e:
            ryucad.find(env=self.env(RYUCAD_EXE=missing))
        self.assertIn("RYUCAD_EXE", str(e.exception))
        with self.assertRaises(ryucad.NotInstalled):
            ryucad.find(missing, env=self.env())
        with self.assertRaises(ryucad.NotInstalled):
            ryucad.find([], env=self.env())
