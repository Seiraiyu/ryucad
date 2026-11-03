#!/bin/sh
# Prints true when $1, a final ryucad tag vX.Y.Z, is at least every final
# vX.Y.Z tag on stdin (the releases published so far), else false. A
# pre-release (vX.Y.Z-...) is never latest. Other products' tags are ignored.
# Used by release.yml: a v0.7.x patch after v0.8.0 mustn't become GitHub's
# latest release, which install.sh and install.ps1 fetch (design 312).
set -eu
tag=$1
final='^v[0-9][0-9]*\.[0-9][0-9]*\.[0-9][0-9]*$'
if ! printf '%s\n' "$tag" | grep -q "$final"; then
  echo false
  exit 0
fi
highest=$( { cat; printf '%s\n' "$tag"; } | grep "$final" | sed 's/^v//' \
  | sort -t. -k1,1n -k2,2n -k3,3n | tail -n 1)
if [ "v$highest" = "$tag" ]; then echo true; else echo false; fi
