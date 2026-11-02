#!/bin/sh
# shellcheck disable=SC2046,SC2086 # cameras and render flags are word lists on purpose
# Builds the README's demo from the gearbox in ryucad's committed corpus
# (crates/ryucad/tests/corpus/gearbox, which an agent made through the CLI
# with gearbox.py): an agent reads the assembly, gives it materials and weighs
# it, cuts it with a section plane, explodes it, changes a part and compares
# the change with Git's HEAD, then exports it and lists its bill of materials.
# Every model frame is `ryucad render`; the terminal beside it shows the
# commands and their real output (JSON condensed). The frames become
# assets/hero.gif (needs git and Python with Pillow; PYTHON names Python,
# python3 by default, and FONT a monospace TrueType font if neither Menlo nor
# DejaVu Sans Mono is found).
#
# Given the desktop app (macOS only: it photographs the window with
# screencapture, and finds it with pyobjc's Quartz, `pip install
# pyobjc-framework-Quartz`), it also opens the real app on the project and,
# at each step, photographs its window into assets/app.gif. Nobody clicks it:
# the agent drives the person's window with `ryucad focus`, `section`,
# `explode show` and `compare`, as an agent shows a person what it did. The
# window opens on the screen while it runs; leave it be.
#
#   scripts/hero.sh path/to/ryucad [assets/hero.gif [path/to/ryucad-app [assets/app.gif]]]
set -eu

HERE=$(cd "$(dirname "$0")/.." && pwd)
R=$(cd "$(dirname "$1")" && pwd)/$(basename "$1")
GIF=${2:-assets/hero.gif}
GIF=$(cd "$(dirname "$GIF")" && pwd)/$(basename "$GIF")
APP=${3:-}
APPGIF=${4:-assets/app.gif}
if [ -n "$APP" ]; then
  APP=$(cd "$(dirname "$APP")" && pwd)/$(basename "$APP")
  APPGIF=$(cd "$(dirname "$APPGIF")" && pwd)/$(basename "$APPGIF")
fi
work=$(mktemp -d)
# The folder names the window: "gearbox — ryucad".
P=$work/gearbox; OUT=$work/acts; APPS=$work/app
pid=
trap '[ -z "$pid" ] || { kill "$pid"; wait "$pid"; } 2>/dev/null; sleep 3; "$R" -C "$P" daemon stop >/dev/null 2>&1 || true; pkill -f "$(basename "$work")/gearbox daemon run" 2>/dev/null || true; rm -rf "$work"' EXIT
mkdir -p "$OUT" "$APPS" "$work/config" "$work/cache" "$work/out"
# Default preferences and an empty cache, for the app and its daemon: not
# the owner's own.
export RYUCAD_CONFIG_DIR="$work/config" RYUCAD_CACHE_DIR="$work/cache"
cp -R "$HERE/crates/ryucad/tests/corpus/gearbox" "$P"
cd "$P"

# Fixed identities and dates, so commits (and what Git prints) repeat.
export GIT_AUTHOR_DATE="2026-11-02T12:00:00Z" GIT_COMMITTER_DATE="2026-11-02T12:00:00Z"
export GIT_CONFIG_NOSYSTEM=1
git init -q -b main
git config user.name demo; git config user.email demo@example.com
"$R" git install >/dev/null
git add -A; git commit -qm gearbox

# Every edit goes through the project's session, so its activity log (and
# the app's) names who made it.
r() { out=$("$R" "$@") || { echo "ryucad $*: $out" >&2; exit 1; }; }
py() { "${PYTHON:-python3}" "$@"; }

# An act is a folder: `title`, `lines` (a tag, a space, the text: `$` a
# command, `o` output, `#` a note) and the model frames shown while it plays.
act=0
A=
begin() { act=$((act+1)); A=$OUT/$(printf %02d $act); mkdir -p "$A"; printf '%s\n' "$1" > "$A/title"; : > "$A/lines"; frames=0; }
say() { printf '%s %s\n' "$1" "$2" >> "$A/lines"; }
cmd() { say '$' "$*"; }
output() { sed 's/^/o /' >> "$A/lines"; }
# Cameras in millimetres: the assembly, and far enough out for its explode.
NEAR="285 -273 321 12 0 48"
FAR="245 -300 300 0 -45 80"
# A camera part of the way (0 to 1) from NEAR to FAR.
cam() {
  awk -v t="$1" -v a="$NEAR" -v b="$FAR" 'BEGIN { split(a, p); split(b, q);
    for (i = 1; i <= 6; i++) v[i] = p[i] + t * (q[i] - p[i]);
    printf "--eye=%.3f,%.3f,%.3f --target=%.3f,%.3f,%.3f --up=0,0,1", v[1], v[2], v[3], v[4], v[5], v[6] }'
}
# The assembly's camera turned about its target by DEG degrees.
turned() {
  awk -v d="$1" -v a="$NEAR" 'BEGIN { split(a, p); x = p[1] - p[4]; y = p[2] - p[5]; t = d * 3.14159265358979 / 180;
    printf "--eye=%.3f,%.3f,%.3f --target=%.3f,%.3f,%.3f --up=0,0,1", p[4] + x * cos(t) - y * sin(t), p[5] + x * sin(t) + y * cos(t), p[3], p[4], p[5], p[6] }'
}
frame() {
  frames=$((frames+1))
  # Each frame is shown after the lines written so far.
  n=$(wc -l < "$A/lines" | tr -d ' ')
  # Only what a frame asks for: not the participant's own planes or explode.
  case " $* " in *" --section "*) cut= ;; *) cut=--no-section ;; esac
  case " $* " in *" --explode "*) boom= ;; *) boom=--no-explode ;; esac
  r render gearbox --out "$A/$(printf %03d $frames)-$n.png" --size 480x420 $cut $boom "$@"
}
# The app's window, once it has settled (two pictures alike), with a caption.
shot=0
window=
app() {
  [ -n "$APP" ] || return 0
  shot=$((shot+1))
  last=$work/settle.png; : > "$last"
  sleep 1
  for _ in $(seq 1 40); do
    sleep 0.5
    screencapture -x -o -l"$window" "$APPS/$shot.png"
    cmp -s "$APPS/$shot.png" "$last" && break
    cp "$APPS/$shot.png" "$last"
  done
  printf '%s\n' "$1" > "$APPS/$shot.txt"
}
# Opens the app on the project and finds its window.
open_app() {
  [ -n "$APP" ] || return 0
  "$APP" "$P" > "$work/app.log" 2>&1 &
  pid=$!
  for _ in $(seq 1 60); do
    sleep 0.5
    window=$(py -c '
import Quartz
for w in Quartz.CGWindowListCopyWindowInfo(Quartz.kCGWindowListOptionOnScreenOnly, 0):
    if w.get("kCGWindowOwnerPID") == '"$pid"' and w.get("kCGWindowLayer") == 0:
        print(w["kCGWindowNumber"]); break')
    [ -z "$window" ] || { sleep 4; return 0; }
  done
  echo "the app's window never showed: $(cat "$work/app.log")" >&2; exit 1
}

begin "an assembly an agent built"
cmd 'ryucad assembly tree gearbox'
"$R" assembly tree gearbox | py -c '
import json, sys
r = json.load(sys.stdin)["result"]
for i in r["instances"]:
    s = i["solve"]
    how = "fixed" if i.get("fixed") else "turns about its axis" if s["dof"] == 1 and s["free"]["rotate"] else "%d dof" % s["dof"]
    print("%-9s %-13s %s" % (i["id"], i["part"], how))' | output
say '#' '15 mates: each shaft on a revolute in its bores,'
say '#' 'each gear fastened to its shaft'
open_app
r focus gearbox
for d in 0 4 8 12 16 20; do frame $(turned $d); done
app "an agent built this gearbox through ryucad's CLI: 9 parts, 15 mates. A person watches in the app"

begin "materials and mass"
r materials set steel --density "7850 kg/m^3" --colour "#8a9099" --default
cmd 'ryucad materials set steel --density "7850 kg/m^3" --default'
r materials set bronze --density "8800 kg/m^3" --colour "#c08a3e"
cmd 'ryucad materials set bronze --density "8800 kg/m^3" --colour "#c08a3e"'
for g in pinion-1 wheel-1 pinion-2 wheel-2; do r props set "$g" material=bronze; done
cmd 'ryucad props set wheel-1 material=bronze     # and the other gears'
frame $(turned 20)
r colour gearbox/housing --opacity 0.3
cmd 'ryucad colour gearbox/housing --opacity 0.3'
frame $(turned 20)
app "the agent gives the gears bronze and makes the housing translucent: the window follows"
cmd 'ryucad mass gearbox'
"$R" mass gearbox | py -c '
import json, sys
r = json.load(sys.stdin)["result"]
m = r["mass"]
print("mass     %.4f kg (within %.4f kg)" % (m["kg"], m["bound_kg"]))
print("volume   %.0f cm^3, %d solids" % (r["volume"]["m3"] * 1e6, r["solids"]))
c = r["centre"]["m"]
print("centre   (%.1f, %.1f, %.1f) mm" % tuple(x * 1000 for x in c))' | output
say '#' 'every value comes with its certified bound'
for d in 20 16 12 8 4 0; do frame $(turned $d); done

begin "a section"
r colour gearbox/housing --no-opacity
cmd 'ryucad colour gearbox/housing --no-opacity'
r section zx --offset -40mm --flip
cmd 'ryucad section zx --offset -40mm --flip'
say '#' 'the window cuts; renders cap and hatch the cut'
for o in -80 -70 -60 -50 -40 -30 -20 -10 0 -10 -20 -30 -40; do frame $(cam 0) --section "zx,offset=${o}mm,flip"; done
app "\`ryucad section\`: the agent cuts the person's window open to show the gears inside"

begin "an exploded view"
r section --off
r explode save gearbox open --steps '[{"instances":["input","middle","output","pinion-1","wheel-1","pinion-2","wheel-2"],"translate":[0,0,"60 mm"]},{"instances":["input","middle","output"],"translate":[0,"-110 mm",0]}]'
cmd 'ryucad explode save gearbox open --steps @open.json'
say '#' 'lift the shafts and gears, then draw the shafts out'
r explode show open --part gearbox --fraction 1
cmd 'ryucad explode show open --fraction 1'
for k in 0 1 2 3 4 5 6 7 8 9 10 11 12; do
  f=$(awk "BEGIN { printf \"%.4f\", $k / 12 }")
  frame $(cam "$f") --explode open --fraction "$f"
done
app "\`ryucad explode show\`: an exploded view, saved in Git beside the assembly"

begin "a change, compared with HEAD"
r explode show --off
r feature edit housing corners --radius 20mm
cmd 'ryucad feature edit housing corners --radius 20mm'
cmd 'ryucad diff --model'
"$R" diff --model | py -c '
import json, sys
r = json.load(sys.stdin)["result"]
for pid, p in sorted(r["parts"].items()):
    for fid, f in sorted((p.get("features") or {}).items()):
        if f.get("status") not in (None, "same"):
            for c in f.get("changes", []):
                a, b = c.get("from"), c.get("to")
                if c.get("at") == "/groups":
                    a, b, what = a[0]["radius"], b[0]["radius"], "radius"
                else:
                    what = c.get("at")
                print("%s/%s %s: %s -> %s" % (pid, fid, what, a, b))
    for sid, s in sorted((p.get("solids") or {}).items()):
        b = s.get("body")
        if b:
            print("%s/%s faces: %d changed, %d new, %d gone" % (pid, sid, len(b["faces"]["changed"]), len(b["faces"]["new"]), len(b["faces"]["gone"])))
    o = p.get("occurrences")
    if o and o.get("changed"):
        print("%s: %d occurrence changed (%s)" % (pid, len(o["changed"]), ", ".join(x if isinstance(x, str) else x.get("path", "?") for x in o["changed"])))' | output
r compare HEAD
cmd 'ryucad compare HEAD          # show the person watching'
frame $(turned 0) --compare HEAD
frame $(turned 0) --compare HEAD
app "\`ryucad compare HEAD\`: the window ghosts the commit and colours the faces that changed"
r compare --off
cmd 'ryucad check'
"$R" check | py -c '
import json, sys
d = json.load(sys.stdin)
c = d["result"] if d["ok"] else d["error"]["details"]["report"]
print("passed: %s, %d findings" % (str(c["passed"]).lower(), len([f for f in c["findings"] if f["severity"] == "error"])))' | output
git add -A; git commit -qm "rounder housing"
cmd 'git commit -am "rounder housing"'

begin "out to other tools"
for f in step 3mf usdz; do
  r export gearbox --out "$work/out/gearbox.$f"
  cmd "ryucad export gearbox --out gearbox.$f"
  printf '%s\n' "$out" | py -c '
import json, sys
r = json.load(sys.stdin)["result"]
print("%s: %.1f MB, %d products" % (r["format"], r["bytes"] / 1e6, len(r.get("products", []))))' | output
done
cmd 'ryucad bom gearbox --flat'
"$R" bom gearbox --flat | py -c '
import json, sys
r = json.load(sys.stdin)["result"]
for row in sorted(r["flat"], key=lambda w: w["items"][0]["item"]):
    print("%d  %-13s %d  %s" % (row["items"][0]["item"], row["part"], row["quantity"]["count"], row["properties"].get("material") or "steel (default)"))' | output
for d in 0 -4 -8 -12 -16 -20 -24 -28 -32 -36; do frame $(turned $d); done

py - "$OUT" "$GIF" <<'PY'
import os, pathlib, sys, textwrap
from PIL import Image, ImageDraw, ImageFont

def font(size):
    for f in [os.environ.get("FONT"), "/System/Library/Fonts/Menlo.ttc",
              "/usr/share/fonts/truetype/dejavu/DejaVuSansMono.ttf",
              "/usr/share/fonts/TTF/DejaVuSansMono.ttf", "C:/Windows/Fonts/consola.ttf"]:
        if f and os.path.exists(f):
            return ImageFont.truetype(f, size)
    sys.exit("no monospace font found: set FONT")

MONO, BOLD = font(13), font(14)
W, H, TW = 480, 420, 600
BG, FG, DIM = (30, 32, 36), (214, 218, 224), (128, 134, 144)
COLOR = {"$": (120, 200, 255), "o": FG, "#": DIM}
CW = MONO.getbbox("M")[2]
COLS = (TW - 28) // CW
LH = 18

def terminal(title, lines):
    img = Image.new("RGB", (TW, H), BG)
    d = ImageDraw.Draw(img)
    d.text((14, 10), title, font=BOLD, fill=(120, 200, 255))
    rows = []
    for tag, text in lines:
        lead = "$ " if tag == "$" else ""
        for part in textwrap.wrap(lead + text, COLS, subsequent_indent="    ",
                                  drop_whitespace=False, break_on_hyphens=False) or [""]:
            rows.append((tag, part))
    rows = rows[-((H - 44) // LH):]
    for i, (tag, part) in enumerate(rows):
        d.text((14, 38 + i * LH), part, font=MONO, fill=COLOR.get(tag, FG))
    return img

def label(model, title):
    m = model.copy()
    d = ImageDraw.Draw(m)
    w = BOLD.getbbox(title)[2]
    d.rounded_rectangle((10, 10, 26 + w, 34), 6, fill=(30, 32, 36))
    d.text((18, 13), title, font=BOLD, fill=(255, 255, 255))
    return m

frames, durations = [], []
def show(model, term, ms):
    f = Image.new("RGB", (W + TW, H))
    f.paste(label(model, title), (0, 0)); f.paste(term, (W, 0))
    frames.append(f); durations.append(ms)

# Each shot was taken after some of its act's lines. Lines appear one
# command at a time; shots taken after no new line (turns, cuts, the
# explode) play quickly; each act holds its last frame.
model = Image.new("RGB", (W, H), (244, 245, 247))
for act in sorted(pathlib.Path(sys.argv[1]).iterdir()):
    title = (act / "title").read_text().strip()
    lines = [tuple(raw.partition(" ")[::2]) for raw in (act / "lines").read_text().splitlines()]
    shown = 0
    for shot in sorted(act.glob("*.png")):
        at = int(shot.stem.split("-")[1])
        while shown < at:
            shown += 1
            if lines[shown - 1][0] == "$" and shown < at:
                show(model, terminal(title, lines[:shown]), 700)
        model = Image.open(shot).convert("RGB")
        show(model, terminal(title, lines[:shown]), 160 if shown < len(lines) else 120)
    while shown < len(lines):
        shown += 1
        if lines[shown - 1][0] == "$" or shown == len(lines):
            show(model, terminal(title, lines[:shown]), 700)
    durations[-1] = 3000

# One palette for every frame, from the held ones and the last.
held = [frames[i] for i, ms in enumerate(durations) if ms == 3000] + [frames[-1]]
sheet = Image.new("RGB", (W + TW, H * len(held)))
for i, f in enumerate(held):
    sheet.paste(f, (0, H * i))
# Text is too thin to win a place in the palette; give its colours one.
d = ImageDraw.Draw(sheet)
for i, c in enumerate(list(COLOR.values()) + [(120, 200, 255), (192, 138, 62)]):
    d.rectangle((i * 60, 0, i * 60 + 59, 59), fill=c)
pal = sheet.quantize(colors=224, method=Image.Quantize.MEDIANCUT)
q = [f.quantize(palette=pal, dither=Image.Dither.NONE) for f in frames]
q[0].save(sys.argv[2], save_all=True, append_images=q[1:], duration=durations, loop=0, optimize=True)
print("wrote %s (%d frames)" % (sys.argv[2], len(q)))
PY

[ -n "$APP" ] || exit 0
py - "$APPS" "$APPGIF" <<'PY'
import os, pathlib, sys
from PIL import Image, ImageDraw, ImageFont

def font(size):
    for f in [os.environ.get("FONT"), "/System/Library/Fonts/Menlo.ttc",
              "/usr/share/fonts/truetype/dejavu/DejaVuSansMono.ttf",
              "/usr/share/fonts/TTF/DejaVuSansMono.ttf", "C:/Windows/Fonts/consola.ttf"]:
        if f and os.path.exists(f):
            return ImageFont.truetype(f, size)
    sys.exit("no monospace font found: set FONT")

WIDTH = int(os.environ.get("APP_WIDTH", "1280"))
CAP = font(17)
shots = sorted(pathlib.Path(sys.argv[1]).glob("*.png"), key=lambda p: int(p.stem))
stills = []
for i, p in enumerate(shots):
    img = Image.open(p).convert("RGB")
    if img.width > WIDTH:
        img = img.resize((WIDTH, round(img.height * WIDTH / img.width)), Image.LANCZOS)
    w, h = img.size
    f = Image.new("RGB", (w, h + 52), (30, 32, 36))
    f.paste(img, (0, 52))
    d = ImageDraw.Draw(f)
    n = "%d/%d  " % (i + 1, len(shots))
    d.text((16, 15), n, font=CAP, fill=(128, 134, 144))
    d.text((16 + CAP.getbbox(n)[2], 15), p.with_suffix(".txt").read_text().strip(), font=CAP, fill=(245, 196, 92))
    stills.append(f)
# Each still held; plain cuts keep the file small.
durations = [4200] * len(stills)
sheet = Image.new("RGB", (stills[0].width, stills[0].height * len(stills)))
for i, f in enumerate(stills):
    sheet.paste(f, (0, stills[0].height * i))
pal = sheet.quantize(colors=224, method=Image.Quantize.MEDIANCUT)
q = [f.quantize(palette=pal, dither=Image.Dither.NONE) for f in stills]
q[0].save(sys.argv[2], save_all=True, append_images=q[1:], duration=durations, loop=0, optimize=True)
print("wrote %s (%d stills)" % (sys.argv[2], len(stills)))
PY
