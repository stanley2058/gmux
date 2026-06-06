use std::path::PathBuf;

use crate::api::schema::{
    EventData, EventEnvelope, EventKind, ResponseResult, TabCreateParams, TabListParams,
    TabRenameParams, TabTarget,
};
use crate::app::{App, Mode};

use super::responses::{encode_error, encode_success};

impl App {
    pub(super) fn handle_tab_list(&mut self, id: String, params: TabListParams) -> String {
        let TabListParams {} = params;
        self.state.collapse_to_single_session();
        let tabs = self
            .state
            .session_tab_entries()
            .filter_map(|entry| self.tab_info(entry.session_idx, entry.tab_idx))
            .collect();

        encode_success(id, ResponseResult::TabList { tabs })
    }

    pub(super) fn handle_tab_get(&mut self, id: String, target: TabTarget) -> String {
        let Some((ws_idx, tab_idx)) = self.parse_tab_id(&target.tab_id) else {
            return tab_not_found(id, &target.tab_id);
        };
        let Some(flat_tab_idx) = self.state.flattened_tab_index(ws_idx, tab_idx) else {
            return tab_not_found(id, &target.tab_id);
        };
        self.state.collapse_to_single_session();
        let Some(tab) = self.tab_info(0, flat_tab_idx) else {
            return tab_not_found(id, &target.tab_id);
        };

        encode_success(id, ResponseResult::TabInfo { tab })
    }

    pub(super) fn handle_tab_create(&mut self, id: String, params: TabCreateParams) -> String {
        let TabCreateParams { cwd, focus, label } = params;
        self.state.collapse_to_single_session();
        let ws_idx = if let Some(ws_idx) = self.state.session_index() {
            ws_idx
        } else {
            let cwd = cwd
                .map(PathBuf::from)
                .unwrap_or_else(|| self.resolve_new_terminal_cwd(None));
            return match self.create_session_with_options(cwd, focus) {
                Ok(ws_idx) => {
                    if let Some(label) = label {
                        let session_id = self
                            .state
                            .session()
                            .expect("new session should be active")
                            .id
                            .clone();
                        let tab_id = self
                            .public_tab_id(ws_idx, 0)
                            .unwrap_or_else(|| format!("{session_id}:1"));
                        if let Some(tab) =
                            self.state.session_mut().and_then(|ws| ws.tabs.get_mut(0))
                        {
                            tab.set_custom_name(label);
                            crate::logging::tab_renamed(&session_id, &tab_id);
                            self.schedule_session_save();
                        }
                    }
                    let tab = self
                        .tab_info(ws_idx, 0)
                        .expect("new session should have an initial tab");
                    let root_pane = self
                        .root_pane_info(ws_idx, 0)
                        .expect("new session should have an initial root pane");
                    self.emit_event(EventEnvelope {
                        event: EventKind::TabCreated,
                        data: EventData::TabCreated { tab: tab.clone() },
                    });
                    self.emit_event(EventEnvelope {
                        event: EventKind::PaneCreated,
                        data: EventData::PaneCreated {
                            pane: root_pane.clone(),
                        },
                    });
                    encode_success(
                        id,
                        self.tab_created_result(ws_idx, 0)
                            .expect("new session should produce a tab create response"),
                    )
                }
                Err(err) => encode_error(id, "tab_create_failed", err.to_string()),
            };
        };
        let cwd = cwd.map(PathBuf::from).unwrap_or_else(|| {
            let follow_cwd = self
                .state
                .focused_runtime_in_session(&self.terminal_runtimes)
                .and_then(|rt| rt.cwd());
            self.resolve_new_terminal_cwd(follow_cwd)
        });
        let (rows, cols) = self.state.estimate_pane_size();
        let default_shell = self.state.default_shell.clone();
        let shell_mode = self.state.shell_mode;
        let scrollback_limit_bytes = self.state.pane_scrollback_limit_bytes;
        let host_terminal_theme = self.state.host_terminal_theme;
        let result = self
            .state
            .session_mut()
            .ok_or_else(|| std::io::Error::other("session state disappeared"))
            .and_then(|ws| {
                ws.create_tab(
                    rows,
                    cols,
                    cwd,
                    scrollback_limit_bytes,
                    host_terminal_theme,
                    crate::pane::PaneShellConfig::new(&default_shell, shell_mode),
                )
            });
        match result {
            Ok((tab_idx, terminal, runtime)) => {
                self.terminal_runtimes.insert(terminal.id.clone(), runtime);
                self.state.terminals.insert(terminal.id.clone(), terminal);
                let Some((root_pane, session_id)) = self.state.session().and_then(|session| {
                    session
                        .tabs
                        .get(tab_idx)
                        .map(|tab| (tab.root_pane, session.id.clone()))
                }) else {
                    return encode_error(id, "tab_create_failed", "session state disappeared");
                };
                self.state.remove_alias_shadowed_by_new_pane(root_pane);
                if let Some(label) = label {
                    let tab_id = self
                        .public_tab_id(ws_idx, tab_idx)
                        .unwrap_or_else(|| format!("{}:{}", session_id, tab_idx + 1));
                    if let Some(tab) = self
                        .state
                        .session_mut()
                        .and_then(|ws| ws.tabs.get_mut(tab_idx))
                    {
                        tab.set_custom_name(label);
                        crate::logging::tab_renamed(&session_id, &tab_id);
                    }
                }
                if focus {
                    self.state.focus_session_tab(ws_idx, tab_idx);
                    self.state.mode = Mode::Terminal;
                }
                self.schedule_session_save();
                let tab = self.tab_info(ws_idx, tab_idx).unwrap();
                let root_pane = self
                    .root_pane_info(ws_idx, tab_idx)
                    .expect("new tab should have a root pane");
                self.emit_event(EventEnvelope {
                    event: EventKind::TabCreated,
                    data: EventData::TabCreated { tab: tab.clone() },
                });
                self.emit_event(EventEnvelope {
                    event: EventKind::PaneCreated,
                    data: EventData::PaneCreated {
                        pane: root_pane.clone(),
                    },
                });
                encode_success(
                    id,
                    self.tab_created_result(ws_idx, tab_idx)
                        .expect("new tab should produce a complete create response"),
                )
            }
            Err(err) => encode_error(id, "tab_create_failed", err.to_string()),
        }
    }

    pub(super) fn handle_tab_focus(&mut self, id: String, target: TabTarget) -> String {
        let Some((ws_idx, tab_idx)) = self.parse_tab_id(&target.tab_id) else {
            return tab_not_found(id, &target.tab_id);
        };
        if !self.state.focus_session_tab(ws_idx, tab_idx) {
            return tab_not_found(id, &target.tab_id);
        }
        let Some(focused_ws_idx) = self.state.session_index() else {
            return tab_not_found(id, &target.tab_id);
        };
        let Some(focused_tab_idx) = self.state.session().map(|session| session.active_tab) else {
            return tab_not_found(id, &target.tab_id);
        };
        let tab = self.tab_info(focused_ws_idx, focused_tab_idx).unwrap();

        encode_success(id, ResponseResult::TabInfo { tab })
    }

    pub(super) fn handle_tab_rename(&mut self, id: String, params: TabRenameParams) -> String {
        let Some((ws_idx, tab_idx)) = self.parse_tab_id(&params.tab_id) else {
            return tab_not_found(id, &params.tab_id);
        };
        let Some(flat_tab_idx) = self.state.flattened_tab_index(ws_idx, tab_idx) else {
            return tab_not_found(id, &params.tab_id);
        };
        self.state.collapse_to_single_session();
        let Some(session_id) = self.state.session().map(|session| session.id.clone()) else {
            return tab_not_found(id, &params.tab_id);
        };
        let tab_id = self
            .public_tab_id(0, flat_tab_idx)
            .unwrap_or_else(|| format!("{}:{}", session_id, flat_tab_idx + 1));
        let Some(tab) = self
            .state
            .session_mut()
            .and_then(|ws| ws.tabs.get_mut(flat_tab_idx))
        else {
            return tab_not_found(id, &params.tab_id);
        };
        tab.set_custom_name(params.label.clone());
        crate::logging::tab_renamed(&session_id, &tab_id);
        self.schedule_session_save();
        self.emit_event(EventEnvelope {
            event: EventKind::TabRenamed,
            data: EventData::TabRenamed {
                tab_id: self.public_tab_id(0, flat_tab_idx).unwrap(),
                label: params.label,
            },
        });
        let tab = self.tab_info(0, flat_tab_idx).unwrap();

        encode_success(id, ResponseResult::TabInfo { tab })
    }

    pub(super) fn handle_tab_close(&mut self, id: String, target: TabTarget) -> String {
        let Some((ws_idx, tab_idx)) = self.parse_tab_id(&target.tab_id) else {
            return tab_not_found(id, &target.tab_id);
        };
        if self.state.flattened_tab_index(ws_idx, tab_idx).is_none() {
            return tab_not_found(id, &target.tab_id);
        }
        if !self.state.focus_session_tab(ws_idx, tab_idx) {
            return tab_not_found(id, &target.tab_id);
        }
        if self.state.session().is_none_or(|ws| ws.tabs.len() <= 1) {
            return encode_error(
                id,
                "tab_close_failed",
                "cannot close the last tab in a session",
            );
        }
        self.state.close_tab();
        self.shutdown_detached_terminal_runtimes();
        self.schedule_session_save();
        self.emit_event(EventEnvelope {
            event: EventKind::TabClosed,
            data: EventData::TabClosed {
                tab_id: target.tab_id,
            },
        });

        encode_success(id, ResponseResult::Ok {})
    }
}

fn tab_not_found(id: String, tab_id: &str) -> String {
    encode_error(id, "tab_not_found", format!("tab {tab_id} not found"))
}
