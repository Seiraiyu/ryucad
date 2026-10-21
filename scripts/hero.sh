#!/bin/sh
# Builds the README's demo model, a flanged hub, with the ryucad CLI and
# renders a frame after each step: the profile revolved round in steps, then
# the ribs, then an orbit. The frames become assets/hero.gif (needs Python with
# Pillow; PYTHON names it, python3 by default).
#
#   scripts/hero.sh path/to/ryucad [assets/hero.gif]
set -eu

R=$(cd "$(dirname "$1")" && pwd)/$(basename "$1")
GIF=${2:-assets/hero.gif}
work=$(mktemp -d)
trap 'rm -rf "$work"' EXIT
P=$work/project; OUT=$work/frames
mkdir -p "$P" "$OUT"
r() { out=$("$R" -C "$P" --no-daemon "$@") || { echo "ryucad $*: $out" >&2; exit 1; }; }
"$R" -C "$P" init --unit mm >/dev/null
r part create hub
r feature add hub sketch --id profile --plane zx
r sketch edit hub profile '[
  {"op":"add","type":"point","at":[0,6]},
  {"op":"add","type":"point","at":[0,30]},
  {"op":"add","type":"point","at":[6,30]},
  {"op":"add","type":"point","at":[6,14]},
  {"op":"add","type":"point","at":[30,14]},
  {"op":"add","type":"point","at":[30,6]},
  {"op":"add","type":"line","start":"p1","end":"p2"},
  {"op":"add","type":"line","start":"p2","end":"p3"},
  {"op":"add","type":"line","start":"p3","end":"p4"},
  {"op":"add","type":"line","start":"p4","end":"p5"},
  {"op":"add","type":"line","start":"p5","end":"p6"},
  {"op":"add","type":"line","start":"p6","end":"p1"},
  {"op":"add","type":"point","at":[-5,0]},
  {"op":"add","type":"point","at":[40,0]},
  {"op":"add","type":"line","start":"p7","end":"p8","construction":true}]'
r feature add hub revolve --id body --sketch profile --axis l7 --angle 30
n=0
frame() { n=$((n+1)); r render --out "$OUT/$(printf %03d $n).png" --size 480x360 "$@"; }
EYE="--eye 63,-77,66 --target 0,0,12 --up 0,0,1"
for a in 30 60 90 120 150 180 210 240 270 300 330; do
  r feature edit hub body --angle $a; frame $EYE
done
r feature edit hub body --angle full; frame $EYE
r feature add hub sketch --id ribs_a --plane zx
r sketch edit hub ribs_a '[
  {"op":"add","type":"point","at":[6,14]},{"op":"add","type":"point","at":[6,27]},{"op":"add","type":"point","at":[22,14]},
  {"op":"add","type":"line","start":"p1","end":"p2"},{"op":"add","type":"line","start":"p2","end":"p3"},{"op":"add","type":"line","start":"p3","end":"p1"},
  {"op":"add","type":"point","at":[6,-14]},{"op":"add","type":"point","at":[22,-14]},{"op":"add","type":"point","at":[6,-27]},
  {"op":"add","type":"line","start":"p4","end":"p5"},{"op":"add","type":"line","start":"p5","end":"p6"},{"op":"add","type":"line","start":"p6","end":"p4"}]'
r feature add hub extrude --id ribs_x --sketch ribs_a --length 4 --extent symmetric
frame $EYE
r feature add hub sketch --id ribs_b --plane yz
r sketch edit hub ribs_b '[
  {"op":"add","type":"point","at":[14,6]},{"op":"add","type":"point","at":[27,6]},{"op":"add","type":"point","at":[14,22]},
  {"op":"add","type":"line","start":"p1","end":"p2"},{"op":"add","type":"line","start":"p2","end":"p3"},{"op":"add","type":"line","start":"p3","end":"p1"},
  {"op":"add","type":"point","at":[-14,6]},{"op":"add","type":"point","at":[-14,22]},{"op":"add","type":"point","at":[-27,6]},
  {"op":"add","type":"line","start":"p4","end":"p5"},{"op":"add","type":"line","start":"p5","end":"p6"},{"op":"add","type":"line","start":"p6","end":"p4"}]'
r feature add hub extrude --id ribs_y --sketch ribs_b --length 4 --extent symmetric
frame $EYE
for k in $(seq 1 36); do
  t=$(awk "BEGIN { a = $k * 3.14159265358979 / 18; printf \"%.6f,%.6f\", 99.5 * cos(a - 0.8850), 99.5 * sin(a - 0.8850) }")
  frame --eye="$t,66" --target 0,0,12 --up 0,0,1
done
"${PYTHON:-python3}" - "$OUT" "$GIF" <<'PY'
import sys, pathlib
from PIL import Image
frames = [Image.open(p).convert("RGB") for p in sorted(pathlib.Path(sys.argv[1]).glob("*.png"))]
pal = frames[-1].quantize(colors=128, method=Image.Quantize.MEDIANCUT)
q = [f.quantize(palette=pal, dither=Image.Dither.NONE) for f in frames]
# 11 steps of the revolve, the full turn, two sets of ribs, the orbit.
d = [150] * 11 + [900, 700, 1200] + [90] * (len(q) - 14)
q[0].save(sys.argv[2], save_all=True, append_images=q[1:], duration=d, loop=0, optimize=True)
PY
echo "wrote $GIF ($n frames)"
