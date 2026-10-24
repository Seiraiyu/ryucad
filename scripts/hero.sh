#!/bin/sh
# Builds the README's demo, a flanged hub, with the ryucad CLI in a real Git
# repository, then has two agents change it on two branches and merges them:
# ryucad's merge driver merges the parameters file by value, records the one
# real conflict, and `ryucad merge status` and `merge resolve` settle it. Every
# model frame is `ryucad render`; the terminal beside it shows the commands and
# their real output (JSON condensed). The frames become assets/hero.gif (needs
# git and Python with Pillow; PYTHON names Python, python3 by default, and FONT
# a monospace TrueType font if neither Menlo nor DejaVu Sans Mono is found).
#
# Given the desktop app (macOS only: it photographs the window with
# screencapture, and finds it with pyobjc's Quartz, `pip install
# pyobjc-framework-Quartz`), it also opens the real app on the project and,
# at each step, photographs its window into assets/compare.gif. Nobody clicks
# it: the agent drives it with `ryucad compare`, as an agent shows a person
# what it did, and the conflict panel comes up on its own. The window opens on
# the screen while it runs; leave it be.
#
#   scripts/hero.sh path/to/ryucad [assets/hero.gif [path/to/ryucad-app [assets/compare.gif]]]
set -eu

R=$(cd "$(dirname "$1")" && pwd)/$(basename "$1")
GIF=${2:-assets/hero.gif}
GIF=$(cd "$(dirname "$GIF")" && pwd)/$(basename "$GIF")
APP=${3:-}
APPGIF=${4:-assets/compare.gif}
if [ -n "$APP" ]; then
  APP=$(cd "$(dirname "$APP")" && pwd)/$(basename "$APP")
  APPGIF=$(cd "$(dirname "$APPGIF")" && pwd)/$(basename "$APPGIF")
fi
work=$(mktemp -d)
# The folder names the window: "hub-demo — ryucad".
P=$work/hub-demo; OUT=$work/acts; APPS=$work/app
pid=
trap '[ -z "$pid" ] || { kill "$pid"; wait "$pid"; } 2>/dev/null; "$R" -C "$P" daemon stop >/dev/null 2>&1 || true; rm -rf "$work"' EXIT
mkdir -p "$P" "$OUT" "$APPS" "$work/config"
# Default preferences, for the app and its daemon: not the owner's own.
export RYUCAD_CONFIG_DIR=$work/config
cd "$P"

# Fixed identities and dates, so commits (and what Git prints) repeat.
export GIT_AUTHOR_DATE="2026-10-23T12:00:00Z" GIT_COMMITTER_DATE="2026-10-23T12:00:00Z"
export GIT_CONFIG_NOSYSTEM=1
git init -q -b main
git config user.name demo; git config user.email demo@example.com
git config advice.mergeConflict false 2>/dev/null || true

# Every edit goes through the project's session, so its activity log (and
# the app's) names who made it.
r() { out=$("$R" "$@") || { echo "ryucad $*: $out" >&2; exit 1; }; }
as() { who=$1; shift; out=$("$R" --as "$who" "$@") || { echo "ryucad $*: $out" >&2; exit 1; }; }

# An act is a folder: `title`, `lines` (a tag, a space, the text: `$` a
# command, `o` output, `+`/`-` added and removed lines, `!` a conflict, `#` a
# note) and the model frames shown while it plays.
act=0
A=
begin() { act=$((act+1)); A=$OUT/$(printf %02d $act); mkdir -p "$A"; printf '%s\n' "$1" > "$A/title"; : > "$A/lines"; frames=0; }
say() { printf '%s %s\n' "$1" "$2" >> "$A/lines"; }
# A command line, shown before its output and before the frame it led to.
cmd() { say '$' "$*"; }
output() { sed 's/^/o /' >> "$A/lines"; }
diffout() { sed -n '/^@@/,$p' | sed '1d' | sed -e 's/^+/+ +/' -e 's/^-/- -/' -e 's/^ /o  /' >> "$A/lines"; }
EYE="--eye 63,-77,66 --target 0,0,12 --up 0,0,1"
frame() {
  frames=$((frames+1))
  # Each frame is shown after the lines written so far.
  n=$(wc -l < "$A/lines" | tr -d ' ')
  r render --out "$A/$(printf %03d $frames)-$n.png" --size 480x420 --no-sketches "$@"
}
# The app's window, once it has settled (two pictures alike), with a caption.
# The status line shows a failed request for 8 s: `app --after-error` waits
# that out.
shot=0
window=
app() {
  [ -n "$APP" ] || return 0
  [ "$1" != --after-error ] || { shift; sleep 8; }
  shot=$((shot+1))
  last=$work/settle.png; : > "$last"
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
    window=$("${PYTHON:-python3}" -c '
import Quartz
for w in Quartz.CGWindowListCopyWindowInfo(Quartz.kCGWindowListOptionOnScreenOnly, 0):
    if w.get("kCGWindowOwnerPID") == '"$pid"' and w.get("kCGWindowLayer") == 0:
        print(w["kCGWindowNumber"]); break')
    [ -z "$window" ] || return 0
  done
  echo "the app's window never showed: $(cat "$work/app.log")" >&2; exit 1
}
# What the agent shows the person watching: a compare, or none.
compare() { "$R" -C "$P" compare "$@" >/dev/null; }
# The faces of bodies whose results differ from REV: changed or new.
changed() {
  "$R" --no-daemon diff --from "$1" --model --part hub | "${PYTHON:-python3}" -c '
import json, sys
d = json.load(sys.stdin)["result"]["parts"]["hub"]["features"]
for f, v in sorted(d.items()):
    b = v.get("model", {}).get("body")
    if b:
        for n in b["faces"]["changed"] + b["faces"]["new"]:
            print("--highlight=hub/%s/%s" % (f, n))'
}

begin "main"
"$R" init --unit mm >/dev/null
cmd 'ryucad init --unit mm'
r part create hub
cmd 'ryucad part create hub'
open_app
r param set hub "bore=12 mm" "flange_d=60 mm" "flange_t=6 mm" "hub_d=28 mm" "height=30 mm" "rib_t=4 mm"
cmd 'ryucad param set hub "bore=12 mm" "flange_d=60 mm" "rib_t=4 mm" ...'
r feature add hub sketch --id profile --plane zx
cmd 'ryucad feature add hub sketch --id profile --plane zx'
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
  {"op":"add","type":"line","start":"p7","end":"p8","construction":true},
  {"op":"add","type":"fix","a":"p7"},
  {"op":"add","type":"horizontal","a":"l7"},
  {"op":"add","type":"distance_x","a":"p7","b":"p8","value":"45 mm"},
  {"op":"add","type":"vertical","a":"l1"},
  {"op":"add","type":"horizontal","a":"l2"},
  {"op":"add","type":"vertical","a":"l3"},
  {"op":"add","type":"horizontal","a":"l4"},
  {"op":"add","type":"vertical","a":"l5"},
  {"op":"add","type":"horizontal","a":"l6"},
  {"op":"add","type":"distance_x","a":"p7","b":"p1","value":"5 mm"},
  {"op":"add","type":"distance_y","a":"p7","b":"p1","value":"bore / 2"},
  {"op":"add","type":"distance_y","a":"p7","b":"p2","value":"flange_d / 2"},
  {"op":"add","type":"distance_x","a":"p1","b":"p3","value":"flange_t"},
  {"op":"add","type":"distance_y","a":"p7","b":"p4","value":"hub_d / 2"},
  {"op":"add","type":"distance_x","a":"p1","b":"p5","value":"height"}]'
cmd 'ryucad sketch edit hub profile @profile.json'
r feature add hub revolve --id body --sketch profile --axis l7 --angle 45
cmd 'ryucad feature add hub revolve --id body --sketch profile --axis l7'
for a in 45 90 135 180 225 270 315; do
  r feature edit hub body --angle $a; frame $EYE
done
r feature edit hub body --angle full; frame $EYE
r feature add hub sketch --id ribs_a --plane zx
r sketch edit hub ribs_a '[
  {"op":"add","type":"point","at":[6,14]},{"op":"add","type":"point","at":[6,27]},{"op":"add","type":"point","at":[22,14]},
  {"op":"add","type":"line","start":"p1","end":"p2"},{"op":"add","type":"line","start":"p2","end":"p3"},{"op":"add","type":"line","start":"p3","end":"p1"},
  {"op":"add","type":"point","at":[6,-14]},{"op":"add","type":"point","at":[22,-14]},{"op":"add","type":"point","at":[6,-27]},
  {"op":"add","type":"line","start":"p4","end":"p5"},{"op":"add","type":"line","start":"p5","end":"p6"},{"op":"add","type":"line","start":"p6","end":"p4"}]'
r feature add hub extrude --id ribs_x --sketch ribs_a --length rib_t --extent symmetric
r feature add hub sketch --id ribs_b --plane yz
r sketch edit hub ribs_b '[
  {"op":"add","type":"point","at":[14,6]},{"op":"add","type":"point","at":[27,6]},{"op":"add","type":"point","at":[14,22]},
  {"op":"add","type":"line","start":"p1","end":"p2"},{"op":"add","type":"line","start":"p2","end":"p3"},{"op":"add","type":"line","start":"p3","end":"p1"},
  {"op":"add","type":"point","at":[-14,6]},{"op":"add","type":"point","at":[-14,22]},{"op":"add","type":"point","at":[-27,6]},
  {"op":"add","type":"line","start":"p4","end":"p5"},{"op":"add","type":"line","start":"p5","end":"p6"},{"op":"add","type":"line","start":"p6","end":"p4"}]'
r feature add hub extrude --id ribs_y --sketch ribs_b --length rib_t --extent symmetric
cmd 'ryucad feature add hub extrude --id ribs_x --sketch ribs_a --length rib_t ...'
git add -A; git commit -qm "hub"
cmd 'git add -A && git commit -m "hub"'
frame $EYE
app "main: the hub as an agent built it, committed, in the app a person watches"

begin "branch alice"
git switch -qc alice main
cmd 'git switch -c alice'
as alice param set hub "bore=16 mm" "rib_t=3 mm"
cmd 'ryucad param set hub "bore=16 mm" "rib_t=3 mm"'
cmd 'git diff parts/hub/parameters.json'
git -c color.ui=never diff -- parts/hub/parameters.json | diffout
git commit -qam "alice: wider bore, thinner ribs"
# shellcheck disable=SC2046
frame $EYE $(changed main)
compare main
app "alice's branch: the agent runs \`ryucad compare main\`; changed faces turn orange, main's bore is ghosted"

begin "branch bob"
git switch -qc bob main
cmd 'git switch -c bob main'
as bob param set hub "flange_d=72 mm" "rib_t=6 mm"
cmd 'ryucad param set hub "flange_d=72 mm" "rib_t=6 mm"'
cmd 'git diff parts/hub/parameters.json'
git -c color.ui=never diff -- parts/hub/parameters.json | diffout
git commit -qam "bob: bigger flange, thicker ribs"
# shellcheck disable=SC2046
frame $EYE $(changed main)
app "bob's branch, still compared with main: the bigger flange and thicker ribs in orange"

begin "merging bob into alice"
compare --off
git switch -q alice
cmd 'git switch alice && git merge bob'
git merge bob > "$work/merge.txt" 2>&1 && { echo "the merge should conflict" >&2; exit 1; }
sed -e 's/^CONFLICT/! CONFLICT/' -e '/^! /!s/^/o /' "$work/merge.txt" >> "$A/lines"
cmd 'ryucad merge status'
"$R" --no-daemon merge status | "${PYTHON:-python3}" -c '
import json, sys
s = json.load(sys.stdin)["result"]
print("#  (JSON, condensed)")
for c in s["conflicts"]:
    w = c["concerns"]
    print("! %s  %s/%s: base %s, ours %s, theirs %s" % (c["kind"], w["part"], w["parameter"], c["base"], c["ours"], c["theirs"]))
' >> "$A/lines"
# What merged by value: parameters that differ from the merge base and
# aren't in the conflict record.
git show "$(git merge-base HEAD MERGE_HEAD)":parts/hub/parameters.json > "$work/base.json"
"${PYTHON:-python3}" - "$work/base.json" parts/hub/parameters.json >> "$A/lines" <<'MERGED'
import json, sys
base, now = (json.load(open(p)) for p in sys.argv[1:])
done = [k for k, v in now["parameters"].items()
        if v != base["parameters"].get(k) and "/parameters/" + k not in now.get("conflicts", {})]
print("#  merged by value: " + ", ".join("%s = %s" % (k, now["parameters"][k]) for k in done))
MERGED
cmd "sed -n '/conflicts/,/^  }/p' parts/hub/parameters.json"
sed -n '/conflicts/,/^  }/p' parts/hub/parameters.json | output
app "git merge bob: bore and flange merged by value; rib thickness waits in the conflict panel"
# During the merge the part doesn't load; show ours, as alice left it.
cp "$OUT/02/001-$(wc -l < "$OUT/02/lines" | tr -d ' ').png" "$A/001-0.png"

begin "merged"
id=$("$R" --no-daemon merge status | "${PYTHON:-python3}" -c 'import json, sys; print(json.load(sys.stdin)["result"]["conflicts"][0]["id"])')
as alice merge resolve "$id" --theirs
cmd "ryucad merge resolve '$id' --theirs"
app --after-error "the agent settles it with \`ryucad merge resolve --theirs\`; the panel says what is left to do"
git add -A
cmd 'git add -A && git commit -m "merge bob"'
git commit -qm "merge bob" > "$work/commit.txt" 2>&1 || { cat "$work/commit.txt" >&2; exit 1; }
cmd 'ryucad check'
"$R" --no-daemon check | "${PYTHON:-python3}" -c '
import json, sys
c = json.load(sys.stdin)["result"]
fs = [f for p in c["parts"] for f in p["features"]]
print("o  passed: %s, %d of %d features built, %d findings" % (str(c["passed"]).lower(), sum(f["ok"] for f in fs), len(fs), len(c["findings"])))' >> "$A/lines"
cmd 'git log --oneline --graph'
git log --oneline --graph | output
# shellcheck disable=SC2046
frame $EYE $(changed main)
compare main
app --after-error "merged and committed, compared with main: both agents' changes in one model"
for k in $(seq 1 36); do
  t=$(awk "BEGIN { a = $k * 3.14159265358979 / 18; printf \"%.6f,%.6f\", 99.5 * cos(a - 0.8850), 99.5 * sin(a - 0.8850) }")
  frame --eye="$t,66" --target 0,0,12 --up 0,0,1
done

"${PYTHON:-python3}" - "$OUT" "$GIF" <<'PY'
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
COLOR = {"$": (120, 200, 255), "o": FG, "+": (126, 211, 126), "-": (240, 120, 120),
         "!": (245, 196, 92), "#": DIM}
CW = MONO.getbbox("M")[2]
COLS = (TW - 28) // CW
LH = 18

def terminal(title, lines):
    img = Image.new("RGB", (TW, H), BG)
    d = ImageDraw.Draw(img)
    d.text((14, 10), title, font=BOLD, fill=(245, 196, 92) if "merging" in title else (120, 200, 255))
    rows = []
    for tag, text in lines:
        lead = "$ " if tag == "$" else ""
        for i, part in enumerate(textwrap.wrap(lead + text, COLS, subsequent_indent="    ",
                                               drop_whitespace=False, break_on_hyphens=False) or [""]):
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
# command at a time; shots taken after no new line (the revolve turning, the
# orbit) play quickly; each act holds where its last line appears.
model = Image.new("RGB", (W, H), (244, 245, 247))
for act in sorted(pathlib.Path(sys.argv[1]).iterdir()):
    title = (act / "title").read_text().strip()
    lines = [tuple(raw.partition(" ")[::2]) for raw in (act / "lines").read_text().splitlines()]
    shown, hold = 0, None
    for shot in sorted(act.glob("*.png")):
        at = int(shot.stem.split("-")[1])
        while shown < at:
            shown += 1
            if lines[shown - 1][0] == "$" and shown < at:
                show(model, terminal(title, lines[:shown]), 450)
        model = Image.open(shot).convert("RGB")
        show(model, terminal(title, lines[:shown]), 140 if hold is None else 90)
        if hold is None and shown == len(lines):
            hold = len(frames) - 1
    while shown < len(lines):
        shown += 1
        if lines[shown - 1][0] == "$" or shown == len(lines):
            show(model, terminal(title, lines[:shown]), 450)
    durations[hold if hold is not None else -1] = 3000

# One palette for every frame, from the held ones (all the terminal's
# colours) and the orbit's last.
held = [frames[i] for i, ms in enumerate(durations) if ms == 3000] + [frames[-1]]
sheet = Image.new("RGB", (W + TW, H * len(held)))
for i, f in enumerate(held):
    sheet.paste(f, (0, H * i))
# Text is too thin to win a place in the palette; give its colours one.
d = ImageDraw.Draw(sheet)
for i, c in enumerate(list(COLOR.values()) + [(245, 196, 92), (120, 200, 255)]):
    d.rectangle((i * 60, 0, i * 60 + 59, 59), fill=c)
pal = sheet.quantize(colors=192, method=Image.Quantize.MEDIANCUT)
q = [f.quantize(palette=pal, dither=Image.Dither.NONE) for f in frames]
q[0].save(sys.argv[2], save_all=True, append_images=q[1:], duration=durations, loop=0, optimize=True)
print("wrote %s (%d frames)" % (sys.argv[2], len(q)))
PY

[ -n "$APP" ] || exit 0
"${PYTHON:-python3}" - "$APPS" "$APPGIF" <<'PY'
import os, pathlib, sys
from PIL import Image, ImageDraw, ImageFont

def font(size):
    for f in [os.environ.get("FONT"), "/System/Library/Fonts/Menlo.ttc",
              "/usr/share/fonts/truetype/dejavu/DejaVuSansMono.ttf",
              "/usr/share/fonts/TTF/DejaVuSansMono.ttf", "C:/Windows/Fonts/consola.ttf"]:
        if f and os.path.exists(f):
            return ImageFont.truetype(f, size)
    sys.exit("no monospace font found: set FONT")

CAP = font(17)
shots = sorted(pathlib.Path(sys.argv[1]).glob("*.png"), key=lambda p: int(p.stem))
stills = []
for i, p in enumerate(shots):
    img = Image.open(p).convert("RGB")
    w, h = img.size
    f = Image.new("RGB", (w, h + 52), (30, 32, 36))
    f.paste(img, (0, 52))
    d = ImageDraw.Draw(f)
    d.text((16, 15), "%d/%d  " % (i + 1, len(shots)), font=CAP, fill=(128, 134, 144))
    d.text((16 + CAP.getbbox("%d/%d  " % (i + 1, len(shots)))[2], 15),
           p.with_suffix(".txt").read_text().strip(), font=CAP, fill=(245, 196, 92))
    stills.append(f)
# Each still held; plain cuts keep the file small.
frames, durations = stills, [4200] * len(stills)
sheet = Image.new("RGB", (stills[0].width, stills[0].height * len(stills)))
for i, f in enumerate(stills):
    sheet.paste(f, (0, stills[0].height * i))
pal = sheet.quantize(colors=224, method=Image.Quantize.MEDIANCUT)
q = [f.quantize(palette=pal, dither=Image.Dither.NONE) for f in frames]
q[0].save(sys.argv[2], save_all=True, append_images=q[1:], duration=durations, loop=0, optimize=True)
print("wrote %s (%d stills)" % (sys.argv[2], len(stills)))
PY
