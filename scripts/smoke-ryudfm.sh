#!/bin/sh
# Drives a built ryudfm binary through a short agent session: a project,
# the committed minimal-clean board imported from its archive (a dry run,
# then for real), inspected, rendered as PNG and SVG and checked, then
# updated from its folder, the update undone and redone through the
# daemon, the activity log, the daemon's status and stop, and a check in
# process.
#
#   scripts/smoke-ryudfm.sh path/to/ryudfm [path/to/corpus]
#
# The corpus defaults to crates/ryudfm/tests/corpus beside this script;
# minimal-clean is copied into the project and named by relative paths, so
# no path is translated on Windows.
set -eu

exe=$(cd "$(dirname "$1")" && pwd)/$(basename "$1")
corpus=${2:-$(dirname "$0")/../crates/ryudfm/tests/corpus}
corpus=$(cd "$corpus" && pwd)
work=$(mktemp -d)
trap 'rm -rf "$work"' EXIT
mkdir -p "$work/project/jobs"
cp "$corpus/minimal-clean.tgz" "$work/project/jobs/"
cp -R "$corpus/minimal-clean" "$work/project/jobs/minimal-clean"
cd "$work/project"
export RYUDFM_RUNTIME_DIR="$work/run" RYUDFM_IDLE_TIMEOUT=60 RYUDFM_CONFIG_DIR="$work/config"

# run ARGS...: the command must print {"ok":true,...}.
run() {
    out=$("$exe" "$@") || {
        echo "FAIL ryudfm $*: $out"
        exit 1
    }
    case "$out" in
        '{"ok":true'*) echo "ok   ryudfm $*" ;;
        *)
            echo "FAIL ryudfm $*: $out"
            exit 1
            ;;
    esac
}

# has TEXT WHAT: the last output must hold TEXT.
has() {
    case "$out" in
        *"$1"*) ;;
        *)
            echo "FAIL $2: $out"
            exit 1
            ;;
    esac
}

run version
run init
run import jobs/minimal-clean.tgz --dry-run
[ ! -d boards ] || { echo "FAIL the dry run wrote a board" && exit 1; }
run import jobs/minimal-clean.tgz
[ -f boards/minimal-clean/input.tgz ] || { echo "FAIL the archive isn't committed" && exit 1; }
run board list
has '"minimal-clean"' "the board is listed"
run inspect minimal-clean
has '"layers"' "inspect summarises the layers"
run render minimal-clean --out top.png --size 400
[ -s top.png ] || { echo "FAIL no PNG written" && exit 1; }
run render minimal-clean --side bottom --out bottom.svg --size 400
[ -s bottom.svg ] || { echo "FAIL no SVG written" && exit 1; }
run check
run import jobs/minimal-clean --update minimal-clean
[ -d boards/minimal-clean/input ] || { echo "FAIL the update didn't commit the folder" && exit 1; }
[ ! -f boards/minimal-clean/input.tgz ] || { echo "FAIL the update left the archive" && exit 1; }
run undo
[ -f boards/minimal-clean/input.tgz ] || { echo "FAIL undo didn't bring the archive back" && exit 1; }
[ ! -d boards/minimal-clean/input ] || { echo "FAIL undo left the folder" && exit 1; }
run redo
[ -d boards/minimal-clean/input ] || { echo "FAIL redo didn't bring the folder back" && exit 1; }
run history
run daemon status
has '"running":true' "a daemon is running"
run daemon stop
run --no-daemon check
echo "ryudfm smoke test passed"
