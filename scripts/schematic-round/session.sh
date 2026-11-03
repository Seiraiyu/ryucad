#!/bin/sh
# Prepares one round session: `sh session.sh <round dir> <projects dir>
# <circuit> <lasting|moves> <png|describe|both> <ryueda>`. Copies the circuit's
# project into <round dir>/<circuit>-<arm>-<see>/project, puts the
# wrapper on a PATH of its own, and prints the session's brief with the
# environment to run it under.
set -eu
round=$1 projects=$2 circuit=$3 arm=$4 see=$5 real=$6
here=$(cd "$(dirname "$0")" && pwd)
dir=$round/$circuit-$arm-$see
[ ! -e "$dir" ] || { echo "$dir exists" >&2; exit 1; }
mkdir -p "$dir/bin"
cp -R "$projects/$circuit" "$dir/project"
cp "$here/ryueda" "$dir/bin/ryueda"
case $arm in
  lasting) arm_text='give lasting instructions: `ryueda place <block> "<instruction>"...`; they are kept and hold on every later layout until you release them. `--move` is not yours.' ;;
  moves) arm_text='give one-off moves: `ryueda place <block> --move "<instruction>"`; each is applied once and its result held where it puts it. Lasting instructions are not yours.' ;;
  *) echo "arm: lasting or moves" >&2; exit 2 ;;
esac
case $see in
  png) see_text='render sheets to PNG (`ryueda render --block <block> --format png --fit --scale 8 --out <file>.png`) and look at them; `ryueda describe` is yours too.' ;;
  describe) see_text='read `ryueda describe` (JSON, each sheet in relative terms with its lint and score). You cannot render or look at pictures.' ;;
  both) see_text='read `ryueda describe` (JSON, each sheet in relative terms with its lint and score), and render sheets to PNG (`ryueda render --block <block> --format png --fit --scale 8 --out <file>.png`, or `place ... --dry-run --render <file>.png` to see a change before making it) and look at them.' ;;
  *) echo "see: png, describe or both" >&2; exit 2 ;;
esac
sed -e "s|{{ARM_TEXT}}|$arm_text|" -e "s|{{SEE_TEXT}}|$see_text|" \
    -e "s|{{PROJECT}}|$dir/project|" -e "s|{{NOTES}}|$dir/notes.md|" \
    "$here/brief.md" > "$dir/brief.txt"
cat > "$dir/env.sh" <<ENV
export PATH="$dir/bin:\$PATH"
export ROUND_RYUEDA="$real"
export ROUND_LOG="$dir/calls.jsonl"
export ROUND_ARM=$arm
export ROUND_SEE=$see
export RYUEDA_CONFIG_DIR="$dir/config"
export RYUEDA_RUNTIME_DIR="$dir/run"
export RYUEDA_PARTICIPANT=round
ENV
echo "$dir"
