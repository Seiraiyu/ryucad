#!/bin/sh
# Tests install-ryudfm.sh against a fake release on disk.
#
#   scripts/test-install-ryudfm.sh [path/to/ryudfm]   (default: target/release/ryudfm)
#
# The fake release holds the given binary for every Unix target under
# ryudfm-v0.0.9, ryudfm-v0.1.0 and a pre-release, ryudfm-v0.2.0-pre.1, and
# lists them as GitHub's API does, newest first, after a newer ryucad
# release (v0.9.0, which is GitHub's "latest") and a newer ryueda one
# (ryueda-v0.3.0); the installer reaches it through RYUDFM_DOWNLOAD_BASE.
# The checks are strings evaluated later, so their $ must not expand early,
# and $status is read only inside them.
# shellcheck disable=SC2016,SC2034
set -eu

root=$(cd "$(dirname "$0")/.." && pwd)
binary=$(cd "$(dirname "${1:-target/release/ryudfm}")" && pwd)/$(basename "${1:-target/release/ryudfm}")
[ -x "$binary" ] || {
    echo "no binary at $binary; run cargo build --release -p ryudfm first" >&2
    exit 1
}

work=$(mktemp -d)
trap 'rm -rf "$work"' EXIT
failures=0
status=0

# entry TAG PRERELEASE: one release as the API lists it, one key a line.
entry() {
    cat <<EOF
  {
    "url": "https://api.github.com/repos/Seiraiyu/ryucad/releases/1",
    "author": {
      "login": "someone"
    },
    "tag_name": "$1",
    "name": "$1",
    "draft": false,
    "prerelease": $2,
    "assets": [
      {
        "name": "SHA256SUMS"
      }
    ]
  },
EOF
}

# release DIR: the fake release.
release() {
    for v in 0.0.9 0.1.0 0.2.0-pre.1; do
        d="$1/download/ryudfm-v$v"
        mkdir -p "$d"
        for t in x86_64-unknown-linux-musl aarch64-unknown-linux-musl aarch64-apple-darwin; do
            s="$work/stage/ryudfm-$v-$t"
            mkdir -p "$s"
            cp "$binary" "$s/ryudfm"
            tar -czf "$d/ryudfm-$v-$t.tar.gz" -C "$work/stage" "ryudfm-$v-$t"
        done
        if command -v sha256sum >/dev/null 2>&1; then
            (cd "$d" && sha256sum -- *.tar.gz >SHA256SUMS)
        else
            (cd "$d" && shasum -a 256 -- *.tar.gz >SHA256SUMS)
        fi
    done
    {
        echo "["
        entry v0.9.0 false
        entry ryueda-v0.3.0 false
        entry ryudfm-v0.2.0-pre.1 true
        entry ryudfm-v0.1.0 false
        entry ryudfm-v0.0.9 false
        echo "  {}"
        echo "]"
    } >"$1/releases"
}

good="$work/good"
release "$good"
bad="$work/bad"
release "$bad"
# A tampered download: the archives no longer match their checksums.
for f in "$bad"/download/ryudfm-v0.1.0/*.tar.gz; do printf x >>"$f"; done
# A repository with no ryudfm release but pre-releases.
early="$work/early"
release "$early"
{
    echo "["
    entry v0.9.0 false
    entry ryueda-v0.3.0 false
    entry ryudfm-v0.2.0-pre.1 true
    echo "  {}"
    echo "]"
} >"$early/releases"

# run NAME BASE [VAR=VALUE...]: run the installer in a fresh HOME (kept
# across runs of the same NAME), capturing output and status.
run() {
    name=$1
    base=$2
    shift 2
    home="$work/home-$name"
    mkdir -p "$home"
    set +e
    env -i HOME="$home" PATH="${TEST_PATH:-/usr/bin:/bin}" RYUDFM_DOWNLOAD_BASE="file://$base" "$@" \
        sh "$root/install-ryudfm.sh" >"$work/out" 2>&1
    status=$?
    set -e
}

check() {
    if eval "$2"; then
        echo "ok   $1"
    else
        echo "FAIL $1"
        sed 's/^/     | /' "$work/out"
        failures=$((failures + 1))
    fi
}

markers() { grep -c "added by the ryudfm installer" "$1" 2>/dev/null || true; }

# A fresh install, then a reinstall over it.
mkdir -p "$work/home-fresh" && : >"$work/home-fresh/.bashrc"
run fresh "$good"
check "fresh install succeeds" '[ "$status" -eq 0 ]'
check "binary installed" '[ -x "$work/home-fresh/.ryudfm/bin/ryudfm" ]'
check "the newest ryudfm release, not ryucad's latest, ryueda's or a pre-release" 'grep -q "installing ryudfm 0.1.0" "$work/out"'
check "reports the installed version" 'grep -q "\"ryudfm\":" "$work/out"'
check "a fresh install says it added the PATH line" 'grep -q "added .*/.ryudfm/bin to PATH in your shell profile" "$work/out"'
check ".profile gets the PATH line" '[ "$(markers "$work/home-fresh/.profile")" = 1 ]'
check ".bashrc gets the PATH line" '[ "$(markers "$work/home-fresh/.bashrc")" = 1 ]'
check ".bash_profile is not created" '[ ! -e "$work/home-fresh/.bash_profile" ]'
if command -v zsh >/dev/null 2>&1; then
    check ".zshenv gets the PATH line" '[ "$(markers "$work/home-fresh/.zshenv")" = 1 ]'
else
    check ".zshenv is not created without zsh" '[ ! -e "$work/home-fresh/.zshenv" ]'
fi
check "nothing of ryucad's or ryueda's is touched" '[ ! -e "$work/home-fresh/.ryucad" ] && [ ! -e "$work/home-fresh/.ryueda" ] && ! grep -q "ryucad installer\|ryueda installer" "$work/home-fresh/.profile"'
run fresh "$good"
check "reinstall succeeds" '[ "$status" -eq 0 ]'
check "reinstall adds no second PATH line" '[ "$(markers "$work/home-fresh/.profile")" = 1 ]'
check "a reinstall says the profile has the line already" 'grep -q "/.ryudfm/bin is in your shell profile but not this shell'"'"'s PATH" "$work/out" && ! grep -q "added .* to PATH" "$work/out"'

# Pinned versions, in each form a person might type.
for v in 0.0.9 v0.0.9 ryudfm-v0.0.9; do
    run "pinned-$v" "$good" RYUDFM_VERSION="$v"
    check "RYUDFM_VERSION=$v installs 0.0.9" '[ "$status" -eq 0 ] && grep -q "installing ryudfm 0.0.9" "$work/out"'
done
run prerelease "$good" RYUDFM_VERSION=ryudfm-v0.2.0-pre.1
check "a pre-release installs when named" '[ "$status" -eq 0 ] && grep -q "installing ryudfm 0.2.0-pre.1" "$work/out"'
run missing "$good" RYUDFM_VERSION=9.9.9
check "a missing version fails" '[ "$status" -ne 0 ] && grep -q "no ryudfm-9.9.9" "$work/out"'
run early "$early"
check "no release yet says to name a pre-release" '[ "$status" -ne 0 ] && grep -q "no ryudfm release yet" "$work/out"'

# A checksum mismatch installs nothing.
run corrupt "$bad"
check "checksum mismatch fails" '[ "$status" -ne 0 ] && grep -q "checksum mismatch" "$work/out"'
check "checksum mismatch installs nothing" '[ ! -e "$work/home-corrupt/.ryudfm/bin/ryudfm" ]'

# Options.
run custom "$good" RYUDFM_HOME="$work/elsewhere" RYUDFM_NO_MODIFY_PATH=1
check "RYUDFM_HOME is honoured" '[ -x "$work/elsewhere/bin/ryudfm" ]'
check "RYUDFM_NO_MODIFY_PATH leaves profiles alone" '[ ! -e "$work/home-custom/.profile" ]'
run onpath "$good" PATH="$work/home-onpath/.ryudfm/bin:/usr/bin:/bin"
check "no PATH edit when already on PATH" '[ "$status" -eq 0 ] && [ ! -e "$work/home-onpath/.profile" ]'

# An unsupported platform, through a fake uname.
mkdir -p "$work/fakebin"
printf '#!/bin/sh\ncase "$1" in -s) echo Plan9 ;; *) echo mips ;; esac\n' >"$work/fakebin/uname"
chmod +x "$work/fakebin/uname"
TEST_PATH="$work/fakebin:/usr/bin:/bin" run plan9 "$good"
check "unsupported platform fails" '[ "$status" -ne 0 ] && grep -q "no ryudfm build for Plan9 mips" "$work/out"'

if [ "$failures" -ne 0 ]; then
    echo "$failures installer test(s) failed"
    exit 1
fi
echo "all ryudfm installer tests passed"
