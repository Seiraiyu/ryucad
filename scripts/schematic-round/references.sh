#!/bin/sh
# The blind review's reference schematics as SVG (plan 2.G): `sh
# references.sh <worktree> <out>`, run inside the devcontainer image, which
# has kicad-cli 9.0.8. On cox-worker-2:
#
#   sg docker -c "docker run --rm -v <worktree>:/w:ro -v <out>:/out \
#     ryucad-dev:local sh /w/scripts/schematic-round/references.sh /w /out"
#
# Exports the RP2040, buck, Ethernet and front end references without
# their drawing sheets into <out>/<circuit>/, drops the front end's
# single-ended amplifier (outside the corpus's cut), and crops the Ethernet
# and buck sheets to the parts the corpus draws (crops checked by eye on
# cox-worker-2's planning renders, 2026-11-11). The breakout's reference is
# Eagle's and can't be exported, as in plan 1.D. images.sh turns these into
# PNGs.
set -eu
w=$1 out=$2
refs=$w/crates/ryueda/tests/schematic_corpus/references
for f in rp2040/Mitayi-Pico-D1 buck/usb-power-port ethernet/ESP32-PoE_Rev_M2 frontend/amp; do
  c=${f%%/*}
  mkdir -p "$out/$c"
  kicad-cli sch export svg --exclude-drawing-sheet -o "$out/$c" "$refs/$f.kicad_sch"
done
rm -f "$out/frontend/amp-SeDiffAmp.svg"
# A sheet symbol's "File: x.kicad_sch" names the tool: its hidden text and
# its stroked glyphs go, so no judge reads it.
for f in "$out"/*/*.svg; do
  python3 -I - "$f" <<'PY'
import re, sys
path = sys.argv[1]
with open(path, encoding="utf-8") as f:
    s = f.read()
s = re.sub(r'<text [^>]*>File: [^<]*</text>\n', "", s)
s = re.sub(r'<g class="stroked-text"><desc>File: [^<]*</desc>.*?</g>\n?', "", s, flags=re.S)
with open(path, "w", encoding="utf-8") as f:
    f.write(s)
PY
done
crop() { # file x y w h: the root <svg>'s viewBox, width and height
  sed -i "0,/viewBox=\"[^\"]*\"/s//viewBox=\"$2 $3 $4 $5\"/; 0,/width=\"[^\"]*\"/s//width=\"$4mm\"/; 0,/height=\"[^\"]*\"/s//height=\"$5mm\"/" "$1"
}
crop "$out/ethernet/ESP32-PoE_Rev_M2.svg" 10 120 240 170   # the Ethernet panel
crop "$out/buck/usb-power-port.svg" 60 105 125 65          # the TPS54331 circuit alone
ls -R "$out"
