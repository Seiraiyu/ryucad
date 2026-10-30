#!/bin/sh
# Drives a built ryueda binary through a short agent session: a project,
# the committed KiCad test library imported (a dry run, then for real), a
# small design (rails, a block, three resistors wired between them, a
# net of its own), its netlist, ERC and check, undo and redo through the
# daemon, the activity log, the daemon's status and stop, and a run in
# process.
#
#   scripts/smoke-ryueda.sh path/to/ryueda [path/to/kicad]
#
# The KiCad files default to crates/ryueda/tests/data/kicad beside this
# script; they are copied into the project and named by relative paths, so
# no path is translated on Windows.
set -eu

exe=$(cd "$(dirname "$1")" && pwd)/$(basename "$1")
kicad=${2:-$(dirname "$0")/../crates/ryueda/tests/data/kicad}
kicad=$(cd "$kicad" && pwd)
work=$(mktemp -d)
trap 'rm -rf "$work"' EXIT
mkdir "$work/project"
cp -R "$kicad" "$work/project/kicad"
cd "$work/project"
export RYUEDA_RUNTIME_DIR="$work/run" RYUEDA_IDLE_TIMEOUT=60 RYUEDA_CONFIG_DIR="$work/config"
# The same IDs every run.
export RYUEDA_ID_SEED=smoke

# run ARGS...: the command must print {"ok":true,...}.
run() {
    out=$("$exe" "$@") || {
        echo "FAIL ryueda $*: $out"
        exit 1
    }
    case "$out" in
        '{"ok":true'*) echo "ok   ryueda $*" ;;
        *)
            echo "FAIL ryueda $*: $out"
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
run import kicad kicad/symbols/Ryu_Test.kicad_sym kicad/footprints/Ryu_Test.pretty --models kicad/3dmodels --dry-run
has '"dry_run":true' "the dry run says so"
[ ! -d library/components ] || { echo "FAIL the dry run wrote the library" && exit 1; }
run import kicad kicad/symbols/Ryu_Test.kicad_sym kicad/footprints/Ryu_Test.pretty --models kicad/3dmodels
[ -f library/components/ryu_test_r.json ] || { echo "FAIL no resistor imported" && exit 1; }
[ -f library/models/r_0402.step ] || { echo "FAIL no model imported" && exit 1; }
run libcomp list
run rail add VIN --id vin --voltage "5 V"
run rail add GND --id gnd
run block create Divider --id top
run component add top --of ryu_test_r --id r1
run component add top --of ryu_test_r --id r2 --value 10k
run component add top --of ryu_test_r --id r3
run connect top/r1/1 rail:VIN
run connect top/r1/2 OUT --new
run connect top/r2/1 OUT
run connect top/r2/2 rail:GND
run connect top/r3/1 rail:VIN
run connect top/r3/2 rail:GND
run design create Divider --root top --id d
run netlist
has '"R3"' "the netlist places R3"
run erc
has '"errors":0,"findings":[],"warnings":0}' "ERC finds nothing"
run check
run undo
[ ! -f designs/d.json ] || { echo "FAIL undo left the design" && exit 1; }
run redo
[ -f designs/d.json ] || { echo "FAIL redo didn't bring the design back" && exit 1; }
run history
run daemon status
has '"running":true' "a daemon is running"
run daemon stop
run --no-daemon check
echo "ryueda smoke test passed"
