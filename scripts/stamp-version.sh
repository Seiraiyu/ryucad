#!/bin/sh
# Stamps a release version into the workspace before it is built:
#   scripts/stamp-version.sh 0.2.0
#   scripts/stamp-version.sh v0.2.0          (ryucad's release tag)
#   scripts/stamp-version.sh ryueda-v0.1.0   (ryueda's release tag)
# The version comes from the release tag, never from a file in the repo,
# where the workspace version stays 0.0.0. Cargo.lock's workspace entries
# are rewritten too, so `cargo build --locked` still works. Line endings
# are kept (Windows checkouts may have CRLF).
set -eu

v=${1:?usage: stamp-version.sh VERSION|TAG}
# A tag of either release line gives its version.
case "$v" in
    ryueda-v*) v=${v#ryueda-v} ;;
    v*) v=${v#v} ;;
esac
printf '%s\n' "$v" | grep -Eq '^[0-9]+\.[0-9]+\.[0-9]+(-[0-9A-Za-z.-]+)?$' ||
    { echo "stamp-version: not a version: $v" >&2; exit 2; }

# Each crate's [package] name: the first `name =` in its manifest.
members=$(for f in crates/*/Cargo.toml; do sed -n 's/^name = "\([^"]*\)".*/\1/p' "$f" | head -n 1; done | tr '\n' '|')
members=${members%|}

V="$v" perl -pi -e 's/^version = "[^"]*"(\r?)$/version = "$ENV{V}"$1/ if /^version = / && !$done++' Cargo.toml
V="$v" M="$members" perl -0pi -e 's/(name = "(?:$ENV{M})"\r?\nversion = )"[^"]*"/$1"$ENV{V}"/g' Cargo.lock

# Every member must now carry the version, in both files.
n=$(echo "$members" | tr '|' '\n' | wc -l)
got=$(V="$v" M="$members" perl -0ne 'my $c = () = /name = "(?:$ENV{M})"\r?\nversion = "\Q$ENV{V}\E"/g; print $c' Cargo.lock)
[ "$got" -eq "$n" ] || { echo "stamp-version: $got of $n workspace packages stamped in Cargo.lock" >&2; exit 1; }
grep -q "^version = \"$v\"" Cargo.toml || { echo "stamp-version: Cargo.toml not stamped" >&2; exit 1; }
echo "stamped $v"
