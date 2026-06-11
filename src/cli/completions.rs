#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum CompletionShell {
    Bash,
    Zsh,
    Fish,
}

pub(super) fn run_completions_command(args: &[String]) -> std::io::Result<i32> {
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

macro_rules! top_commands {
    () => {
        "server status config ls kill-session list-tabs new-tab select-tab rename-tab kill-tab capture-pane select-pane resize-pane send-text send-keys split-pane kill-pane detach tab terminal pane wait session completions completion"
    };
}

macro_rules! session_commands {
    () => {
        "list attach stop rename delete help"
    };
}

macro_rules! server_commands {
    () => {
        "stop live-handoff reload-config help"
    };
}

macro_rules! server_options {
    () => {
        "--all-sessions --import-exe --expected-protocol --expected-version"
    };
}

macro_rules! tab_commands {
    () => {
        "list create get focus rename close help"
    };
}

macro_rules! pane_commands {
    () => {
        "list get read rename split close send-text send-keys run help"
    };
}

macro_rules! wait_commands {
    () => {
        "output help"
    };
}

macro_rules! terminal_commands {
    () => {
        "attach help"
    };
}

macro_rules! config_commands {
    () => {
        "reset-keys help"
    };
}

macro_rules! shells {
    () => {
        "bash zsh fish"
    };
}

const BASH_COMPLETION: &str = concat!(
    r#"# gmux bash completion
__gmux_session_names() {
    command gmux ls --json 2>/dev/null | grep -o '"name":"[^"]*"' | sed 's/^"name":"//;s/"$//'
}

_gmux() {
    local cur prev cmd sub
    COMPREPLY=()
    cur="${COMP_WORDS[COMP_CWORD]}"
    prev="${COMP_WORDS[COMP_CWORD-1]}"
    cmd="${COMP_WORDS[1]}"
    sub="${COMP_WORDS[2]}"

    case "$prev" in
        --session|-s|-t|--target)
            COMPREPLY=( $(compgen -W "$(__gmux_session_names)" -- "$cur") )
            return 0
            ;;
        --source)
            COMPREPLY=( $(compgen -W "visible recent recent-unwrapped" -- "$cur") )
            return 0
            ;;
        --format)
            COMPREPLY=( $(compgen -W "text ansi" -- "$cur") )
            return 0
            ;;
        --direction)
            COMPREPLY=( $(compgen -W "left right up down" -- "$cur") )
            return 0
            ;;
    esac

    if [[ $COMP_CWORD -eq 1 ]]; then
        COMPREPLY=( $(compgen -W '"#,
    top_commands!(),
    r#"' -- "$cur") )
        return 0
    fi

    case "$cmd" in
        completions|completion)
            COMPREPLY=( $(compgen -W '"#,
    shells!(),
    r#"' -- "$cur") )
            ;;
        session)
            if [[ $COMP_CWORD -eq 2 ]]; then
                COMPREPLY=( $(compgen -W '"#,
    session_commands!(),
    r#"' -- "$cur") )
            elif [[ "$sub" == "attach" || "$sub" == "stop" || "$sub" == "rename" || "$sub" == "delete" ]]; then
                COMPREPLY=( $(compgen -W "$(__gmux_session_names)" -- "$cur") )
            else
                COMPREPLY=( $(compgen -W "--json" -- "$cur") )
            fi
            ;;
        attach|kill-session)
            COMPREPLY=( $(compgen -W "-t --target --json" -- "$cur") )
            ;;
        new)
            COMPREPLY=( $(compgen -W "-s --session" -- "$cur") )
            ;;
        server)
            COMPREPLY=( $(compgen -W '"#,
    server_commands!(),
    " ",
    server_options!(),
    r#"' -- "$cur") )
            ;;
        tab)
            COMPREPLY=( $(compgen -W '"#,
    tab_commands!(),
    r#" --cwd --label --focus --no-focus' -- "$cur") )
            ;;
        pane)
            COMPREPLY=( $(compgen -W '"#,
    pane_commands!(),
    r#" --source --lines --format --ansi --raw --direction --cwd --focus --no-focus --clear' -- "$cur") )
            ;;
        wait)
            COMPREPLY=( $(compgen -W '"#,
    wait_commands!(),
    r#" --match --source --lines --timeout --regex --raw' -- "$cur") )
            ;;
        terminal)
            COMPREPLY=( $(compgen -W '"#,
    terminal_commands!(),
    r#" --takeover' -- "$cur") )
            ;;
        config)
            COMPREPLY=( $(compgen -W '"#,
    config_commands!(),
    r#"' -- "$cur") )
            ;;
        ls|list-tabs|detach|status)
            COMPREPLY=( $(compgen -W "--json" -- "$cur") )
            ;;
        *)
            COMPREPLY=( $(compgen -W '"#,
    top_commands!(),
    r#"' -- "$cur") )
            ;;
    esac
}

complete -F _gmux gmux
"#
);

const ZSH_COMPLETION: &str = concat!(
    r#"#compdef gmux

__gmux_session_names() {
    command gmux ls --json 2>/dev/null | grep -o '"name":"[^"]*"' | sed 's/^"name":"//;s/"$//'
}

_gmux() {
    local -a commands session_commands server_commands tab_commands pane_commands wait_commands terminal_commands config_commands shells sessions
    commands=("#,
    top_commands!(),
    r#")
    session_commands=("#,
    session_commands!(),
    r#")
    server_commands=("#,
    server_commands!(),
    r#")
    tab_commands=("#,
    tab_commands!(),
    r#")
    pane_commands=("#,
    pane_commands!(),
    r#")
    wait_commands=("#,
    wait_commands!(),
    r#")
    terminal_commands=("#,
    terminal_commands!(),
    r#")
    config_commands=("#,
    config_commands!(),
    r#")
    shells=("#,
    shells!(),
    r#")
    sessions=($(__gmux_session_names))

    if [[ $CURRENT -eq 2 ]]; then
        compadd -- $commands
        return
    fi

    case "${words[2]}" in
        completions|completion) compadd -- $shells ;;
        session)
            if [[ $CURRENT -eq 3 ]]; then
                compadd -- $session_commands
            elif [[ "${words[3]}" == "attach" || "${words[3]}" == "stop" || "${words[3]}" == "rename" || "${words[3]}" == "delete" ]]; then
                compadd -- $sessions
            else
                compadd -- --json
            fi
            ;;
        attach|kill-session) compadd -- -t --target --json $sessions ;;
        new) compadd -- -s --session ;;
        server) compadd -- $server_commands "#,
    server_options!(),
    r#" ;;
        tab) compadd -- $tab_commands --cwd --label --focus --no-focus ;;
        pane) compadd -- $pane_commands --source --lines --format --ansi --raw --direction --cwd --focus --no-focus --clear ;;
        wait) compadd -- $wait_commands --match --source --lines --timeout --regex --raw ;;
        terminal) compadd -- $terminal_commands --takeover ;;
        config) compadd -- $config_commands ;;
        *) compadd -- $commands ;;
    esac
}

_gmux "$@"
"#
);

const FISH_COMPLETION: &str = concat!(
    r#"# gmux fish completion
function __gmux_session_names
    command gmux ls --json 2>/dev/null | grep -o '"name":"[^"]*"' | sed 's/^"name":"//;s/"$//'
end

complete -c gmux -f -n '__fish_is_first_arg' -a '"#,
    top_commands!(),
    r#"'
complete -c gmux -f -n '__fish_seen_subcommand_from completions completion' -a '"#,
    shells!(),
    r#"'
complete -c gmux -f -n '__fish_seen_subcommand_from session' -a '"#,
    session_commands!(),
    r#"'
complete -c gmux -f -n '__fish_seen_subcommand_from server' -a '"#,
    server_commands!(),
    r#"'
complete -c gmux -f -n '__fish_seen_subcommand_from tab' -a '"#,
    tab_commands!(),
    r#"'
complete -c gmux -f -n '__fish_seen_subcommand_from pane' -a '"#,
    pane_commands!(),
    r#"'
complete -c gmux -f -n '__fish_seen_subcommand_from wait' -a '"#,
    wait_commands!(),
    r#"'
complete -c gmux -f -n '__fish_seen_subcommand_from terminal' -a '"#,
    terminal_commands!(),
    r#"'
complete -c gmux -f -n '__fish_seen_subcommand_from config' -a '"#,
    config_commands!(),
    r#"'

complete -c gmux -s t -l target -xa '(__gmux_session_names)'
complete -c gmux -l session -xa '(__gmux_session_names)'
complete -c gmux -s s -xa '(__gmux_session_names)'
complete -c gmux -l json
complete -c gmux -l cwd -r
complete -c gmux -l label -r
complete -c gmux -l focus
complete -c gmux -l no-focus
complete -c gmux -l source -xa 'visible recent recent-unwrapped'
complete -c gmux -l format -xa 'text ansi'
complete -c gmux -l direction -xa 'left right up down'
complete -c gmux -l lines -r
complete -c gmux -l match -r
complete -c gmux -l timeout -r
complete -c gmux -l regex
complete -c gmux -l raw
complete -c gmux -l ansi
complete -c gmux -l takeover
complete -c gmux -n '__fish_seen_subcommand_from server' -l all-sessions
complete -c gmux -n '__fish_seen_subcommand_from server' -l import-exe -r
complete -c gmux -n '__fish_seen_subcommand_from server' -l expected-protocol -r
complete -c gmux -n '__fish_seen_subcommand_from server' -l expected-version -r
"#
);

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bash_completion_includes_session_name_helper() {
        let script = render_completion(CompletionShell::Bash);
        assert!(script.contains("__gmux_session_names"));
        assert!(script.contains("gmux ls --json"));
        assert!(script.contains("complete -F _gmux gmux"));
        assert!(script.contains("kill-session"));
    }

    #[test]
    fn zsh_completion_includes_session_subcommands() {
        let script = render_completion(CompletionShell::Zsh);
        assert!(script.contains("#compdef gmux"));
        assert!(script.contains("session_commands=(list attach stop rename delete help)"));
        assert!(script.contains("compadd -- $sessions"));
    }

    #[test]
    fn fish_completion_includes_dynamic_session_arguments() {
        let script = render_completion(CompletionShell::Fish);
        assert!(script.contains("function __gmux_session_names"));
        assert!(script.contains("complete -c gmux -s t -l target -xa '(__gmux_session_names)'"));
        assert!(script.contains("complete -c gmux -f -n '__fish_is_first_arg'"));
        assert!(script.contains("-l all-sessions"));
    }
}
