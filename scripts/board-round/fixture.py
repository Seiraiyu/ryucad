#!/usr/bin/env -S python3 -I
"""Turns a board round's call log into the committed fixture:

    python3 -I scripts/board-round/fixture.py <calls.jsonl> <round> \
        > crates/ryueda/tests/support/breakout_board.rs

Keeps every call that succeeded and wrote the board (`board create`,
`edit`, `delete`, `place`, `unplace`, `track|via|hole add|edit|delete`),
in order. It replays `undo` and `redo` itself, so an undone write is left
out. It drops `--expect`, `-C`, `--as`, `--pretty` and `--no-daemon`, and
pins every ID a create or an add was answered with (`--id`, or `"id"` in
each `--ops` object), so the replay makes the same files under any
`RYUEDA_ID_SEED`. Every other call that succeeded is listed on stderr,
for the reader to confirm it wrote nothing.
"""

import json
import sys

# Flags that take one value.
VALUED = {
    "-C", "--as", "--expect", "--design", "--rect", "--outline", "--name",
    "--cutouts", "--at", "--rotation", "--ops", "--net", "--layer", "--width",
    "--id", "--drill", "--diameter", "--out", "--side", "--window", "--size",
    "--format", "--code",
}
# Flags left out of the fixture: how many values each takes.
DROPPED = {"-C": 1, "--as": 1, "--expect": 1, "--pretty": 0, "--no-daemon": 0}
WRITES = [
    ("board", "track", "add"), ("board", "track", "edit"), ("board", "track", "delete"),
    ("board", "via", "add"), ("board", "via", "delete"),
    ("board", "hole", "add"), ("board", "hole", "delete"),
    ("board", "create"), ("board", "edit"), ("board", "delete"),
    ("board", "place"), ("board", "unplace"),
]
HEADER = """//! The sensor breakout's board (design 732, plan 5.D): the board writes
//! agent round `{round}` gave through the CLI, in order, as
//! `scripts/board-round/fixture.py` took them from its call log (undone
//! writes left out, every ID the round was answered pinned with `--id`).
//! Replayed after `breakout::script()`, the board is DRC clean and
//! exports. Change it by running a round again, not by hand.

#![allow(dead_code)]

"""


def split(arg):
    """`--flag=value` as (`--flag`, value); anything else as (arg, None)."""
    if arg.startswith("--") and "=" in arg:
        flag, value = arg.split("=", 1)
        return flag, value
    return arg, None


def positionals(args):
    """The words of `args` that are neither flags nor a flag's values."""
    out, i = [], 0
    while i < len(args):
        flag, inline = split(args[i])
        if flag == "--path" and inline is None:
            i += 1
            while i < len(args) and not args[i].startswith("-"):
                i += 1
            continue
        if flag in VALUED and inline is None:
            i += 2
            continue
        if not flag.startswith("-"):
            out.append(flag)
        i += 1
    return out


def command(args):
    """The board write `args` makes, `("undo",)`, `("redo",)`, or None.
    A call for help (`--help`, `-h`, `help`) writes nothing."""
    words = positionals(args)
    if "--help" in args or "-h" in args or "help" in words[:4]:
        return None
    for w in WRITES:
        if tuple(words[: len(w)]) == w:
            return w
    if words[:1] in (["undo"], ["redo"]):
        return (words[0],)
    return None


def stripped(args):
    """`args` without the flags DROPPED names."""
    out, i = [], 0
    while i < len(args):
        flag, inline = split(args[i])
        if flag in DROPPED:
            i += 1 + (DROPPED[flag] if inline is None else 0)
            continue
        out.append(args[i])
        i += 1
    return out


def value(args, flag):
    """The value `args` gives `flag`, or None."""
    for i, a in enumerate(args):
        f, inline = split(a)
        if f == flag:
            if inline is not None:
                return inline
            return args[i + 1] if i + 1 < len(args) else None
    return None


def set_value(args, flag, new):
    """`args` with `flag`'s value replaced by `new`."""
    out = list(args)
    for i, a in enumerate(out):
        f, inline = split(a)
        if f == flag:
            if inline is not None:
                out[i] = f"{flag}={new}"
            else:
                out[i + 1] = new
            return out
    raise KeyError(flag)


def pinned(cmd, args, answer):
    """`args` with every ID its answer gave pinned."""
    result = (answer or {}).get("result") or {}
    if cmd == ("board", "create"):
        if value(args, "--id") is None:
            args = args + ["--id", result["reference"].split("/", 1)[1]]
        return args
    if len(cmd) == 3 and cmd[2] == "add":
        made = result[cmd[1] + "s"]
        ops = value(args, "--ops")
        if ops is not None:
            asks = json.loads(ops)
            for ask, m in zip(asks, made):
                if not ask.get("id"):
                    ask["id"] = m["id"]
            return set_value(args, "--ops", json.dumps(asks, separators=(",", ":")))
        if value(args, "--id") is None:
            args = args + ["--id", made[0]["id"]]
    return args


def fixture(lines):
    """The writes a log's lines leave applied, as (command, args), and the
    other calls that succeeded."""
    applied, undone, other = [], [], []
    for line in lines:
        if not line.strip():
            continue
        e = json.loads(line)
        if e.get("exit") != 0:
            continue
        cmd = command(e["args"])
        if cmd is None:
            other.append(e["args"])
        elif cmd == ("undo",):
            if applied:
                undone.append(applied.pop())
        elif cmd == ("redo",):
            if undone:
                applied.append(undone.pop())
        else:
            applied.append((cmd, pinned(cmd, stripped(e["args"]), e.get("answer"))))
            undone.clear()
    return applied, other


def rust_str(s):
    """`s` as a Rust string literal."""
    out = []
    for ch in s:
        if ch in '"\\':
            out.append("\\" + ch)
        elif ch == "\n":
            out.append("\\n")
        elif ord(ch) < 0x20 or ord(ch) == 0x7F:
            out.append("\\u{%x}" % ord(ch))
        else:
            out.append(ch)
    return '"' + "".join(out) + '"'


def render(applied, round_name):
    """The fixture's Rust source."""
    boards = [value(a, "--id") for c, a in applied if c == ("board", "create")]
    if not boards:
        raise SystemExit("the round created no board")
    out = [HEADER.format(round=round_name)]
    out.append("/// The board's ID.\n")
    out.append(f"pub const BOARD: &str = {rust_str(boards[-1])};\n\n")
    out.append("/// The board's commands, in the order the round gave them.\n")
    out.append("pub fn script() -> Vec<Vec<String>> {\n")
    out.append("    let commands: &[&[&str]] = &[\n")
    for _, args in applied:
        out.append("        &[" + ", ".join(rust_str(a) for a in args) + "],\n")
    out.append("    ];\n")
    out.append("    commands\n        .iter()\n")
    out.append("        .map(|c| c.iter().map(|s| (*s).to_owned()).collect())\n")
    out.append("        .collect()\n}\n")
    return "".join(out)


def main():
    if len(sys.argv) != 3:
        sys.exit("usage: fixture.py <calls.jsonl> <round name>")
    with open(sys.argv[1], encoding="utf-8") as f:
        applied, other = fixture(f)
    for args in other:
        print("not in the fixture: ryueda " + " ".join(args), file=sys.stderr)
    sys.stdout.write(render(applied, sys.argv[2]))


if __name__ == "__main__":
    main()
