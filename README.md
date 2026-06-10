# gmux

**A terminal multiplexer with persistent sessions, tabs, panes, and a scriptable local socket.**

This is a modified fork of [herdr](https://github.com/ogulcancelik/herdr). Fork modifications began on 2026-06-05 with commit `935d4ce4`; this fork removes agent integrations and focuses on becoming a tmux replacement.

## Install

Install the latest release:

```bash
curl -fsSL https://raw.githubusercontent.com/stanley2058/gmux/main/install.sh | bash
```

Install a specific version:

```bash
curl -fsSL https://raw.githubusercontent.com/stanley2058/gmux/main/install.sh | GMUX_VERSION=v0.1.0 bash
```

By default, the installer writes to `~/.local/bin`. Set `GMUX_INSTALL_DIR` to use another directory.
Shell completions are installed for bash, zsh, and fish by default. Set `GMUX_INSTALL_COMPLETIONS=0` to skip them.

## Build

Requirements:

- Rust toolchain
- `just` for the local task recipes

Build a release binary:

```bash
cargo build --release --locked
./target/release/gmux
```

Run checks:

```bash
just test
just check
```

Install locally if desired:

```bash
just install
```

Print shell completions manually:

```bash
gmux completions bash
gmux completions zsh
gmux completions fish
```

## Quick Start

Start or attach the default session:

```bash
gmux
```

Use the first pane like a normal shell. Press `ctrl+b`, then `%` to split left/right, `ctrl+b`, then `"` to split top/bottom, and `ctrl+b`, then `c` to create a tab.

Detach with `ctrl+b d`. The client exits, but the session server and pane processes keep running. Reattach later with:

```bash
gmux
```

Named sessions are separate server namespaces:

```bash
gmux new -s work
gmux attach -t work
gmux ls
gmux session rename work api
gmux kill-session -t api
```

## Concepts

**Server and client.** A gmux session is owned by a background server. Clients attach to that server, render its panes, and forward input. Detaching closes only the client. `gmux kill-session` stops the server and exits its panes.

**Sessions, tabs, panes.** A session contains tabs, and a tab contains panes. Panes are real PTYs running shells or commands.

**Copy mode.** Press `ctrl+b [` for keyboard copy mode. Move with `h/j/k/l`, `w/b/e`, and `{`/`}`. Start selection with `v` or Space, copy with `y` or Enter, and exit with `q` or Esc. Mouse selection inside panes is supported too.

**Remote attach.** gmux can run on a remote host over SSH while preserving the local terminal experience:

```bash
gmux --remote workbox
gmux --remote ssh://you@server:2222
```

Remote auto-downloads are disabled in this fork. Install a matching `gmux` binary on the remote host manually, use a same-platform local binary, or set `GMUX_REMOTE_BINARY` to a target-compatible binary.

## CLI

Sessions:

```bash
gmux
gmux new -s <name>
gmux attach -t <name>
gmux ls
gmux kill-session -t <name>
gmux session rename <old> <new>
gmux detach
```

`gmux session rename` and `gmux session delete` only support stopped sessions. For the default session, they move or remove only session-owned state/socket files and leave config and logs in place.

Tabs:

```bash
gmux new-tab [-n name] [-c cwd]
gmux list-tabs
gmux select-tab -t <index|id>
gmux rename-tab -t <index|id> <name>
gmux kill-tab -t <index|id>
```

Panes:

```bash
gmux split-pane [-h|-v] [-c cwd] [command ...]
gmux select-pane [-L|-R|-U|-D|-t pane]
gmux resize-pane [-L|-R|-U|-D [N]]
gmux kill-pane -t <pane>
gmux send-keys [-t pane] <key> [key ...]
gmux send-text [-t pane] <text>
gmux capture-pane [-t pane] [-S lines] [-e]
```

Scriptable namespaces such as `gmux session`, `gmux tab`, and `gmux pane` remain available for lower-level automation.

## Keybindings

The default prefix is `ctrl+b`.

| key                     | action              |
| ----------------------- | ------------------- |
| `prefix+c`              | new tab             |
| `prefix+n` / `prefix+p` | next / previous tab |
| `prefix+1..9`           | switch tab          |
| `prefix+g`              | session navigator   |
| `prefix+h/j/k/l`        | focus pane          |
| `prefix+%` / `prefix+"` | split pane          |
| `prefix+x`              | close pane          |
| `prefix+z`              | zoom pane           |
| `prefix+r`              | resize mode         |
| `prefix+[`              | copy mode           |
| `prefix+d`              | detach              |

Mouse is supported for pane focus, pane resizing, tab selection, and scrollback. Resize mode uses `h/j/k/l` and Esc exits.

## Configuration

Config file:

```text
~/.config/gmux/config.toml
```

Print the full default config:

```bash
gmux --default-config
```

gmux supports theme, terminal, remote, mouse, toast, and keybinding settings. Logs are written under `~/.config/gmux/`; in persistent session mode, `gmux-client.log` and `gmux-server.log` are usually the useful files.

## Socket API

The local Unix socket lets scripts create tabs, split panes, send input, read pane output, wait for events, and control sessions without driving the terminal UI.

Useful command mappings:

```bash
gmux capture-pane   # read pane output
gmux send-keys      # send key names
gmux send-text      # send literal text
gmux split-pane     # create another pane
gmux new-tab        # create another tab
```

## Development

Read [`AGENTS.md`](./AGENTS.md) before larger changes.

```bash
just test
just check
```

## License

gmux is licensed under the GNU Affero General Public License v3.0 or later (`AGPL-3.0-or-later`).

See [`NOTICE`](./NOTICE) for upstream provenance and third-party notice information.
