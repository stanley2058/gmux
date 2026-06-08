use std::time::{SystemTime, UNIX_EPOCH};

use serde::Serialize;

use crate::api::client::{ApiClient, ApiClientError};
use crate::api::schema::{
    Method, OutputMatch, PaneDirection, PaneFocusParams, PaneListParams, PaneResizeParams,
    PaneSendInputParams, PaneSplitParams, PaneWaitForOutputParams, ReadFormat, ReadSource, Request,
    SplitDirection,
};

mod completions;
mod pane;
mod server;
mod status;
mod tab;

pub enum CommandOutcome {
    Handled(i32),
    NotCli,
}

pub fn maybe_run(args: &[String]) -> std::io::Result<CommandOutcome> {
    let Some(command) = args.get(1).map(|arg| arg.as_str()) else {
        return Ok(CommandOutcome::NotCli);
    };

    let exit_code = match command {
        "server" => {
            let Some(exit_code) = server::run_server_command(&args[2..])? else {
                return Ok(CommandOutcome::NotCli);
            };
            exit_code
        }
        "status" => status::run_status_command(&args[2..])?,
        "completions" | "completion" => completions::run_completions_command(&args[2..])?,
        "config" => run_config_command(&args[2..])?,
        "ls" => session_list(&args[2..], "usage: gmux ls [--json]")?,
        "kill-session" => kill_session_alias(&args[2..])?,
        "list-tabs" => list_tabs_alias(&args[2..])?,
        "new-tab" => new_tab_alias(&args[2..])?,
        "select-tab" => select_tab_alias(&args[2..])?,
        "rename-tab" => rename_tab_alias(&args[2..])?,
        "kill-tab" => kill_tab_alias(&args[2..])?,
        "capture-pane" => capture_pane_alias(&args[2..])?,
        "select-pane" => select_pane_alias(&args[2..])?,
        "resize-pane" => resize_pane_alias(&args[2..])?,
        "send-text" => send_text_alias(&args[2..])?,
        "send-keys" => send_keys_alias(&args[2..])?,
        "split-pane" => split_pane_alias(&args[2..])?,
        "kill-pane" => kill_pane_alias(&args[2..])?,
        "detach" => detach_alias(&args[2..])?,
        "tab" => tab::run_tab_command(&args[2..])?,
        "terminal" => run_terminal_command(&args[2..])?,
        "pane" => pane::run_pane_command(&args[2..])?,
        "wait" => run_wait_command(&args[2..])?,
        "session" => run_session_command(&args[2..])?,
        _ => return Ok(CommandOutcome::NotCli),
    };

    Ok(CommandOutcome::Handled(exit_code))
}

fn kill_session_alias(args: &[String]) -> std::io::Result<i32> {
    let mut name = None;
    let mut json = false;
    let mut index = 0;
    while index < args.len() {
        match args[index].as_str() {
            "-t" | "--target" => {
                let Some(value) = args.get(index + 1) else {
                    eprintln!("missing value for {}", args[index]);
                    return Ok(2);
                };
                name = Some(value.clone());
                index += 2;
            }
            "--json" => {
                json = true;
                index += 1;
            }
            "help" | "--help" | "-h" => {
                eprintln!("usage: gmux kill-session [-t name] [--json]");
                return Ok(0);
            }
            other => {
                eprintln!("unknown option: {other}");
                return Ok(2);
            }
        }
    }

    let mut session_args = vec![name.unwrap_or_else(|| "default".to_string())];
    if json {
        session_args.push("--json".to_string());
    }
    session_stop(&session_args)
}

fn new_tab_alias(args: &[String]) -> std::io::Result<i32> {
    if matches!(
        args.first().map(String::as_str),
        Some("help" | "--help" | "-h")
    ) {
        eprintln!("usage: gmux new-tab [-n name] [-c cwd] [--focus|--no-focus]");
        return Ok(0);
    }

    let mut tab_args = vec!["create".to_string(), "--focus".to_string()];
    let mut index = 0;
    while index < args.len() {
        match args[index].as_str() {
            "-n" | "--name" => {
                let Some(value) = args.get(index + 1) else {
                    eprintln!("missing value for {}", args[index]);
                    return Ok(2);
                };
                tab_args.extend(["--label".to_string(), value.clone()]);
                index += 2;
            }
            value if value.starts_with("--name=") => {
                tab_args.extend([
                    "--label".to_string(),
                    value
                        .split_once('=')
                        .map(|(_, value)| value.to_string())
                        .unwrap_or_default(),
                ]);
                index += 1;
            }
            "-c" | "--cwd" => {
                let Some(value) = args.get(index + 1) else {
                    eprintln!("missing value for {}", args[index]);
                    return Ok(2);
                };
                tab_args.extend(["--cwd".to_string(), value.clone()]);
                index += 2;
            }
            value if value.starts_with("--cwd=") => {
                tab_args.extend([
                    "--cwd".to_string(),
                    value
                        .split_once('=')
                        .map(|(_, value)| value.to_string())
                        .unwrap_or_default(),
                ]);
                index += 1;
            }
            "--label" | "--focus" | "--no-focus" => {
                tab_args.push(args[index].clone());
                if args[index] == "--label" {
                    let Some(value) = args.get(index + 1) else {
                        eprintln!("missing value for --label");
                        return Ok(2);
                    };
                    tab_args.push(value.clone());
                    index += 2;
                } else {
                    index += 1;
                }
            }
            value if value.starts_with("--label=") => {
                tab_args.extend([
                    "--label".to_string(),
                    value
                        .split_once('=')
                        .map(|(_, value)| value.to_string())
                        .unwrap_or_default(),
                ]);
                index += 1;
            }
            other => {
                eprintln!("unknown option: {other}");
                return Ok(2);
            }
        }
    }
    tab::run_tab_command(&tab_args)
}

fn list_tabs_alias(args: &[String]) -> std::io::Result<i32> {
    if matches!(
        args.first().map(String::as_str),
        Some("help" | "--help" | "-h")
    ) {
        eprintln!("usage: gmux list-tabs");
        return Ok(0);
    }
    if !args.is_empty() {
        eprintln!("usage: gmux list-tabs");
        return Ok(2);
    }
    tab::run_tab_command(&["list".to_string()])
}

fn select_tab_alias(args: &[String]) -> std::io::Result<i32> {
    let tab_id = match parse_tab_target_alias(args, "usage: gmux select-tab [-t tab]") {
        Ok(Some(tab_id)) => tab_id,
        Ok(None) => return Ok(0),
        Err(code) => return Ok(code),
    };
    tab::run_tab_command(&["focus".to_string(), tab_id])
}

fn rename_tab_alias(args: &[String]) -> std::io::Result<i32> {
    if matches!(
        args.first().map(String::as_str),
        Some("help" | "--help" | "-h")
    ) {
        eprintln!("usage: gmux rename-tab [-t tab] <name>");
        return Ok(0);
    }
    let mut target = None;
    let mut label_start = 0;
    if matches!(args.first().map(String::as_str), Some("-t" | "--target")) {
        let Some(value) = args.get(1) else {
            eprintln!("usage: gmux rename-tab [-t tab] <name>");
            return Ok(2);
        };
        target = Some(normalize_session_tab_target(value));
        label_start = 2;
    } else if let Some(value) = args
        .first()
        .and_then(|value| value.strip_prefix("--target="))
    {
        target = Some(normalize_session_tab_target(value));
        label_start = 1;
    }
    let tab_id = if let Some(target) = target {
        target
    } else if let Some(value) = args.first() {
        label_start = 1;
        normalize_session_tab_target(value)
    } else {
        eprintln!("usage: gmux rename-tab [-t tab] <name>");
        return Ok(2);
    };
    if args.len() <= label_start {
        eprintln!("usage: gmux rename-tab [-t tab] <name>");
        return Ok(2);
    }
    let mut tab_args = vec!["rename".to_string(), tab_id];
    tab_args.extend(args[label_start..].iter().cloned());
    tab::run_tab_command(&tab_args)
}

fn kill_tab_alias(args: &[String]) -> std::io::Result<i32> {
    let tab_id = match parse_tab_target_alias(args, "usage: gmux kill-tab [-t tab]") {
        Ok(Some(tab_id)) => tab_id,
        Ok(None) => return Ok(0),
        Err(code) => return Ok(code),
    };
    tab::run_tab_command(&["close".to_string(), tab_id])
}

fn parse_tab_target_alias(args: &[String], usage: &str) -> Result<Option<String>, i32> {
    if matches!(
        args.first().map(String::as_str),
        Some("help" | "--help" | "-h")
    ) {
        eprintln!("{usage}");
        return Ok(None);
    }

    let target = match args {
        [value] if value.starts_with("--target=") => value
            .split_once('=')
            .map(|(_, value)| value)
            .unwrap_or_default(),
        [value] => value,
        [flag, value] if flag == "-t" || flag == "--target" => value,
        _ => {
            eprintln!("{usage}");
            return Err(2);
        }
    };
    if target.trim().is_empty() {
        eprintln!("{usage}");
        return Err(2);
    }
    Ok(Some(normalize_session_tab_target(target)))
}

fn normalize_session_tab_target(value: &str) -> String {
    if value.starts_with("t_") || value.contains(':') {
        value.to_string()
    } else if value.parse::<usize>().is_ok() {
        format!("1:{value}")
    } else {
        value.to_string()
    }
}

fn capture_pane_alias(args: &[String]) -> std::io::Result<i32> {
    if matches!(
        args.first().map(String::as_str),
        Some("help" | "--help" | "-h")
    ) {
        eprintln!(
            "usage: gmux capture-pane [-t pane] [-S lines] [-e|--ansi] [--source visible|recent|recent-unwrapped]"
        );
        return Ok(0);
    }

    let mut target = None;
    let mut pane_args = Vec::new();
    let mut index = 0;
    while index < args.len() {
        match args[index].as_str() {
            "-t" | "--target" => {
                let Some(value) = args.get(index + 1) else {
                    eprintln!("missing value for {}", args[index]);
                    return Ok(2);
                };
                target = Some(value.clone());
                index += 2;
            }
            value if value.starts_with("--target=") => {
                target = Some(
                    value
                        .split_once('=')
                        .map(|(_, value)| value.to_string())
                        .unwrap_or_default(),
                );
                index += 1;
            }
            "-S" | "--lines" => {
                let Some(value) = args.get(index + 1) else {
                    eprintln!("missing value for {}", args[index]);
                    return Ok(2);
                };
                pane_args.extend(["--lines".to_string(), value.clone()]);
                index += 2;
            }
            "-e" | "--ansi" => {
                pane_args.push("--ansi".to_string());
                index += 1;
            }
            "--raw" => {
                pane_args.push("--raw".to_string());
                index += 1;
            }
            "--source" | "--format" => {
                let Some(value) = args.get(index + 1) else {
                    eprintln!("missing value for {}", args[index]);
                    return Ok(2);
                };
                pane_args.extend([args[index].clone(), value.clone()]);
                index += 2;
            }
            other => {
                eprintln!("unknown option: {other}");
                return Ok(2);
            }
        }
    }

    let pane_id = match pane_target_or_focused(target)? {
        Ok(pane_id) => pane_id,
        Err(code) => return Ok(code),
    };
    let mut read_args = vec!["read".to_string(), pane_id];
    read_args.extend(pane_args);
    pane::run_pane_command(&read_args)
}

fn select_pane_alias(args: &[String]) -> std::io::Result<i32> {
    if matches!(
        args.first().map(String::as_str),
        Some("help" | "--help" | "-h")
    ) {
        eprintln!("usage: gmux select-pane [-L|-R|-U|-D|-t pane]");
        return Ok(0);
    }

    let mut target = None;
    let mut direction = None;
    let mut index = 0;
    while index < args.len() {
        match args[index].as_str() {
            "-t" | "--target" => {
                let Some(value) = args.get(index + 1) else {
                    eprintln!("missing value for {}", args[index]);
                    return Ok(2);
                };
                target = Some(value.clone());
                index += 2;
            }
            value if value.starts_with("--target=") => {
                target = Some(
                    value
                        .split_once('=')
                        .map(|(_, value)| value.to_string())
                        .unwrap_or_default(),
                );
                index += 1;
            }
            "-L" | "--left" => {
                direction = Some(PaneDirection::Left);
                index += 1;
            }
            "-R" | "--right" => {
                direction = Some(PaneDirection::Right);
                index += 1;
            }
            "-U" | "--up" => {
                direction = Some(PaneDirection::Up);
                index += 1;
            }
            "-D" | "--down" => {
                direction = Some(PaneDirection::Down);
                index += 1;
            }
            other if other.starts_with('-') => {
                eprintln!("unknown option: {other}");
                return Ok(2);
            }
            value if target.is_none() && direction.is_none() => {
                target = Some(value.to_string());
                index += 1;
            }
            _ => {
                eprintln!("usage: gmux select-pane [-L|-R|-U|-D|-t pane]");
                return Ok(2);
            }
        }
    }

    if target.is_some() && direction.is_some() {
        eprintln!("usage: gmux select-pane [-L|-R|-U|-D|-t pane]");
        return Ok(2);
    }

    let method = if let Some(target) = target {
        let target = target.trim();
        if target.is_empty() {
            eprintln!("missing pane target");
            return Ok(2);
        }
        Method::PaneFocus(PaneFocusParams {
            pane_id: Some(normalize_pane_id(target)),
            direction: None,
        })
    } else if let Some(direction) = direction {
        Method::PaneFocus(PaneFocusParams {
            pane_id: None,
            direction: Some(direction),
        })
    } else {
        eprintln!("usage: gmux select-pane [-L|-R|-U|-D|-t pane]");
        return Ok(2);
    };

    print_response(&send_request(&Request {
        id: "cli:select-pane".into(),
        method,
    })?)
}

fn resize_pane_alias(args: &[String]) -> std::io::Result<i32> {
    if matches!(
        args.first().map(String::as_str),
        Some("help" | "--help" | "-h")
    ) {
        eprintln!("usage: gmux resize-pane [-L|-R|-U|-D [N]] [--amount N]");
        return Ok(0);
    }

    let mut direction = None;
    let mut amount = None;
    let mut index = 0;
    while index < args.len() {
        match args[index].as_str() {
            "-L" | "--left" => {
                direction = Some(PaneDirection::Left);
                index += 1;
            }
            "-R" | "--right" => {
                direction = Some(PaneDirection::Right);
                index += 1;
            }
            "-U" | "--up" => {
                direction = Some(PaneDirection::Up);
                index += 1;
            }
            "-D" | "--down" => {
                direction = Some(PaneDirection::Down);
                index += 1;
            }
            "--direction" => {
                let Some(value) = args.get(index + 1) else {
                    eprintln!("missing value for --direction");
                    return Ok(2);
                };
                direction = Some(parse_pane_direction(value)?);
                index += 2;
            }
            "--amount" => {
                let Some(value) = args.get(index + 1) else {
                    eprintln!("missing value for --amount");
                    return Ok(2);
                };
                amount = Some(parse_u16_flag("--amount", value)?);
                index += 2;
            }
            value if amount.is_none() && direction.is_some() && !value.starts_with('-') => {
                amount = Some(parse_u16_flag("amount", value)?);
                index += 1;
            }
            other => {
                eprintln!("unknown option: {other}");
                return Ok(2);
            }
        }
    }

    let Some(direction) = direction else {
        eprintln!("usage: gmux resize-pane [-L|-R|-U|-D [N]] [--amount N]");
        return Ok(2);
    };

    print_response(&send_request(&Request {
        id: "cli:resize-pane".into(),
        method: Method::PaneResize(PaneResizeParams {
            direction,
            amount: amount.unwrap_or(1),
        }),
    })?)
}

fn send_text_alias(args: &[String]) -> std::io::Result<i32> {
    let (pane_id, text) =
        match parse_pane_target_and_payload(args, "usage: gmux send-text [-t pane] <text>")? {
            Ok(Some(parsed)) => parsed,
            Ok(None) => return Ok(0),
            Err(code) => return Ok(code),
        };
    let mut pane_args = vec!["send-text".to_string(), pane_id];
    pane_args.extend(text);
    pane::run_pane_command(&pane_args)
}

fn send_keys_alias(args: &[String]) -> std::io::Result<i32> {
    let (pane_id, keys) = match parse_pane_target_and_payload(
        args,
        "usage: gmux send-keys [-t pane] <key> [key ...]",
    )? {
        Ok(Some(parsed)) => parsed,
        Ok(None) => return Ok(0),
        Err(code) => return Ok(code),
    };
    let mut pane_args = vec!["send-keys".to_string(), pane_id];
    pane_args.extend(keys);
    pane::run_pane_command(&pane_args)
}

type PaneAliasParseResult = Result<Option<(String, Vec<String>)>, i32>;

fn parse_pane_target_and_payload(
    args: &[String],
    usage: &str,
) -> std::io::Result<PaneAliasParseResult> {
    if matches!(
        args.first().map(String::as_str),
        Some("help" | "--help" | "-h")
    ) {
        eprintln!("{usage}");
        return Ok(Ok(None));
    }

    let mut target = None;
    let mut payload = Vec::new();
    let mut index = 0;
    while index < args.len() {
        match args[index].as_str() {
            "-t" | "--target" => {
                let Some(value) = args.get(index + 1) else {
                    eprintln!("missing value for {}", args[index]);
                    return Ok(Err(2));
                };
                target = Some(value.clone());
                index += 2;
            }
            value if value.starts_with("--target=") => {
                target = Some(
                    value
                        .split_once('=')
                        .map(|(_, value)| value.to_string())
                        .unwrap_or_default(),
                );
                index += 1;
            }
            "--" => {
                payload.extend(args[index + 1..].iter().cloned());
                break;
            }
            _ => {
                payload.extend(args[index..].iter().cloned());
                break;
            }
        }
    }

    if payload.is_empty() {
        eprintln!("{usage}");
        return Ok(Err(2));
    }

    let pane_id = match pane_target_or_focused(target)? {
        Ok(pane_id) => pane_id,
        Err(code) => return Ok(Err(code)),
    };
    Ok(Ok(Some((pane_id, payload))))
}

fn pane_target_or_focused(target: Option<String>) -> std::io::Result<Result<String, i32>> {
    if let Some(target) = target {
        let target = target.trim();
        if target.is_empty() {
            eprintln!("missing pane target");
            return Ok(Err(2));
        }
        return Ok(Ok(normalize_pane_id(target)));
    }

    let response = send_request(&Request {
        id: "cli:pane:focused".into(),
        method: Method::PaneList(PaneListParams::default()),
    })?;
    if let Some(error) = response.get("error") {
        eprintln!("{}", serde_json::to_string(error).unwrap());
        return Ok(Err(1));
    }
    let Some(panes) = response["result"]["panes"].as_array() else {
        eprintln!("pane list response did not include panes");
        return Ok(Err(1));
    };
    let Some(pane_id) = panes.iter().find_map(|pane| {
        (pane["focused"].as_bool() == Some(true))
            .then(|| pane["pane_id"].as_str().map(str::to_string))
            .flatten()
    }) else {
        eprintln!("no focused pane; pass -t <pane>");
        return Ok(Err(1));
    };
    Ok(Ok(pane_id))
}

fn split_pane_alias(args: &[String]) -> std::io::Result<i32> {
    if matches!(
        args.first().map(String::as_str),
        Some("help" | "--help" | "-h")
    ) {
        eprintln!("usage: gmux split-pane [-t pane] [-h|-v|--direction right|down] [-c|--cwd PATH] [--focus|--no-focus] [command ...]");
        return Ok(0);
    }

    let mut target = None;
    let mut direction = None;
    let mut cwd = None;
    let mut focus = true;
    let mut command = Vec::new();
    let mut index = 0;
    while index < args.len() {
        match args[index].as_str() {
            "-t" | "--target" => {
                let Some(value) = args.get(index + 1) else {
                    eprintln!("missing value for {}", args[index]);
                    return Ok(2);
                };
                target = Some(value.clone());
                index += 2;
            }
            value if value.starts_with("--target=") => {
                target = Some(
                    value
                        .split_once('=')
                        .map(|(_, value)| value.to_string())
                        .unwrap_or_default(),
                );
                index += 1;
            }
            "-h" | "--horizontal" => {
                direction = Some(SplitDirection::Right);
                index += 1;
            }
            "-v" | "--vertical" => {
                direction = Some(SplitDirection::Down);
                index += 1;
            }
            "--direction" => {
                let Some(value) = args.get(index + 1) else {
                    eprintln!("missing value for --direction");
                    return Ok(2);
                };
                direction = Some(parse_split_direction(value)?);
                index += 2;
            }
            "-c" | "--cwd" => {
                let Some(value) = args.get(index + 1) else {
                    eprintln!("missing value for {}", args[index]);
                    return Ok(2);
                };
                cwd = Some(value.clone());
                index += 2;
            }
            value if value.starts_with("--cwd=") => {
                cwd = Some(
                    value
                        .split_once('=')
                        .map(|(_, value)| value.to_string())
                        .unwrap_or_default(),
                );
                index += 1;
            }
            "--focus" => {
                focus = true;
                index += 1;
            }
            "--no-focus" => {
                focus = false;
                index += 1;
            }
            "--" => {
                command.extend(args[index + 1..].iter().cloned());
                break;
            }
            other if other.starts_with('-') => {
                eprintln!("unknown option: {other}");
                return Ok(2);
            }
            value if index == 0 && target.is_none() && looks_like_pane_target(value) => {
                target = Some(value.to_string());
                index += 1;
            }
            _ => {
                command.extend(args[index..].iter().cloned());
                break;
            }
        }
    }

    let pane_id = match pane_target_or_focused(target)? {
        Ok(pane_id) => pane_id,
        Err(code) => return Ok(code),
    };
    let response = send_request(&Request {
        id: "cli:split-pane".into(),
        method: Method::PaneSplit(PaneSplitParams {
            target_pane_id: pane_id,
            direction: direction.unwrap_or(SplitDirection::Down),
            cwd,
            focus,
        }),
    })?;
    if response.get("error").is_some() {
        return print_response(&response);
    }

    if !command.is_empty() {
        let Some(new_pane_id) = response["result"]["pane"]["pane_id"].as_str() else {
            eprintln!("split response did not include pane id");
            return Ok(1);
        };
        let send_response = send_request(&Request {
            id: "cli:split-pane:command".into(),
            method: Method::PaneSendInput(PaneSendInputParams {
                pane_id: new_pane_id.to_string(),
                text: command.join(" "),
                keys: vec!["Enter".into()],
            }),
        })?;
        if send_response.get("error").is_some() {
            eprintln!("{}", serde_json::to_string(&send_response).unwrap());
            return Ok(1);
        }
    }

    print_response(&response)
}

fn looks_like_pane_target(value: &str) -> bool {
    value.starts_with("p_") || value.contains('-')
}

fn kill_pane_alias(args: &[String]) -> std::io::Result<i32> {
    if matches!(
        args.first().map(String::as_str),
        Some("help" | "--help" | "-h")
    ) {
        eprintln!("usage: gmux kill-pane [-t pane]");
        return Ok(0);
    }
    let target = match parse_optional_target(args, "usage: gmux kill-pane [-t pane]") {
        Ok(target) => target,
        Err(code) => return Ok(code),
    };
    let pane_id = match pane_target_or_focused(target)? {
        Ok(pane_id) => pane_id,
        Err(code) => return Ok(code),
    };
    pane::run_pane_command(&["close".to_string(), pane_id])
}

fn parse_optional_target(args: &[String], usage: &str) -> Result<Option<String>, i32> {
    match args {
        [] => Ok(None),
        [value] if value.starts_with("--target=") => value
            .split_once('=')
            .map(|(_, value)| value.to_string())
            .filter(|value| !value.trim().is_empty())
            .map(Some)
            .ok_or_else(|| {
                eprintln!("{usage}");
                2
            }),
        [value] => Ok(Some(value.clone())),
        [flag, value] if flag == "-t" || flag == "--target" => Ok(Some(value.clone())),
        _ => {
            eprintln!("{usage}");
            Err(2)
        }
    }
}

fn detach_alias(args: &[String]) -> std::io::Result<i32> {
    if !args.is_empty()
        && !matches!(
            args.first().map(String::as_str),
            Some("help" | "--help" | "-h")
        )
    {
        eprintln!("usage: gmux detach");
        return Ok(2);
    }
    eprintln!("Use the detach keybinding inside gmux: prefix+d.");
    Ok(0)
}

fn run_config_command(args: &[String]) -> std::io::Result<i32> {
    let Some(subcommand) = args.first().map(|arg| arg.as_str()) else {
        print_config_help();
        return Ok(2);
    };

    match subcommand {
        "reset-keys" => config_reset_keys(&args[1..]),
        "help" | "--help" | "-h" => {
            print_config_help();
            Ok(0)
        }
        _ => {
            print_config_help();
            Ok(2)
        }
    }
}

fn config_reset_keys(args: &[String]) -> std::io::Result<i32> {
    if !args.is_empty() {
        eprintln!("usage: gmux config reset-keys");
        return Ok(2);
    }

    let path = crate::config::config_path();
    if !path.exists() {
        println!(
            "No config file found at {}. Built-in v2 keybindings already apply.",
            path.display()
        );
        return Ok(0);
    }

    let content = std::fs::read_to_string(&path)?;
    let parsed = match content.parse::<toml::Value>() {
        Ok(value) => value,
        Err(err) => {
            eprintln!(
                "config file at {} is invalid TOML: {err}. Fix it manually or move it aside to use defaults.",
                path.display()
            );
            return Ok(1);
        }
    };
    let Some(table) = parsed.as_table() else {
        eprintln!(
            "config file at {} is invalid TOML: top-level config must be a table.",
            path.display()
        );
        return Ok(1);
    };

    if !table.contains_key("keys") {
        println!(
            "No [keys] config found in {}. Built-in v2 keybindings already apply.",
            path.display()
        );
        return Ok(0);
    }

    let (updated, removed) = crate::config::remove_keybinding_config_sections(&content);
    if !removed {
        eprintln!(
            "could not safely remove keybinding config from {} without rewriting comments; edit the file manually or remove the top-level keys setting.",
            path.display()
        );
        return Ok(1);
    }
    if let Err(err) = updated.parse::<toml::Value>() {
        eprintln!(
            "removing keybinding config would make {} invalid TOML: {err}; leaving config unchanged",
            path.display()
        );
        return Ok(1);
    }

    let backup_path = key_config_backup_path(&path);
    std::fs::copy(&path, &backup_path)?;
    std::fs::write(&path, updated)?;

    println!("Created backup: {}", backup_path.display());
    println!(
        "Removed [keys], [keys.indexed], and [[keys.command]] from {}.",
        path.display()
    );
    println!("Built-in v2 keybindings will apply after Gmux restarts or reloads config.");
    println!("If a Gmux server is running, run `gmux server reload-config` to apply this now.");
    println!(
        "To restore: cp {} {}",
        backup_path.display(),
        path.display()
    );
    Ok(0)
}

fn key_config_backup_path(path: &std::path::Path) -> std::path::PathBuf {
    let timestamp = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_secs())
        .unwrap_or(0);
    let file_name = path
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or("config.toml");
    path.with_file_name(format!("{file_name}.bak-keybind-v2-{timestamp}"))
}

fn run_terminal_command(args: &[String]) -> std::io::Result<i32> {
    let Some(subcommand) = args.first().map(|arg| arg.as_str()) else {
        print_terminal_help();
        return Ok(2);
    };

    match subcommand {
        "attach" => terminal_attach(&args[1..]),
        "help" | "--help" | "-h" => {
            print_terminal_help();
            Ok(0)
        }
        _ => {
            print_terminal_help();
            Ok(2)
        }
    }
}

fn run_wait_command(args: &[String]) -> std::io::Result<i32> {
    let Some(subcommand) = args.first().map(|arg| arg.as_str()) else {
        print_wait_help();
        return Ok(2);
    };

    match subcommand {
        "output" => wait_output(&args[1..]),
        "help" | "--help" | "-h" => {
            print_wait_help();
            Ok(0)
        }
        _ => {
            print_wait_help();
            Ok(2)
        }
    }
}

fn run_session_command(args: &[String]) -> std::io::Result<i32> {
    let Some(subcommand) = args.first().map(|arg| arg.as_str()) else {
        print_session_help();
        return Ok(2);
    };

    match subcommand {
        "list" => session_list(&args[1..], "usage: gmux session list [--json]"),
        "attach" => session_attach_help(&args[1..]),
        "stop" => session_stop(&args[1..]),
        "rename" => session_rename(&args[1..]),
        "delete" => session_delete(&args[1..]),
        "help" | "--help" | "-h" => {
            print_session_help();
            Ok(0)
        }
        _ => {
            print_session_help();
            Ok(2)
        }
    }
}

fn session_attach_help(args: &[String]) -> std::io::Result<i32> {
    if matches!(
        args.first().map(String::as_str),
        Some("help" | "--help" | "-h")
    ) {
        eprintln!("usage: gmux session attach <name>");
        return Ok(0);
    }
    eprintln!("usage: gmux session attach <name>");
    Ok(2)
}

fn session_list(args: &[String], usage: &str) -> std::io::Result<i32> {
    if matches!(
        args.first().map(String::as_str),
        Some("help" | "--help" | "-h")
    ) {
        eprintln!("{usage}");
        return Ok(0);
    }

    let json = match parse_session_json_only(args, usage) {
        Ok(json) => json,
        Err(code) => return Ok(code),
    };

    let sessions = crate::session::list_sessions()?;
    if json {
        _print_json(&serde_json::json!({
            "sessions": sessions,
        }));
    } else {
        print_session_table(&sessions);
    }
    Ok(0)
}

fn session_stop(args: &[String]) -> std::io::Result<i32> {
    let (name, json) =
        match parse_session_name_and_json(args, "usage: gmux session stop <name> [--json]") {
            Ok(parsed) => parsed,
            Err(code) => return Ok(code),
        };

    let target = match crate::session::parse_target_name(&name) {
        Ok(target) => target,
        Err(message) => {
            print_session_error("invalid_session_name", &message);
            return Ok(1);
        }
    };
    match crate::session::stop_session(target.as_deref()) {
        Ok(session) => {
            if json {
                _print_json(&serde_json::json!({
                    "stopped": true,
                    "session": session,
                }));
            } else {
                println!("stopped session {}", session.name);
            }
            Ok(0)
        }
        Err(message) => {
            print_session_error("session_stop_failed", &message);
            Ok(1)
        }
    }
}

fn session_rename(args: &[String]) -> std::io::Result<i32> {
    let (old_name, new_name, json) = match parse_session_rename_args(args) {
        Ok(parsed) => parsed,
        Err(code) => return Ok(code),
    };

    match crate::session::rename_session(&old_name, &new_name) {
        Ok(session) => {
            if json {
                _print_json(&serde_json::json!({
                    "renamed": true,
                    "from": old_name,
                    "session": session,
                }));
            } else {
                println!("renamed session {old_name} to {new_name}");
            }
            Ok(0)
        }
        Err(message) => {
            print_session_error("session_rename_failed", &message);
            Ok(1)
        }
    }
}

fn session_delete(args: &[String]) -> std::io::Result<i32> {
    let (name, json) =
        match parse_session_name_and_json(args, "usage: gmux session delete <name> [--json]") {
            Ok(parsed) => parsed,
            Err(code) => return Ok(code),
        };

    match crate::session::delete_session(&name) {
        Ok(session) => {
            if json {
                _print_json(&serde_json::json!({
                    "deleted": true,
                    "session": session,
                }));
            } else {
                println!("deleted session {}", session.name);
            }
            Ok(0)
        }
        Err(message) => {
            print_session_error("session_delete_failed", &message);
            Ok(1)
        }
    }
}

fn terminal_attach(args: &[String]) -> std::io::Result<i32> {
    let (terminal_id, takeover) = match parse_attach_target(
        args,
        "usage: gmux terminal attach <terminal_id> [--takeover]",
    ) {
        Ok(parsed) => parsed,
        Err(code) => return Ok(code),
    };
    crate::client::run_terminal_attach(terminal_id, takeover)?;
    Ok(0)
}

pub(super) fn parse_attach_target(args: &[String], usage: &str) -> Result<(String, bool), i32> {
    let Some(target) = args.first() else {
        eprintln!("{usage}");
        return Err(2);
    };
    let mut takeover = false;
    for arg in &args[1..] {
        match arg.as_str() {
            "--takeover" => takeover = true,
            "help" | "--help" | "-h" => {
                eprintln!("{usage}");
                return Err(0);
            }
            other => {
                eprintln!("unknown option: {other}");
                return Err(2);
            }
        }
    }
    Ok((target.clone(), takeover))
}

fn wait_output(args: &[String]) -> std::io::Result<i32> {
    let Some(raw_pane_id) = args.first() else {
        eprintln!("usage: gmux wait output <pane_id> --match <text> [--source visible|recent|recent-unwrapped] [--lines N] [--timeout MS] [--regex]");
        return Ok(2);
    };

    let pane_id = normalize_pane_id(raw_pane_id);
    let mut source = ReadSource::Recent;
    let mut lines = None;
    let mut timeout_ms = None;
    let mut strip_ansi = true;
    let mut regex = false;
    let mut match_value = None;

    let mut index = 1;
    while index < args.len() {
        match args[index].as_str() {
            "--match" => {
                let Some(value) = args.get(index + 1) else {
                    eprintln!("missing value for --match");
                    return Ok(2);
                };
                match_value = Some(value.clone());
                index += 2;
            }
            "--source" => {
                let Some(value) = args.get(index + 1) else {
                    eprintln!("missing value for --source");
                    return Ok(2);
                };
                source = parse_read_source(value)?;
                index += 2;
            }
            "--lines" => {
                let Some(value) = args.get(index + 1) else {
                    eprintln!("missing value for --lines");
                    return Ok(2);
                };
                lines = Some(parse_u32_flag("--lines", value)?);
                index += 2;
            }
            "--timeout" => {
                let Some(value) = args.get(index + 1) else {
                    eprintln!("missing value for --timeout");
                    return Ok(2);
                };
                timeout_ms = Some(parse_u64_flag("--timeout", value)?);
                index += 2;
            }
            "--regex" => {
                regex = true;
                index += 1;
            }
            "--raw" => {
                strip_ansi = false;
                index += 1;
            }
            other => {
                eprintln!("unknown option: {other}");
                return Ok(2);
            }
        }
    }

    let Some(match_value) = match_value else {
        eprintln!("missing required --match");
        return Ok(2);
    };

    let matcher = if regex {
        OutputMatch::Regex { value: match_value }
    } else {
        OutputMatch::Substring { value: match_value }
    };

    let response = send_request(&Request {
        id: "cli:wait:output".into(),
        method: Method::PaneWaitForOutput(PaneWaitForOutputParams {
            pane_id,
            source,
            lines,
            r#match: matcher,
            timeout_ms,
            strip_ansi,
        }),
    })?;

    if response.get("error").is_some() {
        eprintln!("{}", serde_json::to_string(&response).unwrap());
        return Ok(1);
    }

    println!("{}", serde_json::to_string(&response).unwrap());
    Ok(0)
}

pub(super) fn print_response(response: &serde_json::Value) -> std::io::Result<i32> {
    if response.get("error").is_some() {
        eprintln!("{}", serde_json::to_string(response).unwrap());
        return Ok(1);
    }

    println!("{}", serde_json::to_string(response).unwrap());
    Ok(0)
}

pub(super) fn send_ok_request(method: Method) -> std::io::Result<i32> {
    let response = send_request(&Request {
        id: "cli:request".into(),
        method,
    })?;

    if response.get("error").is_some() {
        eprintln!("{}", serde_json::to_string(&response).unwrap());
        return Ok(1);
    }

    Ok(0)
}

pub(super) fn send_request(request: &Request) -> std::io::Result<serde_json::Value> {
    ApiClient::local()
        .request_value(request)
        .map_err(api_client_error_to_io)
}

fn api_client_error_to_io(err: ApiClientError) -> std::io::Error {
    match err {
        ApiClientError::Io(err) => err,
        err => std::io::Error::other(err),
    }
}

pub(super) fn normalize_tab_id(value: &str) -> String {
    value.to_string()
}

pub(super) fn normalize_pane_id(value: &str) -> String {
    value.to_string()
}

pub(super) fn parse_split_direction(value: &str) -> std::io::Result<SplitDirection> {
    match value {
        "right" => Ok(SplitDirection::Right),
        "down" => Ok(SplitDirection::Down),
        _ => Err(std::io::Error::other(format!(
            "invalid split direction: {value}"
        ))),
    }
}

pub(super) fn parse_pane_direction(value: &str) -> std::io::Result<PaneDirection> {
    match value {
        "left" => Ok(PaneDirection::Left),
        "right" => Ok(PaneDirection::Right),
        "up" => Ok(PaneDirection::Up),
        "down" => Ok(PaneDirection::Down),
        _ => Err(std::io::Error::other(format!(
            "invalid pane direction: {value}"
        ))),
    }
}

pub(super) fn parse_read_source(value: &str) -> std::io::Result<ReadSource> {
    match value {
        "visible" => Ok(ReadSource::Visible),
        "recent" => Ok(ReadSource::Recent),
        "recent-unwrapped" | "recent_unwrapped" => Ok(ReadSource::RecentUnwrapped),
        _ => Err(std::io::Error::other(format!(
            "invalid read source: {value}"
        ))),
    }
}

pub(super) fn parse_read_format(value: &str) -> std::io::Result<ReadFormat> {
    match value {
        "text" => Ok(ReadFormat::Text),
        "ansi" => Ok(ReadFormat::Ansi),
        _ => Err(std::io::Error::other(format!(
            "invalid read format: {value}"
        ))),
    }
}

pub(super) fn parse_u32_flag(flag: &str, value: &str) -> std::io::Result<u32> {
    value
        .parse::<u32>()
        .map_err(|_| std::io::Error::other(format!("invalid value for {flag}: {value}")))
}

pub(super) fn parse_u16_flag(flag: &str, value: &str) -> std::io::Result<u16> {
    value
        .parse::<u16>()
        .map_err(|_| std::io::Error::other(format!("invalid value for {flag}: {value}")))
}

pub(super) fn parse_u64_flag(flag: &str, value: &str) -> std::io::Result<u64> {
    value
        .parse::<u64>()
        .map_err(|_| std::io::Error::other(format!("invalid value for {flag}: {value}")))
}

fn parse_session_json_only(args: &[String], usage: &str) -> Result<bool, i32> {
    match args {
        [] => Ok(false),
        [flag] if flag == "--json" => Ok(true),
        _ => {
            eprintln!("{usage}");
            Err(2)
        }
    }
}

fn parse_session_name_and_json(args: &[String], usage: &str) -> Result<(String, bool), i32> {
    let mut name = None;
    let mut json = false;
    for arg in args {
        if arg == "--json" {
            json = true;
        } else if name.is_none() {
            name = Some(arg.clone());
        } else {
            eprintln!("{usage}");
            return Err(2);
        }
    }

    let Some(name) = name else {
        eprintln!("{usage}");
        return Err(2);
    };
    Ok((name, json))
}

fn parse_session_rename_args(args: &[String]) -> Result<(String, String, bool), i32> {
    let usage = "usage: gmux session rename <old> <new> [--json]";
    if matches!(
        args.first().map(String::as_str),
        Some("help" | "--help" | "-h")
    ) {
        eprintln!("{usage}");
        return Err(0);
    }

    let mut old_name = None;
    let mut new_name = None;
    let mut json = false;
    for arg in args {
        if arg == "--json" {
            json = true;
        } else if old_name.is_none() {
            old_name = Some(arg.clone());
        } else if new_name.is_none() {
            new_name = Some(arg.clone());
        } else {
            eprintln!("{usage}");
            return Err(2);
        }
    }

    let (Some(old_name), Some(new_name)) = (old_name, new_name) else {
        eprintln!("{usage}");
        return Err(2);
    };
    Ok((old_name, new_name, json))
}

fn print_session_table(sessions: &[crate::session::SessionInfo]) {
    println!("{:<20} {:<8} {:<48} socket", "name", "status", "directory");
    for session in sessions {
        println!(
            "{:<20} {:<8} {:<48} {}",
            session.name,
            if session.running {
                "running"
            } else {
                "stopped"
            },
            session.session_dir,
            session.socket_path
        );
    }
}

fn print_session_error(code: &str, message: &str) {
    eprintln!(
        "{}",
        serde_json::to_string(&serde_json::json!({
            "error": {
                "code": code,
                "message": message,
            }
        }))
        .unwrap()
    );
}

fn print_config_help() {
    eprintln!("gmux config commands:");
    eprintln!("  gmux config reset-keys  back up config.toml and remove custom keybindings");
}

fn print_terminal_help() {
    eprintln!("gmux terminal commands:");
    eprintln!("  gmux terminal attach <terminal_id> [--takeover]");
    eprintln!("  detach from direct attach with ctrl+b d; send literal ctrl+b with ctrl+b ctrl+b");
}

fn print_wait_help() {
    eprintln!("gmux wait commands:");
    eprintln!("  gmux wait output <pane_id> --match <text> [--source visible|recent|recent-unwrapped] [--lines N] [--timeout MS] [--regex] [--raw]");
}

fn print_session_help() {
    eprintln!("gmux session commands:");
    eprintln!("  gmux ls [--json]");
    eprintln!("  gmux new [-s name]");
    eprintln!("  gmux attach [-t name]");
    eprintln!("  gmux kill-session [-t name] [--json]");
    eprintln!("  gmux session list [--json]");
    eprintln!("  gmux session attach <name>");
    eprintln!("  gmux session stop <name> [--json]");
    eprintln!("  gmux session rename <old> <new> [--json]");
    eprintln!("  gmux session delete <name> [--json]");
    eprintln!("  rename and delete only support stopped sessions");
    eprintln!("  use 'default' as <name> to target the default session");
}

fn _print_json<T: Serialize>(value: &T) {
    println!("{}", serde_json::to_string(value).unwrap());
}

#[cfg(test)]
mod tests {}
