# gmux task runner

# Run fast tests for the normal edit/agent feedback loop
test-fast:
    cargo test --locked --bin gmux
    python3 -m unittest scripts.test_vendor_libghostty_vt

# Run process/PTY/socket integration tests
test-integration:
    cargo test --locked --test api_ping
    cargo test --locked --test auto_detect
    cargo test --locked --test cli_wrapper
    cargo test --locked --test client_mode
    cargo test --locked --test cross_area
    cargo test --locked --test detach_reattach
    cargo test --locked --test live_handoff
    cargo test --locked --test multi_client
    cargo test --locked --test server_headless
    cargo test --locked --test terminal_feature_matrix

# Run tests
test: test-fast test-integration

# Run one cargo test filter, e.g. `just test-one codex_stale_working`
test-one filter:
    cargo test --locked "{{filter}}"

# Run fast local lint checks
lint:
    cargo fmt --check
    cargo clippy --all-targets --locked -- -D warnings

# Run local CI checks
ci: lint
    cargo test --locked

# Check formatting + run unit tests + maintenance script tests
check: ci
    python3 -m unittest scripts.test_vendor_libghostty_vt

# Install repo-local git hooks
install-hooks:
    git config core.hooksPath .githooks
    chmod +x .githooks/pre-commit
    chmod +x .githooks/commit-msg
    @echo "installed git hooks from .githooks"

# Build release binary
build:
    cargo build --release --locked

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
