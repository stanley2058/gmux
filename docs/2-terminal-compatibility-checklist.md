# Terminal Compatibility Checklist

Gmux keeps terminal extension support as a core product requirement. Use
this checklist before larger local updates and after changes to rendering, raw input,
remote attach, pane runtime, or client/server protocol code.

The checked feature matrix lives at:

```text
tests/fixtures/terminal_feature_matrix.tsv
```

`tests/terminal_feature_matrix.rs` verifies that every required feature stays
listed with an automated check and local/remote manual validation notes.

## Required Features

| Feature | Requirement | Terminal Notes |
| --- | --- | --- |
| OSC 52 clipboard | Child pane copy requests reach the host clipboard. | Needs host terminal OSC 52 clipboard support or a platform clipboard helper. |
| OSC 8 hyperlinks | Hyperlink URI metadata survives pane rendering and ANSI client output. | Clickability depends on Ghostty, Kitty, WezTerm, iTerm2, or another OSC 8 aware terminal. |
| Kitty keyboard | Modified keys and CSI-u style key sequences are parsed and forwarded correctly. | Best checked in terminals that enable Kitty keyboard protocol. |
| Kitty graphics | Image upload/place/delete state renders or degrades cleanly. | Requires a host terminal with Kitty graphics support and `experimental.kitty_graphics` enabled. |
| Bracketed paste | Multi-line paste reaches pane apps atomically. | Required for shells, editors, and remote sessions. |
| Mouse mode | SGR mouse events reach mouse-reporting pane apps while gmux controls still work. | Check click, drag, wheel, right-click passthrough, and split resizing. |
| Focus events | Focus in/out tracking reaches child apps that request it. | Depends on host terminal focus event support. |
| Cursor shape | Cursor visibility and shape changes survive attach and render paths. | Host terminal must expose cursor shape changes. |
| Synchronized output | Rapid pane output does not visibly tear or leave stale cells. | Check both semantic frames and ANSI blit output. |
| Wide graphemes | Emoji, flags, combining accents, and CJK text render without corrupting adjacent cells. | Requires Unicode-aware rendering and diff invalidation. |

## Smoke Commands

Run the matrix guard:

```bash
cargo test --locked terminal_feature_matrix
```

Run the broader automated suite when you need a full regression pass:

```bash
cargo test --locked
```

For targeted checks, run the relevant filter from the matrix one at a time:

```bash
cargo test --locked clipboard
cargo test --locked keyboard_protocol
cargo test --locked mouse
```

For remote validation, repeat manual checks through:

```bash
gmux --remote <host>
```

Also verify resize, detach/reattach, and clipboard behavior over SSH because the
remote path exercises client-owned desktop features differently from a direct
remote shell running `gmux`.
