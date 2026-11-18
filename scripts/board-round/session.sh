#!/bin/sh
# Prepares one board round session: `sh session.sh <round dir> <project>
# <name> <ryueda>`. Copies the breakout's project (from schematic_corpus's
# round_projects) into <round dir>/<name>/project, puts the logging
# wrapper on a PATH of its own, and writes the session's brief and the
# environment to run it under.
set -eu
round=$1 project=$2 name=$3 real=$4
here=$(cd "$(dirname "$0")" && pwd)
dir=$round/$name
[ ! -e "$dir" ] || { echo "$dir exists" >&2; exit 1; }
mkdir -p "$dir/bin"
cp -R "$project" "$dir/project"
rm -f "$dir/project/baseline.json"
cp "$here/ryueda" "$dir/bin/ryueda"
chmod +x "$dir/bin/ryueda"
sed -e "s|{{PROJECT}}|$dir/project|g" -e "s|{{NOTES}}|$dir/notes.md|g" \
    "$here/brief.md" > "$dir/brief.txt"
cat > "$dir/env.sh" <<ENV
export PATH="$dir/bin:\$PATH"
export ROUND_RYUEDA="$real"
export ROUND_LOG="$dir/calls.jsonl"
export RYUEDA_CONFIG_DIR="$dir/config"
export RYUEDA_RUNTIME_DIR="$dir/run"
export RYUEDA_PARTICIPANT=round
ENV
echo "$dir"
