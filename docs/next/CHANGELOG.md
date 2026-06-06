# Changelog

## Unreleased

### Added
- Added tmux-style top-level session, tab, and pane commands:
  `gmux new`, `gmux attach`, `gmux ls`, `gmux kill-session`,
  `gmux new-tab`, `gmux list-tabs`, `gmux select-tab`, `gmux rename-tab`,
  `gmux kill-tab`, `gmux split-pane`, `gmux kill-pane`, `gmux send-keys`,
  `gmux send-text`, and `gmux capture-pane`.
- Added a terminal compatibility matrix for release validation, covering OSC 52
  clipboard, OSC 8 hyperlinks, Kitty keyboard input, Kitty graphics, bracketed
  paste, mouse mode, focus events, cursor shape, synchronized output, and wide
  grapheme rendering.
- Added gmux release and preview packaging names for Linux and macOS builds.

### Changed
- Renamed the project, binary, config paths, sockets, environment variables,
  release metadata, and public documentation to gmux.
- Reworked the public model around sessions, tabs, and panes.
- Updated session snapshots to use top-level tabs and pane metadata while keeping
  older saved layouts readable.
- Updated default keybindings to use `ctrl+b` and familiar tmux-style tab and
  pane controls.
- Simplified README and website docs around install, quick start, CLI,
  keybindings, configuration, socket API, persistence, and remote attach.
- Updated Nix, GitHub issue templates, discussion templates, and release
  workflows to use gmux names and artifact paths.

### Removed
- Removed obsolete public command and socket surfaces that do not belong to the
  gmux session, tab, and pane model.
- Removed obsolete public configuration keys and help entries from the default
  user experience.

### Fixed
- Fixed current session persistence fixtures so they represent the current
  tab-based snapshot format instead of an older saved layout shape.
- Fixed remote install guard coverage so remote attach defaults are pinned to
  gmux manifest URLs, binary paths, and environment variables.
