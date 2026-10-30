#!/bin/sh
# Fetches the pinned STEP corpus: every `fetch` line of
# crates/ryustep/tests/corpus/pinned.txt (ryustep's) and of
# crates/ryucad/tests/corpus/pinned.txt (ryucad's assemblies), checked
# against its SHA-256.
#
#   scripts/fetch-corpus.sh [dir]   (default: $RYUCAD_CORPUS, else target/corpus)
#
# A file already there with the right hash is kept; one marked `unzip` is
# unpacked beside it. Needs curl, unzip and sha256sum (or shasum). The
# files are third parties' and never committed; ryukei-bench's `step` test
# reads ryustep's (`pinned_corpus_heals_no_fewer_bodies`), ryucad's
# `corpus` test its own (`pinned_assemblies_build_no_fewer_parts`).
set -eu

root=$(cd "$(dirname "$0")/.." && pwd)
dir=${1:-${RYUCAD_CORPUS:-$root/target/corpus}}
mkdir -p "$dir"

sha() {
    if command -v sha256sum >/dev/null 2>&1; then
        sha256sum "$1" | cut -d' ' -f1
    else
        shasum -a 256 "$1" | cut -d' ' -f1
    fi
}

for list in "$root/crates/ryustep/tests/corpus/pinned.txt" \
    "$root/crates/ryucad/tests/corpus/pinned.txt"; do
    grep '^fetch ' "$list" || true
done | while read -r _ name hash url action; do
    target=$dir/$name
    if [ ! -f "$target" ] || [ "$(sha "$target")" != "$hash" ]; then
        mkdir -p "$(dirname "$target")"
        echo "fetching $name"
        curl -fsSL -o "$target.part" "$url"
        got=$(sha "$target.part")
        if [ "$got" != "$hash" ]; then
            echo "$name: SHA-256 $got, pinned $hash" >&2
            rm -f "$target.part"
            exit 1
        fi
        mv "$target.part" "$target"
    fi
    if [ "${action:-}" = unzip ]; then
        unzip -q -o "$target" -d "$(dirname "$target")"
    fi
done
echo "corpus in $dir"
