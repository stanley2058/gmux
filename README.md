# gmux

<p align="center">
  <img src="assets/logo.png" alt="gmux" width="100" />
</p>

<p align="center">
  <a href="https://gmux.dev">gmux.dev</a> ·
  <a href="#install">install</a> ·
  <a href="#quick-start">quick start</a> ·
  <a href="#keybindings">keybindings</a> ·
  <a href="#configuration">configuration</a> ·
  <a href="#socket-api">socket api</a>
</p>

---

**A terminal multiplexer with persistent sessions, tabs, panes, and a scriptable local socket.**

gmux runs in the terminal you already use. It keeps pane processes alive when clients detach, supports named sessions, provides tmux-style defaults, and preserves the terminal features that matter for real interactive programs: mouse input, focus events, bracketed paste, OSC 52 clipboard, OSC 8 hyperlinks, Kitty keyboard input, and Kitty graphics.

## install

```bash
curl -fsSL https://gmux.dev/install.sh | sh
```

or install with Homebrew:

```bash
brew install gmux
```

or install with mise:

```bash
mise use -g gmux
```

or download a binary from [releases](https://github.com/ogulcancelik/gmux/releases). gmux supports Linux and macOS.

## quick start

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
gmux kill-session -t work
```

## concepts

**Server and client.** A gmux session is owned by a background server. Clients attach to that server, render its panes, and forward input. Detaching closes only the client. `gmux kill-session` stops the server and exits its panes.

**Sessions, tabs, panes.** A session contains tabs, and a tab contains panes. Panes are real PTYs running shells or commands.

**Copy mode.** Press `ctrl+b [` for keyboard copy mode. Move with `h/j/k/l`, `w/b/e`, and `{`/`}`. Start selection with `v` or Space, copy with `y` or Enter, and exit with `q` or Esc. Mouse selection inside panes is supported too.

**Remote attach.** gmux can run on a remote host over SSH while preserving the local terminal experience:

```bash
gmux --remote workbox
gmux --remote ssh://you@server:2222
```

## cli

Sessions:

```bash
gmux
gmux new -s <name>
gmux attach -t <name>
gmux ls
gmux kill-session -t <name>
gmux detach
```

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

## keybindings

The default prefix is `ctrl+b`.

| key | action |
|-----|--------|
| `prefix+c` | new tab |
| `prefix+n` / `prefix+p` | next / previous tab |
| `prefix+1..9` | switch tab |
| `prefix+g` | session navigator |
| `prefix+h/j/k/l` | focus pane |
| `prefix+%` / `prefix+"` | split pane |
| `prefix+x` | close pane |
| `prefix+z` | zoom pane |
| `prefix+r` | resize mode |
| `prefix+[` | copy mode |
| `prefix+d` | detach |

Mouse is supported for pane focus, pane resizing, tab selection, and scrollback. Resize mode uses `h/j/k/l` and Esc exits.

## configuration

Config file:

```text
~/.config/gmux/config.toml
```

Print the full default config:

```bash
gmux --default-config
```

gmux supports theme, terminal, remote, mouse, toast, sound, and keybinding settings. Logs are written under `~/.config/gmux/`; in persistent session mode, `gmux-client.log` and `gmux-server.log` are usually the useful files.

## socket api

The local Unix socket lets scripts create tabs, split panes, send input, read pane output, wait for events, and control sessions without driving the terminal UI.

Useful command mappings:

```bash
gmux capture-pane   # read pane output
gmux send-keys      # send key names
gmux send-text      # send literal text
gmux split-pane     # create another pane
gmux new-tab        # create another tab
```

See [socket API docs](https://gmux.dev/docs/socket-api/) for protocol details.

## update

For installs managed by gmux's installer:

```bash
gmux update
```

Homebrew, mise, and Nix installs update through their package managers. A running server keeps using the old binary until that session is stopped or handed off, so restart sessions after upgrading when you want every server on the new version.

## docs

- [quick start](https://gmux.dev/docs/quick-start/)
- [install](https://gmux.dev/docs/install/)
- [session state](https://gmux.dev/docs/session-state/)
- [configuration](https://gmux.dev/docs/configuration/)
- [socket API](https://gmux.dev/docs/socket-api/)

## development

```bash
git clone https://github.com/ogulcancelik/gmux
cd gmux
cargo build --release
./target/release/gmux

just test
just check
```

Read [`AGENTS.md`](./AGENTS.md) and [`CONTRIBUTING.md`](./CONTRIBUTING.md) before opening larger changes.

## license

gmux is dual-licensed:

1. Open source: GNU Affero General Public License v3.0 or later (AGPL-3.0-or-later).
2. Commercial: commercial licenses are available for organizations that cannot comply with AGPL.

Contact: hey@gmux.dev
