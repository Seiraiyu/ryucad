#!/bin/sh
# Fetches the schematic corpus's references that aren't committed (share-alike
# or unclear licences): every `fetch` line of the manifest, checked against its
# SHA-256.
#
#   scripts/fetch-schematic-refs.sh [dir]
#       (default dir: $RYUEDA_SCHEMATIC_REFS, else target/schematic-refs;
#        the manifest: crates/ryueda/tests/schematic_corpus/manifest.txt)
#
# Each lands in <dir>/<circuit>/<file name>, its URL's last part decoded. A
# file already there with the right SHA-256 is kept. Needs curl and sha256sum
# (or shasum). The files are third parties', under their own licences, and
# never committed.
set -eu

root=$(cd "$(dirname "$0")/.." && pwd)
dir=${1:-${RYUEDA_SCHEMATIC_REFS:-$root/target/schematic-refs}}
list=$root/crates/ryueda/tests/schematic_corpus/manifest.txt
mkdir -p "$dir"

sha() {
    if command -v sha256sum >/dev/null 2>&1; then
        sha256sum "$1" | cut -d' ' -f1
    else
        shasum -a 256 "$1" | cut -d' ' -f1
    fi
}

circuit=
while read -r first second third; do
    case "$first" in
    circuit) circuit=$second ;;
    fetch)
        url=$second
        hash=$third
        name=$(basename "$url" | sed 's/%20/ /g')
        out=$dir/$circuit/$name
        mkdir -p "$dir/$circuit"
        if [ -f "$out" ] && [ "$(sha "$out")" = "$hash" ]; then
            echo "kept $circuit/$name"
            continue
        fi
        curl -fsSL --retry 3 -o "$out.part" "$url"
        got=$(sha "$out.part")
        if [ "$got" != "$hash" ]; then
            rm -f "$out.part"
            echo "$circuit/$name: SHA-256 $got, the manifest pins $hash" >&2
            exit 1
        fi
        mv "$out.part" "$out"
        echo "fetched $circuit/$name"
        ;;
    esac
done <"$list"
