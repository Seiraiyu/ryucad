#!/bin/sh
# Exports ryudfm's KiCad boards as STEP, for step-hunt's `pcba` source
# (design 277): every `commit` and `fetch` board of
# crates/ryudfm/tests/corpus/pinned.txt, fetched by
# scripts/fetch-dfm-corpus.sh --all, exported twice by the pinned kicad-cli:
#
#   <dir>/<name>/<name>.step        the board with its components' models
#   <dir>/<name>/<name>-board.step  the board alone
#   <dir>/<name>/SOURCE             where it came from, its licences, counts
#   <dir>/<name>/kicad.log          what kicad-cli said
#
#   scripts/export-pcba.sh [dir]
#       (default dir: $HUNT_PCBA, else target/pcba; the boards' sources:
#        $RYUDFM_CORPUS, else target/dfm-corpus; the models: $RYUEDA_KICAD_CORPUS,
#        else target/kicad, fetched by scripts/fetch-kicad.sh)
#
# A board already exported from the same line by the same KiCad is kept.
# The boards are their authors' (Apache-2.0, CC-BY-4.0) and the models
# KiCad's (CC-BY-SA-4.0 with KiCad's exception): never committed. Needs what
# fetch-dfm-corpus.sh needs (the devcontainer has it).
set -eu

root=$(cd "$(dirname "$0")/.." && pwd)
dir=${1:-${HUNT_PCBA:-$root/target/pcba}}
dfm=${RYUDFM_CORPUS:-$root/target/dfm-corpus}
models=${RYUEDA_KICAD_CORPUS:-$root/target/kicad}/kicad-packages3D-9.0.9
list=$root/crates/ryudfm/tests/corpus/pinned.txt
if [ ! -d "$models" ]; then
    echo "no KiCad 3D models in $models: run scripts/fetch-kicad.sh first" >&2
    exit 1
fi
sh "$root/scripts/fetch-dfm-corpus.sh" --all "$dfm"
mkdir -p "$dir"
dir=$(cd "$dir" && pwd)
dfm=$(cd "$dfm" && pwd)
models=$(cd "$models" && pwd)

# kicad-cli writes its settings under $HOME, and finds models through these
# (footprints name the variable of the KiCad they were made with).
HOME=$dfm/.src/home
KICAD9_3DMODEL_DIR=$models
KICAD8_3DMODEL_DIR=$models
KICAD7_3DMODEL_DIR=$models
KICAD6_3DMODEL_DIR=$models
KISYS3DMOD=$models
export HOME KICAD9_3DMODEL_DIR KICAD8_3DMODEL_DIR KICAD7_3DMODEL_DIR KICAD6_3DMODEL_DIR KISYS3DMOD
have=$(kicad-cli version)

products() {
    grep -cE '= ?PRODUCT\(' "$1" || true
}

grep -E '^(fetch|commit) ' "$list" | while read -r kind name repo commit board licence hash; do
    pcb=$dfm/.src/$name/$board
    out=$dir/$name
    stamp="$kind $name $repo $commit $board $licence $hash kicad $have models kicad-packages3D-9.0.9"
    if [ -f "$out/SOURCE" ] && [ "$(head -n 1 "$out/SOURCE")" = "$stamp" ]; then
        continue
    fi
    echo "exporting $name as STEP"
    rm -rf "$out"
    mkdir -p "$out"
    kicad-cli pcb export step --subst-models --force -o "$out/$name.step" "$pcb" >"$out/kicad.log" 2>&1
    kicad-cli pcb export step --board-only --force -o "$out/$name-board.step" "$pcb" >>"$out/kicad.log" 2>&1
    {
        echo "$stamp"
        echo "https://github.com/$repo/tree/$commit"
        echo "licence $licence (the board); CC-BY-SA-4.0 with KiCad's exception (its models)"
        echo "products $(products "$out/$name.step") with models, $(products "$out/$name-board.step") alone"
        echo "models not found $(grep -ci 'could not' "$out/kicad.log" || true)"
    } >"$out/SOURCE"
done
echo "PCBA exports in $dir"
