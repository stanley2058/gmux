#!/usr/bin/env bash
set -euo pipefail

repo="stanley2058/gmux"
binary="gmux"
install_dir="${GMUX_INSTALL_DIR:-$HOME/.local/bin}"
version="${GMUX_VERSION:-latest}"

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
    darwin-aarch64) platform="darwin-aarch64" ;;
    *)
        printf 'error: unsupported platform: %s-%s\n' "$os" "$arch" >&2
        printf 'supported platforms: linux-x86_64, darwin-aarch64\n' >&2
        exit 1
        ;;
esac

asset="$binary-$platform.tar.gz"
if [ "$version" = "latest" ]; then
    url="https://github.com/$repo/releases/latest/download/$asset"
else
    url="https://github.com/$repo/releases/download/$version/$asset"
fi

tmp_dir="$(mktemp -d)"
cleanup() {
    rm -rf "$tmp_dir"
}
trap cleanup EXIT

archive="$tmp_dir/$asset"
printf 'downloading %s\n' "$url"
curl -fL --retry 3 --connect-timeout 10 --max-time 120 -o "$archive" "$url"

tar -xzf "$archive" -C "$tmp_dir"
if [ ! -x "$tmp_dir/$binary" ]; then
    printf 'error: archive did not contain executable %s\n' "$binary" >&2
    exit 1
fi

mkdir -p "$install_dir"
install -m 0755 "$tmp_dir/$binary" "$install_dir/$binary"

printf 'installed %s to %s\n' "$binary" "$install_dir/$binary"
"$install_dir/$binary" --version

if [ "${GMUX_INSTALL_COMPLETIONS:-1}" != "0" ]; then
    bash_dir="${GMUX_BASH_COMPLETION_DIR:-$HOME/.local/share/bash-completion/completions}"
    zsh_dir="${GMUX_ZSH_COMPLETION_DIR:-$HOME/.local/share/zsh/site-functions}"
    fish_dir="${GMUX_FISH_COMPLETION_DIR:-$HOME/.config/fish/completions}"

    mkdir -p "$bash_dir" "$zsh_dir" "$fish_dir"
    "$install_dir/$binary" completions bash > "$bash_dir/gmux"
    "$install_dir/$binary" completions zsh > "$zsh_dir/_gmux"
    "$install_dir/$binary" completions fish > "$fish_dir/gmux.fish"

    printf 'installed bash completion to %s\n' "$bash_dir/gmux"
    printf 'installed zsh completion to %s\n' "$zsh_dir/_gmux"
    printf 'installed fish completion to %s\n' "$fish_dir/gmux.fish"
    printf 'zsh users may need to add this before compinit: fpath=(%s $fpath)\n' "$zsh_dir"
fi

case ":$PATH:" in
    *":$install_dir:"*) ;;
    *)
        printf 'warning: %s is not on PATH\n' "$install_dir" >&2
        printf 'add it to PATH or run %s directly\n' "$install_dir/$binary" >&2
        ;;
esac
