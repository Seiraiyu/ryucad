"""Drives ryucad's CLI as an agent would, for the corpus scripts beside
this file: each builds one project into an empty folder through the
commands an agent sends, reading their JSON answers, never writing a
project file itself.

    python3 -I crates/ryucad/tests/corpus/<name>.py <ryucad> <folder>

The project is built in a new temporary folder that no Git repository
holds, and copied to <folder> (which must not exist, or be empty) once it
is done: `ryucad init` inside a repository installs ryucad's Git drivers
and hooks there, and the corpus lies inside ryucad's own repository. Every
command runs in process (RYUCAD_NO_DAEMON), with a private runtime, config
and cache folder, so nothing outside the temporary folder changes and a
run gives the same files on every machine. Standard library only.
"""

import json
import math
import os
import shutil
import subprocess
import sys
import tempfile


class Failed(Exception):
    pass


class Project:
    def __init__(self, exe, out):
        self.exe = os.path.abspath(exe)
        self.out = os.path.abspath(out)
        if os.path.exists(self.out) and os.listdir(self.out):
            raise Failed(f"{self.out} is not empty")
        self.scratch = tempfile.mkdtemp(prefix="ryucad-corpus-")
        self.folder = os.path.join(self.scratch, "project")
        os.makedirs(self.folder)
        repo = in_repository(self.folder)
        if repo:
            raise Failed(f"the temporary folder {self.folder} is inside the Git repository "
                         f"{repo}: set TMPDIR to a folder outside every repository")
        self.env = dict(os.environ)
        self.env.update(
            RYUCAD_NO_DAEMON="1",
            RYUCAD_RUNTIME_DIR=os.path.join(self.scratch, "run"),
            RYUCAD_CONFIG_DIR=os.path.join(self.scratch, "config"),
            RYUCAD_CACHE_DIR=os.path.join(self.scratch, "cache"),
        )
        self.commands = 0

    def run(self, *args):
        """Runs `ryucad ARGS` in the project: its result, or Failed."""
        self.commands += 1
        out = subprocess.run(
            [self.exe, *map(str, args)],
            cwd=self.folder,
            env=self.env,
            capture_output=True,
            text=True,
        )
        try:
            answer = json.loads(out.stdout)
        except ValueError:
            raise Failed(f"ryucad {' '.join(map(str, args))}: {out.stderr}") from None
        if not answer["ok"]:
            raise Failed(f"ryucad {' '.join(map(str, args))}: {json.dumps(answer['error'])}")
        return answer["result"]

    def part(self, pid, assembly=False):
        if assembly:
            self.run("part", "create", pid, "--assembly")
        else:
            self.run("part", "create", pid)
        return pid

    def add(self, part, kind, fid=None, params=None, place=None, **flags):
        """`feature add PART KIND`: params as NAME=QUANTITY, place as
        (origin, z, x) vectors in millimetres, flags as --name value (a
        list repeats the flag, True is a bare flag)."""
        args = ["feature", "add", part, kind]
        if fid:
            args += ["--id", fid]
        for k, v in (params or {}).items():
            args += ["--param", f"{k}={v}"]
        if place:
            origin, z, x = place
            args += ["--origin", ",".join(f"{mm(c)}mm" for c in origin)]
            args += ["--z", vec(z), "--x", vec(x)]
        for k, v in flags.items():
            flag = "--" + k.replace("_", "-")
            for one in v if isinstance(v, list) else [v]:
                if one is True:
                    args.append(flag)
                else:
                    args += [flag, one]
        return self.run(*args)["feature"]["id"]

    def block(self, part, fid, size, centre, z, x, **flags):
        """A box of `size` (along x, z cross x, z) centred at `centre` in
        the frame (z, x): `box` puts its corner at the origin."""
        y = cross(z, x)
        corner = tuple(
            c - size[0] / 2 * a - size[1] / 2 * b - size[2] / 2 * d
            for c, a, b, d in zip(centre, x, y, z)
        )
        params = {"x": size[0], "y": size[1], "z": size[2]}
        return self.add(part, "box", fid, params, (corner, z, x), **flags)

    def sketch(self, part, ops, fid=None, plane=None, place=None, face=None):
        """An empty sketch, then one batch of edits: the sketch's ID."""
        flags = {}
        if plane:
            flags["plane"] = plane
        if face:
            flags["face"] = face
        sid = self.add(part, "sketch", fid=fid, place=place, **flags)
        self.run("sketch", "edit", part, sid, json.dumps(ops))
        return sid

    def bodies(self, part):
        return self.run("inspect", part)["bodies"]

    def edges(self, part, solid, keep):
        """The full references of `solid`'s edges `keep(edge)` is true of."""
        for b in self.bodies(part):
            if b["solid"] == solid:
                return [f"{part}/{solid}/{e['name']}" for e in b["edges"] if keep(e)]
        raise Failed(f"{part} has no solid {solid}")

    def faces(self, part, solid, keep):
        for b in self.bodies(part):
            if b["solid"] == solid:
                return [f"{part}/{solid}/{f['name']}" for f in b["faces"] if keep(f)]
        raise Failed(f"{part} has no solid {solid}")


def mm(x):
    """A length in millimetres as short text: 12.5, 0, -3."""
    t = f"{x:.6f}".rstrip("0").rstrip(".")
    return "0" if t in ("-0", "") else t


def vec(v):
    return ",".join(mm(c) for c in v)


def in_repository(folder):
    """The nearest folder holding `.git` at or above `folder`, if any."""
    here = os.path.abspath(folder)
    while True:
        if os.path.exists(os.path.join(here, ".git")):
            return here
        up = os.path.dirname(here)
        if up == here:
            return None
        here = up


def cross(a, b):
    return (a[1] * b[2] - a[2] * b[1], a[2] * b[0] - a[0] * b[2], a[0] * b[1] - a[1] * b[0])


def polar(r, deg, z=0.0):
    a = math.radians(deg)
    return (r * math.cos(a), r * math.sin(a), z)


def main(build):
    if len(sys.argv) != 3:
        print(f"usage: python3 -I {sys.argv[0]} <ryucad> <empty folder>", file=sys.stderr)
        sys.exit(2)
    try:
        p = Project(sys.argv[1], sys.argv[2])
        try:
            build(p)
            shutil.copytree(p.folder, p.out, dirs_exist_ok=True)
        finally:
            shutil.rmtree(p.scratch, ignore_errors=True)
    except Failed as e:
        print(e, file=sys.stderr)
        sys.exit(1)
    print(f"{p.out}: {p.commands} commands")
