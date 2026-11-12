#!/bin/sh
# The blind review's images and pairs (plan 2.G): `sh images.sh <round dir>
# <projects dir> <ryueda> <out> [<svg-png>]`.
#
# For each circuit, every sheet of every block rendered to PNG twice:
# <out>/<circuit>/before/ from <projects dir>/<circuit> (the corpus from
# nothing, as round_projects left it) and <out>/<circuit>/final/ from
# <round dir>/<circuit>-lasting-both/project (round 3's last state). The
# reference SVGs references.sh wrote into <out>/refs/<circuit>/ become
# <out>/<circuit>/reference/ PNGs by svg-png (ryueda's own renderer; the
# built example's path, else `cargo run --release -p ryueda --example
# svg-png`). Then <out>/pairs.json for blind.py: per circuit
# self-<circuit> (before first, final second) and, but for the breakout,
# ref-<circuit> (final first, the reference second). Every render runs
# without a daemon and under a config folder of its own.
set -eu
round=$1 projects=$2 real=$3 out=$4
svgpng=${5:-}
cfg=$(mktemp -d)
export RYUEDA_NO_DAEMON=1 RYUEDA_CONFIG_DIR=$cfg
mkdir -p "$out"

# `render` for every sheet of every block of a project, into a folder.
sheets() { # project folder
  mkdir -p "$2"
  "$real" -C "$1" describe | python3 -I -c '
import json, sys
env = json.loads(sys.stdin.readline())
for b in env["result"]["blocks"]:
    for s in b["sheets"]:
        print(b["block_id"], s["sheet"])
' | while read -r b s; do
    "$real" -C "$1" render --block "$b" --sheet "$s" --format png --fit --scale 8 \
      --out "$2/$b-$s.png" > /dev/null
  done
}

topng() { # in.svg out.png
  if [ -n "$svgpng" ]; then
    "$svgpng" "$1" "$2" 8
  else
    cargo run -q --release -p ryueda --example svg-png -- "$1" "$2" 8
  fi
}

for c in breakout rp2040 buck frontend ethernet; do
  sheets "$projects/$c" "$out/$c/before"
  sheets "$round/$c-lasting-both/project" "$out/$c/final"
  if [ -d "$out/refs/$c" ]; then
    mkdir -p "$out/$c/reference"
    for f in "$out/refs/$c"/*.svg; do
      topng "$f" "$out/$c/reference/$(basename "$f" .svg).png"
    done
  fi
done

python3 -I - "$out" <<'PY'
import json, os, sys
out = sys.argv[1]
def images(c, side):
    d = os.path.join(out, c, side)
    names = sorted(n for n in os.listdir(d) if n.endswith(".png")) if os.path.isdir(d) else []
    paths = [os.path.join(d, n) for n in names]
    return paths[0] if len(paths) == 1 else paths
pairs = []
for c in ("breakout", "rp2040", "buck", "frontend", "ethernet"):
    pairs.append({"id": f"self-{c}", "first": images(c, "before"), "second": images(c, "final")})
for c in ("breakout", "rp2040", "buck", "frontend", "ethernet"):
    ref = images(c, "reference")
    if ref:
        pairs.append({"id": f"ref-{c}", "first": images(c, "final"), "second": ref})
with open(os.path.join(out, "pairs.json"), "w", encoding="utf-8") as f:
    json.dump(pairs, f, indent=1)
    f.write("\n")
print(len(pairs), "pairs")
PY
rm -rf "$cfg"
