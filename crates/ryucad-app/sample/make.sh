#!/bin/sh
# Makes the sample project Open sample… writes (design 199, Start screen),
# through the CLI as an agent would: a bracket (parameters, a sketch, an
# extrude, four counterbored holes, its corners filleted), a pin, and an
# assembly placing the bracket fixed with the pin in a hole by a
# cylindrical mate. Its files go to project/ beside this script, less the
# manifest and .gitattributes, which `ryucad init` writes where the sample
# is opened. The app embeds them (`core::sample`); run this again after a
# change that rewrites them, and commit the result.
#
#   sh crates/ryucad-app/sample/make.sh path/to/ryucad
set -eu
exe=$(cd "$(dirname "$1")" && pwd)/$(basename "$1")
here=$(cd "$(dirname "$0")" && pwd)
work=$(mktemp -d)
trap 'rm -rf "$work"' EXIT
mkdir "$work/sample"
cd "$work/sample"
export RYUCAD_RUNTIME_DIR="$work/run" RYUCAD_CONFIG_DIR="$work/config" RYUCAD_CACHE_DIR="$work/cache" RYUCAD_NO_DAEMON=1
r() { "$exe" "$@" >"$work/out.json" || { cat "$work/out.json"; exit 1; }; }

r init --unit mm
r part create bracket
r param set bracket "thickness=8 mm" "hole=6.6 mm" "corner=6 mm"
r feature add bracket sketch --id s1
r sketch edit bracket s1 '[
  {"op": "add", "type": "point", "at": [0, 0]},
  {"op": "add", "type": "point", "at": [80, 0]},
  {"op": "add", "type": "point", "at": [80, 50]},
  {"op": "add", "type": "point", "at": [0, 50]},
  {"op": "add", "type": "line", "start": "p1", "end": "p2"},
  {"op": "add", "type": "line", "start": "p2", "end": "p3"},
  {"op": "add", "type": "line", "start": "p3", "end": "p4"},
  {"op": "add", "type": "line", "start": "p4", "end": "p1"},
  {"op": "add", "type": "horizontal", "a": "l1"},
  {"op": "add", "type": "horizontal", "a": "l3"},
  {"op": "add", "type": "vertical", "a": "l2"},
  {"op": "add", "type": "vertical", "a": "l4"},
  {"op": "add", "type": "fix", "a": "p1"},
  {"op": "add", "type": "distance", "a": "p1", "b": "p2", "value": "80 mm"},
  {"op": "add", "type": "distance", "a": "p2", "b": "p3", "value": "50 mm"}]'
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
r feature add bracket fillet --id f1 --radius corner $edges

r part create pin
r param set pin "diameter=6 mm" "length=20 mm"
r feature add pin cylinder --id shank --param "radius=diameter / 2" --param "height=length"

r part create rig --assembly
r feature add rig instance --id plate --of bracket --fixed
r feature add rig instance --id pin --of pin --origin 12,12,-4
# The first hole's wall in the bracket as built (a cylinder about
# x = 12 mm, y = 12 mm of the hole's diameter) and the pin's side.
r inspect bracket
wall=$(jq -r '[.result.bodies[0].faces[]
  | select(.surface.type == "cylinder"
      and ((.surface.radius - 0.0033) | fabs) < 1e-9
      and ((.surface.frame.origin[0] - 0.012) | fabs) < 1e-9
      and ((.surface.frame.origin[1] - 0.012) | fabs) < 1e-9)][0].name' "$work/out.json")
r inspect pin shank
side=$(jq -r '[.result.bodies[0].faces[] | select(.surface.type == "cylinder")][0].name' "$work/out.json")
r feature add rig mate --id m1 --form cylindrical --a "rig/plate/e1/$wall" --b "rig/pin/shank/$side" \
  --b-offset '{"flip": true}'

r rebuild rig
jq -e '.result.parts | all(.features | all(.error == null))' "$work/out.json" >/dev/null || { cat "$work/out.json"; exit 1; }
r check
rm -rf "$here/project"
mkdir "$here/project"
cp -R parts "$here/project/parts"
echo "wrote $here/project"
