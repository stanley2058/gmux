use std::io;

use crossterm::event::{
    DisableBracketedPaste, DisableFocusChange, DisableMouseCapture, EnableBracketedPaste,
    EnableFocusChange, EnableMouseCapture, PopKeyboardEnhancementFlags,
    PushKeyboardEnhancementFlags,
};
use crossterm::execute;

pub(crate) const GMUX_ENV_VAR: &str = "GMUX_ENV";
pub(crate) const GMUX_ENV_VALUE: &str = "1";
const NESTED_GMUX_MESSAGES: [&str; 6] = [
    "inception detected. we need to go deeper... said no one ever.",
    "recursion is a pathway to many abilities some consider to be... unnatural.",
    "you were so preoccupied with whether you could, you didn't stop to think if you should. — dr. malcolm",
    "recursive gmuxing is disabled. somewhere, a call stack breathes a sigh of relief.",
    "recursive descent denied. there is, in fact, such a thing as too much gmux.",
    "recursion detected. base case not found. aborting.",
];

mod api;
mod app;
mod build_info;
mod cli;
mod client;
mod config;
mod events;
mod ghostty;
mod handoff_runtime;
mod input;
mod ipc;
mod kitty_graphics;
mod layout;
mod logging;
mod pane;
mod persist;
mod platform;
mod protocol;
mod pty;
mod raw_input;
mod remote;
mod render_prof;
mod selection;
mod server;
mod session;
mod terminal;
mod terminal_notify;
mod terminal_theme;
mod ui;
mod workspace;

fn init_logging() {
    crate::logging::init_file_logging("gmux.log");
}

const DEFAULT_CONFIG: &str = r##"# gmux configuration
# Place this file at ~/.config/gmux/config.toml

# Show legacy first-run setup on startup.
# Missing skips setup; set true only if you want the legacy setup flow.
# onboarding = false

[theme]
# Built-in themes: catppuccin, terminal, tokyo-night, dracula, nord,
#                  gruvbox, one-dark, solarized, kanagawa, rose-pine,
#                  vesper
# name = "catppuccin"

# Override individual color tokens on top of the base theme.
# Accepts: hex (#rrggbb), named colors, rgb(r,g,b), or panel_bg = "reset"
# [theme.custom]
# panel_bg = "reset"
# accent = "#f5c2e7"
# red = "#ff6188"
# green = "#a6e3a1"

[terminal]
# Executable used for new interactive panes.
# Empty means $SHELL, then /bin/sh.
# default_shell = ""

# Startup mode for new interactive pane shells: "auto", "login", or "non_login".
# "auto" uses login shells on macOS and keeps the current behavior elsewhere.
# shell_mode = "auto"

# CWD policy for new panes and tabs when no explicit --cwd is provided.
# Use "follow" to inherit the source pane/session, falling back to Gmux's
# process directory; "home" for $HOME; "current" for Gmux's process directory;
# or a fixed path such as "~/Projects".
# new_cwd = "follow"

[keys]
# Prefix key to enter prefix mode (default: "ctrl+b")
# Examples: "ctrl+b", "f12", "esc", "-"
# Action bindings use explicit syntax: "prefix+n" requires the prefix;
# "ctrl+alt+n" is a direct terminal-mode shortcut.
# Accepted key syntax: plain keys, ctrl/shift/alt/cmd/super modifiers, and special keys like enter/tab/esc/left/right/up/down.
# Named punctuation such as minus, comma, ampersand, plus, and backtick is also accepted.
# Most reliable direct bindings are ctrl+letter, function keys, and explicit modified chords.
# alt+..., cmd/super, and punctuation-with-modifiers may depend on your terminal/tmux setup.
# prefix = "ctrl+b"

# Prefix-mode actions
# help = "prefix+?"
# settings = "prefix+s"
# detach = ["prefix+d", "prefix+q"]
# reload_config = "prefix+shift+r"
# open_notification_target = "prefix+o"
# goto = "prefix+g"
# new_tab = "prefix+c"
# rename_tab = ["prefix+comma", "prefix+shift+t"]
# previous_tab = "prefix+p"
# next_tab = "prefix+n"
# switch_tab = "prefix+1..9"
# close_tab = ["prefix+ampersand", "prefix+shift+x"]
# rename_pane = "prefix+shift+p"
# edit_scrollback = "prefix+e"
# focus_pane_left = ["prefix+h", "prefix+left"]
# focus_pane_down = ["prefix+j", "prefix+down"]
# focus_pane_up = ["prefix+k", "prefix+up"]
# focus_pane_right = ["prefix+l", "prefix+right"]
# cycle_pane_next = "prefix+tab"
# cycle_pane_previous = "prefix+shift+tab"
# last_pane = ""          # optional, unset by default; bind e.g. "prefix+tab" for global back-and-forth
# split_vertical = ["prefix+percent", "prefix+v"]
# split_horizontal = ["prefix+double_quote", "prefix+minus"]
# close_pane = "prefix+x"
# zoom = "prefix+z"       # legacy alias: fullscreen
# resize_mode = "prefix+r"
# toggle_sidebar = "prefix+b"

# Navigate-mode movement. These local shortcuts win while navigate mode is open.
# They are independent from focus_pane_*. Do not include prefix+, esc, enter, tab, or 1..9 here.
# navigate_pane_left = "h"      # left arrow always focuses the pane to the left
# navigate_pane_down = "j"
# navigate_pane_up = "k"
# navigate_pane_right = "l"     # right arrow always focuses the pane to the right

# Custom commands use the same binding syntax.
# type = "shell" runs detached in the background.
# type = "pane" opens a temporary pane and closes it when the command exits.
# [[keys.command]]
# key = "prefix+alt+g"
# type = "pane"
# command = "lazygit"

# Legacy indexed shortcut config is still parsed for compatibility.
# Prefer switch_tab for new configs.
# [keys.indexed]
# tabs = ""       # e.g. "ctrl" makes ctrl+1..9 switch tabs directly

[ui]
# Sidebar width (auto-scaled based on session item names, this sets the default)
# sidebar_width = 26

# Minimum sidebar width when expanded (columns)
# sidebar_min_width = 18

# Maximum sidebar width when expanded (columns)
# sidebar_max_width = 36

# Terminal width at or below which Gmux uses the mobile single-column layout.
# Increase this for foldables, tablets, or wide phone terminals.
# mobile_width_threshold = 64

# Capture mouse input for Gmux's mouse UI.
# Set false to let the terminal handle normal clicks, such as Cmd-clicking URLs.
# Pane apps like lazygit and btop can still receive mouse when they request it.
# mouse_capture = true

# Optional modifier that forwards right-click hold/drag gestures to pane apps instead of opening Gmux's pane menu.
# Empty/off disables this. Shift is intentionally unsupported because terminals commonly reserve Shift+mouse.
# right_click_passthrough_modifier = ""

# Force a full redraw when the outer terminal regains focus.
# Set false to reduce visible flashing when switching back to Gmux.
# Trade-off: rare host terminal surface corruption may persist until the next full redraw.
# redraw_on_focus_gained = true

# Pane scrollback lines to scroll per mouse wheel notch.
# mouse_scroll_lines = 3

# Ask for confirmation before closing a session item.
# confirm_close = true

# Ask for a tab name before creating a new tab.
# Set false to create tabs immediately with generated names.
# prompt_new_tab_name = true

# Accent color for highlights, borders, and navigation UI.
# Accepts: hex (#89b4fa), named colors (cyan, blue, magenta), or rgb(r,g,b)
# accent = "cyan"

# Background notification popup delivery
[ui.toast]
# off = disable pop-up notifications
# gmux = show top-right in-app toasts
# terminal = ask the outer terminal to show a desktop notification
# system = ask the OS notification service directly
# delivery = "off"

[remote]
# Whether gmux manages the ssh config used for the `gmux --remote` bridge.
# When true (default), gmux runs the bridge ssh through a generated config that
# includes your ~/.ssh/config first and adds ServerAliveInterval/
# ServerAliveCountMax as a fallback (so any keepalive you set yourself still
# wins) to survive idle network/NAT timeouts. Set false to run plain ssh against
# your ssh config unchanged — this does not force keepalive off, it only stops
# gmux from adding its own.
# manage_ssh_config = true

[experimental]
# Allow launching gmux from inside a gmux-managed pane.
# allow_nested = false
# Experimental local Kitty graphics rendering for attached clients.
# Requires a Kitty graphics-compatible outer terminal.
# kitty_graphics = false
# Save recent pane screen history across full server restarts.
pane_history = false
# While prefix mode is active, temporarily switch the macOS host input
# source to an ASCII-capable keyboard layout so prefix commands register
# even when a CJK IME is active, then restore the previous input source
# when prefix mode exits. macOS only; best-effort. Default: false.
# switch_ascii_input_source_in_prefix = false
# Expose the focused pane's cursor to the outer terminal so macOS input
# methods keep tracking the candidate window when TUIs paint their own
# cursor. Trade-off: extra cursor visible for apps that hide it without
# painting a replacement (vim normal mode, etc.).
# reveal_hidden_cursor_for_cjk_ime = false
# Cursor shape rendered when reveal_hidden_cursor_for_cjk_ime is true.
# Values: block, steady_block (default), underline, steady_underline, bar, steady_bar.
# cjk_ime_cursor_shape = "steady_block"

[advanced]
# Maximum scrollback buffer size in bytes retained per pane terminal.
# Matches Ghostty's default scrollback-limit behavior.
# scrollback_limit_bytes = 10000000
"##;

fn should_block_nested(config: &config::Config) -> bool {
    should_block_nested_for_env(config, std::env::var(GMUX_ENV_VAR).ok().as_deref())
}

fn should_block_nested_for_env(config: &config::Config, gmux_env: Option<&str>) -> bool {
    !config.experimental.allow_nested && gmux_env == Some(GMUX_ENV_VALUE)
}

fn random_nested_message() -> &'static str {
    use std::time::{SystemTime, UNIX_EPOCH};

    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.subsec_nanos() as usize)
        .unwrap_or(0);
    let index = (nanos ^ (std::process::id() as usize)) % NESTED_GMUX_MESSAGES.len();
    NESTED_GMUX_MESSAGES[index]
}

fn exit_if_nested_disabled(config: &config::Config) {
    if should_block_nested(config) {
        eprintln!("\x1b[1merror:\x1b[0m nested gmux is disabled by default.");
        eprintln!("see configuration if you want to enable it.");
        eprintln!();
        eprintln!("\x1b[2m\"{}\"\x1b[0m", random_nested_message());
        std::process::exit(1);
    }
}

fn main() -> io::Result<()> {
    let raw_args: Vec<String> = std::env::args().collect();
    let args = match session::configure_from_args(&raw_args) {
        Ok(args) => args,
        Err(err) => {
            eprintln!("error: {err}");
            eprintln!("run 'gmux --help' for usage");
            std::process::exit(2);
        }
    };
    let (args, remote_launch) = match remote::extract_remote_args(&args) {
        Ok(parsed) => parsed,
        Err(err) => {
            eprintln!("error: {err}");
            eprintln!("run 'gmux --help' for usage");
            std::process::exit(2);
        }
    };

    if remote_launch.is_some()
        && args.get(1).is_some()
        && !args.iter().any(|a| {
            matches!(
                a.as_str(),
                "--help" | "-h" | "--version" | "-V" | "--default-config"
            )
        })
    {
        eprintln!("error: --remote can only be used with the default launch command");
        eprintln!("run 'gmux --help' for usage");
        std::process::exit(2);
    }

    if let cli::CommandOutcome::Handled(code) = cli::maybe_run(&args)? {
        std::process::exit(code);
    }

    // Subcommands and flags (no TUI, no logging needed)
    if args.get(1).map(|s| s.as_str()) == Some("remote-client-bridge") {
        return remote::run_remote_client_bridge();
    }

    if args.get(1).map(|s| s.as_str()) == Some("server") {
        return server::headless::run_server();
    }

    // Hidden client mode: connect to an existing server's client socket.
    if args.get(1).map(|s| s.as_str()) == Some("client") {
        let loaded_config = config::Config::load();
        exit_if_nested_disabled(&loaded_config.config);
        return client::run_client();
    }

    if args.iter().any(|a| a == "--help" || a == "-h") {
        println!("gmux — terminal multiplexer and session manager");
        println!();
        println!("Usage: gmux [options]");
        println!("       gmux new [-s name]");
        println!("       gmux attach [-t name]");
        println!("       gmux --session <name> [options]");
        println!("       gmux --remote <ssh-target> [--session <name>]");
        println!("       gmux ls [--json]");
        println!("       gmux kill-session [-t name] [--json]");
        println!("       gmux detach");
        println!("       gmux list-tabs");
        println!("       gmux new-tab [-n name] [-c cwd]");
        println!("       gmux select-tab [-t tab]");
        println!("       gmux rename-tab [-t tab] <name>");
        println!("       gmux kill-tab [-t tab]");
        println!("       gmux capture-pane [-t pane] [-S lines] [-e]");
        println!("       gmux select-pane [-L|-R|-U|-D|-t pane]");
        println!("       gmux resize-pane [-L|-R|-U|-D [N]]");
        println!("       gmux send-text [-t pane] <text>");
        println!("       gmux send-keys [-t pane] <key> [key ...]");
        println!("       gmux split-pane [-t pane] [-h|-v] [command ...]");
        println!("       gmux kill-pane [-t pane]");
        println!("       gmux server stop");
        println!("       gmux server reload-config");
        println!("       gmux config <subcommand> ...");
        println!("       gmux tab <subcommand> ...");
        println!("       gmux pane <subcommand> ...");
        println!("       gmux session <subcommand> ...");
        println!();
        println!("Common commands:");
        for (command, description) in [
            ("gmux", "Launch or attach to the persistent session"),
            ("gmux new [-s name]", "Create or attach to a named session"),
            ("gmux attach [-t name]", "Attach to a named session"),
            ("gmux ls", "List persistent sessions"),
            ("gmux kill-session", "Stop the default or targeted session"),
            ("gmux detach", "Show the detach keybinding"),
            ("gmux list-tabs", "List tabs in the session"),
            ("gmux new-tab", "Create and focus a tab"),
            ("gmux select-tab -t tab", "Focus a tab"),
            ("gmux kill-tab -t tab", "Close a tab"),
            ("gmux capture-pane", "Print pane output"),
            ("gmux select-pane", "Focus a pane"),
            ("gmux resize-pane", "Resize the focused pane"),
            ("gmux send-text", "Send text to a pane"),
            ("gmux send-keys", "Send key names to a pane"),
            ("gmux split-pane", "Split the focused or targeted pane"),
            ("gmux kill-pane", "Close the focused or targeted pane"),
            (
                "gmux status [server|client]",
                "Show local client and running server status",
            ),
            (
                "gmux server stop",
                "Stop the running server via the API socket",
            ),
            (
                "gmux server reload-config",
                "Reload config.toml in the running server",
            ),
            (
                "gmux config reset-keys",
                "Back up config.toml and remove custom keybindings",
            ),
            (
                "gmux session <subcommand>",
                "Manage named persistent sessions",
            ),
        ] {
            println!("  {command:<32} {description}");
        }
        println!();
        println!("Advanced commands:");
        println!("  {:<32} Run as headless server", "gmux server");
        println!();
        println!("Options:");
        println!("  --no-session        Run monolithically (no server/client, escape hatch)");
        println!("  --session <name>    Use or create a named persistent session");
        println!("  --remote <target>   Attach through SSH to a remote Gmux server");
        println!("  --remote-keybindings <local|server>");
        println!("                      Keybindings for --remote app attach (default: local)");
        println!("  --handoff           Opt into live handoff for remote attach");
        println!("  --default-config    Print default configuration and exit");
        println!("  --version, -V       Print version and exit");
        println!("  --help, -h          Show this help");
        println!();
        println!("Config: {}", config::config_path().display());
        println!("Logs:   {}", logging::help_log_paths_summary());
        println!("Env:    GMUX_CONFIG_PATH overrides config file path");
        println!("Home:   https://github.com/stanley2058/gmux");
        return Ok(());
    }

    if args.iter().any(|a| a == "--version" || a == "-V") {
        println!("gmux {}", crate::build_info::version());
        return Ok(());
    }

    if args.iter().any(|a| a == "--default-config") {
        print!("{DEFAULT_CONFIG}");
        return Ok(());
    }

    // Reject unknown flags
    let known_flags = [
        "--no-session",
        "--session",
        "--remote",
        "--remote-keybindings",
        "--version",
        "-V",
        "--default-config",
        "--help",
        "-h",
    ];
    for arg in &args[1..] {
        let arg_name = arg.split_once('=').map(|(name, _)| name).unwrap_or(arg);
        if arg.starts_with('-') && !known_flags.contains(&arg_name) {
            eprintln!("unknown option: {arg}");
            eprintln!("run 'gmux --help' for usage");
            std::process::exit(1);
        }
        if !arg.starts_with('-')
            && ![
                "server",
                "client",
                "remote-client-bridge",
                "new",
                "attach",
                "ls",
                "kill-session",
                "detach",
                "list-tabs",
                "new-tab",
                "select-tab",
                "rename-tab",
                "kill-tab",
                "capture-pane",
                "select-pane",
                "resize-pane",
                "send-text",
                "send-keys",
                "split-pane",
                "kill-pane",
                "status",
                "config",
                "pane",
                "wait",
                "session",
            ]
            .contains(&arg.as_str())
        {
            eprintln!("unknown command: {arg}");
            eprintln!("run 'gmux --help' for usage");
            std::process::exit(1);
        }
    }

    if let Some(remote_launch) = remote_launch {
        return remote::run_remote(remote_launch);
    }

    let loaded_config = config::Config::load();
    exit_if_nested_disabled(&loaded_config.config);

    let no_session = args.iter().any(|a| a == "--no-session");

    // Auto-detect launch: when --no-session is NOT set, use server/client mode.
    // Check if a server is running, spawn one if needed, then attach as client.
    if !no_session {
        if let Err(err) = server::autodetect::auto_detect_launch() {
            eprintln!("gmux: {err}");
            std::process::exit(1);
        }
        return Ok(());
    }

    // --- Monolithic mode (--no-session escape hatch) ---
    // This is the pre-mission single-process behavior.

    init_logging();

    let (api_tx, api_rx) = tokio::sync::mpsc::unbounded_channel();
    let event_hub = api::EventHub::default();
    let _api_server = match api::start_server_with_capabilities(api_tx, event_hub.clone(), None) {
        Ok(server) => server,
        Err(err) if err.kind() == io::ErrorKind::AddrInUse => {
            eprintln!("error: gmux is already running");
            eprintln!("socket: {}", api::socket_path().display());
            std::process::exit(1);
        }
        Err(err) => return Err(err),
    };

    let modify_other_keys_mode = crate::input::host_modify_other_keys_mode(
        std::env::var("TMUX").is_ok(),
        std::env::var("TERM_PROGRAM").ok().as_deref(),
        std::env::var_os("WEZTERM_PANE").is_some(),
    );

    let original_hook = std::panic::take_hook();
    let panic_resets_modify_other_keys = modify_other_keys_mode.is_some();
    std::panic::set_hook(Box::new(move |info| {
        tracing::error!("PANIC: {info}");
        if panic_resets_modify_other_keys {
            let _ = std::io::Write::write_all(&mut io::stdout(), b"\x1b[>4;0m");
        }
        if crate::kitty_graphics::is_enabled() {
            let _ = crate::kitty_graphics::clear_all_host_graphics();
        }
        let _ = execute!(
            io::stdout(),
            PopKeyboardEnhancementFlags,
            DisableFocusChange,
            DisableBracketedPaste,
            DisableMouseCapture
        );
        ratatui::restore();
        original_hook(info);
    }));

    let config = &loaded_config.config;
    let config_diagnostic = config::config_diagnostic_summary(&loaded_config.diagnostics);
    logging::startup("app");

    let rt = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
        .expect("failed to create tokio runtime");

    let result = rt.block_on(async {
        let mut terminal = ratatui::init();
        if config.ui.mouse_capture {
            execute!(io::stdout(), EnableMouseCapture)?;
        } else {
            execute!(io::stdout(), DisableMouseCapture)?;
        }
        execute!(
            io::stdout(),
            EnableBracketedPaste,
            EnableFocusChange,
            PushKeyboardEnhancementFlags(crate::input::ime_compatible_keyboard_enhancement_flags())
        )?;

        // Some hosts do not honor Kitty keyboard enhancement pushes for
        // Shift+Enter. Enable xterm modifyOtherKeys only on hosts where we
        // know it is needed and parseable, so modified Enter stays distinct.
        if let Some(mode) = modify_other_keys_mode {
            use std::io::Write;
            std::io::stdout().write_all(mode.set_sequence())?;
            std::io::stdout().flush()?;
        }

        let mut app = app::App::new(
            config,
            true, // no_session — monolithic mode never saves/restores sessions
            config_diagnostic,
            api_rx,
            event_hub,
        );
        let result = app.run(&mut terminal).await;

        // Reset modifyOtherKeys if we enabled it.
        if modify_other_keys_mode.is_some() {
            use std::io::Write;
            std::io::stdout().write_all(b"\x1b[>4;0m")?;
            std::io::stdout().flush()?;
        }

        if crate::kitty_graphics::is_enabled() {
            crate::kitty_graphics::clear_all_host_graphics()?;
        }
        execute!(
            io::stdout(),
            PopKeyboardEnhancementFlags,
            DisableFocusChange,
            DisableBracketedPaste,
            DisableMouseCapture
        )?;
        ratatui::restore();

        // Drop app (and all session panes) before runtime shuts down
        drop(app);

        result
    });

    // Shut down runtime immediately — kills lingering PTY reader/writer tasks
    rt.shutdown_timeout(std::time::Duration::from_millis(100));

    logging::shutdown("app");
    result
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn nested_gmux_blocks_when_env_is_set() {
        let config = config::Config::default();
        assert!(should_block_nested_for_env(&config, Some(GMUX_ENV_VALUE)));
    }

    #[test]
    fn nested_gmux_does_not_block_when_allowed() {
        let config: config::Config =
            toml::from_str("[experimental]\nallow_nested = true\n").unwrap();
        assert!(!should_block_nested_for_env(&config, Some(GMUX_ENV_VALUE)));
    }

    #[test]
    fn nested_gmux_does_not_block_without_env() {
        let config = config::Config::default();
        assert!(!should_block_nested_for_env(&config, None));
    }

    #[test]
    fn random_nested_message_comes_from_known_set() {
        let message = random_nested_message();
        assert!(NESTED_GMUX_MESSAGES.contains(&message));
    }

    #[test]
    fn nested_message_strings_no_longer_repeat_gmux_prefix() {
        assert!(NESTED_GMUX_MESSAGES
            .iter()
            .all(|message| !message.starts_with("gmux:")));
    }
}
