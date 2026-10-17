#!/bin/sh
# Drives a built ryucad binary through a short agent session: every
# primitive, a rebuild, inspection, the live session (participants, camera,
# status, watch), settings, a mesh, a render, undo, doctor and history, and
# the daemon started and stopped. Given the desktop app too, it checks that
# the app loads the project and draws a frame offscreen (`--smoke`); with no
# graphics adapter that is skipped, unless RYUCAD_REQUIRE_GPU=1.
#
#   scripts/smoke.sh path/to/ryucad [path/to/ryucad-app]
set -eu

exe=$(cd "$(dirname "$1")" && pwd)/$(basename "$1")
app=""
[ $# -ge 2 ] && app=$(cd "$(dirname "$2")" && pwd)/$(basename "$2")
work=$(mktemp -d)
trap 'rm -rf "$work"' EXIT
mkdir "$work/project"
cd "$work/project"
export RYUCAD_RUNTIME_DIR="$work/run" RYUCAD_IDLE_TIMEOUT=60 RYUCAD_CONFIG_DIR="$work/config"

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
run participants
run camera --view iso
run status "smoke testing"
run config set display.quality coarse
run config get display.quality
case "$out" in *'"value":"coarse"'*) ;; *) echo "FAIL the preference didn't stick" && exit 1 ;; esac
run mesh smoke --out "$work/mesh.bin"
[ -s "$work/mesh.bin" ] || { echo "FAIL no mesh written" && exit 1; }
run render --size 320x240 --out "$work/render.png"
[ "$(head -c 4 "$work/render.png" | tail -c 3)" = "PNG" ] || { echo "FAIL not a PNG" && exit 1; }
run activity
run undo
run redo
run doctor
case "$out" in *'"healthy":true'*) ;; *) echo "FAIL the project isn't healthy" && exit 1 ;; esac
run history
run watch --topic model --timeout 1
if [ -n "$app" ]; then
    out=$("$app" --smoke "$work/project") || true
    case "$out" in
        '{"ok":true'*) echo "ok   ryucad-app --smoke: $out" ;;
        *"No graphics adapter"*)
            if [ "${RYUCAD_REQUIRE_GPU:-}" = 1 ]; then
                echo "FAIL ryucad-app --smoke: $out"
                exit 1
            fi
            echo "skip ryucad-app --smoke: no graphics adapter"
            ;;
        *)
            echo "FAIL ryucad-app --smoke: $out"
            exit 1
            ;;
    esac
fi
run daemon status
case "$out" in *'"running":true'*) ;; *) echo "FAIL no daemon is running" && exit 1 ;; esac
run daemon stop
run --no-daemon rebuild smoke
echo "smoke test passed"
