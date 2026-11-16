"""The session: one ``ryucad session --stdio``, its requests by name,
answered one at a time in order (docs/claude/ryucad-scripting.md). Every
line is compact JSON in UTF-8 ending ``\\n``; an answer announcing
``binary: {length}`` is followed by exactly that many bytes."""

from __future__ import annotations

import json
import os
import re
import subprocess
import threading
import warnings
from collections.abc import Callable, Mapping, Sequence
from typing import IO, Any

from ._errors import Error, Incompatible, ProtocolError, SessionClosed, refusal
from ._find import find
from ._meta import VERSION

#: The oldest protocol this client speaks: 3 has ``batch`` and ``part.ensure``.
PROTOCOL = 3

Progress = Callable[[dict[str, Any]], None]


class Answer(dict[str, Any]):
    """A request's result as the session wrote it, its ``warnings`` left in
    it, and ``data``: the bytes an answer announcing ``binary`` is followed
    by (``render``, ``mesh.get`` with ``transfer``, an ``export`` without
    ``out``), else ``None``."""

    data: bytes | None

    def __init__(self, result: Mapping[str, Any], data: bytes | None = None) -> None:
        super().__init__(result)
        self.data = data


class Session:
    """One ``ryucad session --stdio``, made by :func:`connect`. Requests go
    one at a time, in order: threads sharing a session take turns.
    ``version`` is ``ryucad version``'s answer; ``notifications`` collects
    the notifications a script's own ``subscribe`` brings, for it to drain."""

    def __init__(self, proc: subprocess.Popen[bytes], version: dict[str, Any],
                 on_progress: Progress | None = None) -> None:
        if proc.stdin is None or proc.stdout is None:
            raise ValueError("the session's stdin and stdout must be pipes")
        self._proc = proc
        self._in: IO[bytes] = proc.stdin
        self._out: IO[bytes] = proc.stdout
        self._lock = threading.Lock()
        self._id = 0
        self._dead: SessionClosed | None = None
        self._raised: Exception | None = None
        self.version = version
        self.on_progress = on_progress
        self.notifications: list[dict[str, Any]] = []

    def __enter__(self) -> Session:
        return self

    def __exit__(self, *exc: object) -> None:
        self.close()

    def call(self, method: str, params: Mapping[str, Any] | None = None, /,
             **kwargs: Any) -> Answer:
        """Any request by name: ``params`` and keyword params merged (a
        param named like a Python keyword, such as ``from``, goes in the
        dict). Its result, or the refusal raised as a :class:`RyucadError`."""
        return self._request(method, {**(params or {}), **kwargs})

    def close(self) -> int:
        """Ends the session (EOF on its stdin) and waits for it to exit: its
        exit code, 0 after a clean end. Later calls raise SessionClosed."""
        with self._lock:
            if self._dead is None:
                self._dead = SessionClosed("the session was closed")
            for pipe in (self._in, self._out):
                try:
                    pipe.close()
                except OSError:
                    pass
            return self._proc.wait()

    def _request(self, method: str, params: Mapping[str, Any]) -> Answer:
        message: dict[str, Any] = {"jsonrpc": "2.0", "id": 0, "method": method}
        if params:
            message["params"] = dict(params)
        with self._lock:
            if self._dead is not None:
                raise SessionClosed(str(self._dead), self._dead.returncode)
            self._id += 1
            message["id"] = self._id
            line = json.dumps(message, ensure_ascii=False, separators=(",", ":"),
                              allow_nan=False)
            self._send(line.encode("utf-8") + b"\n")
            return self._answer(self._id)

    def _send(self, line: bytes) -> None:
        try:
            self._in.write(line)
            self._in.flush()
        except OSError:
            raise self._last_words() from None

    def _last_words(self) -> Error:
        """The session ended before a request could be written: its own
        refusal if it wrote one (a null id: a failed start), else
        SessionClosed."""
        try:
            rest = self._out.read()
        except OSError:
            rest = b""
        closed = self._ended()
        for raw in rest.splitlines():
            try:
                message = json.loads(raw)
            except ValueError:
                continue
            if isinstance(message, dict) and message.get("id") is None and "error" in message:
                return refusal(message["error"])
        return closed

    def _answer(self, rid: int) -> Answer:
        while True:
            try:
                answer = self._next(rid)
            except Error:
                self._raised = None
                raise
            if answer is None:
                continue
            raised, self._raised = self._raised, None
            if raised is not None:
                raise raised
            return answer

    def _notified(self, message: dict[str, Any]) -> None:
        """A line that isn't an answer: a ``progress`` line goes to
        ``on_progress`` (dropped without one), any other notification to
        ``notifications``. An exception from ``on_progress`` is raised once
        the answer is read, so the stream stays in step."""
        if message.get("method") != "progress":
            self.notifications.append(message)
            return
        params = message.get("params")
        if self.on_progress is None or not isinstance(params, dict):
            return
        try:
            self.on_progress(params)
        except Exception as e:
            if self._raised is None:
                self._raised = e

    def _next(self, rid: int) -> Answer | None:
        """Reads one message: the answer to ``rid``, or None after a line
        that isn't an answer."""
        message, data = self._read()
        if "method" in message:
            self._notified(message)
            return None
        if message.get("id") not in (rid, None):
            raise self._broken(f"an answer to {message.get('id')!r} while waiting for {rid}")
        if "error" in message:
            error = refusal(message["error"])
            if isinstance(error, ProtocolError):
                raise self._broken(str(error))
            raise error
        if message.get("id") is None:
            raise self._broken("a result with a null id")
        result = message.get("result")
        if not isinstance(result, dict):
            raise self._broken(f"a result that isn't an object: {result!r:.200}")
        return Answer(result, data)

    def _read(self) -> tuple[dict[str, Any], bytes | None]:
        line = self._out.readline()
        if not line.endswith(b"\n"):
            raise self._ended()
        try:
            message = json.loads(line)
        except ValueError:
            raise self._broken(f"a line that isn't JSON: {line[:200]!r}") from None
        if not isinstance(message, dict) or message.get("jsonrpc") != "2.0":
            raise self._broken(f"a line that isn't JSON-RPC 2.0: {line[:200]!r}")
        return message, self._binary(message)

    def _binary(self, message: dict[str, Any]) -> bytes | None:
        """The bytes an answer announcing ``binary: {length}`` is followed
        by, read exactly (as the session's own ``Frame::of`` judges it)."""
        result = message.get("result")
        announced = result.get("binary") if isinstance(result, dict) else None
        length = announced.get("length") if isinstance(announced, dict) else None
        if not isinstance(length, int) or isinstance(length, bool) or length < 0:
            return None
        chunks: list[bytes] = []
        left = length
        while left:
            chunk = self._out.read(left)
            if not chunk:
                raise self._ended()
            chunks.append(chunk)
            left -= len(chunk)
        return b"".join(chunks)

    def _ended(self) -> SessionClosed:
        """The session's process has gone: waits for it, and every later
        call raises the same."""
        try:
            code = self._proc.wait(timeout=30)
        except subprocess.TimeoutExpired:
            self._proc.kill()
            code = self._proc.wait()
        self._dead = SessionClosed(
            f"the ryucad session ended (exit {code}); a request in flight may have been applied",
            code,
        )
        return self._dead

    def _broken(self, why: str) -> ProtocolError:
        """The stream is out of step: the session is ended."""
        self._proc.kill()
        code = self._proc.wait()
        self._dead = SessionClosed(f"the session was ended after a protocol error: {why}", code)
        return ProtocolError(why)


def connect(
    folder: str | os.PathLike[str] | None = None,
    *,
    as_: str | None = None,
    no_daemon: bool = False,
    exe: str | os.PathLike[str] | Sequence[str] | None = None,
    env: Mapping[str, str] | None = None,
    on_progress: Progress | None = None,
) -> Session:
    """Starts ``ryucad [-C folder] [--as as_] [--no-daemon] session --stdio``.
    ``folder`` is the project (default: the current folder), and relative
    paths in requests mean it, as ``ryucad -C`` does. ``exe`` is the binary
    or a command line to run in its place (default: :func:`find`'s); ``env``
    the child's whole environment (default: this process's), which
    ``RYUCAD_EXE`` is read from too. ``on_progress`` is given each
    ``progress`` line of a request sent with ``progress=True``."""
    child_env = dict(os.environ if env is None else env)
    command = find(exe, env=child_env)
    version = _version(command, child_env)
    argv = list(command)
    if folder is not None:
        argv += ["-C", os.path.abspath(os.path.expanduser(os.fspath(folder)))]
    if as_ is not None:
        argv += ["--as", as_]
    if no_daemon:
        argv.append("--no-daemon")
    argv += ["session", "--stdio"]
    proc = subprocess.Popen(argv, stdin=subprocess.PIPE, stdout=subprocess.PIPE, env=child_env)
    return Session(proc, version, on_progress)


def _version(command: Sequence[str], env: dict[str, str]) -> dict[str, Any]:
    """``ryucad version``'s answer, checked."""
    run = subprocess.run([*command, "version"], check=False, stdin=subprocess.DEVNULL,
                         capture_output=True, env=env, timeout=60)
    try:
        info = json.loads(run.stdout)["result"]
    except (ValueError, KeyError, TypeError):
        info = None
    if not isinstance(info, dict):
        raise Incompatible(
            f"{' '.join(command)} version didn't answer as ryucad does "
            f"(exit {run.returncode}): {run.stdout[:200]!r} {run.stderr[:200]!r}"
        )
    check_version(info, VERSION)
    return info


def check_version(info: Mapping[str, Any], package: str) -> None:
    """Incompatible below protocol 3; a warning when the binary is an older
    release than the package (every method passes through by name, so it
    answers what it knows and refuses the rest with ``usage``)."""
    protocol = info.get("protocol")
    if not isinstance(protocol, int) or protocol < PROTOCOL:
        raise Incompatible(
            f"ryucad {info.get('ryucad')} speaks protocol {protocol}; this package "
            f"(ryucad {package} for Python) needs {PROTOCOL} or newer: update ryucad"
        )
    binary, ours = _release(info.get("ryucad")), _release(package)
    if binary is None or ours is None or (0, 0, 0) in (binary, ours):
        return
    if binary < ours:
        warnings.warn(
            f"ryucad {info.get('ryucad')} is older than this package ({package}): "
            "requests it doesn't know are refused as `usage`; update ryucad",
            stacklevel=4,
        )


def _release(version: object) -> tuple[int, int, int] | None:
    """``X.Y.Z`` of a version, its pre-release dropped."""
    if not isinstance(version, str):
        return None
    m = re.match(r"(\d+)\.(\d+)\.(\d+)", version)
    return (int(m[1]), int(m[2]), int(m[3])) if m else None
