use std::path::Path;

use crate::api::schema::{EmptyParams, Method, Request, ServerLiveHandoffParams};

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
    if !args.is_empty() {
        eprintln!("usage: gmux server stop");
        return Ok(2);
    }

    super::send_ok_request(Method::ServerStop(EmptyParams::default()))
}

fn server_reload_config(args: &[String]) -> std::io::Result<i32> {
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
    let Some(mut params) = parse_live_handoff_params(args) else {
        eprintln!(
            "usage: gmux server live-handoff [--import-exe <path>] [--expected-protocol <n>] [--expected-version <version>]"
        );
        return Ok(2);
    };

    if params.import_exe.is_none() {
        let current_exe = std::env::current_exe().map_err(|err| {
            std::io::Error::new(
                err.kind(),
                format!("failed to determine gmux executable path: {err}"),
            )
        })?;
        apply_default_import_exe(&mut params, &current_exe);
    }

    let response = super::send_request(&Request {
        id: "cli:server:live-handoff".into(),
        method: Method::ServerLiveHandoff(params),
    })?;
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

fn parse_live_handoff_params(args: &[String]) -> Option<ServerLiveHandoffParams> {
    let mut params = ServerLiveHandoffParams::default();
    let mut idx = 0;
    while idx < args.len() {
        let arg = &args[idx];
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
    Some(params)
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

        let params = parse_live_handoff_params(&args).expect("params");

        assert_eq!(
            params.import_exe.as_deref(),
            Some("/home/me/.local/bin/gmux")
        );
        assert_eq!(params.expected_protocol, Some(9));
        assert_eq!(params.expected_version.as_deref(), Some("0.6.2"));
    }

    #[test]
    fn live_handoff_defaults_import_exe_to_invoking_binary() {
        let mut params = parse_live_handoff_params(&[]).expect("params");

        apply_default_import_exe(&mut params, Path::new("/tmp/gmux-dev"));

        assert_eq!(params.import_exe.as_deref(), Some("/tmp/gmux-dev"));
    }

    #[test]
    fn live_handoff_default_import_exe_preserves_explicit_value() {
        let args = vec!["--import-exe".to_string(), "/tmp/explicit-gmux".to_string()];
        let mut params = parse_live_handoff_params(&args).expect("params");

        apply_default_import_exe(&mut params, Path::new("/tmp/gmux-dev"));

        assert_eq!(params.import_exe.as_deref(), Some("/tmp/explicit-gmux"));
    }
}
