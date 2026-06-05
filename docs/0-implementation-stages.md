**Implementation Stages**

### Stage 0: Baseline Import Decision

Goal: establish the working source tree before making product changes.

Deliverables:

- Decide whether `herdr-upstream/` remains reference-only or Herdr source is copied into repo root.
- Recommended: copy Herdr into repo root and keep `herdr-upstream/` as reference until gmux builds independently.
- Preserve Herdr license notices.
- Confirm baseline build works before edits.

Validation:

- `cargo check`
- `cargo test` if upstream tests are practical
- Run basic Herdr binary locally if build succeeds

Exit criteria:

- Repo root contains buildable source for gmux work.
- `herdr-upstream/` remains untouched reference material.

---

### Stage 1: Mechanical Fork/Rename

Goal: rename Herdr to gmux without changing behavior.

Deliverables:

- Rename crate/package/binary from `herdr` to `gmux`.
- Rename visible product strings.
- Rename env vars:
  - `HERDR_*` to `GMUX_*`
- Rename config/socket paths:
  - `~/.config/herdr` to `~/.config/gmux`
  - `herdr.sock` to `gmux.sock`
- Rename docs/help text enough that the binary presents as gmux.
- Keep internal module names like `workspace` temporarily if changing them would be risky.

Do not:

- Remove agent code yet.
- Collapse workspace model yet.
- Rewrite persistence yet.

Validation:

- `cargo check`
- `cargo test`
- `gmux --help`
- Start local server/client if possible

Exit criteria:

- Binary is called `gmux`.
- Existing behavior still compiles and mostly works.

---

### Stage 2: Product Surface Reduction

Goal: hide agent/product bloat from normal UX while keeping internals compiling.

Deliverables:

- Remove or hide agent-focused help entries from default CLI output.
- Change defaults so gmux opens a plain shell session instead of agent-oriented flows.
- Remove agent/sidebar/status UI elements from default render path where possible.
- Disable agent notifications/sounds in defaults.
- Keep structs/types if removing them would cause wide breakage.

Recommended approach:

- Prefer “unused but still compiled” over “deleted” in this stage.
- Add temporary internal defaults rather than refactoring deep state immediately.

Validation:

- `cargo check`
- Launch gmux into a shell.
- Create panes/tabs using existing mechanisms.
- Detach/reattach still works.

Exit criteria:

- A normal user sees a terminal multiplexer, not an agent product.
- Agent internals may still exist, but they are not primary UX.

---

### Stage 3: tmux-Like Keybindings

Goal: make interaction feel familiar before deeper architecture work.

Deliverables:

- Set default prefix to `Ctrl+b`.
- Add or map common tmux-style bindings:
  - `prefix d`: detach
  - `prefix c`: new tab/window
  - `prefix &`: close tab/window
  - `prefix ,`: rename tab/window
  - `prefix %`: vertical split
  - `prefix "`: horizontal split
  - `prefix x`: close pane
  - `prefix z`: zoom pane
  - `prefix [`: copy/scrollback mode
  - `prefix arrow/hjkl`: pane navigation
  - `prefix n/p`: next/previous tab
  - `prefix 0..9`: tab selection
- Keep existing Herdr bindings only if they do not conflict.

Validation:

- Unit tests for keybinding parser if available.
- Manual smoke test of splits, navigation, detach, copy mode.

Exit criteria:

- gmux is usable with tmux muscle memory even before structural cleanup.

---

### Stage 4: CLI Surface Simplification

Goal: reshape commands around `sessions -> tabs -> panes`.

Deliverables:

- Define target CLI:
  - `gmux`
  - `gmux new [-s name]`
  - `gmux attach [-t name]`
  - `gmux ls`
  - `gmux kill-session [-t name]`
  - `gmux detach`
  - `gmux new-tab`
  - `gmux rename-tab`
  - `gmux split-pane`
  - `gmux kill-pane`
- Remove or hide:
  - `agent`
  - `integration`
  - `worktree`
  - user-visible `workspace` commands
- Preserve remote attach commands if Herdr already has them, but rename semantics to gmux sessions.

Implementation note:

- This stage can still map “session” commands internally onto Herdr workspace APIs.
- Avoid large state rewrites here.

Validation:

- `gmux --help`
- Command smoke tests against a local server.
- Existing attach/detach still works.

Exit criteria:

- Public CLI now speaks gmux concepts.
- Workspace remains internal only.

---

### Stage 5: Session Model Refactor

Goal: collapse Herdr’s visible hierarchy into gmux’s desired hierarchy.

Current Herdr model:

```text
named server/session
  -> workspaces
     -> tabs
        -> panes
```

Target gmux model:

```text
sessions
  -> tabs
     -> panes
```

Deliverables:

- Decide whether to:
  - Rename `Workspace` to `SessionState`, or
  - Keep `Workspace` internally but expose exactly one workspace per session.
- Recommended first step: one workspace per session, then rename later.
- Move session metadata to the top-level gmux session object.
- Remove workspace switching from UX.
- Ensure tabs/panes remain mostly unchanged.

High-risk areas:

- App state
- Server protocol
- Persistence snapshot
- Status/sidebar rendering
- CLI command routing
- Tests that assume workspaces

Validation:

- Create session.
- Attach/detach session.
- Create multiple tabs.
- Split panes.
- Kill panes/tabs.
- Kill session.
- Restart server if persistence is active.

Exit criteria:

- No user-visible workspace layer remains.
- Sessions directly contain tabs and panes from the user’s perspective.

---

### Stage 6: Neutralize Agent State

Goal: remove agent metadata from terminal/session state without yet deleting every source file.

Deliverables:

- Simplify `TerminalState`.
- Remove or stub fields related to:
  - detected agent
  - hook authority
  - persisted agent session
  - pending resume plan
  - agent status rollups
  - agent metadata
- Replace agent-aware UI state with generic pane/tab/session state.
- Remove agent-specific status calculations.
- Make pane titles derive from shell/process/title escape sequences, not agent metadata.

Recommended approach:

- First make fields optional/unused.
- Then remove fields once compile errors are understood.
- Keep changes small and incremental.

Validation:

- `cargo check` after each removal cluster.
- Pane title tests/manual check.
- Render smoke test.
- Persistence smoke test if snapshots still exist.

Exit criteria:

- Core terminal/session state no longer depends on agent concepts.

---

### Stage 7: Delete Agent Subsystems

Goal: remove now-unused agent machinery.

Deliverables:

- Delete or remove module references for:
  - `src/detect/`
  - `src/integration/`
  - `src/agent_resume.rs`
  - `src/app/agent_resume.rs`
  - `src/cli/agent.rs`
  - `src/cli/integration.rs`
  - `src/cli/worktree.rs`
- Remove related dependencies from `Cargo.toml`.
- Remove agent-specific tests/docs.
- Remove related protocol messages only after confirming they are not needed.

Do not remove:

- OSC 52 clipboard handling
- OSC 8 hyperlinks
- Kitty keyboard support
- Kitty graphics support
- bracketed paste
- mouse/focus handling
- raw input transport
- remote attach transport

Validation:

- `cargo check`
- `cargo test`
- Dependency tree review
- Manual shell session smoke test

Exit criteria:

- Agent/product code is actually gone, not just hidden.
- Terminal multiplexer features still work.

---

### Stage 8: Persistence Rewrite

Goal: produce a clean gmux session snapshot format.

Deliverables:

- Define snapshot around:
  - sessions
  - tabs
  - panes
  - cwd
  - command/shell
  - layout tree
  - active tab/pane
  - pane titles
  - scrollback/runtime state if supported
- Remove workspace and agent fields from persisted model.
- Decide whether old Herdr snapshots are ignored or migrated.
- Recommended: no migration unless there is a concrete need.
- Add versioned gmux snapshot format.

Validation:

- Start gmux.
- Create tabs/panes.
- Detach.
- Stop/restart server.
- Reattach.
- Confirm layout restored.
- Confirm no agent/workspace fields are written.

Exit criteria:

- Persistence format is clean gmux-owned state.

---

### Stage 9: Remote Attach Cleanup

Goal: preserve Herdr’s remote/session-manager strengths under gmux naming.

Deliverables:

- Rename remote install/attach paths and commands.
- Remove agent-specific remote bootstrap assumptions.
- Confirm SSH attach path forwards:
  - raw input
  - terminal resize
  - OSC 52 clipboard
  - OSC 8 hyperlinks
  - Kitty keyboard
  - Kitty graphics where possible
- Update env/config names for remote use.

Validation:

- Local attach smoke test.
- SSH attach smoke test if environment is available.
- Clipboard over SSH.
- Hyperlink rendering.
- Bracketed paste.
- Mouse/focus.
- Kitty keyboard sequences.
- Kitty graphics sample.

Exit criteria:

- Remote attach remains a first-class gmux feature.

---

### Stage 10: Terminal Extension Validation

Goal: explicitly protect the reason for using Herdr.

Deliverables:

- Add manual or automated validation fixtures for:
  - OSC 52 clipboard
  - OSC 8 hyperlinks
  - Kitty keyboard protocol
  - Kitty graphics
  - bracketed paste
  - mouse mode
  - focus events
  - cursor shape
  - synchronized output
  - wide Unicode/graphemes
- Document what terminal features require Ghostty/Kitty/WezTerm/etc.
- Add regression notes around the ANSI blitter and semantic frame path.

Validation:

- Run terminal feature matrix locally.
- Confirm no agent removal regressed raw terminal behavior.

Exit criteria:

- gmux has an explicit terminal compatibility checklist.

---

### Stage 11: Docs, Packaging, Polish

Goal: make the fork coherent as a project.

Deliverables:

- Rewrite README around gmux.
- Document:
  - install
  - start/attach/list/kill
  - tabs/panes
  - keybindings
  - config
  - remote attach
  - terminal feature support
- Remove Herdr/agent docs.
- Update examples.
- Update license notices.
- Optional: add shell completions/manpage later.

Validation:

- Fresh build from clean checkout.
- Follow README manually.
- `gmux --help` matches docs.

Exit criteria:

- Project is presentable as gmux, not a partially renamed Herdr fork.

---

**Recommended Landing Order**

1. Baseline import
2. Mechanical rename
3. Hide agent UX
4. tmux keybindings
5. CLI simplification
6. Workspace-to-session collapse
7. Neutralize agent state
8. Delete agent subsystems
9. Rewrite persistence
10. Remote attach cleanup
11. Terminal extension validation
12. Docs/package polish

**Important Principle**

Do not delete agent internals before the public surface is working. First make gmux usable as a plain multiplexer, then collapse state, then delete agent code once compile errors and behavior boundaries are clear.
