#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ValueKind {
    Session,
    Pane,
    Tab,
    ReadSource,
    ReadFormat,
    SplitDirection,
    PaneDirection,
    RemoteKeybindings,
    Opaque,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct OptionSpec {
    pub(crate) names: &'static [&'static str],
    pub(crate) description: &'static str,
    pub(crate) value: Option<ValueKind>,
}

impl OptionSpec {
    pub(crate) const fn flag(names: &'static [&'static str], description: &'static str) -> Self {
        Self {
            names,
            description,
            value: None,
        }
    }

    pub(crate) const fn value(
        names: &'static [&'static str],
        description: &'static str,
        value: ValueKind,
    ) -> Self {
        Self {
            names,
            description,
            value: Some(value),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct CommandSpec {
    pub(crate) name: &'static str,
    pub(crate) description: &'static str,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[cfg(test)]
pub(crate) struct PublicCommandSpec {
    pub(crate) path: &'static [&'static str],
    pub(crate) usage: &'static str,
    pub(crate) completion_required: bool,
}

pub(crate) const GLOBAL_SESSION_OPTION: OptionSpec = OptionSpec::value(
    &["--session"],
    "Use or create a named session",
    ValueKind::Session,
);

pub(crate) const HELP_OPTION: OptionSpec = OptionSpec::flag(&["--help"], "Show help");

pub(crate) const TOP_LEVEL_COMMANDS: &[CommandSpec] = &[
    CommandSpec::new("new", "Create or attach to a named session"),
    CommandSpec::new("attach", "Attach to a named session"),
    CommandSpec::new("ls", "List sessions"),
    CommandSpec::new("kill-session", "Stop a session"),
    CommandSpec::new("detach", "Show detach keybinding"),
    CommandSpec::new("status", "Show client/server status"),
    CommandSpec::new("server", "Manage the gmux server"),
    CommandSpec::new("session", "Manage sessions"),
    CommandSpec::new("tab", "Manage tabs"),
    CommandSpec::new("pane", "Manage panes"),
    CommandSpec::new("terminal", "Attach to a terminal"),
    CommandSpec::new("wait", "Wait for events"),
    CommandSpec::new("config", "Manage config"),
    CommandSpec::new("list-tabs", "List tabs"),
    CommandSpec::new("new-tab", "Create a tab"),
    CommandSpec::new("select-tab", "Focus a tab"),
    CommandSpec::new("rename-tab", "Rename a tab"),
    CommandSpec::new("kill-tab", "Close a tab"),
    CommandSpec::new("capture-pane", "Print pane output"),
    CommandSpec::new("select-pane", "Focus a pane"),
    CommandSpec::new("resize-pane", "Resize focused pane"),
    CommandSpec::new("send-text", "Send text to a pane"),
    CommandSpec::new("send-keys", "Send key names to a pane"),
    CommandSpec::new("split-pane", "Split a pane"),
    CommandSpec::new("kill-pane", "Close a pane"),
    CommandSpec::new("completions", "Generate shell completions"),
    CommandSpec::new("completion", "Generate shell completions"),
];

#[cfg(test)]
pub(crate) const PUBLIC_COMMANDS: &[PublicCommandSpec] = &[
    PublicCommandSpec::new(&["new"], "gmux new [-s name]"),
    PublicCommandSpec::new(&["attach"], "gmux attach [-t name]"),
    PublicCommandSpec::new(&["ls"], "gmux ls [--json]"),
    PublicCommandSpec::new(&["kill-session"], "gmux kill-session [-t name] [--json]"),
    PublicCommandSpec::new(&["detach"], "gmux detach"),
    PublicCommandSpec::new(&["status"], "gmux status [server|client] [--json]"),
    PublicCommandSpec::new(&["status", "server"], "gmux status server [--json]"),
    PublicCommandSpec::new(&["status", "client"], "gmux status client [--json]"),
    PublicCommandSpec::new(&["server"], "gmux server <subcommand>"),
    PublicCommandSpec::new(&["server", "stop"], "gmux server stop"),
    PublicCommandSpec::new(
        &["server", "live-handoff"],
        "gmux server live-handoff [--all-sessions] [--import-exe PATH] [--expected-protocol N] [--expected-version VERSION]",
    ),
    PublicCommandSpec::new(&["server", "reload-config"], "gmux server reload-config"),
    PublicCommandSpec::new(&["session"], "gmux session <subcommand>"),
    PublicCommandSpec::new(&["session", "list"], "gmux session list [--json]"),
    PublicCommandSpec::new(&["session", "attach"], "gmux session attach <name>"),
    PublicCommandSpec::new(&["session", "stop"], "gmux session stop <name> [--json]"),
    PublicCommandSpec::new(&["session", "rename"], "gmux session rename <old> <new> [--json]"),
    PublicCommandSpec::new(&["session", "delete"], "gmux session delete <name> [--json]"),
    PublicCommandSpec::new(&["tab"], "gmux tab <subcommand>"),
    PublicCommandSpec::new(&["tab", "list"], "gmux tab list"),
    PublicCommandSpec::new(
        &["tab", "create"],
        "gmux tab create [--cwd PATH] [--label TEXT] [--focus] [--no-focus] [command ...]",
    ),
    PublicCommandSpec::new(&["tab", "get"], "gmux tab get <tab_id>"),
    PublicCommandSpec::new(&["tab", "focus"], "gmux tab focus <tab_id>"),
    PublicCommandSpec::new(&["tab", "rename"], "gmux tab rename <tab_id> <label>"),
    PublicCommandSpec::new(&["tab", "close"], "gmux tab close <tab_id>"),
    PublicCommandSpec::new(&["pane"], "gmux pane <subcommand>"),
    PublicCommandSpec::new(&["pane", "list"], "gmux pane list"),
    PublicCommandSpec::new(&["pane", "get"], "gmux pane get <pane_id>"),
    PublicCommandSpec::new(
        &["pane", "read"],
        "gmux pane read <pane_id> [--source visible|recent|recent-unwrapped] [--lines N] [--format text|ansi] [--ansi] [--raw]",
    ),
    PublicCommandSpec::new(&["pane", "rename"], "gmux pane rename <pane_id> <label>|--clear"),
    PublicCommandSpec::new(
        &["pane", "split"],
        "gmux pane split <pane_id> --direction right|down [--cwd PATH] [--focus] [--no-focus] [command ...]",
    ),
    PublicCommandSpec::new(
        &["pane", "popup"],
        "gmux pane popup <pane_id> [--width N|N%] [--height N|N%] [--x N|N%|C] [--y N|N%|C] [--cwd PATH] [--focus] [--no-focus] [command ...]",
    ),
    PublicCommandSpec::new(&["pane", "close"], "gmux pane close <pane_id>"),
    PublicCommandSpec::new(&["pane", "send-text"], "gmux pane send-text <pane_id> <text>"),
    PublicCommandSpec::new(&["pane", "send-keys"], "gmux pane send-keys <pane_id> <key> [key ...]"),
    PublicCommandSpec::new(&["pane", "run"], "gmux pane run <pane_id> <command>"),
    PublicCommandSpec::new(&["terminal"], "gmux terminal <subcommand>"),
    PublicCommandSpec::new(&["terminal", "attach"], "gmux terminal attach <terminal_id> [--takeover]"),
    PublicCommandSpec::new(&["wait"], "gmux wait <subcommand>"),
    PublicCommandSpec::new(
        &["wait", "output"],
        "gmux wait output <pane_id> --match <text> [--source visible|recent|recent-unwrapped] [--lines N] [--timeout MS] [--regex] [--raw]",
    ),
    PublicCommandSpec::new(&["config"], "gmux config <subcommand>"),
    PublicCommandSpec::new(&["config", "reset-keys"], "gmux config reset-keys"),
    PublicCommandSpec::new(&["list-tabs"], "gmux list-tabs"),
    PublicCommandSpec::new(
        &["new-tab"],
        "gmux new-tab [-n name] [-c cwd] [--label TEXT] [--focus|--no-focus] [command ...]",
    ),
    PublicCommandSpec::new(&["select-tab"], "gmux select-tab [-t tab]"),
    PublicCommandSpec::new(&["rename-tab"], "gmux rename-tab [-t tab] <name>"),
    PublicCommandSpec::new(&["kill-tab"], "gmux kill-tab [-t tab]"),
    PublicCommandSpec::new(
        &["capture-pane"],
        "gmux capture-pane [-t pane] [-S lines] [-e|--ansi] [--raw] [--source visible|recent|recent-unwrapped] [--format text|ansi]",
    ),
    PublicCommandSpec::new(&["select-pane"], "gmux select-pane [-L|-R|-U|-D|-t pane]"),
    PublicCommandSpec::new(&["resize-pane"], "gmux resize-pane [-L|-R|-U|-D [N]] [--amount N]"),
    PublicCommandSpec::new(&["send-text"], "gmux send-text [-t pane] <text>"),
    PublicCommandSpec::new(&["send-keys"], "gmux send-keys [-t pane] <key> [key ...]"),
    PublicCommandSpec::new(
        &["split-pane"],
        "gmux split-pane [-t pane] [-h|-v|--direction right|down] [-c|--cwd PATH] [--focus|--no-focus] [command ...]",
    ),
    PublicCommandSpec::new(&["kill-pane"], "gmux kill-pane [-t pane]"),
    PublicCommandSpec::new(&["completions"], "gmux completions <bash|zsh|fish>"),
    PublicCommandSpec::new(&["completion"], "gmux completion <bash|zsh|fish>"),
];

pub(crate) const SESSION_COMMANDS: &[CommandSpec] = &[
    CommandSpec::new("list", "List sessions"),
    CommandSpec::new("attach", "Attach to a session"),
    CommandSpec::new("stop", "Stop a session"),
    CommandSpec::new("rename", "Rename a stopped session"),
    CommandSpec::new("delete", "Delete a stopped session"),
    CommandSpec::new("help", "Show session help"),
];

pub(crate) const SERVER_COMMANDS: &[CommandSpec] = &[
    CommandSpec::new("stop", "Stop the server"),
    CommandSpec::new("live-handoff", "Hand off live panes"),
    CommandSpec::new("reload-config", "Reload config.toml"),
    CommandSpec::new("help", "Show server help"),
];

pub(crate) const TAB_COMMANDS: &[CommandSpec] = &[
    CommandSpec::new("list", "List tabs"),
    CommandSpec::new("create", "Create a tab"),
    CommandSpec::new("get", "Show tab details"),
    CommandSpec::new("focus", "Focus a tab"),
    CommandSpec::new("rename", "Rename a tab"),
    CommandSpec::new("close", "Close a tab"),
    CommandSpec::new("help", "Show tab help"),
];

pub(crate) const PANE_COMMANDS: &[CommandSpec] = &[
    CommandSpec::new("list", "List panes"),
    CommandSpec::new("get", "Show pane details"),
    CommandSpec::new("read", "Read pane output"),
    CommandSpec::new("rename", "Rename a pane"),
    CommandSpec::new("split", "Split a pane"),
    CommandSpec::new("popup", "Open a floating popup pane"),
    CommandSpec::new("close", "Close a pane"),
    CommandSpec::new("send-text", "Send literal text"),
    CommandSpec::new("send-keys", "Send key names"),
    CommandSpec::new("run", "Run command in pane"),
    CommandSpec::new("help", "Show pane help"),
];

pub(crate) const WAIT_COMMANDS: &[CommandSpec] = &[
    CommandSpec::new("output", "Wait for pane output"),
    CommandSpec::new("help", "Show wait help"),
];

pub(crate) const TERMINAL_COMMANDS: &[CommandSpec] = &[
    CommandSpec::new("attach", "Attach directly to terminal"),
    CommandSpec::new("help", "Show terminal help"),
];

pub(crate) const CONFIG_COMMANDS: &[CommandSpec] = &[
    CommandSpec::new("reset-keys", "Remove custom keybindings"),
    CommandSpec::new("help", "Show config help"),
];

pub(crate) const SHELL_COMMANDS: &[CommandSpec] = &[
    CommandSpec::new("bash", "Generate bash completions"),
    CommandSpec::new("zsh", "Generate zsh completions"),
    CommandSpec::new("fish", "Generate fish completions"),
];

pub(crate) const GLOBAL_OPTIONS: &[OptionSpec] = &[
    OptionSpec::flag(&["--no-session"], "Run without persistent session"),
    GLOBAL_SESSION_OPTION,
    OptionSpec::value(&["--remote"], "Attach through SSH", ValueKind::Opaque),
    OptionSpec::value(
        &["--remote-keybindings"],
        "Remote keybinding mode",
        ValueKind::RemoteKeybindings,
    ),
    OptionSpec::flag(&["--handoff"], "Opt into live handoff"),
    OptionSpec::flag(&["--default-config"], "Print default config"),
    OptionSpec::flag(&["--version", "-V"], "Print version"),
    OptionSpec::flag(&["--help", "-h"], "Show help"),
];

pub(crate) const JSON_OPTIONS: &[OptionSpec] = &[OptionSpec::flag(&["--json"], "Print JSON")];

pub(crate) const SESSION_NAME_OPTIONS: &[OptionSpec] = &[OptionSpec::value(
    &["-s", "--session"],
    "Name the session",
    ValueKind::Session,
)];

pub(crate) const SESSION_TARGET_OPTIONS: &[OptionSpec] = &[OptionSpec::value(
    &["-t", "--target"],
    "Target session",
    ValueKind::Session,
)];

pub(crate) const KILL_SESSION_OPTIONS: &[OptionSpec] = &[
    OptionSpec::value(&["-t", "--target"], "Target session", ValueKind::Session),
    OptionSpec::flag(&["--json"], "Print JSON"),
];

pub(crate) const TAB_CREATE_OPTIONS: &[OptionSpec] = &[
    OptionSpec::value(&["--cwd"], "Working directory", ValueKind::Opaque),
    OptionSpec::value(&["--label"], "Tab label", ValueKind::Opaque),
    OptionSpec::flag(&["--focus"], "Focus created tab"),
    OptionSpec::flag(&["--no-focus"], "Leave focus unchanged"),
];

pub(crate) const NEW_TAB_OPTIONS: &[OptionSpec] = &[
    OptionSpec::value(&["-n", "--name"], "Tab name", ValueKind::Opaque),
    OptionSpec::value(&["-c", "--cwd"], "Working directory", ValueKind::Opaque),
    OptionSpec::value(&["--label"], "Tab label", ValueKind::Opaque),
    OptionSpec::flag(&["--focus"], "Focus created tab"),
    OptionSpec::flag(&["--no-focus"], "Leave focus unchanged"),
];

pub(crate) const TAB_TARGET_OPTIONS: &[OptionSpec] = &[OptionSpec::value(
    &["-t", "--target"],
    "Target tab",
    ValueKind::Tab,
)];

pub(crate) const PANE_TARGET_OPTIONS: &[OptionSpec] = &[OptionSpec::value(
    &["-t", "--target"],
    "Target pane",
    ValueKind::Pane,
)];

pub(crate) const PANE_READ_OPTIONS: &[OptionSpec] = &[
    OptionSpec::value(&["--source"], "Read source", ValueKind::ReadSource),
    OptionSpec::value(&["--lines"], "Number of lines", ValueKind::Opaque),
    OptionSpec::value(&["--format"], "Output format", ValueKind::ReadFormat),
    OptionSpec::flag(&["--ansi"], "ANSI format"),
    OptionSpec::flag(&["--raw"], "Raw ANSI output"),
];

pub(crate) const PANE_RENAME_OPTIONS: &[OptionSpec] =
    &[OptionSpec::flag(&["--clear"], "Clear manual label")];

pub(crate) const PANE_SPLIT_OPTIONS: &[OptionSpec] = &[
    OptionSpec::value(
        &["--direction"],
        "Split direction",
        ValueKind::SplitDirection,
    ),
    OptionSpec::value(&["--cwd"], "Working directory", ValueKind::Opaque),
    OptionSpec::flag(&["--focus"], "Focus new pane"),
    OptionSpec::flag(&["--no-focus"], "Leave focus unchanged"),
];

pub(crate) const PANE_POPUP_OPTIONS: &[OptionSpec] = &[
    OptionSpec::value(&["--cwd"], "Working directory", ValueKind::Opaque),
    OptionSpec::value(&["--width"], "Popup width", ValueKind::Opaque),
    OptionSpec::value(&["--height"], "Popup height", ValueKind::Opaque),
    OptionSpec::value(&["--x"], "Popup x position", ValueKind::Opaque),
    OptionSpec::value(&["--y"], "Popup y position", ValueKind::Opaque),
    OptionSpec::flag(&["--focus"], "Focus popup pane"),
    OptionSpec::flag(&["--no-focus"], "Leave focus unchanged"),
];

pub(crate) const CAPTURE_PANE_OPTIONS: &[OptionSpec] = &[
    OptionSpec::value(&["-t", "--target"], "Target pane", ValueKind::Pane),
    OptionSpec::value(&["-S", "--lines"], "Number of lines", ValueKind::Opaque),
    OptionSpec::flag(&["-e", "--ansi"], "Include ANSI escapes"),
    OptionSpec::flag(&["--raw"], "Do not strip ANSI escapes"),
    OptionSpec::value(&["--source"], "Read source", ValueKind::ReadSource),
    OptionSpec::value(&["--format"], "Output format", ValueKind::ReadFormat),
];

pub(crate) const SELECT_PANE_OPTIONS: &[OptionSpec] = &[
    OptionSpec::value(&["-t", "--target"], "Target pane", ValueKind::Pane),
    OptionSpec::flag(&["-L", "--left"], "Left pane"),
    OptionSpec::flag(&["-R", "--right"], "Right pane"),
    OptionSpec::flag(&["-U", "--up"], "Up pane"),
    OptionSpec::flag(&["-D", "--down"], "Down pane"),
];

pub(crate) const RESIZE_PANE_OPTIONS: &[OptionSpec] = &[
    OptionSpec::flag(&["-L", "--left"], "Resize left"),
    OptionSpec::flag(&["-R", "--right"], "Resize right"),
    OptionSpec::flag(&["-U", "--up"], "Resize up"),
    OptionSpec::flag(&["-D", "--down"], "Resize down"),
    OptionSpec::value(
        &["--direction"],
        "Resize direction",
        ValueKind::PaneDirection,
    ),
    OptionSpec::value(&["--amount"], "Resize amount", ValueKind::Opaque),
];

pub(crate) const SPLIT_PANE_OPTIONS: &[OptionSpec] = &[
    OptionSpec::value(&["-t", "--target"], "Target pane", ValueKind::Pane),
    OptionSpec::flag(&["-h", "--horizontal"], "Horizontal split"),
    OptionSpec::flag(&["-v", "--vertical"], "Vertical split"),
    OptionSpec::value(
        &["--direction"],
        "Split direction",
        ValueKind::SplitDirection,
    ),
    OptionSpec::value(&["-c", "--cwd"], "Working directory", ValueKind::Opaque),
    OptionSpec::flag(&["--focus"], "Focus new pane"),
    OptionSpec::flag(&["--no-focus"], "Leave focus unchanged"),
];

pub(crate) const SERVER_LIVE_HANDOFF_OPTIONS: &[OptionSpec] = &[
    OptionSpec::flag(&["--all-sessions"], "Hand off every running session"),
    OptionSpec::value(&["--import-exe"], "Executable to import", ValueKind::Opaque),
    OptionSpec::value(
        &["--expected-protocol"],
        "Expected protocol",
        ValueKind::Opaque,
    ),
    OptionSpec::value(
        &["--expected-version"],
        "Expected version",
        ValueKind::Opaque,
    ),
];

pub(crate) const WAIT_OUTPUT_OPTIONS: &[OptionSpec] = &[
    OptionSpec::value(&["--match"], "Text to wait for", ValueKind::Opaque),
    OptionSpec::value(&["--source"], "Read source", ValueKind::ReadSource),
    OptionSpec::value(&["--lines"], "Number of lines", ValueKind::Opaque),
    OptionSpec::value(&["--timeout"], "Timeout in ms", ValueKind::Opaque),
    OptionSpec::flag(&["--regex"], "Treat match as regex"),
    OptionSpec::flag(&["--raw"], "Do not strip ANSI escapes"),
];

pub(crate) const TERMINAL_ATTACH_OPTIONS: &[OptionSpec] = &[OptionSpec::flag(
    &["--takeover"],
    "Take over existing client",
)];

pub(crate) const COMMAND_GLOBAL_OPTIONS: &[OptionSpec] = &[GLOBAL_SESSION_OPTION, HELP_OPTION];

impl CommandSpec {
    pub(crate) const fn new(name: &'static str, description: &'static str) -> Self {
        Self { name, description }
    }
}

#[cfg(test)]
impl PublicCommandSpec {
    pub(crate) const fn new(path: &'static [&'static str], usage: &'static str) -> Self {
        Self {
            path,
            usage,
            completion_required: true,
        }
    }
}

pub(crate) fn is_top_level_command(name: &str) -> bool {
    TOP_LEVEL_COMMANDS
        .iter()
        .any(|command| command.name == name)
}

#[cfg(test)]
mod tests {
    use std::collections::HashSet;

    use super::*;

    #[test]
    fn top_level_command_names_are_unique() {
        let mut names = HashSet::new();
        for command in TOP_LEVEL_COMMANDS {
            assert!(
                names.insert(command.name),
                "duplicate command {}",
                command.name
            );
        }
    }

    #[test]
    fn shared_top_level_recognition_includes_terminal() {
        assert!(is_top_level_command("terminal"));
        assert!(!is_top_level_command("remote-client-bridge"));
    }

    #[test]
    fn public_command_metadata_is_complete() {
        let mut paths = HashSet::new();
        for command in PUBLIC_COMMANDS {
            assert!(!command.path.is_empty(), "public command path is empty");
            assert!(
                !command.usage.is_empty(),
                "missing usage for {:?}",
                command.path
            );
            assert!(
                command.completion_required,
                "completion metadata must be explicit for {:?}",
                command.path
            );
            assert!(
                paths.insert(command.path),
                "duplicate public command path {:?}",
                command.path
            );
        }
    }

    #[test]
    fn command_lists_have_public_metadata() {
        for command in TOP_LEVEL_COMMANDS {
            assert_public_path(&[command.name]);
        }
        assert_nested_public_paths("session", SESSION_COMMANDS);
        assert_nested_public_paths("server", SERVER_COMMANDS);
        assert_nested_public_paths("tab", TAB_COMMANDS);
        assert_nested_public_paths("pane", PANE_COMMANDS);
        assert_nested_public_paths("wait", WAIT_COMMANDS);
        assert_nested_public_paths("terminal", TERMINAL_COMMANDS);
        assert_nested_public_paths("config", CONFIG_COMMANDS);
    }

    fn assert_nested_public_paths(parent: &'static str, commands: &[CommandSpec]) {
        for command in commands {
            if command.name == "help" {
                continue;
            }
            assert_public_path(&[parent, command.name]);
        }
    }

    fn assert_public_path(path: &[&str]) {
        assert!(
            PUBLIC_COMMANDS.iter().any(|command| command.path == path),
            "missing public command metadata for {path:?}"
        );
    }
}
