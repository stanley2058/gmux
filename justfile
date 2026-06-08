# gmux task runner

# Run tests
test:
    cargo test --locked
    python3 -m unittest scripts.test_vendor_libghostty_vt

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
