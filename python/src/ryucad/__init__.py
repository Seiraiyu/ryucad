"""The official Python client for ryucad: one ``ryucad session --stdio``
per :func:`connect`, ryucad's requests by name, typed errors.

    import ryucad

    with ryucad.connect("~/work/gearbox") as cad:
        cad.batch([ryucad.op("part.ensure", part="shaft", features=[...])], prune=True)

Methods, params and answers are the protocol's, as ``ryucad <command>
--help`` and docs/claude/ryucad-scripting.md name them.
"""

from ._codes import EXIT, Code
from ._meta import VERSION as __version__

__all__ = [
    "Code",
    "EXIT",
    "__version__",
]
