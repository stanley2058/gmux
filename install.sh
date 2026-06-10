#!/usr/bin/env bash
set -euo pipefail

repo="stanley2058/gmux"
binary="gmux"
install_dir="${GMUX_INSTALL_DIR:-$HOME/.local/bin}"
install_binary_path="${GMUX_INSTALL_BINARY_PATH:-}"
version="${GMUX_VERSION:-latest}"

log() {
    if [ "${GMUX_INSTALL_QUIET:-0}" != "1" ]; then
        printf '%s\n' "$*"
    fi
}

warn() {
    printf 'warning: %s\n' "$*" >&2
}

require_command() {
    if ! command -v "$1" >/dev/null 2>&1; then
        printf 'error: required command not found: %s\n' "$1" >&2
        exit 1
    fi
}

require_command curl
require_command tar
require_command install
require_command mktemp

if command -v sha256sum >/dev/null 2>&1; then
    checksum_command="sha256sum"
elif command -v shasum >/dev/null 2>&1; then
    checksum_command="shasum -a 256"
else
    printf 'error: required command not found: sha256sum or shasum\n' >&2
    exit 1
fi

if [ -n "$install_binary_path" ]; then
    install_dir="$(dirname "$install_binary_path")"
    binary="$(basename "$install_binary_path")"
fi

case "$(uname -s)" in
    Linux) os="linux" ;;
    Darwin) os="darwin" ;;
    *)
        printf 'error: unsupported OS: %s\n' "$(uname -s)" >&2
        exit 1
        ;;
esac

case "$(uname -m)" in
    x86_64|amd64) arch="x86_64" ;;
    aarch64|arm64) arch="aarch64" ;;
    *)
        printf 'error: unsupported architecture: %s\n' "$(uname -m)" >&2
        exit 1
        ;;
esac

case "$os-$arch" in
    linux-x86_64) platform="linux-x86_64" ;;
    linux-aarch64) platform="linux-aarch64" ;;
    darwin-x86_64) platform="darwin-x86_64" ;;
    darwin-aarch64) platform="darwin-aarch64" ;;
    *)
        printf 'error: unsupported platform: %s-%s\n' "$os" "$arch" >&2
        printf 'supported platforms: linux-x86_64, linux-aarch64, darwin-x86_64, darwin-aarch64\n' >&2
        exit 1
        ;;
esac

asset="gmux-$platform.tar.gz"
if [ "$version" = "latest" ]; then
    url="https://github.com/$repo/releases/latest/download/$asset"
    checksums_url="https://github.com/$repo/releases/latest/download/SHA256SUMS"
else
    url="https://github.com/$repo/releases/download/$version/$asset"
    checksums_url="https://github.com/$repo/releases/download/$version/SHA256SUMS"
fi

tmp_dir="$(mktemp -d)"
cleanup() {
    rm -rf "$tmp_dir"
}
trap cleanup EXIT

archive="$tmp_dir/$asset"
checksums="$tmp_dir/SHA256SUMS"
log "downloading $url"
curl -fL --retry 3 --connect-timeout 10 --max-time 120 -o "$archive" "$url"
log "downloading $checksums_url"
curl -fL --retry 3 --connect-timeout 10 --max-time 120 -o "$checksums" "$checksums_url"

expected_checksum="$(awk -v asset="$asset" '$2 == asset { print $1; found = 1 } END { if (!found) exit 1 }' "$checksums")" || {
    printf 'error: checksum file did not contain %s\n' "$asset" >&2
    exit 1
}
actual_checksum="$($checksum_command "$archive" | awk '{ print $1 }')"
if [ "$actual_checksum" != "$expected_checksum" ]; then
    printf 'error: checksum mismatch for %s\n' "$asset" >&2
    printf 'expected: %s\n' "$expected_checksum" >&2
    printf 'actual:   %s\n' "$actual_checksum" >&2
    exit 1
fi

tar -xzf "$archive" -C "$tmp_dir"
if [ ! -x "$tmp_dir/gmux" ]; then
    printf 'error: archive did not contain executable gmux\n' >&2
    exit 1
fi

mkdir -p "$install_dir"
target="$install_dir/$binary"
tmp_target="$install_dir/.$binary.tmp.$$"
install -m 0755 "$tmp_dir/gmux" "$tmp_target"
mv -f "$tmp_target" "$target"

log "installed $binary to $target"
"$target" --version

if [ "${GMUX_INSTALL_COMPLETIONS:-1}" != "0" ]; then
    bash_dir="${GMUX_BASH_COMPLETION_DIR:-$HOME/.local/share/bash-completion/completions}"
    zsh_dir="${GMUX_ZSH_COMPLETION_DIR:-$HOME/.local/share/zsh/site-functions}"
    fish_dir="${GMUX_FISH_COMPLETION_DIR:-$HOME/.config/fish/completions}"

    mkdir -p "$bash_dir" "$zsh_dir" "$fish_dir"
    "$target" completions bash > "$bash_dir/gmux" || warn "failed to install bash completion"
    "$target" completions zsh > "$zsh_dir/_gmux" || warn "failed to install zsh completion"
    "$target" completions fish > "$fish_dir/gmux.fish" || warn "failed to install fish completion"

    log "installed bash completion to $bash_dir/gmux"
    log "installed zsh completion to $zsh_dir/_gmux"
    log "installed fish completion to $fish_dir/gmux.fish"
    log "zsh users may need to add this before compinit: fpath=($zsh_dir \$fpath)"
fi

case ":$PATH:" in
*":$install_dir:"*) ;;
*)
        warn "$install_dir is not on PATH"
        warn "add it to PATH or run $target directly"
        ;;
esac
