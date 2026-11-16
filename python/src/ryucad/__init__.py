"""The official Python client for ryucad: one ``ryucad session --stdio``
per :func:`connect`, ryucad's requests by name, typed errors.

    import ryucad

    with ryucad.connect("~/work/gearbox") as cad:
        cad.batch([ryucad.op("part.ensure", part="shaft", features=[...])], prune=True)

Methods, params and answers are the protocol's, as ``ryucad <command>
--help`` and docs/claude/ryucad-scripting.md name them.
"""

from ._codes import EXIT, Code
from ._errors import (
    AssemblyOverconstrained,
    Cancelled,
    Conflict,
    DaemonError,
    Error,
    ExternalError,
    FixableError,
    HasDependents,
    Incompatible,
    NotFound,
    NotInstalled,
    ProjectError,
    ProtocolError,
    RebuildFailed,
    RyucadError,
    SessionClosed,
    UsageError,
)
from ._find import find
from ._meta import VERSION as __version__
from ._session import Answer, Session, connect

__all__ = [
    "Answer",
    "Session",
    "connect",
    "Code",
    "EXIT",
    "AssemblyOverconstrained",
    "Cancelled",
    "Conflict",
    "DaemonError",
    "Error",
    "ExternalError",
    "FixableError",
    "HasDependents",
    "Incompatible",
    "NotFound",
    "NotInstalled",
    "ProjectError",
    "ProtocolError",
    "RebuildFailed",
    "RyucadError",
    "SessionClosed",
    "UsageError",
    "__version__",
    "find",
]
