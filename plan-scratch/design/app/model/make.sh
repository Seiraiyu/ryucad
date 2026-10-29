#!/bin/sh
# Makes the bracket the mockups' viewport shows, through the CLI as an agent
# would (a sketch, an extrude, four counterbored holes, the four corners
# filleted), and renders it iso at the viewport's size and the start
# screen's thumbnail size: viewport.png and thumb.png beside this script.
#
#   sh design/app/model/make.sh path/to/ryucad
set -eu
exe=$(cd "$(dirname "$1")" && pwd)/$(basename "$1")
here=$(cd "$(dirname "$0")" && pwd)
work=$(mktemp -d)
trap 'rm -rf "$work"' EXIT
mkdir "$work/bracket"
cd "$work/bracket"
export RYUCAD_RUNTIME_DIR="$work/run" RYUCAD_CONFIG_DIR="$work/config" RYUCAD_CACHE_DIR="$work/cache" RYUCAD_NO_DAEMON=1
r() { "$exe" "$@" >"$work/out.json" || { cat "$work/out.json"; exit 1; }; }

r init --unit mm
r part create bracket
r param set bracket "thickness=8 mm" "hole=6.6 mm"
r feature add bracket sketch --id s1
r sketch edit bracket s1 '[
  {"op": "add", "type": "point", "at": [0, 0]},
  {"op": "add", "type": "point", "at": [80, 0]},
  {"op": "add", "type": "point", "at": [80, 50]},
  {"op": "add", "type": "point", "at": [0, 50]},
  {"op": "add", "type": "line", "start": "p1", "end": "p2"},
  {"op": "add", "type": "line", "start": "p2", "end": "p3"},
  {"op": "add", "type": "line", "start": "p3", "end": "p4"},
  {"op": "add", "type": "line", "start": "p4", "end": "p1"}]'
r feature add bracket extrude --id e1 --sketch s1 --length thickness
r feature add bracket sketch --id s2 --origin 0,0,8
r sketch edit bracket s2 '[
  {"op": "add", "type": "point", "at": [12, 12]}, {"op": "add", "type": "fix", "a": "p1"},
  {"op": "add", "type": "point", "at": [68, 12]}, {"op": "add", "type": "fix", "a": "p2"},
  {"op": "add", "type": "point", "at": [68, 38]}, {"op": "add", "type": "fix", "a": "p3"},
  {"op": "add", "type": "point", "at": [12, 38]}, {"op": "add", "type": "fix", "a": "p4"}]'
r feature add bracket hole --id h1 --sketch s2 --form counterbore --diameter hole \
  --counterbore-diameter 11 --counterbore-depth 3 --depth through_all
# The four vertical edges: both ends at one (x, y), z from 0 to 8 mm.
r inspect bracket e1
edges=$(jq -r '.result.bodies[0] as $b
  | ($b.vertices | map({key: .name, value: .point}) | from_entries) as $v
  | $b.edges[]
  | select(($v[.start][0] == $v[.end][0]) and ($v[.start][1] == $v[.end][1]))
  | "--edge\nbracket/e1/" + .name' "$work/out.json")
# shellcheck disable=SC2086
r feature add bracket fillet --id f1 --radius 6 $edges
r rebuild bracket
jq -e '.result.parts[0].features | all(.error == null)' "$work/out.json" >/dev/null
r render bracket --view iso --size 2080x1456 --no-sketches --out "$here/viewport.png"
r render bracket --view iso --size 512x300 --no-sketches --out "$here/thumb.png"
echo "wrote $here/viewport.png and $here/thumb.png"
