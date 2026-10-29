#!/bin/sh
# Fetches the pinned KiCad corpus: every `fetch` line of
# crates/ryueda/tests/kicad/pinned.txt, checked against its SHA-256 and
# unpacked beside it.
#
#   scripts/fetch-kicad.sh [dir]   (default: $RYUEDA_KICAD_CORPUS, else target/kicad)
#
# An archive already there with the right hash is kept, and unpacked
# again only when its folder is missing. Needs curl, tar and sha256sum
# (or shasum). KiCad's libraries (CC-BY-SA 4.0 with KiCad's exception)
# are never committed; ryueda's `kicad_corpus` test reads them.
set -eu

root=$(cd "$(dirname "$0")/.." && pwd)
dir=${1:-${RYUEDA_KICAD_CORPUS:-$root/target/kicad}}
list=$root/crates/ryueda/tests/kicad/pinned.txt
mkdir -p "$dir"

sha() {
    if command -v sha256sum >/dev/null 2>&1; then
        sha256sum "$1" | cut -d' ' -f1
    else
        shasum -a 256 "$1" | cut -d' ' -f1
    fi
}

grep '^fetch ' "$list" | while read -r _ name hash url; do
    target=$dir/$name
    if [ ! -f "$target" ] || [ "$(sha "$target")" != "$hash" ]; then
        echo "fetching $name"
        curl -fsSL -o "$target.part" "$url"
        got=$(sha "$target.part")
        if [ "$got" != "$hash" ]; then
            echo "$name: SHA-256 $got, pinned $hash" >&2
            rm -f "$target.part"
            exit 1
        fi
        mv "$target.part" "$target"
        rm -rf "$dir/${name%.tar.gz}"
    fi
    if [ ! -d "$dir/${name%.tar.gz}" ]; then
        echo "unpacking $name"
        tar -xzf "$target" -C "$dir"
    fi
done
echo "KiCad corpus in $dir"
