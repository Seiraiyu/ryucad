#!/bin/sh
# Fetches the pinned KiCad boards of the ODB++ corpus and exports each with
# the pinned kicad-cli: every `fetch` line of the list (and `commit` lines too
# with --all), checked against its board file's SHA-256.
#
#   scripts/fetch-dfm-corpus.sh [--all] [dir]
#       (default dir: $RYUDFM_CORPUS, else target/dfm-corpus;
#        the list: $RYUDFM_PINNED, else the corpus's pinned.txt)
#
# Each board lands in <dir>/<name>/: odb/ (the uncompressed export),
# <name>.zip (the same, zipped), pos.csv (KiCad's component positions, both
# sides, mm), netlist.d356 (its IPC-D-356 netlist of the board) and SOURCE,
# as the committed boards' tests/corpus/reference/<name>/ hold them. Sources
# are kept in <dir>/.src; a board already exported from the same line by the
# same KiCad is kept. Needs curl, tar, sha256sum (or shasum) and kicad-cli at
# the pinned version (the devcontainer has it). The boards are third
# parties', under their own licences, and never committed.
set -eu

root=$(cd "$(dirname "$0")/.." && pwd)
kinds=fetch
if [ "${1:-}" = --all ]; then
    kinds="fetch commit"
    shift
fi
dir=${1:-${RYUDFM_CORPUS:-$root/target/dfm-corpus}}
list=${RYUDFM_PINNED:-$root/crates/ryudfm/tests/corpus/pinned.txt}
mkdir -p "$dir/.src"
dir=$(cd "$dir" && pwd)

sha() {
    if command -v sha256sum >/dev/null 2>&1; then
        sha256sum "$1" | cut -d' ' -f1
    else
        shasum -a 256 "$1" | cut -d' ' -f1
    fi
}

# kicad-cli writes its settings under $HOME; keep them out of a read-only home.
HOME=$dir/.src/home
export HOME
mkdir -p "$HOME"

want=$(sed -n 's/^kicad //p' "$list")
options=$(sed -n 's/^options //p' "$list")
command -v kicad-cli >/dev/null 2>&1 || {
    echo "kicad-cli not found: the corpus is exported with KiCad $want" >&2
    exit 1
}
have=$(kicad-cli version)
if [ "$have" != "$want" ]; then
    echo "kicad-cli is $have, the corpus is pinned to $want" >&2
    exit 1
fi

grep -E '^(fetch|commit) ' "$list" | while read -r kind name repo commit board licence hash; do
    case " $kinds " in *" $kind "*) ;; *) continue ;; esac
    src=$dir/.src/$name
    pcb=$src/$board
    if [ ! -f "$pcb" ] || [ "$(sha "$pcb")" != "$hash" ]; then
        echo "fetching $name ($repo at $commit)"
        rm -rf "$src"
        mkdir -p "$src"
        curl -fsSL -o "$src.tar.gz.part" "https://github.com/$repo/archive/$commit.tar.gz"
        tar -xzf "$src.tar.gz.part" -C "$src" --strip-components=1
        rm -f "$src.tar.gz.part"
        if [ ! -f "$pcb" ]; then
            echo "$name: no $board in $repo at $commit" >&2
            exit 1
        fi
        got=$(sha "$pcb")
        if [ "$got" != "$hash" ]; then
            echo "$name: $board has SHA-256 $got, pinned $hash" >&2
            exit 1
        fi
    fi

    out=$dir/$name
    stamp="$kind $name $repo $commit $board $licence $hash kicad $have $options"
    if [ -f "$out/SOURCE" ] && [ "$(head -n 1 "$out/SOURCE")" = "$stamp" ]; then
        continue
    fi
    echo "exporting $name"
    rm -rf "$out"
    mkdir -p "$out"
    # shellcheck disable=SC2086 # options are words
    kicad-cli pcb export odb $options --compression none -o "$out/odb" "$pcb"
    # shellcheck disable=SC2086
    kicad-cli pcb export odb $options --compression zip -o "$out/$name.zip" "$pcb"
    kicad-cli pcb export pos --format csv --units mm --side both -o "$out/pos.csv" "$pcb"
    kicad-cli pcb export ipcd356 -o "$out/netlist.d356" "$pcb"
    {
        echo "$stamp"
        echo "https://github.com/$repo/tree/$commit"
        echo "licence $licence"
    } >"$out/SOURCE"
done
echo "dfm corpus in $dir"
