"""What the unit tests share: the fake session, and a session on it."""

import os
import sys

import ryucad

HERE = os.path.dirname(os.path.abspath(__file__))
FAKE = os.path.join(HERE, "fake_session.py")
SRC = os.path.abspath(os.path.join(HERE, "..", "src"))


def fake(on_progress=None, **env):
    """A session on the fake, with ``env`` added to this process's."""
    return ryucad.connect(exe=[sys.executable, FAKE], env=dict(os.environ, **env),
                          on_progress=on_progress)
