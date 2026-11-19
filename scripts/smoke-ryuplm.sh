#!/bin/sh
# Drives a built ryuplm binary through a short agent session: a catalogue,
# a numbering scheme and a class, items created (one by its scheme's next
# number, one with a legacy number and an AML), edited, renumbered, shown
# and listed, checked (the legacy number a warning), a duplicate refused,
# a change undone and redone through the daemon, a kit's own line, its
# BOM, a loop refused, where-used, the BOM diff and an unresolved link,
# the activity log, an item master imported and imported again, the
# ledger turned on and asked offline (the local number and its warning),
# the daemon's status and stop, a check in process, and the hosted
# service's contract (version's schema_outputs, a list window, git
# attributes, reps, resolve and index).
#
#   scripts/smoke-ryuplm.sh path/to/ryuplm
set -eu

exe=$(cd "$(dirname "$1")" && pwd)/$(basename "$1")
work=$(mktemp -d)
trap 'rm -rf "$work"' EXIT
mkdir -p "$work/catalogue"
cd "$work/catalogue"
export RYUPLM_RUNTIME_DIR="$work/run" RYUPLM_IDLE_TIMEOUT=60 RYUPLM_CONFIG_DIR="$work/config"

# run ARGS...: the command must print {"ok":true,...}.
run() {
    out=$("$exe" "$@") || {
        echo "FAIL ryuplm $*: $out"
        exit 1
    }
    case "$out" in
        '{"ok":true'*) echo "ok   ryuplm $*" ;;
        *)
            echo "FAIL ryuplm $*: $out"
            exit 1
            ;;
    esac
}

# fails CODE ARGS...: the command must fail with error CODE.
fails() {
    code=$1
    shift
    out=$("$exe" "$@") && {
        echo "FAIL ryuplm $* succeeded: $out"
        exit 1
    }
    case "$out" in
        *"\"code\":\"$code\""*) echo "ok   ryuplm $* (refused: $code)" ;;
        *)
            echo "FAIL ryuplm $*: expected $code: $out"
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
[ -f ryuplm.json ] || { echo "FAIL no manifest" && exit 1; }
# A repository of its own, so diff has a HEAD to compare with.
git init -q
git add -A
git -c user.name=smoke -c user.email=smoke@example.com commit -qm catalogue
run settings edit --scheme mech --pattern 'MECH-\d{5}' --next-template 'MECH-{seq:5}'
run settings edit --class machined
run item create --next --class machined --name Bracket
has '"MECH-00001"' "the scheme's next number"
run item create 100-0042 --name Legacy --aml '[{"manufacturer":"Acme","mpn":"X-1","status":"active"}]'
has '"Acme"' "the AML is kept"
run item edit MECH-00001 --description '6061, anodized'
run item renumber MECH-00001 MECH-00100
has '"previous_numbers"' "the old number is kept"
run item show MECH-00001
has '"MECH-00100"' "an old number still finds its item"
run item list --text legacy
has '"100-0042"' "list finds by text"
run check
has '"part_number_unschemed"' "the legacy number is a warning"
fails part_number_duplicate item create 100-0042
run item delete 100-0042
run undo
run item show 100-0042
run redo
fails item_not_found item show 100-0042
run history
run diff
# Structure: a kit naming the bracket by an own line, its BOM, a loop
# refused, where-used, the BOM diff, and a link no folder holds (a
# warning, never a failure).
run item create KIT-1 --name Kit
run bom add KIT-1 MECH-00100 2 --find 10
run bom show KIT-1
has '"MECH-00100"' "the kit's own line names the bracket"
fails bom_cycle bom add MECH-00100 KIT-1 1
run where-used MECH-00100 --working --no-dependents
has '"KIT-1"' "where-used finds the kit"
run bom diff
has '"KIT-1"' "the BOM diff shows the kit's new line"
run item link KIT-1 ryucad 0123456789abcdef0123456789abcdef kit
run check
has '"item_link_unresolved"' "a link no folder holds is a warning"
# An item master through a mapping the agent saves: guessed, checked,
# imported, imported again unchanged.
printf 'Item ID,Part Number,Description\n9001,200-0001,Spacer\n9002,200-0002,Standoff\n' > master.csv
run import guess master.csv --save erp
has '"imports/erp.json"' "the mapping is saved"
run import check erp master.csv
run import run erp master.csv
has '"written":2' "two items imported"
run import run erp master.csv
has '"written":0' "an unchanged re-import writes nothing"
# The ledger (design 936): turned on, then asked with no service to
# reach. --next falls back to the local number with a ledger_unavailable
# warning; number who has nothing to fall back to. The live round (a
# reservation, a push binding it, another catalogue's PR refused) is
# proved by ryupdm-service's own tests and its e2e.
export RYUPLM_REVIEW_URL=http://127.0.0.1:9 RYUPLM_REVIEW_TOKEN=smoke
git remote add origin https://github.com/acme/widgets.git
run settings edit --ledger on
has '"ledger":true' "the ledger is on"
run item create --next --class machined --name Clip
has '"reserved":false' "offline, the number is local"
has 'ledger_unavailable' "offline, a warning says so"
fails ledger_unavailable number who MECH-00101
run settings edit --ledger off

run daemon status
has '"running":true' "a daemon is running"
run daemon stop
run --no-daemon check
# The service's contract (design 938): the probe, a window of the list,
# the driver's lines, what the catalogue reaches, a reference, and the
# index.
case "$("$exe" version)" in
    *'"schema_outputs":1'*) echo "ok   ryuplm version (schema_outputs 1)" ;;
    *)
        echo "FAIL ryuplm version has no schema_outputs"
        exit 1
        ;;
esac
run item list --sort number --offset 0 --limit 1
has '"total":' "a window says how many match"
run git attributes
run reps
kit=$("$exe" item show KIT-1 | sed -n 's/.*"id":"\([0-9a-z]*\)".*/\1/p')
run resolve "item/$kit"
has '"standing":"here"' "the kit's reference stands"
run index --out "$work/index.jsonl"
[ -s "$work/index.jsonl" ] || {
    echo "FAIL ryuplm index wrote nothing"
    exit 1
}
echo "ryuplm smoke test passed"
