#!/bin/sh
# Install ryucad from a GitHub release: Linux, macOS and WSL.
#
#   curl -fsSL https://github.com/Seiraiyu/ryucad/releases/latest/download/install.sh | sh
#
# Environment:
#   RYUCAD_VERSION          tag to install (default: the latest release)
#   RYUCAD_HOME             install root (default: ~/.ryucad); the binary goes in bin/
#   RYUCAD_NO_MODIFY_PATH   set to 1 to leave shell profiles alone
#   GH_TOKEN, GITHUB_TOKEN  a token for a private repository (else `gh auth token`)
#   RYUCAD_DOWNLOAD_BASE    a mirror laid out as <base>/latest and
#                           <base>/download/<tag>/<asset>; used by tests
set -eu

REPO="Seiraiyu/ryucad"
API="https://api.github.com/repos/$REPO"
MARKER="# added by the ryucad installer"

say() { printf 'ryucad-install: %s\n' "$*" >&2; }
die() {
    say "error: $*"
    exit 1
}

# The release target for this machine, or exit with the supported list.
detect_target() {
    os=$(uname -s)
    arch=$(uname -m)
    case "$arch" in
        x86_64 | amd64) arch=x86_64 ;;
        aarch64 | arm64) arch=aarch64 ;;
    esac
    # An x86_64 shell under Rosetta on Apple silicon still wants arm64.
    if [ "$os" = Darwin ] && [ "$arch" = x86_64 ] &&
        [ "$(sysctl -n sysctl.proc_translated 2>/dev/null || echo 0)" = 1 ]; then
        arch=aarch64
    fi
    case "$os-$arch" in
        Linux-x86_64) echo x86_64-unknown-linux-musl ;;
        Linux-aarch64) echo aarch64-unknown-linux-musl ;;
        Darwin-aarch64) echo aarch64-apple-darwin ;;
        *) die "no ryucad build for $os $arch. Supported: Linux x86_64 and aarch64, macOS arm64 (and Windows x64 through install.ps1)" ;;
    esac
}

# A token for the GitHub API, or nothing for anonymous access.
find_token() {
    if [ -n "${GH_TOKEN:-}" ]; then
        echo "$GH_TOKEN"
    elif [ -n "${GITHUB_TOKEN:-}" ]; then
        echo "$GITHUB_TOKEN"
    elif command -v gh >/dev/null 2>&1; then
        gh auth token 2>/dev/null || true
    fi
}

# fetch URL OUT [HEADER...]: download URL to OUT (`-` for stdout).
fetch() {
    url=$1
    out=$2
    shift 2
    if command -v curl >/dev/null 2>&1; then
        # The remaining arguments are headers.
        n=$#
        for h; do set -- "$@" -H "$h"; done
        shift "$n"
        curl -fsSL --proto '=https,file' "$@" -o "$out" "$url"
    elif command -v wget >/dev/null 2>&1; then
        [ "$#" -eq 0 ] || die "downloading with a token needs curl"
        wget -q -O "$out" "$url"
    else
        die "need curl or wget"
    fi
}

# GitHub answers 404 both for a missing release and a private repository.
private_hint() {
    if [ -n "${auth:-}" ]; then
        die "could not reach $1: no such release, or the token cannot read $REPO"
    fi
    die "could not reach $1. If the repository is private, set GH_TOKEN or run 'gh auth login' first"
}

main() {
    target=$(detect_target)
    token=$(find_token)
    base=${RYUCAD_DOWNLOAD_BASE:-}
    auth=""
    [ -n "$token" ] && auth="Authorization: Bearer $token"

    tmp=$(mktemp -d)
    trap 'rm -rf "$tmp"' EXIT

    # Which version.
    tag=${RYUCAD_VERSION:-}
    if [ -z "$tag" ]; then
        if [ -n "$base" ]; then
            fetch "$base/latest" "$tmp/latest" || die "no latest version at $base"
            tag=$(tr -d ' \r\n' <"$tmp/latest")
        else
            if [ -n "$auth" ]; then
                fetch "$API/releases/latest" "$tmp/latest.json" "$auth" || private_hint "$API"
            else
                fetch "$API/releases/latest" "$tmp/latest.json" || private_hint "$API"
            fi
            tag=$(sed -n 's/.*"tag_name": *"\([^"]*\)".*/\1/p' "$tmp/latest.json" | head -n 1)
        fi
    fi
    case "$tag" in v*) ;; *) tag="v$tag" ;; esac
    [ "$tag" != v ] || die "could not work out the latest version"
    version=${tag#v}
    asset="ryucad-$version-$target.tar.gz"
    say "installing ryucad $version for $target"

    # Download the archive and the checksums.
    if [ -n "$base" ]; then
        fetch "$base/download/$tag/$asset" "$tmp/$asset" || die "no $asset in release $tag"
        fetch "$base/download/$tag/SHA256SUMS" "$tmp/SHA256SUMS" || die "no SHA256SUMS in release $tag"
    elif [ -n "$auth" ]; then
        # The API's asset endpoint works for private and public repositories.
        fetch "$API/releases/tags/$tag" "$tmp/release.json" "$auth" || private_hint "release $tag"
        for name in "$asset" SHA256SUMS; do
            url=$(awk -v want="$name" '
                /"url": *"[^"]*\/releases\/assets\/[0-9]+"/ { u = $0; sub(/.*"url": *"/, "", u); sub(/".*/, "", u) }
                /"name": *"/ { n = $0; sub(/.*"name": *"/, "", n); sub(/".*/, "", n); if (n == want && u != "") { print u; exit } }
            ' "$tmp/release.json")
            [ -n "$url" ] || die "no $name in release $tag"
            fetch "$url" "$tmp/$name" "$auth" "Accept: application/octet-stream" || die "could not download $name"
        done
    else
        dl="https://github.com/$REPO/releases/download/$tag"
        fetch "$dl/$asset" "$tmp/$asset" || private_hint "$dl/$asset"
        fetch "$dl/SHA256SUMS" "$tmp/SHA256SUMS" || die "no SHA256SUMS in release $tag"
    fi

    # Verify before anything is installed.
    want=$(awk -v f="$asset" '$2 == f || $2 == "*" f { print $1 }' "$tmp/SHA256SUMS")
    [ -n "$want" ] || die "$asset is not listed in SHA256SUMS"
    if command -v sha256sum >/dev/null 2>&1; then
        got=$(sha256sum "$tmp/$asset" | awk '{ print $1 }')
    else
        got=$(shasum -a 256 "$tmp/$asset" | awk '{ print $1 }')
    fi
    [ "$got" = "$want" ] || die "checksum mismatch for $asset: expected $want, got $got"

    # Install: a rename in the same directory replaces a running binary atomically.
    tar -xzf "$tmp/$asset" -C "$tmp"
    home=${RYUCAD_HOME:-$HOME/.ryucad}
    bin="$home/bin"
    mkdir -p "$bin"
    cp "$tmp/ryucad-$version-$target/ryucad" "$bin/.ryucad.new"
    chmod 755 "$bin/.ryucad.new"
    mv -f "$bin/.ryucad.new" "$bin/ryucad"
    # Daemons from the old version must not keep running.
    "$bin/ryucad" daemon stop --all >/dev/null 2>&1 || true

    # PATH.
    case ":$PATH:" in
        *":$bin:"*) ;;
        *)
            if [ "${RYUCAD_NO_MODIFY_PATH:-0}" = 1 ]; then
                say "add $bin to your PATH"
            else
                line="export PATH=\"$bin:\$PATH\""
                # .profile always; bash's own files when they exist (a login
                # bash skips .profile if .bash_profile exists); zsh reads
                # .zshenv in every shell, so it is created when zsh is installed.
                rcs="$HOME/.profile"
                for rc in "$HOME/.bash_profile" "$HOME/.bashrc"; do
                    [ -f "$rc" ] && rcs="$rcs $rc"
                done
                command -v zsh >/dev/null 2>&1 && rcs="$rcs ${ZDOTDIR:-$HOME}/.zshenv"
                for rc in $rcs; do
                    grep -qF "$MARKER" "$rc" 2>/dev/null || printf '\n%s\n%s\n' "$MARKER" "$line" >>"$rc"
                done
                fish="$HOME/.config/fish/conf.d"
                if [ -d "$HOME/.config/fish" ]; then
                    mkdir -p "$fish"
                    printf '%s\nfish_add_path "%s"\n' "$MARKER" "$bin" >"$fish/ryucad.fish"
                fi
                say "added $bin to PATH in your shell profile; open a new shell or run: $line"
            fi
            ;;
    esac

    "$bin/ryucad" version
}

main "$@"
