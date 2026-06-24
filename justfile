# gmux task runner

# Run fast tests for the normal edit/agent feedback loop
test-fast:
    cargo test --locked --bin gmux -- --format terse
    python3 -m unittest scripts.test_vendor_libghostty_vt

# Run process/PTY/socket integration tests
test-integration:
    python3 scripts/test_integration_parallel.py

# Run process/PTY/socket integration tests serially for debugging
test-integration-serial:
    cargo test --locked --test api_ping -- --format terse
    cargo test --locked --test auto_detect -- --format terse
    cargo test --locked --test cli_wrapper -- --format terse
    cargo test --locked --test client_mode -- --format terse
    cargo test --locked --test cross_area -- --format terse
    cargo test --locked --test detach_reattach -- --format terse
    cargo test --locked --test live_handoff -- --format terse
    cargo test --locked --test multi_client -- --format terse
    cargo test --locked --test server_headless -- --format terse
    cargo test --locked --test terminal_feature_matrix -- --format terse

# Run tests
test: test-fast test-integration

# Run one cargo test filter, e.g. `just test-one codex_stale_working`
test-one filter:
    cargo test --locked "{{filter}}"

# Benchmark the Ghostty pane render path with a dense ANSI scene.
bench-render-pipeline:
    cargo test --locked render_pipeline_complex_scene_benchmark -- --ignored --nocapture

# Benchmark interaction latency through the headless server render path.
bench-input-latency:
    cargo test --release --locked --test client_mode held_key_live_input_latency_benchmark -- --ignored --nocapture

# Benchmark held-key interaction latency through the in-process server harness.
bench-input-latency-synthetic:
    cargo test --release --locked held_key_interaction_latency_benchmark -- --ignored --nocapture

# Benchmark cursor-only interaction latency over a dense in-process server frame.
bench-cursor-latency-synthetic:
    cargo test --release --locked cursor_motion_interaction_latency_benchmark -- --ignored --nocapture

# Benchmark host scrollback latency for wheel ticks vs PageDown repeats.
bench-host-scroll-latency:
    cargo test --release --locked host_scrollback_input_latency_benchmark -- --ignored --nocapture

# Benchmark copy-mode cursor and viewport movement over complex scrollback.
bench-copy-scrollback-latency:
    cargo test --release --locked complex_copy_mode_scrollback_latency_benchmark -- --ignored --nocapture

# Run fast local lint checks
lint:
    cargo fmt --check
    cargo clippy --all-targets --locked -- -D warnings

# Run local CI checks
ci: lint test

# Check formatting + run tests
check: ci

# Install repo-local git hooks
install-hooks:
    git config core.hooksPath .githooks
    chmod +x .githooks/pre-commit
    chmod +x .githooks/commit-msg
    @echo "installed git hooks from .githooks"

# Build release binary
build:
    cargo build --release --locked

# Bump Cargo version, run fast tests, commit, and tag a release
release version:
    @scripts/release.sh "{{version}}"

# Push the current release commit and tag
release-push version:
    @scripts/release_push.sh "{{version}}"

# Build release binary and install it locally
install:
    cargo build --release --locked
    mkdir -p "${GMUX_INSTALL_DIR:-$HOME/.local/bin}"
    install -m 0755 target/release/gmux "${GMUX_INSTALL_DIR:-$HOME/.local/bin}/gmux"
    scripts/install_completions.sh "${GMUX_INSTALL_DIR:-$HOME/.local/bin}/gmux"
    "${GMUX_INSTALL_DIR:-$HOME/.local/bin}/gmux" --version

# Install shell completions for the current gmux binary
install-completions:
    scripts/install_completions.sh "${GMUX_INSTALL_DIR:-$HOME/.local/bin}/gmux"

# Build the vendored libghostty-vt source dist
build-libghostty-vt:
    scripts/build_vendored_libghostty_vt.sh

# Download local Zig required by vendored libghostty-vt
bootstrap-zig:
    scripts/bootstrap_zig_0_15_2.sh

# Print default config
default-config:
    cargo run --release --locked -- --default-config
