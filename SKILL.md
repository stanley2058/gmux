---
name: gmux
description: "Control gmux from inside a gmux-managed pane. Manage persistent sessions, tabs, panes, popups, pane output, input, and waits through the installed gmux CLI and its local socket. Use only when GMUX_ENV=1."
---

# gmux agent skill

Before using this skill, check that `GMUX_ENV=1`. If it is not set to `1`, say you are not running inside a gmux-managed pane and stop. Do not inspect or control the focused gmux pane from outside gmux.

You are running inside gmux, a terminal multiplexer and session manager. A gmux session is owned by a background server. Clients attach to that server, render tabs and panes, and forward input. Detaching closes only the client; pane processes keep running until their session, tab, or pane is closed.

Use the installed `gmux` binary in `PATH`. Commands talk to the running gmux server through the local socket. Prefer public CLI commands over private socket details, and use `gmux --help` or subcommand help when in doubt.

## core model

**Sessions** are persistent server namespaces. The default session is used by `gmux`; named sessions use `gmux new -s name`, `gmux attach -t name`, or `gmux --session name`.

**Tabs** are top-level layouts inside a session. Each tab has one or more panes.

**Panes** are real PTYs running shells, commands, servers, logs, editors, agents, or TUIs.

**Popups** are floating panes created over a target pane. Use them for transient tools such as `lazygit`, pagers, scratch shells, or quick commands.

**Ids** are compact public ids for the current live session. Tab ids look like `1:1`; pane ids look like `1-1`. Re-read ids from `gmux tab list`, `gmux pane list`, or command JSON responses before targeting a pane or tab. Do not treat ids as durable after closing tabs or panes.

There are no public `workspace` commands and no public `wait agent-status` command in current gmux. Coordinate by reading panes and waiting for output.

## safety rules

- Check `GMUX_ENV=1` before using this skill.
- Treat the focused pane as yours. Use `gmux pane list` to identify neighboring panes before reading or sending input.
- Prefer `--no-focus` when creating helper panes unless the task requires focus to move.
- Read before you write to a pane you did not create.
- Use `pane read` or `capture-pane` for existing output. Use `wait output` for future output you expect next.
- Parse new ids from JSON responses instead of guessing them.

## discover context

List panes and find the focused pane:

```bash
gmux pane list
```

List tabs:

```bash
gmux tab list
```

Show session or client/server status:

```bash
gmux status
gmux status --json
gmux status server --json
gmux status client --json
```

Useful environment variables inside panes:

- `GMUX_ENV=1` means the process is running inside gmux.
- `GMUX_ACTIVE_PANE_ID` is set for shell commands launched by gmux keybindings.
- `GMUX_ACTIVE_TAB_ID` is set for shell commands launched by gmux keybindings.
- `GMUX_BIN_PATH` points keybinding commands at the active gmux binary when available.

## sessions

Start or attach the default persistent session:

```bash
gmux
```

Create or attach a named session:

```bash
gmux new -s work
gmux attach -t work
gmux --session work
```

List and stop sessions:

```bash
gmux ls
gmux ls --json
gmux kill-session -t work
gmux kill-session -t work --json
```

Manage stopped sessions:

```bash
gmux session list
gmux session stop work --json
gmux session rename work api --json
gmux session delete old-name --json
```

Use top-level `gmux attach -t work` to attach to an existing named session.

Detach the current client without stopping panes:

```bash
gmux detach
```

## tabs

Use tmux-style aliases for common interactive actions:

```bash
gmux list-tabs
gmux new-tab -n logs -c /path/to/project
gmux select-tab -t 2
gmux rename-tab -t 2 logs
gmux kill-tab -t 2
```

Use scriptable commands when you need JSON responses and exact ids:

```bash
gmux tab list
gmux tab create --cwd /path/to/project --label logs --no-focus
gmux tab create --label tests --focus cargo test
gmux tab get 1:2
gmux tab focus 1:2
gmux tab rename 1:2 logs
gmux tab close 1:2
```

`gmux tab create` prints JSON on success. The new tab is at `result.tab`, and its root pane is at `result.root_pane`.

## panes

Use tmux-style aliases for the focused or targeted pane:

```bash
gmux capture-pane -S 80
gmux capture-pane -t 1-2 -S 80 --source recent-unwrapped
gmux split-pane -h -t 1-2 cargo test
gmux split-pane -v -t 1-2 npm run dev
gmux select-pane -L
gmux select-pane -t 1-3
gmux resize-pane -R 5
gmux send-text -t 1-3 "hello"
gmux send-keys -t 1-3 Enter
gmux kill-pane -t 1-3
```

Use scriptable commands when you need exact ids, JSON responses, or less ambiguous behavior:

```bash
gmux pane list
gmux pane get 1-2
gmux pane rename 1-2 logs
gmux pane rename 1-2 --clear
gmux pane close 1-3
```

Read pane output:

```bash
gmux pane read 1-2 --source recent --lines 80
gmux pane read 1-2 --source recent-unwrapped --lines 80
gmux pane read 1-2 --source visible --format ansi
gmux pane read 1-2 --source visible --raw
```

Read sources:

- `visible` reads the current viewport.
- `recent` reads recent scrollback as rendered in the pane.
- `recent-unwrapped` reads recent terminal text with soft wraps joined back together.

Read formats:

- `--format text` is plain text and strips ANSI by default.
- `--format ansi` or `--ansi` returns a rendered ANSI snapshot.
- `--raw` returns ANSI without stripping it.

Split panes with scriptable commands:

```bash
gmux pane split 1-2 --direction right --cwd /path/to/project --no-focus cargo test
gmux pane split 1-2 --direction down --no-focus npm run dev
```

`gmux pane split` prints JSON on success. The new pane id is at `result.pane.pane_id`.

Create a popup pane:

```bash
gmux pane popup 1-2 --width 80% --height 80% --cwd /path/to/project lazygit
gmux pane popup 1-2 --width 100 --height 30 --x C --y C --no-focus bash
```

Send input:

```bash
gmux pane send-text 1-3 "echo hello"
gmux pane send-keys 1-3 Enter
gmux pane run 1-3 "echo hello"
```

`pane send-text` sends literal text without Enter. `pane send-keys` sends key names. `pane run` sends text followed by a real `Enter` key.

## wait for output

Wait until a pane emits matching output:

```bash
gmux wait output 1-3 --match "ready" --timeout 30000
```

Use a regex:

```bash
gmux wait output 1-3 --match "server.*ready" --regex --timeout 30000
```

Choose the source when needed:

```bash
gmux wait output 1-3 --source recent-unwrapped --lines 120 --match "test result" --timeout 60000
```

For `--source recent`, output matching uses unwrapped recent terminal text, so pane width and soft wrapping do not break matches. If you want to inspect the same transcript the waiter matches, use `gmux pane read <pane_id> --source recent-unwrapped`.

If a wait times out, the command exits nonzero.

## server and config

Reload config in the running server:

```bash
gmux server reload-config
```

Stop the running server through the API socket:

```bash
gmux server stop
```

Use live handoff only when explicitly needed:

```bash
gmux server live-handoff
gmux server live-handoff --all-sessions
```

Back up `config.toml` and remove custom keybindings:

```bash
gmux config reset-keys
```

## recipes

### inspect the current gmux session

```bash
gmux status --json
gmux tab list
gmux pane list
```

### run tests in a helper pane

```bash
NEW_PANE=$(gmux pane split 1-2 --direction down --no-focus cargo test | python3 -c 'import sys,json; print(json.load(sys.stdin)["result"]["pane"]["pane_id"])')
gmux wait output "$NEW_PANE" --match "test result" --timeout 60000
gmux pane read "$NEW_PANE" --source recent --lines 40
```

### start a server and wait until it is ready

```bash
NEW_PANE=$(gmux pane split 1-2 --direction right --no-focus npm run dev | python3 -c 'import sys,json; print(json.load(sys.stdin)["result"]["pane"]["pane_id"])')
gmux wait output "$NEW_PANE" --match "ready" --timeout 30000
gmux pane read "$NEW_PANE" --source recent --lines 30
```

### coordinate with a sibling pane

```bash
gmux pane list
gmux pane read 1-3 --source recent --lines 80
gmux wait output 1-3 --match "done" --timeout 120000
gmux pane read 1-3 --source recent-unwrapped --lines 120
```

### open a transient popup tool

```bash
gmux pane popup 1-2 --width 80% --height 80% --cwd /path/to/project lazygit
```

## command output notes

- `gmux tab list`, `gmux tab create`, `gmux tab get`, `gmux tab focus`, `gmux tab rename`, `gmux tab close`, `gmux pane list`, `gmux pane get`, `gmux pane split`, `gmux pane popup`, `gmux pane close`, `gmux pane rename`, and `gmux wait output` print JSON on success.
- `gmux status --json`, `gmux ls --json`, and session commands with `--json` print JSON.
- `gmux pane read` and `gmux capture-pane` print pane text, not JSON.
- `gmux pane send-text`, `gmux pane send-keys`, and `gmux pane run` print nothing on success.
- `gmux tab create` returns `result.tab` and `result.root_pane`.
- `gmux pane split` and `gmux pane popup` return the created pane at `result.pane`.
- Use `--help` on any command or subcommand to confirm current installed behavior.
