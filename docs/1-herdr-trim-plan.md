# Herdr Trim Plan for gmux

## Goal

Build `gmux` as a simple, tmux-like session manager with excellent terminal extension support, using Herdr as the starting point.

Target model:

```text
sessions
  -> tabs
     -> panes
```

Primary goals:

- Preserve Herdr's hard terminal work: server-owned PTYs, detach/reattach, remote SSH attach, Ghostty VT integration, OSC 52, OSC 8, Kitty keyboard, Kitty graphics, mouse/focus, bracketed paste, copy mode, and efficient frame streaming.
- Remove AI-agent product features: detection, integrations, agent state UI, agent resume, agent-specific notifications, agent CLI/API, worktree workflows, and agent docs.
- Make default keybindings feel close to tmux so existing muscle memory works.
- Keep the fork AGPL-compatible.

Reference checkout:

- `herdr-upstream/` is a read-only reference clone of `https://github.com/ogulcancelik/herdr` at version `0.6.8`.

## Current Herdr Shape

Herdr's user-visible hierarchy is:

```text
Herdr named session/server namespace
  -> workspaces
     -> tabs
        -> panes
```

The desired gmux hierarchy removes `workspace` from the user model and promotes Herdr's named session concept into the main UX:

```text
gmux session
  -> tabs
     -> panes
```

This means the largest structural change is collapsing or renaming `Workspace` into the session state. Tabs and panes already map well.

## What To Keep

### Terminal Core

Keep these modules nearly intact at first:

- `src/ghostty/`
- `src/pane/`
- `src/pane.rs`
- `src/pty/`
- `src/terminal/runtime.rs`
- `src/terminal/runtime_registry.rs`
- `src/input/`
- `src/raw_input.rs`
- `src/protocol/`
- `src/kitty_graphics.rs`
- `src/selection.rs`
- `src/layout.rs`

Why:

- `src/ghostty/` wraps vendored Ghostty VT FFI and exposes APIs we need, including hyperlink URI extraction and Kitty graphics placement state.
- `src/pane.rs` owns the PTY plus Ghostty terminal runtime and handles scrollback, rendering, cursor state, input, paste, mouse/focus, OSC handling, and child terminal responses.
- `src/pty/` already has a nonblocking PTY actor with read/write/resize/handoff mechanics.
- `src/input/` and `src/raw_input.rs` parse and encode Kitty keyboard, legacy keys, mouse, focus, bracketed paste, UTF-8, and host terminal replies.
- `src/protocol/` already has a useful server/client protocol with semantic frames, ANSI terminal frames, clipboard, graphics, resize, and raw input.
- `src/kitty_graphics.rs` is important for image support inside pane layouts.
- `src/selection.rs` is useful for pane-only text copy and OSC 52 clipboard copy.
- `src/layout.rs` is a reusable BSP pane layout engine.

### Server/Client Split

Keep and rename these areas:

- `src/server/headless.rs`
- `src/server/client_transport.rs`
- `src/server/client_accept.rs`
- `src/server/clients.rs`
- `src/server/render_stream.rs`
- `src/server/socket_paths.rs`
- `src/server/terminal_attach.rs`
- `src/client/`
- `src/remote.rs`
- `src/session.rs`

Why:

- Herdr already implements the persistent background server plus thin terminal client model.
- Client render messages are split into reliable control and droppable render frames. Keep this. Slow clients should not accumulate render backlog.
- Remote attach over SSH is close to the desired user experience.
- Direct terminal attach is useful as a debug/escape path and as a possible `gmux attach-pane` feature.

### Useful UI Pieces

Keep these, but simplify them:

- `src/ui/panes.rs`
- `src/ui/tabs.rs`
- `src/ui/scrollbar.rs`
- `src/ui/widgets.rs`
- `src/ui/menus.rs`
- `src/ui/dialogs.rs`
- parts of `src/ui/keybind_help.rs`

Why:

- Pane borders, terminal rendering, scrollbars, mouse resizing, copy highlight, and tab bar are still useful.
- Agent sidebar, onboarding, settings integrations, and product announcement UI should not survive long-term.

### Config Pieces

Keep and prune:

- `src/config/keybinds.rs`
- `src/config/io.rs`
- `src/config/model.rs`
- `src/config/theme.rs`

Why:

- The keybinding parser is already robust and supports prefix/direct bindings.
- Config path handling should be renamed to gmux.
- Theme support is useful, but settings should be simplified.

## What To Remove

### Agent Detection

Remove:

- `src/detect/`
- `mod detect` from `src/main.rs`
- all `Agent`, `AgentState`, and screen heuristic logic after neutral replacements are in place.

Current risk:

- `AgentState` is used as a broad product state primitive in terminal state, workspace rollups, navigator/sidebar, API responses, waits, notifications, tests, and persistence. Do not delete this first. Hide/neutralize consumers first.

### Agent Resume

Remove:

- `src/agent_resume.rs`
- `src/app/agent_resume.rs`
- `TerminalState.pending_agent_resume_plan`
- `TerminalState.persisted_agent_session`
- snapshot fields for agent sessions
- config `[session].resume_agents_on_restore`

Replacement:

- Restore panes as shells in their saved cwd.
- Optionally keep neutral `launch_argv` for panes created from explicit commands.

### Integrations

Remove:

- `src/integration/`
- `src/integration/assets/**`
- `src/cli/integration.rs`
- `src/app/api/integrations.rs`
- integration settings UI and badges
- `herdr integration ...` commands

### Agent CLI/API

Remove:

- `src/cli/agent.rs`
- top-level `agent` dispatch in `src/cli.rs`
- `src/app/api/agents.rs`
- `src/app/agents.rs`
- agent target resolution in `src/app/terminal_targets.rs`

Remove or convert these API methods:

- `agent.list`
- `agent.get`
- `agent.read`
- `agent.send`
- `agent.rename`
- `agent.focus`
- `agent.start`
- `pane.report_agent`
- `pane.report_agent_session`
- `pane.report_metadata`
- `pane.clear_agent_authority`
- `pane.release_agent`

Convert useful behavior to pane/session/tab commands:

- `agent read` -> `pane read` / `capture-pane`
- `agent send` -> `pane send` / `send-keys`
- `agent attach` -> `terminal attach` or `pane attach`
- `agent start` -> `split-pane [command...]`

### Workspaces and Worktrees

Remove user-visible workspace functionality:

- `src/cli/workspace.rs`
- workspace picker/navigator as a workspace selector
- workspace commands and docs

Collapse internally:

- `Workspace` should become the session's tab container or be replaced by a new `SessionState`/`MuxState` type.

Remove worktree support entirely:

- `src/worktree.rs`
- `src/workspace/git/`
- `src/cli/worktree.rs`
- `[worktrees]` config
- worktree UI/docs/keybindings

### Agent Product UI

Remove or rewrite:

- `src/ui/sidebar.rs` agent panel
- `src/ui/status.rs` agent icons/state labels
- `src/ui/onboarding.rs` agent/integration copy
- `src/ui/settings.rs` integration and agent sound sections
- `src/ui/navigator.rs` agent state filters
- `src/product_announcements.rs`
- `src/ui/release_notes.rs` if update flow is removed

### Notifications and Sounds

Remove initially:

- per-agent sounds
- agent blocked/done notifications
- `assets/sounds/done.mp3`
- `assets/sounds/request.mp3`
- `[ui.sound.agents]`

Possible later replacement:

- generic bell/activity alerts per pane or tab.

### Update/Channel System

Defer or remove initially:

- `src/update.rs`
- `src/release_notes.rs`
- `herdr update`
- `herdr channel`
- Herdr release manifest URLs

Remote attach should not depend on Herdr release assets. If remote auto-install is kept, point it at gmux assets later.

## What To Change

### Product Rename

Rename everywhere:

```text
herdr -> gmux
Herdr -> Gmux or gmux
HERDR_* -> GMUX_*
~/.config/herdr -> ~/.config/gmux
herdr.sock -> gmux.sock
herdr-client.sock -> gmux-client.sock
```

Important files:

- `Cargo.toml`
- `src/main.rs`
- `src/config/io.rs`
- `src/session.rs`
- `src/server/socket_paths.rs`
- `src/remote.rs`
- `README.md`
- docs under `docs/next/website/src/content/docs/`
- install/release scripts if kept

### Session Model

Current `src/session.rs` is already useful and should become the primary user concept.

Keep:

- named sessions
- session list
- per-session data directories
- per-session sockets
- default session

Change:

- `HERDR_SESSION` -> `GMUX_SESSION`
- `session attach <name>` remains as a compatibility command or explicit subcommand
- top-level tmux-like aliases should exist

Suggested data paths:

```text
~/.config/gmux/gmux.sock
~/.config/gmux/gmux-client.sock
~/.config/gmux/sessions/<name>/gmux.sock
~/.config/gmux/sessions/<name>/gmux-client.sock
```

Suggested env vars:

```text
GMUX_SESSION
GMUX_SOCKET
GMUX_CLIENT_SOCKET
GMUX_CONFIG_PATH
GMUX_REMOTE_BINARY
```

### State Model

Replace this:

```text
AppState
  workspaces: Vec<Workspace>
```

With this:

```text
AppState
  session: SessionUiState
    tabs: Vec<Tab>
    active_tab: usize
```

Long-term replacement for `TerminalState`:

```rust
pub struct TerminalState {
    pub id: TerminalId,
    pub cwd: PathBuf,
    pub manual_label: Option<String>,
    pub launch_argv: Option<Vec<String>>,
    pub respawn_shell_on_exit: bool,
    pub revision: u64,
}
```

Remove from `TerminalState`:

- detected agent
- hook authority
- fallback state
- agent metadata
- persisted agent session
- pending resume plan
- agent name
- agent state arbitration methods

Temporary compatibility strategy:

- First make `effective_title` / `border_label` return manual labels and terminal titles only.
- Keep dummy neutral methods while call sites are removed.
- Delete agent fields after UI/API/persistence are neutralized.

### Persistence

Current Herdr snapshot shape effectively stores:

```text
SessionSnapshot
  workspaces[]
    tabs[]
      panes/layout
```

Target gmux snapshot shape:

```text
SessionSnapshot
  version
  active_tab
  tabs[]
    name
    layout
    panes[]
      terminal_id
      cwd
      label
      launch_argv
      scrollback/history reference
```

Keep:

- tab layout restore
- pane cwd restore
- pane labels
- optional screen history replay

Remove:

- workspace metadata
- git/worktree metadata
- agent panel scope
- agent sessions
- resume agent plans

Compatibility decision:

- For the first gmux version, prefer a clean new snapshot version.
- Optionally support importing old Herdr snapshots by taking the active workspace's tabs or flattening all workspaces into tabs, but do not build this unless there is a concrete need.

### CLI

Primary tmux-like UX:

```bash
gmux                         # attach default session, creating it if needed
gmux new -s <name>            # create named session
gmux attach -t <name>         # attach named session
gmux ls                      # list sessions
gmux kill-session -t <name>   # stop a session
gmux detach                  # detach current client
```

Keep scriptable namespaces:

```bash
gmux session list
gmux session new <name>
gmux session attach <name>
gmux session kill <name>
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
gmux split-pane [-h|-v] [-c cwd] [command...]
gmux select-pane [-L|-R|-U|-D|-t pane]
gmux resize-pane [-L|-R|-U|-D N]
gmux kill-pane [-t pane]
gmux send-keys [-t pane] <keys...>
gmux send-text [-t pane] <text>
gmux capture-pane [-t pane] [-S lines] [-e]
```

Remote:

```bash
gmux --remote <host> attach [-t session]
gmux --remote ssh://user@host:2222 attach [-t session]
```

Map Herdr commands:

```text
herdr session list      -> gmux ls / gmux session list
herdr session attach    -> gmux attach -t
herdr session stop      -> gmux kill-session -t
herdr tab create        -> gmux new-tab
herdr tab close         -> gmux kill-tab
herdr pane split        -> gmux split-pane
herdr pane read         -> gmux capture-pane
herdr pane send-keys    -> gmux send-keys
herdr terminal attach   -> gmux attach-pane or gmux terminal attach
```

### Keybindings

Defaults should be close to tmux.

Recommended initial defaults:

```toml
[keys]
prefix = "ctrl+b"

help = "prefix+?"
detach = "prefix+d"
reload_config = "prefix+shift+r"

new_tab = "prefix+c"
rename_tab = "prefix+comma"
close_tab = "prefix+ampersand"
previous_tab = "prefix+p"
next_tab = "prefix+n"
switch_tab = "prefix+0..9"

split_vertical = "prefix+percent"
split_horizontal = "prefix+doublequote"
close_pane = "prefix+x"
zoom = "prefix+z"
copy_mode = "prefix+["
last_pane = "prefix+semicolon"

focus_pane_left = ["prefix+left", "prefix+h"]
focus_pane_down = ["prefix+down", "prefix+j"]
focus_pane_up = ["prefix+up", "prefix+k"]
focus_pane_right = ["prefix+right", "prefix+l"]

resize_mode = "prefix+r"
```

Parser changes needed:

- Support indexed `0..9`, not just `1..9`.
- Ensure punctuation names work: `percent`, `doublequote`, `quote`, `comma`, `ampersand`, `semicolon`, `colon`.
- Remove agent key fields: `previous_agent`, `next_agent`, `focus_agent`, `[keys.indexed].agents`.

### Render/Terminal Extension Requirements

These are non-negotiable keepers from Herdr:

- OSC 52 clipboard forwarding from child PTY to local client.
- OSC 8 hyperlink URI preservation through cell metadata and ANSI blit output.
- Kitty keyboard protocol parsing and child-side key encoding.
- Bracketed paste parsing and forwarding.
- Mouse mode tracking and SGR mouse forwarding.
- Focus mode tracking and focus event forwarding.
- Kitty graphics state extraction, clipping, upload/place/delete handling.
- Synchronized output around frame blits.
- Correct cursor positioning, visibility, and shape handling.
- Wide grapheme invalidation during diff rendering.
- Host terminal reply buffering so OSC replies do not leak into panes.

## Staged Implementation Plan

### Stage 0: Reference and Baseline

- Keep `herdr-upstream/` as the untouched reference checkout.
- Decide whether to fork by copying Herdr into repo root or by turning this repo into a direct fork.
- Run upstream `just test` or at least `cargo test` in `herdr-upstream/` to establish baseline if the vendored build works locally.

Deliverable:

- This plan document.

### Stage 1: Mechanical Fork/Rename

- Copy Herdr source to repo root or replace the root with Herdr's tree.
- Rename crate/binary to `gmux`.
- Rename config paths, sockets, env vars, logs, and docs references.
- Keep all behavior compiling, even agent behavior, during the rename.

Validation:

- `cargo test`
- smoke run `target/debug/gmux --help` or equivalent
- attach/detach default session locally

### Stage 2: Hide Agent UI and Defaults

- Remove agent panel from default UI.
- Disable integration prompts/onboarding.
- Disable agent labels on pane borders.
- Disable agent resume by default.
- Remove agent keybindings from defaults.
- Keep internal code compiling with temporary shims.

Validation:

- Local attach shows only tabs/panes.
- No agent/integration copy in default UI.
- Basic split/tab/detach still works.

### Stage 3: CLI Surface Simplification

- Introduce tmux-like top-level commands.
- Keep scriptable subcommands for `session`, `tab`, and `pane`.
- Remove `agent`, `integration`, `worktree`, `workspace` commands from user help.
- Convert any useful agent command behavior to pane commands.

Validation:

- `gmux`, `gmux ls`, `gmux attach -t`, `gmux kill-session -t` work.
- `gmux split-pane`, `gmux capture-pane`, `gmux send-keys` work.

### Stage 4: Collapse Workspace Layer

- Replace `AppState.workspaces` with session-owned tabs.
- Move tab list/active tab to the session state.
- Remove workspace picker and workspace sidebar.
- Keep tab bar and pane renderer.
- Remove worktree/git workspace code.

Validation:

- Multiple tabs and panes persist within a named session.
- No user-visible workspace layer remains.

### Stage 5: Neutralize TerminalState

- Replace agent-heavy `TerminalState` with neutral terminal metadata.
- Remove hook authority, detected agent, metadata, and resume fields.
- Replace effective label logic with pane title/manual label/cwd/process title.

Validation:

- Pane labels and terminal titles still render.
- API/CLI pane listing has no agent fields.

### Stage 6: Remove Agent Detection, Integrations, Resume

- Delete `src/detect/`.
- Delete `src/integration/`.
- Delete agent resume modules.
- Remove agent API schemas, event variants, config fields, tests, docs.

Validation:

- Full build/test passes.
- Grep for `Agent`, `agent`, `integration`, and `worktree` only returns intentional docs/compatibility references.

### Stage 7: Persistence Format Rewrite

- Define clean gmux snapshot schema.
- Store sessions -> tabs -> panes.
- Remove workspace and agent snapshot fields.
- Decide if old Herdr snapshot import is unsupported or one-time best-effort.

Validation:

- Create tabs/panes, detach, reattach.
- Stop server, restart, restore shape/history if enabled.

### Stage 8: Remote Attach Cleanup

- Rename remote env vars and generated SSH config labels.
- Remove Herdr release URLs from remote auto-install.
- Keep remote local-client behavior and session targeting.

Validation:

- `gmux --remote host attach -t session` starts/attaches remote server.
- Clipboard over SSH works via local client.

### Stage 9: Docs and Distribution

- Rewrite README around sessions/tabs/panes.
- Remove agent/integration docs.
- Document terminal extension support explicitly.
- Decide whether to keep website/update systems.

Validation:

- Docs match CLI/config behavior.

## Risk Register

### Highest Risk: AgentState Is Cross-Cutting

`AgentState` is not isolated to detection. It drives UI badges, notifications, API schemas, workspace rollups, waits, and tests. Removing it before replacing consumers will cause widespread breakage.

Mitigation:

- Hide UI and API agent surfaces first.
- Add neutral pane status only if needed.
- Delete detection last.

### High Risk: TerminalState Is Agent-Heavy

`TerminalState` combines labels, cwd, agent detection, hook state, metadata, persisted agent sessions, and resume plans.

Mitigation:

- Introduce a neutral `TerminalState` shape behind existing accessors.
- Migrate call sites from `effective_agent_label` to `terminal_label` or `pane_title`.

### High Risk: Workspace Collapse Touches Everything

Workspace is user-visible and structural in AppState, persistence, UI, API, CLI, and docs.

Mitigation:

- Do not combine workspace collapse with agent deletion.
- First make one implicit workspace/session container, then rename/remove user-facing workspace commands.

### Medium Risk: Snapshot Compatibility

Existing Herdr snapshots contain workspace and agent fields.

Mitigation:

- Prefer clean gmux snapshot version.
- Optionally write an importer later.

### Medium Risk: Remote Auto-Install

Herdr remote attach may download matching Herdr releases.

Mitigation:

- Disable auto-download initially unless `GMUX_REMOTE_BINARY` is set.
- Re-enable after gmux release assets exist.

### Medium Risk: Kitty Graphics Coupling

`kitty_graphics.rs` traverses Herdr `AppState`, workspace, tab, pane, and runtime structures.

Mitigation:

- Keep it during early stages.
- Later split pure graphics encoding/cache from AppState traversal.

## Validation Matrix

Minimum scenarios to preserve through the fork:

- Start default session, open shell, detach, reattach.
- Create named session, list sessions, attach by name, kill by name.
- Create tab, rename tab, switch tabs with prefix number keys.
- Split pane vertically and horizontally.
- Resize panes and zoom pane.
- Run `nvim` in a pane over SSH attach and verify clipboard behavior.
- OSC 52 from remote child writes to local clipboard.
- OSC 8 hyperlinks remain clickable in supported terminals.
- Bracketed paste into `nvim` works.
- Mouse reporting works in apps that request it.
- Focus reporting works in apps that request it.
- Kitty keyboard sequences reach child apps correctly.
- Kitty graphics either renders correctly or degrades cleanly.
- Slow attached client does not build unbounded render backlog.
- Direct terminal attach still works or is intentionally removed.

## Immediate Next Steps

1. Decide whether to replace repo root with Herdr source or keep `herdr-upstream/` and copy selectively.
2. Run Herdr baseline tests/build locally.
3. Start with mechanical rename to `gmux` while keeping behavior intact.
4. Add a temporary feature flag or config default that disables agent UI.
5. Then begin staged removal from the CLI/docs outward before deleting core types.
