"""A stand-in for ``ryucad`` in the unit tests, standard library only:

    python3 fake_session.py version
    python3 fake_session.py [-C DIR] [--as NAME] [--no-daemon] session --stdio

The session answers one JSON-RPC line at a time, as ``ryucad session
--stdio`` does, by method: ``fail`` refuses with ``params.code``;
``bytes`` answers ``binary`` followed by ``params.pattern`` (latin-1)
``params.times`` times; ``progress`` writes ``params.lines`` progress
lines first; ``notify`` a notification first; ``garbage`` a line that
isn't JSON; ``wrong_id`` an answer to another id; ``not_an_object`` a
result that is a list; ``die`` exits ``params.code`` without answering;
``slow`` waits for SIGINT and answers ``cancelled``, exit 130; anything
else answers ``{method, params}``. Its environment steers the rest:
FAKE_RYUCAD and FAKE_PROTOCOL (what ``version`` says), FAKE_ARGV (a file
its arguments are written to), FAKE_LOG (a file each request line is
appended to, as received), FAKE_REFUSE (a code: refuse at the start with
a null id and exit 1), FAKE_MARKER (a file ``slow`` creates once it waits),
FAKE_CANCEL_DELAY (seconds ``slow`` takes to answer a SIGINT) and
FAKE_ANSWERED (a file it writes the time of that answer to).
"""

import json
import os
import signal
import sys
import time

out = sys.stdout.buffer


def write(message, data=b""):
    out.write(json.dumps(message, separators=(",", ":")).encode("utf-8") + b"\n" + data)
    out.flush()


def refuse(rid, code, message, details=None):
    data = {"code": code, "message": message}
    if details:
        data["details"] = details
    write({"jsonrpc": "2.0", "id": rid,
           "error": {"code": -32000, "message": message, "data": data}})


def answer(rid, result, data=b""):
    write({"jsonrpc": "2.0", "id": rid, "result": result}, data)


def session():
    log = os.environ.get("FAKE_LOG")
    for raw in sys.stdin.buffer:
        if log:
            with open(log, "ab") as f:
                f.write(raw)
        request = json.loads(raw)
        rid, method = request["id"], request["method"]
        params = request.get("params", {})
        if method == "fail":
            refuse(rid, params["code"], params.get("message", "refused"), params.get("details"))
        elif method == "bytes":
            data = params["pattern"].encode("latin-1") * params["times"]
            answer(rid, {"binary": {"encoding": "raw", "length": len(data)}}, data)
        elif method == "progress":
            for i in range(params.get("lines", 2)):
                write({"jsonrpc": "2.0", "method": "progress", "params": {"done": i}})
            answer(rid, {"progressed": True})
        elif method == "notify":
            write({"jsonrpc": "2.0", "method": "changed", "params": {"part": "p"}})
            answer(rid, {"notified": True})
        elif method == "garbage":
            out.write(b"not json\n")
            out.flush()
        elif method == "wrong_id":
            answer(rid + 1000, {})
        elif method == "not_an_object":
            write({"jsonrpc": "2.0", "id": rid, "result": [1, 2]})
        elif method == "die":
            return params.get("code", 3)
        elif method == "slow":
            def cancelled(signum, frame):
                # As ryucad does: the cancel takes a moment, then the
                # answer, then exit 130. FAKE_ANSWERED records when.
                time.sleep(float(os.environ.get("FAKE_CANCEL_DELAY", "0")))
                if os.environ.get("FAKE_ANSWERED"):
                    with open(os.environ["FAKE_ANSWERED"], "w", encoding="utf-8") as f:
                        f.write(repr(time.time()))
                refuse(rid, "cancelled", "cancelled by Ctrl+C")
                sys.exit(130)

            signal.signal(signal.SIGINT, cancelled)
            with open(os.environ["FAKE_MARKER"], "w", encoding="utf-8") as f:
                f.write("waiting")
            time.sleep(60)
            answer(rid, {"slow": "never cancelled"})
        else:
            answer(rid, {"method": method, "params": params})
    return 0


def main(argv):
    if os.environ.get("FAKE_ARGV"):
        with open(os.environ["FAKE_ARGV"], "w", encoding="utf-8") as f:
            json.dump(argv, f)
    if argv == ["version"]:
        result = {
            "ryucad": os.environ.get("FAKE_RYUCAD", "0.0.0"),
            "ryukei": "0.0.0",
            "commit": "fake",
            "target": "fake",
            "protocol": int(os.environ.get("FAKE_PROTOCOL", "3")),
        }
        print(json.dumps({"ok": True, "result": result, "warnings": []}))
        return 0
    if argv[-2:] != ["session", "--stdio"]:
        print(f"fake_session: unexpected arguments {argv}", file=sys.stderr)
        return 2
    if os.environ.get("FAKE_REFUSE"):
        refuse(None, os.environ["FAKE_REFUSE"], "refused at the start")
        return 1
    return session()


if __name__ == "__main__":
    sys.exit(main(sys.argv[1:]))
