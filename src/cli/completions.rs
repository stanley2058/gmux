use crate::api::client::{ApiClient, ConnectionTarget};
use crate::api::schema::{Method, PaneListParams, Request, TabListParams};

use super::spec::{self, OptionSpec, ValueKind};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum CompletionShell {
    Bash,
    Zsh,
    Fish,
}

#[derive(Debug, Clone)]
struct CompletionCandidate {
    value: String,
    description: String,
}

impl CompletionCandidate {
    fn new(value: impl Into<String>, description: impl Into<String>) -> Self {
        Self {
            value: value.into(),
            description: description.into(),
        }
    }

    fn prefixed(&self, prefix: &str) -> Self {
        Self::new(format!("{prefix}{}", self.value), self.description.clone())
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum TargetKind {
    Session,
    Pane,
    Tab,
    Terminal,
}

type NestedCompleter =
    fn(&str, &[String], &CompletionContext, usize, usize, &str) -> Vec<CompletionCandidate>;

#[derive(Debug, Clone, PartialEq, Eq)]
struct CompletionContext {
    explicit_session: Option<Option<String>>,
}

impl CompletionContext {
    fn from_words(words: &[String], current_index: usize) -> Self {
        Self {
            explicit_session: explicit_session_target(words, current_index),
        }
    }

    fn api_client(&self) -> ApiClient {
        match &self.explicit_session {
            Some(session) => ApiClient::for_target(ConnectionTarget::LocalSession(session.clone())),
            None => ApiClient::local(),
        }
    }
}

pub(super) fn run_completions_command(args: &[String]) -> std::io::Result<i32> {
    match args.first().map(String::as_str) {
        Some("__complete") => return run_complete_request(&args[1..]),
        Some("__sessions") => return print_candidates(session_candidates()),
        Some("__panes") => {
            return print_candidates(pane_candidates(&CompletionContext {
                explicit_session: None,
            }));
        }
        Some("__tabs") => {
            return print_candidates(tab_candidates(&CompletionContext {
                explicit_session: None,
            }));
        }
        Some("__terminals") => {
            return print_candidates(terminal_candidates(&CompletionContext {
                explicit_session: None,
            }));
        }
        _ => {}
    }

    if super::is_help_request(args) {
        print_completions_help();
        return Ok(0);
    }

    let Some(shell) = args.first().map(String::as_str) else {
        print_completions_help();
        return Ok(2);
    };
    if args.len() != 1 {
        print_completions_help();
        return Ok(2);
    }

    let Some(shell) = parse_shell(shell) else {
        print_completions_help();
        return Ok(2);
    };
    print!("{}", render_completion(shell));
    Ok(0)
}

fn run_complete_request(args: &[String]) -> std::io::Result<i32> {
    let Some(shell) = args.first().and_then(|value| parse_shell(value)) else {
        return Ok(0);
    };
    let Some(separator) = args.iter().position(|arg| arg == "--") else {
        let Some(current) = args.get(1) else {
            return Ok(0);
        };
        let words = Vec::new();
        let candidates = complete_words(shell, current, words);
        return print_candidates(candidates);
    };
    let Some(current) = args.get(separator + 1) else {
        return Ok(0);
    };
    let words = args[separator + 2..].to_vec();

    let candidates = complete_words(shell, current, words);
    print_candidates(candidates)
}

fn print_candidates(candidates: Vec<CompletionCandidate>) -> std::io::Result<i32> {
    for candidate in candidates {
        println!(
            "{}\t{}",
            completion_field(&candidate.value),
            completion_field(&candidate.description)
        );
    }
    Ok(0)
}

fn completion_field(value: &str) -> String {
    value
        .chars()
        .map(|ch| match ch {
            '\t' | '\r' | '\n' => ' ',
            ch => ch,
        })
        .collect::<String>()
        .trim()
        .to_string()
}

fn parse_shell(value: &str) -> Option<CompletionShell> {
    match value {
        "bash" => Some(CompletionShell::Bash),
        "zsh" => Some(CompletionShell::Zsh),
        "fish" => Some(CompletionShell::Fish),
        "help" | "--help" | "-h" => None,
        _ => None,
    }
}

fn print_completions_help() {
    eprintln!("usage: gmux completions <bash|zsh|fish>");
}

fn render_completion(shell: CompletionShell) -> &'static str {
    match shell {
        CompletionShell::Bash => BASH_COMPLETION,
        CompletionShell::Zsh => ZSH_COMPLETION,
        CompletionShell::Fish => FISH_COMPLETION,
    }
}

const BASH_COMPLETION: &str = r#"# gmux bash completion
__gmux_complete() {
    local cur value description
    local -a candidates
    COMPREPLY=()
    cur="${COMP_WORDS[COMP_CWORD]}"
    candidates=()

    while IFS=$'\t' read -r value description; do
        if [[ -n "$value" ]]; then
            candidates+=("$value")
        fi
    done < <(command gmux completions __complete bash -- "$cur" "${COMP_WORDS[@]}" 2>/dev/null)

    COMPREPLY=( $(compgen -W "${candidates[*]}" -- "$cur") )
}

complete -F __gmux_complete gmux
"#;

const ZSH_COMPLETION: &str = r#"#compdef gmux

_gmux() {
    local current value description
    local -a values descriptions
    values=()
    descriptions=()
    current="${words[CURRENT]}"

    while IFS=$'\t' read -r value description; do
        if [[ -n "$value" ]]; then
            values+=("$value")
            descriptions+=("${description:-$value}")
        fi
    done < <(command gmux completions __complete zsh -- "$current" "${words[@]}" 2>/dev/null)

    compadd -d descriptions -a values
}

_gmux "$@"
"#;

const FISH_COMPLETION: &str = r#"# gmux fish completion
function __gmux_complete
    set -l current (commandline -ct)
    command gmux completions __complete fish -- "$current" (commandline -opc) 2>/dev/null
end

complete -c gmux -f -a '(__gmux_complete)'
"#;

fn complete_words(
    _shell: CompletionShell,
    current: &str,
    mut words: Vec<String>,
) -> Vec<CompletionCandidate> {
    normalize_words(current, &mut words);
    let Some(current_index) = words.len().checked_sub(1) else {
        return filtered(current, top_level_candidates());
    };
    let context = CompletionContext::from_words(&words, current_index);

    if let Some(candidates) = complete_equals_value(current, &words, &context) {
        return filtered(current, candidates);
    }
    if let Some(flag) = previous_word(&words, current_index) {
        if let Some(candidates) = complete_flag_value(flag, &words, &context) {
            return filtered(current, candidates);
        }
    }

    let Some((command_index, command)) = command_at(&words) else {
        return filtered(current, top_or_global_candidates(current));
    };

    if command_index == current_index && !is_top_level_command(command) {
        return filtered(current, top_or_global_candidates(current));
    }

    let candidates = match command {
        "completions" | "completion" => {
            if current.starts_with('-') {
                command_global_option_candidates()
            } else {
                shell_candidates()
            }
        }
        "session" => complete_nested_command(
            current,
            &words,
            &context,
            current_index,
            command_index,
            session_command_candidates(),
            complete_session_subcommand,
        ),
        "server" => complete_nested_command(
            current,
            &words,
            &context,
            current_index,
            command_index,
            server_command_candidates(),
            complete_server_subcommand,
        ),
        "tab" => complete_nested_command(
            current,
            &words,
            &context,
            current_index,
            command_index,
            tab_command_candidates(),
            complete_tab_subcommand,
        ),
        "pane" => complete_nested_command(
            current,
            &words,
            &context,
            current_index,
            command_index,
            pane_command_candidates(),
            complete_pane_subcommand,
        ),
        "wait" => complete_nested_command(
            current,
            &words,
            &context,
            current_index,
            command_index,
            wait_command_candidates(),
            complete_wait_subcommand,
        ),
        "terminal" => complete_nested_command(
            current,
            &words,
            &context,
            current_index,
            command_index,
            terminal_command_candidates(),
            complete_terminal_subcommand,
        ),
        "config" => complete_nested_command(
            current,
            &words,
            &context,
            current_index,
            command_index,
            config_command_candidates(),
            complete_config_subcommand,
        ),
        "status" => complete_status(current, &words, current_index, command_index),
        "new" => options_if_option(current, spec::SESSION_NAME_OPTIONS),
        "attach" => options_if_option(current, spec::SESSION_TARGET_OPTIONS),
        "kill-session" => options_if_option(current, spec::KILL_SESSION_OPTIONS),
        "ls" => options_if_option(current, spec::JSON_OPTIONS),
        "list-tabs" | "detach" => options_if_option(current, &[]),
        "new-tab" => options_if_option(current, spec::NEW_TAB_OPTIONS),
        "select-tab" | "kill-tab" => complete_alias_target_or_options(
            current,
            &words,
            &context,
            current_index,
            command_index,
            TargetKind::Tab,
            spec::TAB_TARGET_OPTIONS,
        ),
        "rename-tab" => complete_alias_target_or_options(
            current,
            &words,
            &context,
            current_index,
            command_index,
            TargetKind::Tab,
            spec::TAB_TARGET_OPTIONS,
        ),
        "capture-pane" => options_if_option(current, spec::CAPTURE_PANE_OPTIONS),
        "select-pane" | "kill-pane" => complete_alias_target_or_options(
            current,
            &words,
            &context,
            current_index,
            command_index,
            TargetKind::Pane,
            spec::SELECT_PANE_OPTIONS,
        ),
        "resize-pane" => options_if_option(current, spec::RESIZE_PANE_OPTIONS),
        "send-text" => options_if_option(current, spec::PANE_TARGET_OPTIONS),
        "send-keys" => {
            let mut candidates = options_if_option(current, spec::PANE_TARGET_OPTIONS);
            if !current.starts_with('-')
                && has_non_option_after(&words, command_index + 1, current_index)
            {
                candidates.extend(key_candidates());
            }
            candidates
        }
        "split-pane" => complete_alias_target_or_options(
            current,
            &words,
            &context,
            current_index,
            command_index,
            TargetKind::Pane,
            spec::SPLIT_PANE_OPTIONS,
        ),
        _ => top_or_global_candidates(current),
    };

    filtered(current, candidates)
}

fn normalize_words(current: &str, words: &mut Vec<String>) {
    if !words.is_empty() {
        words.remove(0);
    }
    if words.last().map(String::as_str) != Some(current) {
        words.push(current.to_string());
    }
}

fn previous_word(words: &[String], current_index: usize) -> Option<&str> {
    current_index
        .checked_sub(1)
        .and_then(|index| words.get(index))
        .map(String::as_str)
}

fn explicit_session_target(words: &[String], current_index: usize) -> Option<Option<String>> {
    let mut target = None;
    let mut index = 0;
    while index < words.len() {
        if index == current_index {
            index += 1;
            continue;
        }
        let word = words[index].as_str();
        if word == "--session" {
            let value_index = index + 1;
            if value_index != current_index {
                if let Some(value) = words.get(value_index).filter(|value| !value.is_empty()) {
                    if let Ok(parsed) = crate::session::parse_target_name(value) {
                        target = Some(parsed);
                    }
                }
            }
            index += 2;
            continue;
        }
        if let Some(value) = word.strip_prefix("--session=") {
            if !value.is_empty() {
                if let Ok(parsed) = crate::session::parse_target_name(value) {
                    target = Some(parsed);
                }
            }
        }
        index += 1;
    }
    target
}

fn command_at(words: &[String]) -> Option<(usize, &str)> {
    let mut index = 0;
    while index < words.len() {
        let word = words[index].as_str();
        if word == "--" {
            return None;
        }
        if global_option_takes_value(word) {
            index += 2;
            continue;
        }
        if is_global_option(word) || global_option_with_value_prefix(word).is_some() {
            index += 1;
            continue;
        }
        if word.starts_with('-') && !word.is_empty() {
            index += 1;
            continue;
        }
        return Some((index, word));
    }
    None
}

fn is_top_level_command(value: &str) -> bool {
    spec::is_top_level_command(value)
}

fn global_option_takes_value(value: &str) -> bool {
    spec::GLOBAL_OPTIONS
        .iter()
        .any(|option| option.names.contains(&value) && option.value.is_some())
}

fn is_global_option(value: &str) -> bool {
    spec::GLOBAL_OPTIONS
        .iter()
        .any(|option| option.names.contains(&value))
}

fn global_option_with_value_prefix(value: &str) -> Option<&'static str> {
    spec::GLOBAL_OPTIONS
        .iter()
        .filter(|option| option.value.is_some())
        .flat_map(|option| option.names.iter())
        .find_map(|name| {
            let prefix = format!("{name}=");
            value.starts_with(&prefix).then_some(*name)
        })
}

fn complete_equals_value(
    current: &str,
    words: &[String],
    context: &CompletionContext,
) -> Option<Vec<CompletionCandidate>> {
    complete_option_value_for_equals(current, words, context)
}

fn complete_flag_value(
    flag: &str,
    words: &[String],
    context: &CompletionContext,
) -> Option<Vec<CompletionCandidate>> {
    match flag {
        "--session" | "-s" => Some(target_candidates(TargetKind::Session, context)),
        "--target" | "-t" => Some(target_candidates(target_kind_for_words(words), context)),
        "--direction" => Some(direction_candidates(words)),
        _ => option_value_kind(flag).map(|kind| value_kind_candidates(kind, context)),
    }
}

fn value_kind_candidates(kind: ValueKind, context: &CompletionContext) -> Vec<CompletionCandidate> {
    match kind {
        ValueKind::Session => target_candidates(TargetKind::Session, context),
        ValueKind::Pane => target_candidates(TargetKind::Pane, context),
        ValueKind::Tab => target_candidates(TargetKind::Tab, context),
        ValueKind::ReadSource => read_source_candidates(),
        ValueKind::ReadFormat => read_format_candidates(),
        ValueKind::SplitDirection => {
            static_candidates(&[("right", "Split right"), ("down", "Split down")])
        }
        ValueKind::PaneDirection => static_candidates(&[
            ("left", "Left"),
            ("right", "Right"),
            ("up", "Up"),
            ("down", "Down"),
        ]),
        ValueKind::RemoteKeybindings => remote_keybinding_candidates(),
        ValueKind::Opaque => Vec::new(),
    }
}

fn option_value_kind(name: &str) -> Option<ValueKind> {
    option_spec_groups()
        .iter()
        .flat_map(|group| group.iter())
        .find(|option| option.names.contains(&name))
        .and_then(|option| option.value)
}

fn option_spec_groups() -> &'static [&'static [OptionSpec]] {
    &[
        spec::GLOBAL_OPTIONS,
        spec::JSON_OPTIONS,
        spec::SESSION_NAME_OPTIONS,
        spec::SESSION_TARGET_OPTIONS,
        spec::TAB_CREATE_OPTIONS,
        spec::NEW_TAB_OPTIONS,
        spec::TAB_TARGET_OPTIONS,
        spec::PANE_TARGET_OPTIONS,
        spec::PANE_READ_OPTIONS,
        spec::PANE_RENAME_OPTIONS,
        spec::PANE_SPLIT_OPTIONS,
        spec::PANE_POPUP_OPTIONS,
        spec::CAPTURE_PANE_OPTIONS,
        spec::SELECT_PANE_OPTIONS,
        spec::RESIZE_PANE_OPTIONS,
        spec::SPLIT_PANE_OPTIONS,
        spec::SERVER_LIVE_HANDOFF_OPTIONS,
        spec::WAIT_OUTPUT_OPTIONS,
        spec::TERMINAL_ATTACH_OPTIONS,
    ]
}

fn option_name_takes_value(name: &str) -> bool {
    option_value_kind(name).is_some()
}

fn option_value_kind_for_equals(current: &str) -> Option<(&str, &str, ValueKind)> {
    let (flag, partial) = current.split_once('=')?;
    let kind = option_value_kind(flag)?;
    Some((flag, partial, kind))
}

fn complete_option_value_for_equals(
    current: &str,
    words: &[String],
    context: &CompletionContext,
) -> Option<Vec<CompletionCandidate>> {
    let (flag, partial, kind) = option_value_kind_for_equals(current)?;
    let values = match flag {
        "--target" | "-t" => target_candidates(target_kind_for_words(words), context),
        "--direction" => direction_candidates(words),
        _ => value_kind_candidates(kind, context),
    };
    Some(
        values
            .into_iter()
            .map(|candidate| candidate.prefixed(&format!("{flag}=")))
            .filter(|candidate| candidate.value[flag.len() + 1..].starts_with(partial))
            .collect(),
    )
}

fn target_kind_for_words(words: &[String]) -> TargetKind {
    match command_at(words).map(|(_, command)| command) {
        Some("attach" | "kill-session" | "session") => TargetKind::Session,
        Some("select-tab" | "rename-tab" | "kill-tab" | "tab") => TargetKind::Tab,
        Some("terminal") => TargetKind::Terminal,
        _ => TargetKind::Pane,
    }
}

fn complete_nested_command(
    current: &str,
    words: &[String],
    context: &CompletionContext,
    current_index: usize,
    command_index: usize,
    subcommands: Vec<CompletionCandidate>,
    complete_subcommand: NestedCompleter,
) -> Vec<CompletionCandidate> {
    let subcommand_index = command_index + 1;
    let Some(subcommand) = words.get(subcommand_index).map(String::as_str) else {
        return subcommands;
    };
    if current_index == subcommand_index && current.starts_with('-') {
        return command_global_option_candidates();
    }
    if current_index == subcommand_index
        && !subcommands
            .iter()
            .any(|candidate| candidate.value == subcommand)
    {
        return subcommands;
    }
    complete_subcommand(
        current,
        words,
        context,
        current_index,
        subcommand_index,
        subcommand,
    )
}

fn complete_session_subcommand(
    current: &str,
    words: &[String],
    context: &CompletionContext,
    current_index: usize,
    subcommand_index: usize,
    subcommand: &str,
) -> Vec<CompletionCandidate> {
    match subcommand {
        "attach" => complete_required_first_positional_or_options(
            current,
            words,
            context,
            current_index,
            subcommand_index,
            TargetKind::Session,
            &[],
        ),
        "stop" | "delete" => complete_required_first_positional_or_options(
            current,
            words,
            context,
            current_index,
            subcommand_index,
            TargetKind::Session,
            spec::JSON_OPTIONS,
        ),
        "rename" => complete_required_first_positional_or_options(
            current,
            words,
            context,
            current_index,
            subcommand_index,
            TargetKind::Session,
            spec::JSON_OPTIONS,
        ),
        "list" => options_if_option(current, spec::JSON_OPTIONS),
        _ => Vec::new(),
    }
}

fn complete_server_subcommand(
    current: &str,
    _words: &[String],
    _context: &CompletionContext,
    _current_index: usize,
    _subcommand_index: usize,
    subcommand: &str,
) -> Vec<CompletionCandidate> {
    match subcommand {
        "live-handoff" => options_if_option(current, spec::SERVER_LIVE_HANDOFF_OPTIONS),
        "stop" | "reload-config" => options_if_option(current, &[]),
        _ => Vec::new(),
    }
}

fn complete_tab_subcommand(
    current: &str,
    words: &[String],
    context: &CompletionContext,
    current_index: usize,
    subcommand_index: usize,
    subcommand: &str,
) -> Vec<CompletionCandidate> {
    match subcommand {
        "list" => options_if_option(current, &[]),
        "get" | "focus" | "rename" | "close" => complete_required_first_positional_or_options(
            current,
            words,
            context,
            current_index,
            subcommand_index,
            TargetKind::Tab,
            &[],
        ),
        "create" => options_if_option(current, spec::TAB_CREATE_OPTIONS),
        _ => Vec::new(),
    }
}

fn complete_pane_subcommand(
    current: &str,
    words: &[String],
    context: &CompletionContext,
    current_index: usize,
    subcommand_index: usize,
    subcommand: &str,
) -> Vec<CompletionCandidate> {
    match subcommand {
        "list" => options_if_option(current, &[]),
        "get" | "close" => complete_required_first_positional_or_options(
            current,
            words,
            context,
            current_index,
            subcommand_index,
            TargetKind::Pane,
            &[],
        ),
        "read" => complete_required_first_positional_or_options(
            current,
            words,
            context,
            current_index,
            subcommand_index,
            TargetKind::Pane,
            spec::PANE_READ_OPTIONS,
        ),
        "rename" => complete_required_first_positional_or_options(
            current,
            words,
            context,
            current_index,
            subcommand_index,
            TargetKind::Pane,
            spec::PANE_RENAME_OPTIONS,
        ),
        "split" => complete_required_first_positional_or_options(
            current,
            words,
            context,
            current_index,
            subcommand_index,
            TargetKind::Pane,
            spec::PANE_SPLIT_OPTIONS,
        ),
        "popup" => complete_required_first_positional_or_options(
            current,
            words,
            context,
            current_index,
            subcommand_index,
            TargetKind::Pane,
            spec::PANE_POPUP_OPTIONS,
        ),
        "send-text" | "run" => complete_required_first_positional_or_options(
            current,
            words,
            context,
            current_index,
            subcommand_index,
            TargetKind::Pane,
            &[],
        ),
        "send-keys" => {
            let has_target = has_non_option_after(words, subcommand_index + 1, current_index);
            if !has_target || current.starts_with('-') {
                return complete_required_first_positional_or_options(
                    current,
                    words,
                    context,
                    current_index,
                    subcommand_index,
                    TargetKind::Pane,
                    &[],
                );
            }
            key_candidates()
        }
        _ => Vec::new(),
    }
}

fn complete_wait_subcommand(
    current: &str,
    words: &[String],
    context: &CompletionContext,
    current_index: usize,
    subcommand_index: usize,
    subcommand: &str,
) -> Vec<CompletionCandidate> {
    match subcommand {
        "output" => complete_required_first_positional_or_options(
            current,
            words,
            context,
            current_index,
            subcommand_index,
            TargetKind::Pane,
            spec::WAIT_OUTPUT_OPTIONS,
        ),
        _ => Vec::new(),
    }
}

fn complete_terminal_subcommand(
    current: &str,
    words: &[String],
    context: &CompletionContext,
    current_index: usize,
    subcommand_index: usize,
    subcommand: &str,
) -> Vec<CompletionCandidate> {
    match subcommand {
        "attach" => complete_required_first_positional_or_options(
            current,
            words,
            context,
            current_index,
            subcommand_index,
            TargetKind::Terminal,
            spec::TERMINAL_ATTACH_OPTIONS,
        ),
        _ => Vec::new(),
    }
}

fn complete_config_subcommand(
    current: &str,
    _words: &[String],
    _context: &CompletionContext,
    _current_index: usize,
    _subcommand_index: usize,
    subcommand: &str,
) -> Vec<CompletionCandidate> {
    match subcommand {
        "reset-keys" => options_if_option(current, &[]),
        _ => Vec::new(),
    }
}

fn complete_status(
    current: &str,
    _words: &[String],
    _current_index: usize,
    _command_index: usize,
) -> Vec<CompletionCandidate> {
    if current.starts_with('-') {
        return option_and_global_candidates(spec::JSON_OPTIONS);
    }
    vec![
        CompletionCandidate::new("server", "Show server status"),
        CompletionCandidate::new("client", "Show client status"),
        CompletionCandidate::new("--json", "Print JSON"),
    ]
}

fn complete_alias_target_or_options(
    current: &str,
    words: &[String],
    context: &CompletionContext,
    current_index: usize,
    command_index: usize,
    kind: TargetKind,
    options: &[OptionSpec],
) -> Vec<CompletionCandidate> {
    complete_first_positional_or_options(
        current,
        words,
        context,
        current_index,
        command_index,
        kind,
        options,
    )
}

fn complete_required_first_positional_or_options(
    current: &str,
    words: &[String],
    context: &CompletionContext,
    current_index: usize,
    parent_index: usize,
    kind: TargetKind,
    options: &[OptionSpec],
) -> Vec<CompletionCandidate> {
    if current.starts_with('-') {
        return option_and_global_candidates(options);
    }
    if !has_non_option_after(words, parent_index + 1, current_index) {
        return target_candidates(kind, context);
    }
    options_if_option(current, options)
}

fn complete_first_positional_or_options(
    current: &str,
    words: &[String],
    context: &CompletionContext,
    current_index: usize,
    parent_index: usize,
    kind: TargetKind,
    options: &[OptionSpec],
) -> Vec<CompletionCandidate> {
    if current.starts_with('-') {
        return option_and_global_candidates(options);
    }
    if !has_non_option_after(words, parent_index + 1, current_index) {
        let mut candidates = target_candidates(kind, context);
        candidates.extend(option_and_global_candidates(options));
        return candidates;
    }
    options_if_option(current, options)
}

fn has_non_option_after(words: &[String], start: usize, end: usize) -> bool {
    let mut index = start;
    while index < end {
        let word = words[index].as_str();
        if option_takes_value(word) {
            index += 2;
            continue;
        }
        if word.starts_with('-') {
            index += 1;
            continue;
        }
        return true;
    }
    false
}

fn option_takes_value(value: &str) -> bool {
    option_name_takes_value(value)
}

fn options_if_option(current: &str, options: &[OptionSpec]) -> Vec<CompletionCandidate> {
    if current.starts_with('-') || current.is_empty() {
        option_and_global_candidates(options)
    } else {
        Vec::new()
    }
}

fn option_candidates(options: &[OptionSpec]) -> Vec<CompletionCandidate> {
    options
        .iter()
        .flat_map(|option| {
            option
                .names
                .iter()
                .map(|name| CompletionCandidate::new(*name, option.description))
        })
        .collect()
}

fn option_and_global_candidates(options: &[OptionSpec]) -> Vec<CompletionCandidate> {
    let mut candidates = option_candidates(options);
    for candidate in command_global_option_candidates() {
        if !candidates
            .iter()
            .any(|existing| existing.value == candidate.value)
        {
            candidates.push(candidate);
        }
    }
    candidates
}

fn target_candidates(kind: TargetKind, context: &CompletionContext) -> Vec<CompletionCandidate> {
    match kind {
        TargetKind::Session => session_candidates(),
        TargetKind::Pane => pane_candidates(context),
        TargetKind::Tab => tab_candidates(context),
        TargetKind::Terminal => terminal_candidates(context),
    }
}

fn session_candidates() -> Vec<CompletionCandidate> {
    crate::session::list_sessions()
        .map(|sessions| {
            sessions
                .into_iter()
                .map(|session| {
                    let status = if session.running {
                        "running"
                    } else {
                        "stopped"
                    };
                    let default = if session.default { ", default" } else { "" };
                    CompletionCandidate::new(session.name, format!("{status} session{default}"))
                })
                .collect()
        })
        .unwrap_or_default()
}

fn pane_candidates(context: &CompletionContext) -> Vec<CompletionCandidate> {
    pane_list_response(context)
        .and_then(|response| response["result"]["panes"].as_array().cloned())
        .map(|panes| {
            panes
                .iter()
                .filter_map(|pane| {
                    let pane_id = pane["pane_id"].as_str()?;
                    Some(CompletionCandidate::new(pane_id, pane_description(pane)))
                })
                .collect()
        })
        .unwrap_or_default()
}

fn tab_candidates(context: &CompletionContext) -> Vec<CompletionCandidate> {
    let Some(response) = super::send_request_to_client(
        context.api_client(),
        &Request {
            id: "cli:completion:tab:list".into(),
            method: Method::TabList(TabListParams::default()),
        },
    )
    .ok()
    .filter(|response| response.get("error").is_none()) else {
        return Vec::new();
    };

    response["result"]["tabs"]
        .as_array()
        .map(|tabs| {
            tabs.iter()
                .filter_map(|tab| {
                    let tab_id = tab["tab_id"].as_str()?;
                    let label = tab["label"].as_str().unwrap_or("untitled");
                    let number = tab["number"].as_u64().unwrap_or_default();
                    let focused = if tab["focused"].as_bool() == Some(true) {
                        ", focused"
                    } else {
                        ""
                    };
                    Some(CompletionCandidate::new(
                        tab_id,
                        format!("tab {number}: {label}{focused}"),
                    ))
                })
                .collect()
        })
        .unwrap_or_default()
}

fn terminal_candidates(context: &CompletionContext) -> Vec<CompletionCandidate> {
    pane_list_response(context)
        .and_then(|response| response["result"]["panes"].as_array().cloned())
        .map(|panes| {
            panes
                .iter()
                .filter_map(|pane| {
                    let terminal_id = pane["terminal_id"].as_str()?;
                    Some(CompletionCandidate::new(
                        terminal_id,
                        format!("terminal for {}", pane_description(pane)),
                    ))
                })
                .collect()
        })
        .unwrap_or_default()
}

fn pane_list_response(context: &CompletionContext) -> Option<serde_json::Value> {
    super::send_request_to_client(
        context.api_client(),
        &Request {
            id: "cli:completion:pane:list".into(),
            method: Method::PaneList(PaneListParams::default()),
        },
    )
    .ok()
    .filter(|response| response.get("error").is_none())
}

fn pane_description(pane: &serde_json::Value) -> String {
    let pane_id = pane["pane_id"].as_str().unwrap_or("pane");
    let tab_id = pane["tab_id"].as_str().unwrap_or("unknown tab");
    let mut parts = Vec::new();
    if pane["focused"].as_bool() == Some(true) {
        parts.push("focused".to_string());
    }
    if let Some(label) = pane["label"].as_str().or_else(|| pane["title"].as_str()) {
        if !label.trim().is_empty() {
            parts.push(label.trim().to_string());
        }
    }
    if let Some(cwd) = pane["foreground_cwd"]
        .as_str()
        .or_else(|| pane["cwd"].as_str())
    {
        if !cwd.trim().is_empty() {
            parts.push(cwd.trim().to_string());
        }
    }
    if parts.is_empty() {
        format!("pane {pane_id} in {tab_id}")
    } else {
        format!("pane {pane_id} in {tab_id}: {}", parts.join(", "))
    }
}

fn top_or_global_candidates(current: &str) -> Vec<CompletionCandidate> {
    if current.starts_with('-') {
        global_option_candidates()
    } else {
        let mut candidates = top_level_candidates();
        candidates.extend(global_option_candidates());
        candidates
    }
}

fn top_level_candidates() -> Vec<CompletionCandidate> {
    command_spec_candidates(spec::TOP_LEVEL_COMMANDS)
}

fn global_option_candidates() -> Vec<CompletionCandidate> {
    option_candidates(spec::GLOBAL_OPTIONS)
}

fn command_global_option_candidates() -> Vec<CompletionCandidate> {
    option_candidates(spec::COMMAND_GLOBAL_OPTIONS)
}

fn session_command_candidates() -> Vec<CompletionCandidate> {
    command_spec_candidates(spec::SESSION_COMMANDS)
}

fn server_command_candidates() -> Vec<CompletionCandidate> {
    command_spec_candidates(spec::SERVER_COMMANDS)
}

fn tab_command_candidates() -> Vec<CompletionCandidate> {
    command_spec_candidates(spec::TAB_COMMANDS)
}

fn pane_command_candidates() -> Vec<CompletionCandidate> {
    command_spec_candidates(spec::PANE_COMMANDS)
}

fn wait_command_candidates() -> Vec<CompletionCandidate> {
    command_spec_candidates(spec::WAIT_COMMANDS)
}

fn terminal_command_candidates() -> Vec<CompletionCandidate> {
    command_spec_candidates(spec::TERMINAL_COMMANDS)
}

fn config_command_candidates() -> Vec<CompletionCandidate> {
    command_spec_candidates(spec::CONFIG_COMMANDS)
}

fn shell_candidates() -> Vec<CompletionCandidate> {
    command_spec_candidates(spec::SHELL_COMMANDS)
}

fn read_source_candidates() -> Vec<CompletionCandidate> {
    static_candidates(&[
        ("visible", "Visible viewport"),
        ("recent", "Recent scrollback"),
        ("recent-unwrapped", "Recent scrollback, unwrap soft wraps"),
    ])
}

fn read_format_candidates() -> Vec<CompletionCandidate> {
    static_candidates(&[("text", "Plain text"), ("ansi", "ANSI output")])
}

fn direction_candidates(words: &[String]) -> Vec<CompletionCandidate> {
    match command_at(words).map(|(_, command)| command) {
        Some("resize-pane") => static_candidates(&[
            ("left", "Left"),
            ("right", "Right"),
            ("up", "Up"),
            ("down", "Down"),
        ]),
        _ => static_candidates(&[("right", "Split right"), ("down", "Split down")]),
    }
}

fn remote_keybinding_candidates() -> Vec<CompletionCandidate> {
    static_candidates(&[
        ("local", "Use local keybindings"),
        ("server", "Use server keybindings"),
    ])
}

fn key_candidates() -> Vec<CompletionCandidate> {
    static_candidates(&[
        ("Enter", "Enter key"),
        ("Tab", "Tab key"),
        ("Esc", "Escape key"),
        ("Backspace", "Backspace key"),
        ("Up", "Up arrow"),
        ("Down", "Down arrow"),
        ("Left", "Left arrow"),
        ("Right", "Right arrow"),
        ("C-c", "Control-C"),
        ("ctrl+c", "Control-C"),
    ])
}

fn static_candidates(values: &[(&str, &str)]) -> Vec<CompletionCandidate> {
    values
        .iter()
        .map(|(value, description)| CompletionCandidate::new(*value, *description))
        .collect()
}

fn command_spec_candidates(commands: &[spec::CommandSpec]) -> Vec<CompletionCandidate> {
    commands
        .iter()
        .map(|command| CompletionCandidate::new(command.name, command.description))
        .collect()
}

fn filtered(current: &str, candidates: Vec<CompletionCandidate>) -> Vec<CompletionCandidate> {
    candidates
        .into_iter()
        .filter(|candidate| candidate.value.starts_with(current))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn values(candidates: Vec<CompletionCandidate>) -> Vec<String> {
        candidates
            .into_iter()
            .map(|candidate| candidate.value)
            .collect()
    }

    #[test]
    fn scripts_call_hidden_completion_engine() {
        assert!(render_completion(CompletionShell::Bash).contains("__complete bash"));
        assert!(render_completion(CompletionShell::Zsh).contains("compadd -d descriptions"));
        assert!(render_completion(CompletionShell::Fish).contains("__complete fish"));
    }

    #[test]
    fn top_level_completion_includes_described_commands() {
        let candidates =
            complete_words(CompletionShell::Zsh, "pa", vec!["gmux".into(), "pa".into()]);
        let values = values(candidates);
        assert!(values.contains(&"pane".to_string()));
    }

    #[test]
    fn every_public_command_completes_help() {
        for command in spec::PUBLIC_COMMANDS {
            if !command.completion_required {
                continue;
            }
            let mut words = vec!["gmux".to_string()];
            words.extend(command.path.iter().map(|part| part.to_string()));
            words.push("--".to_string());
            let candidates = complete_words(CompletionShell::Bash, "--", words);
            let values = values(candidates);
            assert!(
                values.contains(&"--help".to_string()),
                "missing --help completion for {:?}; got {values:?}",
                command.path
            );
        }
    }

    #[test]
    fn completion_does_not_offer_stale_json_options() {
        let list_tabs = values(complete_words(
            CompletionShell::Bash,
            "--",
            vec!["gmux".into(), "list-tabs".into(), "--".into()],
        ));
        assert!(!list_tabs.contains(&"--json".to_string()));

        let detach = values(complete_words(
            CompletionShell::Bash,
            "--",
            vec!["gmux".into(), "detach".into(), "--".into()],
        ));
        assert!(!detach.contains(&"--json".to_string()));

        let session_attach = values(complete_words(
            CompletionShell::Bash,
            "--",
            vec![
                "gmux".into(),
                "session".into(),
                "attach".into(),
                "work".into(),
                "--".into(),
            ],
        ));
        assert!(!session_attach.contains(&"--json".to_string()));
    }

    #[test]
    fn pane_completion_includes_read_subcommand() {
        let candidates = complete_words(
            CompletionShell::Zsh,
            "r",
            vec!["gmux".into(), "pane".into(), "r".into()],
        );
        let values = values(candidates);
        assert!(values.contains(&"read".to_string()));
        assert!(values.contains(&"rename".to_string()));
        assert!(values.contains(&"run".to_string()));
    }

    #[test]
    fn pane_read_completes_source_values() {
        let candidates = complete_words(
            CompletionShell::Bash,
            "",
            vec![
                "gmux".into(),
                "pane".into(),
                "read".into(),
                "p_1".into(),
                "--source".into(),
                "".into(),
            ],
        );
        assert_eq!(
            values(candidates),
            vec!["visible", "recent", "recent-unwrapped"]
        );
    }

    #[test]
    fn pane_read_options_include_global_session() {
        let candidates = complete_words(
            CompletionShell::Zsh,
            "--",
            vec![
                "gmux".into(),
                "pane".into(),
                "read".into(),
                "p_1".into(),
                "--".into(),
            ],
        );
        let values = values(candidates);
        assert!(values.contains(&"--session".to_string()));
        assert!(values.contains(&"--format".to_string()));
    }

    #[test]
    fn completion_context_reads_session_before_target_slot() {
        let mut words = vec![
            "pane".to_string(),
            "read".to_string(),
            "--session".to_string(),
            "lilac-mono".to_string(),
            "".to_string(),
        ];
        let current_index = words.len() - 1;
        let context = CompletionContext::from_words(&words, current_index);
        assert_eq!(
            context.explicit_session,
            Some(Some("lilac-mono".to_string()))
        );

        words[2] = "--session=default".to_string();
        words.remove(3);
        let current_index = words.len() - 1;
        let context = CompletionContext::from_words(&words, current_index);
        assert_eq!(context.explicit_session, Some(None));
    }

    #[test]
    fn equals_form_completes_with_flag_prefix() {
        let candidates = complete_words(
            CompletionShell::Fish,
            "--format=a",
            vec![
                "gmux".into(),
                "pane".into(),
                "read".into(),
                "p_1".into(),
                "--format=a".into(),
            ],
        );
        assert_eq!(values(candidates), vec!["--format=ansi"]);
    }

    #[test]
    fn send_keys_completes_supported_key_names_after_target() {
        let candidates = complete_words(
            CompletionShell::Bash,
            "E",
            vec![
                "gmux".into(),
                "pane".into(),
                "send-keys".into(),
                "p_1".into(),
                "E".into(),
            ],
        );
        assert_eq!(values(candidates), vec!["Enter", "Esc"]);
    }
}
