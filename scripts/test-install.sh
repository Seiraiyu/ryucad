#!/bin/sh
# Tests install.sh against a fake release on disk.
#
#   scripts/test-install.sh [path/to/ryucad]   (default: target/release/ryucad)
#
# The fake release holds the given binary for every Unix target under two
# versions, with a stand-in for the desktop app and the files that register
# it, and the installer reaches it through RYUCAD_DOWNLOAD_BASE.
# The checks are strings evaluated later, so their $ must not expand early,
# and $status is read only inside them.
# shellcheck disable=SC2016,SC2034
set -eu

root=$(cd "$(dirname "$0")/.." && pwd)
binary=$(cd "$(dirname "${1:-target/release/ryucad}")" && pwd)/$(basename "${1:-target/release/ryucad}")
[ -x "$binary" ] || {
    echo "no binary at $binary; run cargo build --release -p ryucad first" >&2
    exit 1
}

work=$(mktemp -d)
trap 'rm -rf "$work"' EXIT
failures=0
status=0

# release DIR: a fake release with v0.0.9 and v0.1.0 (latest).
release() {
    for v in 0.0.9 0.1.0; do
        d="$1/download/v$v"
        mkdir -p "$d"
        for t in x86_64-unknown-linux-musl aarch64-unknown-linux-musl aarch64-apple-darwin; do
            s="$work/stage/ryucad-$v-$t"
            mkdir -p "$s/share"
            cp "$binary" "$s/ryucad"
            printf '#!/bin/sh\necho "{\\"ryucad\\":\\"%s\\"}"\n' "$v" >"$s/ryucad-app"
            chmod 755 "$s/ryucad-app"
            cp "$root/packaging/linux/ryucad.desktop" "$root/packaging/linux/ryucad.xml" \
                "$root/packaging/macos/ryucad.applescript" "$s/share/"
            tar -czf "$d/ryucad-$v-$t.tar.gz" -C "$work/stage" "ryucad-$v-$t"
        done
        if command -v sha256sum >/dev/null 2>&1; then
            (cd "$d" && sha256sum -- *.tar.gz >SHA256SUMS)
        else
            (cd "$d" && shasum -a 256 -- *.tar.gz >SHA256SUMS)
        fi
    done
    echo v0.1.0 >"$1/latest"
}

good="$work/good"
release "$good"
bad="$work/bad"
release "$bad"
# A tampered download: the archives no longer match their checksums.
for f in "$bad"/download/v0.1.0/*.tar.gz; do printf x >>"$f"; done

# run NAME BASE [VAR=VALUE...]: run the installer in a fresh HOME (kept
# across runs of the same NAME), capturing output and status.
run() {
    name=$1
    base=$2
    shift 2
    home="$work/home-$name"
    mkdir -p "$home"
    set +e
    env -i HOME="$home" PATH="${TEST_PATH:-/usr/bin:/bin}" RYUCAD_DOWNLOAD_BASE="file://$base" "$@" \
        sh "$root/install.sh" >"$work/out" 2>&1
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

markers() { grep -c "added by the ryucad installer" "$1" 2>/dev/null || true; }

# A fresh install, then a reinstall over it.
mkdir -p "$work/home-fresh" && : >"$work/home-fresh/.bashrc"
run fresh "$good"
check "fresh install succeeds" '[ "$status" -eq 0 ]'
check "binary installed" '[ -x "$work/home-fresh/.ryucad/bin/ryucad" ]'
check "reports the installed version" 'grep -q "\"ryucad\":" "$work/out"'
check ".profile gets the PATH line" '[ "$(markers "$work/home-fresh/.profile")" = 1 ]'
check ".bashrc gets the PATH line" '[ "$(markers "$work/home-fresh/.bashrc")" = 1 ]'
check ".bash_profile is not created" '[ ! -e "$work/home-fresh/.bash_profile" ]'
if command -v zsh >/dev/null 2>&1; then
    check ".zshenv gets the PATH line" '[ "$(markers "$work/home-fresh/.zshenv")" = 1 ]'
else
    check ".zshenv is not created without zsh" '[ ! -e "$work/home-fresh/.zshenv" ]'
fi
run fresh "$good"
check "reinstall succeeds" '[ "$status" -eq 0 ]'
check "reinstall adds no second PATH line" '[ "$(markers "$work/home-fresh/.profile")" = 1 ]'

# The desktop app and what lets the desktop open projects in it.
check "app installed" '[ -x "$work/home-fresh/.ryucad/bin/ryucad-app" ]'
desktop="$work/home-fresh/.local/share/applications/ryucad.desktop"
case "$(uname -s)" in
    Linux)
        # Unquoted when the path needs no quoting: xdg-open's generic mode
        # can't run a quoted program.
        check "desktop entry runs the installed app" 'grep -qx "Exec=$work/home-fresh/.ryucad/bin/ryucad-app %f" "$desktop"'
        if command -v desktop-file-validate >/dev/null 2>&1; then
            check "desktop entry is valid" 'desktop-file-validate "$desktop"'
        fi
        check "project MIME type registered" '[ -f "$work/home-fresh/.local/share/mime/packages/ryucad.xml" ]'
        ;;
    Darwin)
        if command -v osacompile >/dev/null 2>&1; then
            check "launcher in ~/Applications" '[ -f "$work/home-fresh/Applications/ryucad.app/Contents/Info.plist" ]'
        fi
        ;;
esac
if [ "$(uname -s)" = Linux ]; then
    run "with space" "$good"
    spaced="$work/home-with space/.local/share/applications/ryucad.desktop"
    check "a path with a space is quoted" 'grep -qx "Exec=\"$work/home-with space/.ryucad/bin/ryucad-app\" %f" "$spaced"'
    if command -v desktop-file-validate >/dev/null 2>&1; then
        check "the quoted entry is valid" 'desktop-file-validate "$spaced"'
    fi
fi
run noapp "$good" RYUCAD_NO_APP=1
check "RYUCAD_NO_APP installs the command only" '[ "$status" -eq 0 ] && [ -x "$work/home-noapp/.ryucad/bin/ryucad" ] && [ ! -e "$work/home-noapp/.ryucad/bin/ryucad-app" ]'
check "RYUCAD_NO_APP registers nothing" '[ ! -e "$work/home-noapp/.local/share/applications" ] && [ ! -e "$work/home-noapp/Applications" ]'

# Pinned versions.
run pinned "$good" RYUCAD_VERSION=0.0.9
check "a pinned version installs" '[ "$status" -eq 0 ] && grep -q "installing ryucad 0.0.9" "$work/out"'
run missing "$good" RYUCAD_VERSION=v9.9.9
check "a missing version fails" '[ "$status" -ne 0 ] && grep -q "no ryucad-9.9.9" "$work/out"'

# A checksum mismatch installs nothing.
run corrupt "$bad"
check "checksum mismatch fails" '[ "$status" -ne 0 ] && grep -q "checksum mismatch" "$work/out"'
check "checksum mismatch installs nothing" '[ ! -e "$work/home-corrupt/.ryucad/bin/ryucad" ]'

# Options.
run custom "$good" RYUCAD_HOME="$work/elsewhere" RYUCAD_NO_MODIFY_PATH=1
check "RYUCAD_HOME is honoured" '[ -x "$work/elsewhere/bin/ryucad" ]'
check "RYUCAD_NO_MODIFY_PATH leaves profiles alone" '[ ! -e "$work/home-custom/.profile" ]'
run onpath "$good" PATH="$work/home-onpath/.ryucad/bin:/usr/bin:/bin"
check "no PATH edit when already on PATH" '[ "$status" -eq 0 ] && [ ! -e "$work/home-onpath/.profile" ]'

# An unsupported platform, through a fake uname.
mkdir -p "$work/fakebin"
printf '#!/bin/sh\ncase "$1" in -s) echo Plan9 ;; *) echo mips ;; esac\n' >"$work/fakebin/uname"
chmod +x "$work/fakebin/uname"
TEST_PATH="$work/fakebin:/usr/bin:/bin" run plan9 "$good"
check "unsupported platform fails" '[ "$status" -ne 0 ] && grep -q "no ryucad build for Plan9 mips" "$work/out"'

if [ "$failures" -ne 0 ]; then
    echo "$failures installer test(s) failed"
    exit 1
fi
echo "all installer tests passed"
