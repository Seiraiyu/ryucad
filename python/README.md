# ryucad for Python

The official Python client for [ryucad](https://github.com/Seiraiyu/ryucad),
CAD built for AI agents to drive. It runs an installed `ryucad` as
`ryucad session --stdio`, one process per connection, and sends ryucad's
requests by name: the framing, binary answers, progress and typed errors
are handled for you. Standard library only, Python 3.10 or newer,
Apache-2.0.

## Install

The package finds an installed ryucad; it ships none. Install ryucad first:

    curl -fsSL https://github.com/Seiraiyu/ryucad/releases/latest/download/install.sh | sh

or on Windows (PowerShell):

    irm https://github.com/Seiraiyu/ryucad/releases/latest/download/install.ps1 | iex

then the package:

    pip install ryucad

It looks for ryucad in `connect(exe=...)`, then `RYUCAD_EXE`, then `ryucad`
on `PATH`, then the installers' `~/.ryucad/bin` (`RYUCAD_HOME`'s `bin`
when that is set), and raises `ryucad.NotInstalled` with the two commands
above when there is none. It needs ryucad's protocol 3 or newer (the one
with `batch`) and warns when ryucad is older than the package.

## A build script

```python
import ryucad

def plate(name, width):
    return ryucad.op(
        "part.ensure",
        part=name,
        params={"w": f"{width} mm"},
        features=[{"id": "base", "type": "box", "params": {"x": "w", "y": 30, "z": 6}}],
    )

with ryucad.connect("~/work/fixture") as cad:
    answer = cad.batch([plate("left", 40), plate("right", 60)], prune=True)
    print(answer["parts"])   # {"left": {"status": "created", ...}, ...}

    again = cad.batch([plate("left", 40), plate("right", 60)], prune=True)
    assert again["written"] == 0   # nothing changed, nothing written

    cad.call("export", part="left", out="left.step")   # relative to the project
    stl = cad.call("export", part="left", format="stl")
    print(len(stl.data), "bytes of STL")
```

- `cad.call(method, params=None, **params)` sends any request by name and
  returns its `result` as a dict; an answer that carries bytes has them in
  `.data`. A param named like a Python keyword (`from`) goes in the dict.
- `cad.batch(ops, expect=None, dry_run=False, rebuild=None, prune=False)`
  runs many edits as one checked, logged, undoable change; `ryucad.op`
  builds each op. `cad.ensure(part, features, ...)` is `part.ensure` alone,
  and `cad.undo()` takes back this participant's latest change.
- `connect(folder, as_=..., no_daemon=..., exe=..., env=..., on_progress=...)`:
  `as_` names the participant in the activity log; `on_progress` receives
  each `progress` line of a request sent with `progress=True`; after the
  script's own `subscribe`, notifications collect in `cad.notifications`.
- One request at a time per session (threads take turns); open two
  sessions for two at once. Ctrl+C cancels a long request and waits for its
  `cancelled` answer before `KeyboardInterrupt` reaches the script.

## Errors

A refusal raises `ryucad.RyucadError` with ryucad's stable `code`, a
`message`, its `details`, and `pointer`, `path`, `op` and `method` when it
names them (a batch's failing op). It is one of `FixableError` (the agent
can fix it), `UsageError`, `ProjectError` (no usable project) or
`ExternalError` (not the agent's fault), with named classes for the codes
scripts branch on: `Conflict`, `NotFound`, `RebuildFailed`,
`HasDependents`, `AssemblyOverconstrained`, `Cancelled`, `DaemonError`.
Every other code is matched by `e.code == ryucad.Code.SOME_CODE`. The
client's own errors are `NotInstalled`, `Incompatible`, `SessionClosed` and
`ProtocolError`; all derive from `ryucad.Error`.

```python
try:
    cad.batch(ops)
except ryucad.FixableError as e:
    print(e.code, e.op, e.pointer, e.message)
```

## The protocol

Methods, params and answers are ryucad's own: `ryucad <command> --help`
and `--json` name them, and the repository's
[scripting notes](https://github.com/Seiraiyu/ryucad/blob/main/docs/claude/ryucad-scripting.md)
describe `session --stdio`, `batch` and `part.ensure`. The package adds no
second API.

## Versions

The package is released with ryucad, from the same tag: ryucad `vX.Y.Z`
publishes `ryucad` `X.Y.Z` here, and a pre-release `vX.Y.Z-pre.N`
publishes `X.Y.ZrcN` (install it with `pip install --pre`). Keep the
package and ryucad at the same version.
