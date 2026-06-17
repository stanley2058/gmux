# gmux

Terminal multiplexer and session manager. Rust + ratatui.

## Principles

- **State is separated from runtime.** `AppState` is pure data, testable without PTYs or async. `PaneState` is separate from `PaneRuntime`. Workspace logic should not need real terminals.
- **Render is pure.** `compute_view()` handles geometry and mutations. `render()` takes `&AppState` and only draws. Never mutate state during render.
- **No god objects.** If a module is doing too many things, split it. `app/` is already split into state, actions, and input. Keep it that way.
- **Platform code is isolated.** OS-specific behavior lives in `src/platform/`. Core modules should not grow scattered `#[cfg(target_os)]` branches.
- **Detection is evidence-based.** When changing screen detection, first capture representative pane output with `gmux pane read <pane_id> --source recent --format text` and, when styling or alternate screen behavior matters, `--format ansi`. Match stable visible controls, not incidental whole-pane text.
- **UI patterns should be reused.** Gmux is a mouse-first TUI. New dialogs, onboarding, settings, and overlays should follow the existing UI language and interaction patterns.
- **Input latency is sacred.** Keyboard and mouse input must not be blocked by rendering, serialization, mirror clients, or slow client writers. Active-client input should flush immediately.
- **Active clients win.** In multi-client rendering, the active client must behave like a single client as much as possible. Background clients may receive deferred, dropped, or slightly delayed updates rather than increasing active-client frame time.
- **Performance changes need evidence.** Rendering pipeline changes should preserve or improve active-client `input->frame`, frame interval, server render/build/prepare/serialize/send timing, and queue-depth metrics. If adding work to the hot path, instrument it or justify why it is negligible.

## Development

Use `just` recipes by default:

```bash
just test-fast
just test
just check
```

Use `just test-fast` for the normal edit/agent feedback loop; it runs in-source Rust tests plus maintenance tests without spawning the full process/PTY/socket integration suite. Use `just test-integration` for the parallel integration tier, `just test-integration-serial` when debugging order-sensitive integration failures, and `just test` for the full test suite.

Run `just check` before committing unless a narrower validation is explicitly acceptable. Do not bypass failing checks; fix the failure or explain exactly why the narrower check is enough.

Unit tests live next to the code (`#[cfg(test)] mod tests`). New `AppState` or `Workspace` behavior should be testable with `AppState::test_new()` and `Workspace::test_new()` without PTYs.

The vendored `libghostty-vt` build requires Zig 0.15.2. If `vendor/zig-0.15.2/zig` is absent, run `just bootstrap-zig` before `just check`. The downloaded Zig tree is intentionally gitignored; `build.rs` uses it automatically when `ZIG` is unset.

## Vendored libghostty-vt

`vendor/libghostty-vt.vendor.json` records the upstream source commit currently vendored.

Local patches on top of the vendored source must be tracked in `vendor/libghostty-vt.patches.md` and stored as patch files under `vendor/patches/libghostty-vt/`. Each entry should say why the patch exists, the touched files, verification, and the exact removal condition.

When updating libghostty-vt, check every active patch in `vendor/libghostty-vt.patches.md`. If the new upstream commit contains the fix, remove the local patch and index entry, then rerun the listed verification. If not, reapply the patch on top of the new vendored source.

`just check` runs maintenance tests that verify local libghostty-vt patch files are listed in the index and reverse-apply cleanly against the vendored tree. Do not leave a patch file untracked or an indexed patch unapplied.

## Code Conventions

- Rust: no `unwrap()` in production code. Use `tracing` for logging. Use `#[allow]` only with a comment explaining why.
- Do not add dependencies without a reason. Check whether existing dependencies cover the need first.
- When changing the server/client wire protocol, update `src/protocol/wire.rs::PROTOCOL_VERSION` only if the current source protocol is not already greater than the latest compatible protocol. Update hardcoded protocol expectations and manual protocol fixtures in tests.
- Keep documentation local and source-oriented. Prefer instructions that match the workflows maintained in this repository.

## Commit Style

Use lowercase conventional commits, no emojis, and no AI co-author lines.
