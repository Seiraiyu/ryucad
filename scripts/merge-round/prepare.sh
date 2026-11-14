#!/bin/sh
# Prepares the merge round: `sh prepare.sh <round dir> <projects dir>
# <circuit> <ryueda>`. <projects dir> is what `round_projects` wrote
# (schematic_corpus.rs). Makes <round dir>/repo (the circuit committed on
# main with ryueda's driver and hook), worktrees a and b on branches a
# and b, and for each of a, b and merge a bin/ with the logging wrapper,
# an env.sh and a brief.txt; prints the three folders.
set -eu
round=$1 projects=$2 circuit=$3 real=$4
here=$(cd "$(dirname "$0")" && pwd)
[ ! -e "$round" ] || { echo "$round exists" >&2; exit 1; }
mkdir -p "$round"
git_() {
  GIT_CONFIG_GLOBAL="$round/gitconfig" GIT_CONFIG_NOSYSTEM=1 \
  GIT_AUTHOR_NAME=Round GIT_AUTHOR_EMAIL=round@example.com \
  GIT_COMMITTER_NAME=Round GIT_COMMITTER_EMAIL=round@example.com \
  git -c init.defaultBranch=main -c commit.gpgsign=false "$@"
}
git_ init -q "$round/repo"
cp -R "$projects/$circuit" "$round/repo/$circuit"
rm -f "$round/repo/$circuit/baseline.json"
( cd "$round/repo/$circuit" && RYUEDA_NO_DAEMON=1 RYUEDA_CONFIG_DIR="$round/config" \
    GIT_CONFIG_GLOBAL="$round/gitconfig" "$real" git install >/dev/null )
( cd "$round/repo" && git_ add -A && git_ commit -q -m "$circuit" )
for b in a b; do
  ( cd "$round/repo" && git_ worktree add -q -b "$b" "$round/$b" main )
done
for who in a b merge; do
  dir=$round/session-$who
  mkdir -p "$dir/bin"
  cp "$here/ryueda" "$dir/bin/ryueda"
  case $who in
    merge) role=merge brief=brief-merge.md project=$round/repo/$circuit ;;
    *) role=branch brief=brief-branch.md project=$round/$who/$circuit ;;
  esac
  sed -e "s|{{PROJECT}}|$project|" -e "s|{{NOTES}}|$dir/notes.md|" \
      -e "s|{{BRANCH}}|$who|" -e "s|{{REPO}}|$round/repo|" "$here/$brief" > "$dir/brief.txt"
  cat > "$dir/env.sh" <<ENV
export PATH="$dir/bin:\$PATH"
export ROUND_RYUEDA="$real"
export ROUND_LOG="$dir/calls.jsonl"
export ROUND_ROLE=$role
export RYUEDA_CONFIG_DIR="$round/config"
export RYUEDA_RUNTIME_DIR="$dir/run"
export RYUEDA_PARTICIPANT=round
export GIT_CONFIG_GLOBAL="$round/gitconfig"
export GIT_AUTHOR_NAME=Round GIT_AUTHOR_EMAIL=round@example.com
export GIT_COMMITTER_NAME=Round GIT_COMMITTER_EMAIL=round@example.com
ENV
  echo "$dir"
done
