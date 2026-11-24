#!/bin/sh
# usage: sh scripts/no-enclosing-install.sh <command...>
# Runs the command with TMPDIR inside a fresh Git repository, then fails if
# anything installed ryucad's or ryueda's drivers or hooks into it (#1004).
set -eu
# Under /tmp, not $TMPDIR: a long $TMPDIR one level deeper puts the
# daemons' sockets past sun_path's 108 bytes.
outer=$(mktemp -d /tmp/ryu-guard.XXXXXX)
git -C "$outer" init -q
mkdir "$outer/tmp"
before=$(git -C "$outer" config --local --list | sort)
code=0
TMPDIR="$outer/tmp" "$@" || code=$?
after=$(git -C "$outer" config --local --list | sort)
status=0
if [ "$before" != "$after" ]; then
  echo "FAIL: the enclosing repo's config changed:"
  printf '%s\n' "$after" | grep -E '^(merge|diff)\.ryu|^merge\.renames' || true
  status=1
fi
for h in pre-commit pre-merge-commit; do
  if [ -e "$outer/.git/hooks/$h" ]; then echo "FAIL: hook $h installed"; status=1; fi
done
rm -rf "$outer"
[ $status -eq 0 ] && echo "ok: nothing installed into the enclosing repo"
[ $code -ne 0 ] && exit $code
exit $status
