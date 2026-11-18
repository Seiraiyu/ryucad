#!/bin/sh
# Drives a built ryueda binary through a short agent session: a project,
# the committed KiCad test library imported (a dry run, then for real), a
# small design (rails, a block, three resistors wired between them, a
# net of its own), its netlist, ERC and check, a library component
# released (PDM) in a repository of its own, undo and redo through the
# daemon, the activity log, a board placed, routed, checked, rendered and
# exported, made four layers with a track on in1 and exported again, a
# part turned over to the bottom and back, the daemon's status and stop,
# and a run in process.
#
#   scripts/smoke-ryueda.sh path/to/ryueda [path/to/kicad] [path/to/ryueda-app]
#
# The KiCad files default to crates/ryueda/tests/data/kicad beside this
# script; they are copied into the project and named by relative paths, so
# no path is translated on Windows.
# With the app, its --smoke frame is drawn on the session's project; no adapter is a skip unless RYUEDA_REQUIRE_GPU=1.
set -eu

exe=$(cd "$(dirname "$1")" && pwd)/$(basename "$1")
kicad=${2:-$(dirname "$0")/../crates/ryueda/tests/data/kicad}
kicad=$(cd "$kicad" && pwd)
app=""
if [ -n "${3:-}" ]; then
    app=$(cd "$(dirname "$3")" && pwd)/$(basename "$3")
fi
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
run net add top OUT --id out
run connect top/r1/2 OUT
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
# PDM (design 629): a library project in a repository of its own, its
# imported resistor numbered, proposed for release and listed.
mkdir -p "$work/pdm/lib"
cp -R "$kicad" "$work/pdm/lib/kicad"
git -C "$work/pdm" init -q -b main
cd "$work/pdm/lib"
run init --library
run import kicad kicad/symbols/Ryu_Test.kicad_sym kicad/footprints/Ryu_Test.pretty --models kicad/3dmodels
rm -rf kicad
run pdm init
run props set ryu_test_r number=100-0001
git -C "$work/pdm" add -A
git -C "$work/pdm" -c user.name=smoke -c user.email=smoke@example.com -c commit.gpgsign=false commit -q -m start
run release propose ryu_test_r
has '"label":"A"' "the resistor is proposed as A"
run release list
has '"status":"proposed"' "the record is listed as proposed"
run where-used ryu_test_r --no-dependents
has '"working":[]' "no design places the resistor yet"
run daemon stop
cd "$work/project"
run bom
has '"columns":["number","name","description","part","revision","quantity","uom","make_buy","refdes"' "the design's BOM has ryupdm's and ryueda's columns"
run undo
[ ! -f designs/d.json ] || { echo "FAIL undo left the design" && exit 1; }
run redo
[ -f designs/d.json ] || { echo "FAIL redo didn't bring the design back" && exit 1; }
run history
# A board (design 732): the three resistors in a row, their three nets
# routed on the top side, nothing found, rendered and exported. R_0402's
# pads sit 0.51 mm either side of its origin.
run board create Main --design d --rect 9,9 --id main
run board place main R1 --at 3,3
run board place main R2 --at 6,3
run board place main R3 --at 4.5,6
run board show main --pads
has '"net":"out"' "R1/2 and R2/1 are on OUT"
run board track add main --net out --layer top --path R1/2 R2/1 --id t1
run board track add main --net rail:vin --layer top --path R1/1 2.49,6 R3/1 --id t2
run board track add main --net rail:gnd --layer top --path R2/2 6.51,6 R3/2 --id t3
run board status main
has '"open":0' "every net is routed"
run board drc main
has '"findings":[]' "DRC finds nothing"
run board render main --out board.png
[ -s board.png ] || { echo "FAIL board render wrote nothing" && exit 1; }
run board export main --out fab
has '"allow_errors":false' "a clean board exports without --allow-errors"
n=$(find fab -type f | wc -l | tr -d ' ')
# Nine Gerber layers, two BOMs, two pick and place files, the manifest;
# no drill file, since nothing is drilled.
[ "$n" = 14 ] || { echo "FAIL the package has $n files, not 14: $(find fab -type f)" && exit 1; }
# Four copper layers (ryueda 7.A): OUT down two vias to in1 and back,
# exported again with two more copper files and the vias' drill file.
run board edit main --layers 4
has '"stackup"' "the board has a stackup"
run board via add main --net out --at 4.2,3 --id v1
run board via add main --net out --at 4.8,3 --id v2
run board track add main --net out --layer in1 --path 4.2,3 4.8,3 --id t4
has '"layer":"in1"' "a track on in1"
run board drc main
has '"findings":[]' "four layers, DRC finds nothing"
run board export main --out fab4
n=$(find fab4 -type f | wc -l | tr -d ' ')
# Eleven Gerber layers, the plated drills, two BOMs, two pick and place
# files, the manifest.
[ "$n" = 17 ] || { echo "FAIL the four-layer package has $n files, not 17: $(find fab4 -type f)" && exit 1; }
[ -s fab4/main-copper-in1.g2 ] || { echo "FAIL no in1 copper file" && exit 1; }
# R3 turned over to the bottom: its tracks end on the top, so its two nets
# open; its pick and place row says bottom.
run board place main R3 --at 4.5,6 --side bottom
has '"side":"bottom"' "R3 is on the bottom"
run board status main
has '"open":2' "R3's two nets are open"
run board export main --out fabb --allow-errors
grep -q '^R3,.*,bottom$' fabb/main-pos.csv || { echo "FAIL R3's row isn't bottom: $(cat fabb/main-pos.csv)" && exit 1; }
# And back on top, where its tracks meet it.
run board place main R3 --at 4.5,6
run board status main
has '"open":0' "R3 on top again, every net is routed"
if [ -n "$app" ]; then
    out=$("$app" --smoke "$work/project") || true
    case "$out" in
        '{"ok":true'*) echo "ok   ryueda-app --smoke: $out" ;;
        *"No graphics adapter"*)
            if [ "${RYUEDA_REQUIRE_GPU:-}" = 1 ]; then
                echo "FAIL ryueda-app --smoke: $out"
                exit 1
            fi
            echo "skip ryueda-app --smoke: no graphics adapter"
            ;;
        *)
            echo "FAIL ryueda-app --smoke: $out"
            exit 1
            ;;
    esac
fi
run daemon status
has '"running":true' "a daemon is running"
run daemon stop
run --no-daemon check
echo "ryueda smoke test passed"
