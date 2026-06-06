use bytes::Bytes;

use crate::api::schema::{
    EventData, EventEnvelope, EventKind, PaneDirection, PaneFocusParams, PaneInfo, PaneListParams,
    PaneReadParams, PaneReadResult, PaneRenameParams, PaneResizeParams, PaneSendInputParams,
    PaneSendKeysParams, PaneSendTextParams, PaneSplitParams, PaneTarget, ReadFormat, ReadSource,
    ResponseResult,
};
use crate::app::{App, Mode};
use crate::layout::{NavDirection, PaneId};

use super::super::api_helpers::{encode_api_keys, encode_api_text};
use super::responses::{encode_error, encode_success};

impl App {
    pub(super) fn handle_pane_split(&mut self, id: String, params: PaneSplitParams) -> String {
        let Some((ws_idx, target_pane_id)) = self.parse_pane_id(&params.target_pane_id) else {
            return pane_not_found(id, &params.target_pane_id);
        };
        let Some(target_tab_idx) = self
            .state
            .sessions()
            .get(ws_idx)
            .and_then(|ws| ws.find_tab_index_for_pane(target_pane_id))
        else {
            return pane_not_found(id, &params.target_pane_id);
        };
        let Some(flat_tab_idx) = self.state.flattened_tab_index(ws_idx, target_tab_idx) else {
            return pane_not_found(id, &params.target_pane_id);
        };
        let (rows, cols) = self.state.estimate_pane_size();
        let split_cwd = params.cwd.map(std::path::PathBuf::from).or_else(|| {
            let follow_cwd = self.state.sessions().get(ws_idx).and_then(|container| {
                container.tabs.get(target_tab_idx)?.cwd_for_pane(
                    target_pane_id,
                    &self.state.terminals,
                    &self.terminal_runtimes,
                )
            });
            Some(self.resolve_new_terminal_cwd(follow_cwd))
        });
        let default_shell = self.state.default_shell.clone();
        let shell_mode = self.state.shell_mode;
        let scrollback_limit_bytes = self.state.pane_scrollback_limit_bytes;
        let host_terminal_theme = self.state.host_terminal_theme;
        let previous_focus = self.state.current_pane_focus_target();
        self.state.collapse_to_single_session();
        let ws_idx = 0;
        let Some(ws) = self.state.sessions_mut().get_mut(ws_idx) else {
            return pane_not_found(id, &params.target_pane_id);
        };
        let direction = match params.direction {
            crate::api::schema::SplitDirection::Right => ratatui::layout::Direction::Horizontal,
            crate::api::schema::SplitDirection::Down => ratatui::layout::Direction::Vertical,
        };
        let (target_tab_idx, new_pane) = match ws.split_pane(
            target_pane_id,
            direction,
            rows,
            cols,
            split_cwd,
            scrollback_limit_bytes,
            host_terminal_theme,
            crate::pane::PaneShellConfig::new(&default_shell, shell_mode),
            params.focus,
        ) {
            Some(Ok(result)) => result,
            Some(Err(err)) => return encode_error(id, "pane_split_failed", err.to_string()),
            None => return pane_not_found(id, &params.target_pane_id),
        };
        debug_assert_eq!(target_tab_idx, flat_tab_idx);
        if params.focus {
            self.state.focus_session_tab(ws_idx, target_tab_idx);
            self.state
                .record_pane_focus_change(previous_focus, ws_idx, new_pane.pane_id);
            self.state.mode = Mode::Terminal;
        }
        self.terminal_runtimes
            .insert(new_pane.terminal.id.clone(), new_pane.runtime);
        self.state
            .remove_alias_shadowed_by_new_pane(new_pane.pane_id);
        self.state
            .terminals
            .insert(new_pane.terminal.id.clone(), new_pane.terminal);
        self.schedule_session_save();
        let pane = self.pane_info(ws_idx, new_pane.pane_id).unwrap();
        self.emit_event(EventEnvelope {
            event: EventKind::PaneCreated,
            data: EventData::PaneCreated { pane: pane.clone() },
        });

        encode_success(id, ResponseResult::PaneInfo { pane })
    }

    pub(super) fn handle_pane_list(&mut self, id: String, params: PaneListParams) -> String {
        let PaneListParams {} = params;
        self.state.collapse_to_single_session();
        encode_success(
            id,
            ResponseResult::PaneList {
                panes: self.collect_panes(),
            },
        )
    }

    pub(super) fn handle_pane_get(&mut self, id: String, target: PaneTarget) -> String {
        let Some((ws_idx, _, pane_id)) = self.canonicalize_pane_target(&target.pane_id) else {
            return pane_not_found(id, &target.pane_id);
        };
        let Some(pane) = self.pane_info(ws_idx, pane_id) else {
            return pane_not_found(id, &target.pane_id);
        };

        encode_success(id, ResponseResult::PaneInfo { pane })
    }

    pub(super) fn handle_pane_focus(&mut self, id: String, params: PaneFocusParams) -> String {
        match (params.pane_id, params.direction) {
            (Some(pane_id), None) => {
                let Some((ws_idx, _, raw_pane_id)) = self.canonicalize_pane_target(&pane_id) else {
                    return pane_not_found(id, &pane_id);
                };
                self.state.focus_pane_in_session_at(ws_idx, raw_pane_id);
                self.state.mode = Mode::Terminal;
                let Some(pane) = self.pane_info_by_raw_id(raw_pane_id) else {
                    return pane_not_found(id, &pane_id);
                };
                encode_success(id, ResponseResult::PaneInfo { pane })
            }
            (None, Some(direction)) => {
                self.state.navigate_pane(nav_direction_from_api(direction));
                self.state.mode = Mode::Terminal;
                let Some(pane) = self.focused_pane_info() else {
                    return encode_error(id, "no_focused_pane", "no focused pane");
                };
                encode_success(id, ResponseResult::PaneInfo { pane })
            }
            (Some(_), Some(_)) => encode_error(
                id,
                "invalid_request",
                "pane.focus accepts either pane_id or direction, not both",
            ),
            (None, None) => encode_error(
                id,
                "invalid_request",
                "pane.focus requires pane_id or direction",
            ),
        }
    }

    pub(super) fn handle_pane_resize(&mut self, id: String, params: PaneResizeParams) -> String {
        if params.amount == 0 {
            return encode_error(
                id,
                "invalid_request",
                "pane.resize amount must be at least 1",
            );
        }
        if params.amount > 100 {
            return encode_error(
                id,
                "invalid_request",
                "pane.resize amount must be no greater than 100",
            );
        }

        let direction = nav_direction_from_api(params.direction);
        for _ in 0..params.amount {
            self.state.resize_pane(direction);
        }
        self.state.mode = Mode::Terminal;
        let Some(pane) = self.focused_pane_info() else {
            return encode_error(id, "no_focused_pane", "no focused pane");
        };

        encode_success(id, ResponseResult::PaneInfo { pane })
    }

    pub(super) fn handle_pane_rename(&mut self, id: String, params: PaneRenameParams) -> String {
        let Some((ws_idx, _, pane_id)) = self.canonicalize_pane_target(&params.pane_id) else {
            return pane_not_found(id, &params.pane_id);
        };
        let Some(terminal_id) = self
            .state
            .sessions()
            .get(ws_idx)
            .and_then(|ws| ws.terminal_id(pane_id))
            .cloned()
        else {
            return pane_not_found(id, &params.pane_id);
        };
        let Some(terminal) = self.state.terminals.get_mut(&terminal_id) else {
            return pane_not_found(id, &params.pane_id);
        };
        match params.label.map(|label| label.trim().to_string()) {
            Some(label) if !label.is_empty() => terminal.set_manual_label(label),
            _ => terminal.clear_manual_label(),
        }
        self.state.mark_session_dirty();
        let Some(pane) = self.pane_info_by_raw_id(pane_id) else {
            return pane_not_found(id, &params.pane_id);
        };

        encode_success(id, ResponseResult::PaneInfo { pane })
    }

    pub(super) fn handle_pane_read(&mut self, id: String, params: PaneReadParams) -> String {
        let Some((ws_idx, tab_idx, pane_id)) = self.canonicalize_pane_target(&params.pane_id)
        else {
            return pane_not_found(id, &params.pane_id);
        };
        let Some(pane) = self.lookup_runtime(ws_idx, pane_id) else {
            return pane_not_found(id, &params.pane_id);
        };
        let requested_lines = params.lines.unwrap_or(80).min(1000) as usize;
        let text = match params.format {
            ReadFormat::Text => match params.source {
                ReadSource::Visible => pane.visible_text(),
                ReadSource::Recent => pane.recent_text(requested_lines),
                ReadSource::RecentUnwrapped => pane.recent_unwrapped_text(requested_lines),
            },
            ReadFormat::Ansi => match params.source {
                ReadSource::Visible => pane.visible_ansi(),
                ReadSource::Recent => pane.recent_ansi(requested_lines),
                ReadSource::RecentUnwrapped => pane.recent_unwrapped_ansi(requested_lines),
            },
        };

        encode_success(
            id,
            ResponseResult::PaneRead {
                read: PaneReadResult {
                    pane_id: params.pane_id,
                    tab_id: self.public_tab_id(ws_idx, tab_idx).unwrap(),
                    source: params.source,
                    format: params.format,
                    text,
                    revision: 0,
                    truncated: false,
                },
            },
        )
    }

    pub(super) fn handle_pane_send_text(
        &mut self,
        id: String,
        params: PaneSendTextParams,
    ) -> String {
        let Some((ws_idx, _, pane_id)) = self.canonicalize_pane_target(&params.pane_id) else {
            return pane_not_found(id, &params.pane_id);
        };
        let Some(runtime) = self.lookup_runtime_sender(ws_idx, pane_id) else {
            return pane_not_found(id, &params.pane_id);
        };
        if let Err(err) = runtime.try_send_bytes(Bytes::from(params.text)) {
            return encode_error(id, "pane_send_failed", err.to_string());
        }

        encode_success(id, ResponseResult::Ok {})
    }

    pub(super) fn handle_pane_send_input(
        &mut self,
        id: String,
        params: PaneSendInputParams,
    ) -> String {
        let Some((ws_idx, _, pane_id)) = self.canonicalize_pane_target(&params.pane_id) else {
            return pane_not_found(id, &params.pane_id);
        };
        let Some(runtime) = self.lookup_runtime_sender(ws_idx, pane_id) else {
            return pane_not_found(id, &params.pane_id);
        };
        let encoded_keys = match encode_api_keys(runtime, &params.keys) {
            Ok(encoded_keys) => encoded_keys,
            Err(key) => return encode_error(id, "invalid_key", format!("unsupported key {key}")),
        };
        if !params.text.is_empty() {
            let text_bytes = encode_api_text(runtime, &params.text);
            if let Err(err) = runtime.try_send_bytes(Bytes::from(text_bytes)) {
                return encode_error(id, "pane_send_failed", err.to_string());
            }
        }
        for bytes in encoded_keys {
            if let Err(err) = runtime.try_send_bytes(Bytes::from(bytes)) {
                return encode_error(id, "pane_send_failed", err.to_string());
            }
        }

        encode_success(id, ResponseResult::Ok {})
    }

    pub(super) fn handle_pane_close(&mut self, id: String, target: PaneTarget) -> String {
        let Some((ws_idx, _, pane_id)) = self.canonicalize_pane_target(&target.pane_id) else {
            return pane_not_found(id, &target.pane_id);
        };

        self.state.focus_pane_in_session_at(ws_idx, pane_id);
        self.state.close_pane();
        self.shutdown_detached_terminal_runtimes();
        self.schedule_session_save();
        self.emit_event(EventEnvelope {
            event: EventKind::PaneClosed,
            data: EventData::PaneClosed {
                pane_id: target.pane_id,
            },
        });

        encode_success(id, ResponseResult::Ok {})
    }

    pub(super) fn handle_pane_send_keys(
        &mut self,
        id: String,
        params: PaneSendKeysParams,
    ) -> String {
        let Some((ws_idx, _, pane_id)) = self.canonicalize_pane_target(&params.pane_id) else {
            return pane_not_found(id, &params.pane_id);
        };
        let Some(runtime) = self.lookup_runtime_sender(ws_idx, pane_id) else {
            return pane_not_found(id, &params.pane_id);
        };
        let encoded_keys = match encode_api_keys(runtime, &params.keys) {
            Ok(encoded_keys) => encoded_keys,
            Err(key) => return encode_error(id, "invalid_key", format!("unsupported key {key}")),
        };
        for bytes in encoded_keys {
            if let Err(err) = runtime.try_send_bytes(Bytes::from(bytes)) {
                return encode_error(id, "pane_send_failed", err.to_string());
            }
        }

        encode_success(id, ResponseResult::Ok {})
    }
}

fn pane_not_found(id: String, pane_id: &str) -> String {
    encode_error(id, "pane_not_found", format!("pane {pane_id} not found"))
}

fn nav_direction_from_api(direction: PaneDirection) -> NavDirection {
    match direction {
        PaneDirection::Left => NavDirection::Left,
        PaneDirection::Right => NavDirection::Right,
        PaneDirection::Up => NavDirection::Up,
        PaneDirection::Down => NavDirection::Down,
    }
}

impl App {
    fn canonicalize_pane_target(&mut self, public_pane_id: &str) -> Option<(usize, usize, PaneId)> {
        let (_, pane_id) = self.parse_pane_id(public_pane_id)?;
        self.state.collapse_to_single_session();
        self.state
            .sessions()
            .iter()
            .enumerate()
            .find_map(|(ws_idx, ws)| {
                ws.find_tab_index_for_pane(pane_id)
                    .map(|tab_idx| (ws_idx, tab_idx, pane_id))
            })
    }

    fn focused_pane_info(&self) -> Option<PaneInfo> {
        let ws_idx = self.state.session_index()?;
        let pane_id = self.state.session()?.focused_pane_id()?;
        self.pane_info(ws_idx, pane_id)
    }

    fn pane_info_by_raw_id(&self, pane_id: crate::layout::PaneId) -> Option<PaneInfo> {
        self.state
            .sessions()
            .iter()
            .enumerate()
            .find_map(|(ws_idx, ws)| {
                ws.find_tab_index_for_pane(pane_id)
                    .and_then(|_| self.pane_info(ws_idx, pane_id))
            })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{api::schema::SuccessResponse, config::Config, workspace::Workspace};

    fn app_with_session_container() -> App {
        let (_api_tx, api_rx) = tokio::sync::mpsc::unbounded_channel();
        let mut app = App::new(
            &Config::default(),
            true,
            None,
            api_rx,
            crate::api::EventHub::default(),
        );
        app.state.sessions = vec![Workspace::test_new("issue")];
        app.state.ensure_test_terminals();
        app
    }

    #[test]
    fn api_pane_close_closes_single_pane_session() {
        let mut app = app_with_session_container();
        let pane_id = app.state.sessions[0].tabs[0].root_pane;
        let public_pane_id = app.public_pane_id(0, pane_id).unwrap();

        let response = app.handle_pane_close(
            "req".into(),
            PaneTarget {
                pane_id: public_pane_id,
            },
        );

        let success: SuccessResponse = serde_json::from_str(&response).unwrap();
        assert_eq!(success.id, "req");
        assert!(app.state.sessions.is_empty());
    }

    #[test]
    fn api_pane_focus_targets_pane() {
        let mut app = app_with_session_container();
        let pane_id = app.state.sessions[0].tabs[0].root_pane;
        let public_pane_id = app.public_pane_id(0, pane_id).unwrap();

        let response = app.handle_pane_focus(
            "req".into(),
            PaneFocusParams {
                pane_id: Some(public_pane_id.clone()),
                direction: None,
            },
        );

        let success: SuccessResponse = serde_json::from_str(&response).unwrap();
        assert_eq!(success.id, "req");
        assert_eq!(app.state.active, Some(0));
        assert_eq!(app.state.mode, Mode::Terminal);
        let ResponseResult::PaneInfo { pane } = success.result else {
            panic!("expected pane info response");
        };
        assert_eq!(pane.pane_id, public_pane_id);
        assert!(pane.focused);
    }

    #[test]
    fn api_pane_focus_collapses_legacy_workspace_target() {
        let mut app = app_with_session_container();
        let first = Workspace::test_new("one");
        let second = Workspace::test_new("two");
        let pane_id = second.tabs[0].root_pane;
        app.state.sessions = vec![first, second];
        app.state.ensure_test_terminals();
        app.state.active = Some(0);
        app.state.selected = 0;
        let public_pane_id = app.public_pane_id(1, pane_id).unwrap();

        let response = app.handle_pane_focus(
            "req".into(),
            PaneFocusParams {
                pane_id: Some(public_pane_id.clone()),
                direction: None,
            },
        );

        let success: SuccessResponse = serde_json::from_str(&response).unwrap();
        assert_eq!(success.id, "req");
        assert_eq!(app.state.sessions.len(), 1);
        assert_eq!(app.state.active, Some(0));
        assert_eq!(app.state.selected, 0);
        assert_eq!(app.state.sessions[0].active_tab, 1);
        let ResponseResult::PaneInfo { pane } = success.result else {
            panic!("expected pane info response");
        };
        assert_eq!(pane.pane_id, public_pane_id);
        assert!(pane.focused);
    }

    #[test]
    fn api_pane_focus_rejects_ambiguous_target() {
        let mut app = app_with_session_container();

        let response = app.handle_pane_focus(
            "req".into(),
            PaneFocusParams {
                pane_id: Some("1-1".into()),
                direction: Some(PaneDirection::Left),
            },
        );

        let value: serde_json::Value = serde_json::from_str(&response).unwrap();
        assert_eq!(value["error"]["code"], "invalid_request");
    }

    #[test]
    fn api_pane_resize_rejects_zero_amount() {
        let mut app = app_with_session_container();

        let response = app.handle_pane_resize(
            "req".into(),
            PaneResizeParams {
                direction: PaneDirection::Right,
                amount: 0,
            },
        );

        let value: serde_json::Value = serde_json::from_str(&response).unwrap();
        assert_eq!(value["error"]["code"], "invalid_request");
    }
}
