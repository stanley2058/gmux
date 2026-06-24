use std::path::{Path, PathBuf};

use crate::api::client::{ApiClient, ConnectionTarget};
use crate::api::schema::{EmptyParams, Method, Request, ServerLiveHandoffParams};

struct LiveHandoffCliArgs {
    params: ServerLiveHandoffParams,
    all_sessions: bool,
}

pub(super) fn run_server_command(args: &[String]) -> std::io::Result<Option<i32>> {
    let Some(subcommand) = args.first().map(|arg| arg.as_str()) else {
        return Ok(None);
    };

    match subcommand {
        "stop" => server_stop(&args[1..]).map(Some),
        "live-handoff" => server_live_handoff(&args[1..]).map(Some),
        "--handoff-import" => Ok(None),
        "reload-config" => server_reload_config(&args[1..]).map(Some),
        "help" | "--help" | "-h" => {
            print_server_help();
            Ok(Some(0))
        }
        _ => {
            print_server_help();
            Ok(Some(2))
        }
    }
}

fn server_stop(args: &[String]) -> std::io::Result<i32> {
    if super::is_help_request(args) {
        eprintln!("usage: gmux server stop");
        return Ok(0);
    }
    if !args.is_empty() {
        eprintln!("usage: gmux server stop");
        return Ok(2);
    }

    super::send_ok_request(Method::ServerStop(EmptyParams::default()))
}

fn server_reload_config(args: &[String]) -> std::io::Result<i32> {
    if super::is_help_request(args) {
        eprintln!("usage: gmux server reload-config");
        return Ok(0);
    }
    if !args.is_empty() {
        eprintln!("usage: gmux server reload-config");
        return Ok(2);
    }

    super::print_response(&super::send_request(&Request {
        id: "cli:server:reload-config".into(),
        method: Method::ServerReloadConfig(EmptyParams::default()),
    })?)
}

fn server_live_handoff(args: &[String]) -> std::io::Result<i32> {
    if super::is_help_request(args) {
        eprintln!(
            "usage: gmux server live-handoff [--all-sessions] [--import-exe <path>] [--expected-protocol <n>] [--expected-version <version>]"
        );
        return Ok(0);
    }
    let Some(mut parsed) = parse_live_handoff_args(args) else {
        eprintln!(
            "usage: gmux server live-handoff [--all-sessions] [--import-exe <path>] [--expected-protocol <n>] [--expected-version <version>]"
        );
        return Ok(2);
    };

    if parsed.params.import_exe.is_none() {
        let current_exe = std::env::current_exe().map_err(|err| {
            std::io::Error::new(
                err.kind(),
                format!("failed to determine gmux executable path: {err}"),
            )
        })?;
        apply_default_import_exe(&mut parsed.params, &current_exe);
    }

    if parsed.all_sessions {
        return server_live_handoff_all_sessions(parsed.params);
    }

    let response = send_live_handoff_request("cli:server:live-handoff", parsed.params, None)?;
    if response.get("error").is_some() {
        let rendered = serde_json::to_string(&response).unwrap_or_else(|err| {
            format!(
                "{{\"error\":{{\"code\":\"render_failed\",\"message\":\"failed to render error response: {err}\"}}}}"
            )
        });
        eprintln!("{rendered}");
        return Ok(1);
    }

    eprintln!(
        "live handoff complete; server log: {}",
        crate::session::data_dir().join("gmux-server.log").display()
    );
    Ok(0)
}

fn server_live_handoff_all_sessions(params: ServerLiveHandoffParams) -> std::io::Result<i32> {
    let running_sessions = crate::session::list_sessions()?
        .into_iter()
        .filter(|session| session.running)
        .collect::<Vec<_>>();

    if running_sessions.is_empty() {
        eprintln!("no running gmux sessions to hand off");
        return Ok(0);
    }

    let mut failed = false;
    for session in running_sessions {
        let request_id = format!("cli:server:live-handoff:{}", session.name);
        let target = Some(PathBuf::from(&session.socket_path));
        let response = match send_live_handoff_request(&request_id, params.clone(), target) {
            Ok(response) => response,
            Err(err) => {
                failed = true;
                eprintln!("session {}: live handoff failed: {err}", session.name);
                continue;
            }
        };
        if response.get("error").is_some() {
            failed = true;
            let rendered = serde_json::to_string(&response).unwrap_or_else(|err| {
                format!(
                    "{{\"error\":{{\"code\":\"render_failed\",\"message\":\"failed to render error response: {err}\"}}}}"
                )
            });
            eprintln!("session {}: {rendered}", session.name);
        } else {
            eprintln!(
                "session {}: live handoff complete; server log: {}",
                session.name,
                PathBuf::from(&session.session_dir)
                    .join("gmux-server.log")
                    .display()
            );
        }
    }

    Ok(if failed { 1 } else { 0 })
}

fn send_live_handoff_request(
    id: &str,
    params: ServerLiveHandoffParams,
    socket_path: Option<PathBuf>,
) -> std::io::Result<serde_json::Value> {
    let request = Request {
        id: id.to_string(),
        method: Method::ServerLiveHandoff(params),
    };
    match socket_path {
        Some(socket_path) => super::send_request_to_client(
            ApiClient::for_target(ConnectionTarget::SocketPath(socket_path)),
            &request,
        ),
        None => super::send_request(&request),
    }
}

fn parse_live_handoff_args(args: &[String]) -> Option<LiveHandoffCliArgs> {
    let mut params = ServerLiveHandoffParams::default();
    let mut all_sessions = false;
    let mut idx = 0;
    while idx < args.len() {
        let arg = &args[idx];
        if arg == "--all-sessions" {
            all_sessions = true;
            idx += 1;
            continue;
        }
        let (flag, value) = if let Some((flag, value)) = arg.split_once('=') {
            (flag, Some(value.to_string()))
        } else {
            let value = args.get(idx + 1).cloned();
            idx += 1;
            (arg.as_str(), value)
        };
        let value = value?;
        match flag {
            "--import-exe" => params.import_exe = Some(value),
            "--expected-protocol" => {
                params.expected_protocol = Some(value.parse().ok()?);
            }
            "--expected-version" => params.expected_version = Some(value),
            _ => return None,
        }
        idx += 1;
    }
    Some(LiveHandoffCliArgs {
        params,
        all_sessions,
    })
}

fn apply_default_import_exe(params: &mut ServerLiveHandoffParams, current_exe: &Path) {
    if params.import_exe.is_none() {
        params.import_exe = Some(current_exe.display().to_string());
    }
}

fn print_server_help() {
    eprintln!("gmux server commands:");
    eprintln!("  gmux server                run as headless server");
    eprintln!("  gmux server stop           stop the running server via the API socket");
    eprintln!("  gmux server live-handoff   hand off live panes to a new local server");
    eprintln!("    --all-sessions           hand off every running local session");
    eprintln!("    --import-exe PATH        executable to import after handoff");
    eprintln!("    --expected-protocol N    require the imported server protocol");
    eprintln!("    --expected-version TEXT  require the imported server version");
    eprintln!("  gmux server reload-config  reload config.toml in the running server");
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn live_handoff_params_parse_remote_update_fields() {
        let args = vec![
            "--import-exe".to_string(),
            "/home/me/.local/bin/gmux".to_string(),
            "--expected-protocol=9".to_string(),
            "--expected-version".to_string(),
            "0.6.2".to_string(),
        ];

        let parsed = parse_live_handoff_args(&args).expect("params");

        assert_eq!(
            parsed.params.import_exe.as_deref(),
            Some("/home/me/.local/bin/gmux")
        );
        assert_eq!(parsed.params.expected_protocol, Some(9));
        assert_eq!(parsed.params.expected_version.as_deref(), Some("0.6.2"));
        assert!(!parsed.all_sessions);
    }

    #[test]
    fn live_handoff_defaults_import_exe_to_invoking_binary() {
        let mut params = parse_live_handoff_args(&[]).expect("params").params;

        apply_default_import_exe(&mut params, Path::new("/tmp/gmux-dev"));

        assert_eq!(params.import_exe.as_deref(), Some("/tmp/gmux-dev"));
    }

    #[test]
    fn live_handoff_default_import_exe_preserves_explicit_value() {
        let args = vec!["--import-exe".to_string(), "/tmp/explicit-gmux".to_string()];
        let mut params = parse_live_handoff_args(&args).expect("params").params;

        apply_default_import_exe(&mut params, Path::new("/tmp/gmux-dev"));

        assert_eq!(params.import_exe.as_deref(), Some("/tmp/explicit-gmux"));
    }

    #[test]
    fn live_handoff_parses_all_sessions_without_value() {
        let args = vec![
            "--all-sessions".to_string(),
            "--expected-protocol".to_string(),
            "15".to_string(),
        ];

        let parsed = parse_live_handoff_args(&args).expect("params");

        assert!(parsed.all_sessions);
        assert_eq!(parsed.params.expected_protocol, Some(15));
    }
}
