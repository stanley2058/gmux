#!/usr/bin/env bash
set -euo pipefail

binary="${1:-gmux}"

if [ ! -x "$binary" ] && ! command -v "$binary" >/dev/null 2>&1; then
    printf 'error: gmux binary not found: %s\n' "$binary" >&2
    exit 1
fi

bash_dir="${GMUX_BASH_COMPLETION_DIR:-$HOME/.local/share/bash-completion/completions}"
zsh_dir="${GMUX_ZSH_COMPLETION_DIR:-$HOME/.local/share/zsh/site-functions}"
fish_dir="${GMUX_FISH_COMPLETION_DIR:-$HOME/.config/fish/completions}"

mkdir -p "$bash_dir" "$zsh_dir" "$fish_dir"

"$binary" completions bash > "$bash_dir/gmux"
"$binary" completions zsh > "$zsh_dir/_gmux"
"$binary" completions fish > "$fish_dir/gmux.fish"

printf 'installed bash completion to %s\n' "$bash_dir/gmux"
printf 'installed zsh completion to %s\n' "$zsh_dir/_gmux"
printf 'installed fish completion to %s\n' "$fish_dir/gmux.fish"

printf '\n'
printf 'zsh users may need to add this to ~/.zshrc before compinit:\n'
printf '  fpath=(%s $fpath)\n' "$zsh_dir"
