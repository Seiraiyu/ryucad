#!/bin/sh
# Fetches .devcontainer/debs.txt's packages for one architecture into a
# folder: each kept when already there with its SHA-256, else fetched with
# retries and backoff (snapshot.debian.org answers 503 when busy: #557) and
# checked.
#
#   sh .devcontainer/fetch-debs.sh <folder> [<architecture>]
#
# The architecture is dpkg's, by default this machine's.
set -eu
here=$(cd "$(dirname "$0")" && pwd)
out=$1
arch=${2:-$(dpkg --print-architecture 2>/dev/null || echo amd64)}
mkdir -p "$out"
grep -v '^#' "$here/debs.txt" | while read -r a sum url; do
    [ "$a" = "$arch" ] || continue
    name=$(basename "$url" | sed 's/%2B/+/g')
    file="$out/$name"
    if [ -f "$file" ] && echo "$sum  $file" | sha256sum -c - >/dev/null 2>&1; then
        echo "kept $name"
        continue
    fi
    rm -f "$file"
    # curl's own backoff: 1 s, doubling, ten tries, on any failure.
    curl -sSfL --retry 10 --retry-all-errors --retry-max-time 900 \
        --connect-timeout 30 -o "$file.part" "$url"
    if ! echo "$sum  $file.part" | sha256sum -c - >/dev/null; then
        rm -f "$file.part"
        echo "fetch-debs: $name doesn't have its SHA-256" >&2
        exit 1
    fi
    mv "$file.part" "$file"
    echo "fetched $name"
done
[ -n "$(ls "$out"/*_"$arch".deb 2>/dev/null)" ] || {
    echo "fetch-debs: no package for $arch in debs.txt" >&2
    exit 1
}
