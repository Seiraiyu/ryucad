"""What the package raises: ryucad's refusals by their stable codes, and
the client's own failures. Everything is an :class:`Error`."""

from __future__ import annotations

from collections.abc import Mapping
from typing import Any

from ._codes import EXIT, Code


class Error(Exception):
    """The base of everything this package raises."""


class RyucadError(Error):
    """A refusal: ryucad answered with a stable error code. ``code`` is a
    :class:`Code`, or the plain string for a code this package is too old
    to know; ``details`` the structured details, and from them ``pointer``,
    ``path``, ``op`` and ``method`` (a batch's failing op), each ``None``
    when absent."""

    def __init__(self, code: Code | str, message: str,
                 details: Mapping[str, Any] | None = None) -> None:
        super().__init__(message)
        self.code: Code | str = code
        self.message = message
        self.details: dict[str, Any] = dict(details or {})

    def __str__(self) -> str:
        code = self.code.value if isinstance(self.code, Code) else self.code
        return f"{code}: {self.message}"

    def _text(self, key: str) -> str | None:
        value = self.details.get(key)
        return value if isinstance(value, str) else None

    @property
    def pointer(self) -> str | None:
        """The JSON pointer into the request (or a file) at fault."""
        return self._text("pointer")

    @property
    def path(self) -> str | None:
        """The file at fault."""
        return self._text("path")

    @property
    def method(self) -> str | None:
        """A batch's failing op's method."""
        return self._text("method")

    @property
    def op(self) -> int | None:
        """A batch's failing op's index."""
        value = self.details.get("op")
        return value if isinstance(value, int) and not isinstance(value, bool) else None


class FixableError(RyucadError):
    """Exit class 1: the agent can fix it."""


class UsageError(RyucadError):
    """Exit class 2: a request ryucad doesn't take."""


class ProjectError(RyucadError):
    """Exit class 3: no usable project."""


class ExternalError(RyucadError):
    """Exit class 4: not the agent's fault."""


class Conflict(FixableError):
    """``conflict``: the part changed since the agent last saw it."""


class NotFound(FixableError):
    """``not_found``: no such part, feature or file."""


class RebuildFailed(FixableError):
    """``rebuild_failed``: the kernel refused a feature."""


class HasDependents(FixableError):
    """``has_dependents``: something still uses it (a batch's ``prune``)."""


class AssemblyOverconstrained(FixableError):
    """``assembly_overconstrained``: mates that held no longer can."""


class Cancelled(ExternalError):
    """``cancelled``: the request was cancelled (Ctrl+C)."""


class DaemonError(ExternalError):
    """``daemon``: the daemon couldn't be reached or answered badly; a
    request in flight may have been applied."""


class NotInstalled(Error):
    """No ryucad binary was found."""


class Incompatible(Error):
    """The ryucad found is too old for this package."""


class SessionClosed(Error):
    """The session has ended: its process exited (``returncode``) or was
    closed. A request in flight may have been applied."""

    def __init__(self, message: str, returncode: int | None = None) -> None:
        super().__init__(message)
        self.returncode = returncode


class ProtocolError(Error):
    """A line that isn't the JSON-RPC the contract promises: a bug in the
    client or the binary, never the script's."""


_BY_EXIT: dict[int, type[RyucadError]] = {
    1: FixableError,
    2: UsageError,
    3: ProjectError,
    4: ExternalError,
}

#: The codes scripts branch on, by their own class.
NAMED: dict[Code, type[RyucadError]] = {
    Code.CONFLICT: Conflict,
    Code.NOT_FOUND: NotFound,
    Code.REBUILD_FAILED: RebuildFailed,
    Code.HAS_DEPENDENTS: HasDependents,
    Code.ASSEMBLY_OVERCONSTRAINED: AssemblyOverconstrained,
    Code.CANCELLED: Cancelled,
    Code.DAEMON: DaemonError,
}


def refusal(error: Any) -> Error:
    """The exception for a JSON-RPC ``error`` object as the session writes
    it (``{code, message, data: {code, message, details}}``)."""
    data = error.get("data") if isinstance(error, dict) else None
    if not isinstance(data, dict) or not isinstance(data.get("code"), str):
        return ProtocolError(f"an error without a stable code: {error!r:.200}")
    name: str = data["code"]
    message = data.get("message")
    if not isinstance(message, str):
        message = str(error.get("message", ""))
    details = data.get("details")
    details = details if isinstance(details, dict) else {}
    try:
        code = Code(name)
    except ValueError:
        return RyucadError(name, message, details)
    cls = NAMED.get(code) or _BY_EXIT.get(EXIT[code], RyucadError)
    return cls(code, message, details)
