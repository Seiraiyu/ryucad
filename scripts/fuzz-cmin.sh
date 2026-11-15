#!/bin/sh
# Minimises fuzz/corpus/<target> (design 600, plan 30.D): cargo fuzz cmin
# under the nightly's ASan options with a per-unit timeout, which drops a
# unit slower than it, then puts back every seed fuzz/pinned.txt names
# that cmin dropped. A pinned seed slower than the timeout alone moves to
# fuzz/slow/<target>/, which the nightly doesn't replay; the owning
# crate's release regression test keeps it. A pinned seed that crashes
# stops the script: that is a find, not a slow seed.
#
#   sh scripts/fuzz-cmin.sh <target> <timeout s> <rss MB>
#
# Run from the repository root, with nightly and cargo-fuzz on PATH; the
# fuzz binary is looked for under $CARGO_TARGET_DIR (default fuzz/target).
set -eu
t=$1
timeout=$2
rss=$3
corpus=fuzz/corpus/$t
slow=fuzz/slow/$t
bin=${CARGO_TARGET_DIR:-$(pwd)/fuzz/target}/x86_64-unknown-linux-gnu/release/$t
keep=$(mktemp -d)
before=$(ls "$corpus" | wc -l)
grep -v '^#' fuzz/pinned.txt | grep "^$t/" | while read -r p; do
  if [ -f "fuzz/corpus/$p" ]; then cp "fuzz/corpus/$p" "$keep/"; fi
done
export ASAN_OPTIONS=quarantine_size_mb=64:allocator_release_to_os_interval_ms=1000
(cd fuzz && cargo +nightly fuzz cmin "$t" -- -timeout="$timeout" \
  -rss_limit_mb="$rss" -malloc_limit_mb=2048)
kept=$(ls "$corpus" | wc -l)
back=0
apart=0
for f in "$keep"/*; do
  [ -e "$f" ] || continue
  n=$(basename "$f")
  [ -f "$corpus/$n" ] && continue
  set +e
  "$bin" -timeout="$timeout" -rss_limit_mb="$rss" -malloc_limit_mb=2048 "$f" \
    > "$keep/$n.log" 2>&1
  rc=$?
  set -e
  case $rc in
    0) cp "$f" "$corpus/$n"; back=$((back + 1)) ;;
    70) mkdir -p "$slow"; cp "$f" "$slow/$n"; apart=$((apart + 1))
        echo "set apart: $t/$n (over $timeout s)" ;;
    *) echo "pinned seed $t/$n failed alone (exit $rc):"; tail -30 "$keep/$n.log"; exit 1 ;;
  esac
done
after=$(ls "$corpus" | wc -l)
echo "$t: $before seeds; cmin kept $kept; $back pinned put back; $apart set apart; $after now"
rm -rf "$keep"
