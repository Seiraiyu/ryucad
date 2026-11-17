"""Finding the ryucad binary. The package never downloads or bundles one:
the installers install ryucad, and this finds it."""

from __future__ import annotations

import os
import shutil
import sys
from collections.abc import Mapping, Sequence

from ._errors import NotInstalled

NAME = "ryucad.exe" if sys.platform == "win32" else "ryucad"

INSTALL = """\
Install ryucad with
  curl -fsSL https://github.com/Seiraiyu/ryucad/releases/latest/download/install.sh | sh
or, on Windows (PowerShell),
  irm https://github.com/Seiraiyu/ryucad/releases/latest/download/install.ps1 | iex
or name the binary with RYUCAD_EXE or connect(exe=...)."""


def find(
    exe: str | os.PathLike[str] | Sequence[str] | None = None,
    *,
    env: Mapping[str, str] | None = None,
) -> list[str]:
    """The command line that runs ryucad: ``exe`` (a path, or a command line
    as a list); else ``RYUCAD_EXE``; else ``ryucad`` on ``PATH``; else the
    installers' ``bin`` (``RYUCAD_HOME``'s, else ``~/.ryucad``'s).
    :class:`NotInstalled` when none is there. ``env`` is read in place of
    this process's environment."""
    env = os.environ if env is None else env
    if exe is not None and not isinstance(exe, (str, os.PathLike)):
        command = [os.fspath(a) for a in exe]
        if not command:
            raise NotInstalled("exe is an empty command line")
        return command
    if exe is not None:
        return [_named(os.fspath(exe), "exe", env)]
    if env.get("RYUCAD_EXE"):
        return [_named(env["RYUCAD_EXE"], "RYUCAD_EXE", env)]
    on_path = shutil.which(NAME, path=env.get("PATH", ""))
    if on_path:
        return [on_path]
    installed = os.path.join(_home(env), "bin", NAME)
    if os.path.isfile(installed):
        return [installed]
    raise NotInstalled(f"ryucad isn't installed: it isn't on PATH or in {installed}.\n{INSTALL}")


def _named(path: str, source: str, env: Mapping[str, str]) -> str:
    if os.path.isfile(path):
        return os.path.abspath(path)
    found = shutil.which(path, path=env.get("PATH", ""))
    if found:
        return found
    raise NotInstalled(f"{source} names {path}, which isn't a ryucad binary.\n{INSTALL}")


def _home(env: Mapping[str, str]) -> str:
    """Where the installers put ryucad: RYUCAD_HOME, else ~/.ryucad."""
    if env.get("RYUCAD_HOME"):
        return env["RYUCAD_HOME"]
    user = env.get("USERPROFILE" if sys.platform == "win32" else "HOME")
    return os.path.join(user or os.path.expanduser("~"), ".ryucad")
