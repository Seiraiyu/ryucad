#!/bin/sh
# Drives a built ryucad binary through a short agent session: every
# primitive, a rebuild, inspection, and the daemon started and stopped.
#
#   scripts/smoke.sh path/to/ryucad
set -eu

exe=$(cd "$(dirname "$1")" && pwd)/$(basename "$1")
work=$(mktemp -d)
trap 'rm -rf "$work"' EXIT
mkdir "$work/project"
cd "$work/project"
export RYUCAD_RUNTIME_DIR="$work/run" RYUCAD_IDLE_TIMEOUT=60

# run ARGS...: the command must print {"ok":true,...}.
run() {
    out=$("$exe" "$@") || {
        echo "FAIL ryucad $*: $out"
        exit 1
    }
    case "$out" in
        '{"ok":true'*) echo "ok   ryucad $*" ;;
        *)
            echo "FAIL ryucad $*: $out"
            exit 1
            ;;
    esac
}

run init --unit mm
run part create smoke
run feature add smoke box --param x=10 --param y=20 --param z=30
run feature add smoke cylinder --param radius=5 --param "height=1 in" --origin 0,0,30
run feature add smoke cone --param radius=5 --param height=10 --param top_radius=2 --z 1,0,0
run feature add smoke sphere --param radius=8 --origin 50,0,0
run feature add smoke torus --param major_radius=20 --param minor_radius=3 --x 0,1,0
run rebuild
run inspect smoke torus-1
run check-format
run daemon status
case "$out" in *'"running":true'*) ;; *) echo "FAIL no daemon is running" && exit 1 ;; esac
run daemon stop
run --no-daemon rebuild smoke
echo "smoke test passed"
