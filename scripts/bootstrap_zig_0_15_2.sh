#!/usr/bin/env bash
set -eu

version="0.15.2"
repo_root="$(git rev-parse --show-toplevel 2>/dev/null || pwd)"
vendor_dir="$repo_root/vendor/zig-$version"
download_dir="$repo_root/vendor/.zig-downloads"

if [ -x "$vendor_dir/zig" ] && [ "$($vendor_dir/zig version)" = "$version" ]; then
    printf 'zig %s already available at %s\n' "$version" "$vendor_dir/zig"
    exit 0
fi

case "$(uname -s)" in
    Linux) os="linux" ;;
    Darwin) os="macos" ;;
    *)
        printf 'unsupported OS for Zig bootstrap: %s\n' "$(uname -s)" >&2
        exit 1
        ;;
esac

case "$(uname -m)" in
    x86_64|amd64) arch="x86_64" ;;
    aarch64|arm64) arch="aarch64" ;;
    *)
        printf 'unsupported architecture for Zig bootstrap: %s\n' "$(uname -m)" >&2
        exit 1
        ;;
esac

target="$arch-$os"
archive="zig-$target-$version.tar.xz"
url="https://ziglang.org/download/$version/$archive"
tmp_archive="$download_dir/$archive"
tmp_dir="$download_dir/extract-$target"

mkdir -p "$download_dir"
rm -rf "$tmp_dir"

printf 'downloading %s\n' "$url"
curl -fL --retry 3 --connect-timeout 10 --max-time 120 -o "$tmp_archive" "$url"

mkdir -p "$tmp_dir"
tar -xJf "$tmp_archive" -C "$tmp_dir" --strip-components=1

rm -rf "$vendor_dir"
mv "$tmp_dir" "$vendor_dir"

actual="$($vendor_dir/zig version)"
if [ "$actual" != "$version" ]; then
    printf 'bootstrapped Zig version mismatch: expected %s, got %s\n' "$version" "$actual" >&2
    exit 1
fi

printf 'zig %s available at %s\n' "$version" "$vendor_dir/zig"
