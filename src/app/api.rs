use std::sync::atomic::Ordering;
use std::time::{Duration, Instant};

mod panes;
mod responses;
mod tabs;

use super::{App, Mode, OverlayPaneState, ToastKind};
use crate::events::AppEvent;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum RuntimeExitAction {
    RespawnShell,
    ClosePane,
}

impl App {
    pub(crate) fn handle_internal_event(&mut self, ev: AppEvent) {
        if let AppEvent::ClipboardWrite { content } = ev {
            #[cfg(not(test))]
            crate::selection::write_osc52_bytes(&content);
            #[cfg(test)]
            let _ = content;
            self.show_clipboard_feedback();
            return;
        }

        if let AppEvent::PaneDied { pane_id } = &ev {
            if self.runtime_exit_action(*pane_id) == RuntimeExitAction::RespawnShell
                && self.respawn_shell_for_launch_pane(*pane_id)
            {
                self.overlay_panes.remove(pane_id);
                self.render_dirty.store(true, Ordering::Release);
                self.render_notify.notify_one();
                return;
            }
        }

        let overlay_state = if let AppEvent::PaneDied { pane_id } = &ev {
            self.overlay_panes.remove(pane_id)
        } else {
            None
        };

        if let AppEvent::PaneDied { pane_id } = &ev {
            if let Some((ws_idx, _)) = self.find_pane(*pane_id) {
                if let Some(public_pane_id) = self.public_pane_id(ws_idx, *pane_id) {
                    self.emit_event(crate::api::schema::EventEnvelope {
                        event: crate::api::schema::EventKind::PaneExited,
                        data: crate::api::schema::EventData::PaneExited {
                            pane_id: public_pane_id,
                        },
                    });
                }
            }
        }

        let previous_toast = self.state.toast.clone();
        self.state.handle_app_event(ev);
        if let Some(overlay) = overlay_state {
            self.restore_overlay_after_exit(overlay);
        }

        self.sync_toast_deadline(previous_toast);
        self.shutdown_detached_terminal_runtimes();
    }

    pub(crate) fn show_clipboard_feedback(&mut self) {
        self.state.copy_feedback = Some(crate::app::state::CopyFeedback {
            message: "copied to clipboard".to_string(),
        });
        self.copy_feedback_deadline = Some(Instant::now() + super::COPY_FEEDBACK_DURATION);
    }

    fn restore_overlay_after_exit(&mut self, overlay: OverlayPaneState) {
        for temp_file in &overlay.temp_files {
            let _ = std::fs::remove_file(temp_file);
        }

        if self.state.session_index() != Some(overlay.ws_idx) {
            return;
        }

        let Some(ws) = self.state.session_mut() else {
            return;
        };
        if overlay.tab_idx >= ws.tabs.len() {
            return;
        }

        ws.active_tab = overlay.tab_idx;
        let tab = &mut ws.tabs[overlay.tab_idx];
        if tab.panes.contains_key(&overlay.previous_focus) {
            tab.focus_pane(overlay.previous_focus);
        }
        tab.zoomed = overlay.previous_zoomed;

        self.state.mode = Mode::Terminal;
    }

    fn runtime_exit_action(&self, pane_id: crate::layout::PaneId) -> RuntimeExitAction {
        let Some((_, pane_state)) = self.find_pane(pane_id) else {
            return RuntimeExitAction::ClosePane;
        };
        let Some(terminal) = self.state.terminals.get(&pane_state.attached_terminal_id) else {
            return RuntimeExitAction::ClosePane;
        };

        if terminal.respawn_shell_on_exit {
            RuntimeExitAction::RespawnShell
        } else {
            RuntimeExitAction::ClosePane
        }
    }

    fn respawn_shell_for_launch_pane(&mut self, pane_id: crate::layout::PaneId) -> bool {
        let Some((ws_idx, pane_state)) = self.find_pane(pane_id) else {
            return false;
        };
        let terminal_id = pane_state.attached_terminal_id.clone();
        let Some(terminal) = self.state.terminals.get(&terminal_id) else {
            return false;
        };

        let cwd = terminal.cwd.clone();
        let (rows, cols) = self
            .terminal_runtimes
            .get(&terminal_id)
            .map(|runtime| runtime.current_size())
            .unwrap_or_else(|| self.state.estimate_pane_size());
        let runtime = match crate::terminal::TerminalRuntime::spawn(
            pane_id,
            rows,
            cols,
            cwd,
            self.state.pane_scrollback_limit_bytes,
            self.state.host_terminal_theme,
            crate::pane::PaneShellConfig::new(&self.state.default_shell, self.state.shell_mode)
                .with_term(&self.state.pane_term),
            self.event_tx.clone(),
            self.render_notify.clone(),
            self.render_dirty.clone(),
        ) {
            Ok(runtime) => runtime,
            Err(err) => {
                tracing::warn!(
                    pane = pane_id.raw(),
                    terminal = %terminal_id,
                    err = %err,
                    "failed to respawn shell after launch command exited"
                );
                return false;
            }
        };

        self.terminal_runtimes.insert(terminal_id.clone(), runtime);
        if let Some(terminal) = self.state.terminals.get_mut(&terminal_id) {
            terminal.clear_launch_metadata_after_respawn();
        }
        self.state.focus_pane_in_session_at(ws_idx, pane_id);
        self.schedule_session_save();
        true
    }

    pub(super) fn sync_toast_deadline(
        &mut self,
        previous_toast: Option<crate::app::state::ToastNotification>,
    ) {
        if self.state.toast != previous_toast {
            self.toast_deadline = self.state.toast.as_ref().map(|toast| {
                let duration = match toast.kind {
                    ToastKind::NeedsAttention => Duration::from_secs(8),
                    ToastKind::Finished => Duration::from_secs(5),
                };
                Instant::now() + duration
            });
        }
    }

    pub(super) fn emit_event(&self, event: crate::api::schema::EventEnvelope) {
        self.event_hub.push(event);
    }

    pub(crate) fn sync_focus_events(&mut self) {
        let current_focus = self.state.session_index().and_then(|idx| {
            self.state
                .session()
                .and_then(|ws| ws.focused_pane_id().map(|pane_id| (idx, pane_id)))
        });
        if current_focus == self.last_focus {
            return;
        }

        if let Some((ws_idx, pane_id)) = self.last_focus {
            self.send_pane_focus_event(ws_idx, pane_id, crate::ghostty::FocusEvent::Lost);
        }
        if let Some((ws_idx, pane_id)) = current_focus {
            self.send_pane_focus_event(ws_idx, pane_id, crate::ghostty::FocusEvent::Gained);
            let Some(active_tab) = self.state.session().map(|session| session.active_tab) else {
                return;
            };
            if let Some(tab_id) = self.public_tab_id(ws_idx, active_tab) {
                self.emit_event(crate::api::schema::EventEnvelope {
                    event: crate::api::schema::EventKind::TabFocused,
                    data: crate::api::schema::EventData::TabFocused { tab_id },
                });
            }
            if let Some(public_pane_id) = self.public_pane_id(ws_idx, pane_id) {
                self.emit_event(crate::api::schema::EventEnvelope {
                    event: crate::api::schema::EventKind::PaneFocused,
                    data: crate::api::schema::EventData::PaneFocused {
                        pane_id: public_pane_id,
                    },
                });
            }
        }

        self.last_focus = current_focus;
    }

    fn send_pane_focus_event(
        &self,
        ws_idx: usize,
        pane_id: crate::layout::PaneId,
        event: crate::ghostty::FocusEvent,
    ) {
        let Some(runtime) =
            self.state
                .runtime_for_pane_in_session_at(&self.terminal_runtimes, ws_idx, pane_id)
        else {
            return;
        };
        runtime.try_send_focus_event(event);
    }

    pub(crate) fn handle_api_request(&mut self, request: crate::api::schema::Request) -> String {
        self.drain_all_internal_events();
        self.handle_api_request_after_internal_events_drained(request)
    }

    pub(crate) fn handle_api_request_after_internal_events_drained(
        &mut self,
        request: crate::api::schema::Request,
    ) -> String {
        use crate::api::schema::{
            ErrorBody, ErrorResponse, Method, ResponseResult, SuccessResponse,
        };

        let response = match request.method {
            Method::ServerStop(_) => {
                self.state.should_quit = true;
                SuccessResponse {
                    id: request.id,
                    result: ResponseResult::Ok {},
                }
            }
            Method::ServerLiveHandoff(_) => {
                let response = ErrorResponse {
                    id: request.id,
                    error: ErrorBody {
                        code: "unsupported_in_app_mode".into(),
                        message: "live handoff is only supported by the headless server".into(),
                    },
                };
                return serde_json::to_string(&response).unwrap_or_else(|_| "{}".to_string());
            }
            Method::ServerReloadConfig(_) => {
                let report = self.reload_config();
                SuccessResponse {
                    id: request.id,
                    result: ResponseResult::ConfigReload {
                        status: report.status,
                        diagnostics: report.diagnostics,
                    },
                }
            }
            Method::TabList(params) => return self.handle_tab_list(request.id, params),
            Method::TabGet(target) => return self.handle_tab_get(request.id, target),
            Method::TabCreate(params) => return self.handle_tab_create(request.id, params),
            Method::TabFocus(target) => return self.handle_tab_focus(request.id, target),
            Method::TabRename(params) => return self.handle_tab_rename(request.id, params),
            Method::TabClose(target) => return self.handle_tab_close(request.id, target),
            Method::PaneSplit(params) => return self.handle_pane_split(request.id, params),
            Method::PanePopup(params) => return self.handle_pane_popup(request.id, params),
            Method::PaneList(params) => return self.handle_pane_list(request.id, params),
            Method::PaneGet(target) => return self.handle_pane_get(request.id, target),
            Method::PaneFocus(params) => return self.handle_pane_focus(request.id, params),
            Method::PaneResize(params) => return self.handle_pane_resize(request.id, params),
            Method::PaneRename(params) => return self.handle_pane_rename(request.id, params),
            Method::PaneRead(params) => return self.handle_pane_read(request.id, params),
            Method::PaneSendText(params) => return self.handle_pane_send_text(request.id, params),
            Method::PaneSendInput(params) => {
                return self.handle_pane_send_input(request.id, params)
            }
            Method::PaneClose(target) => return self.handle_pane_close(request.id, target),
            Method::PaneSendKeys(params) => return self.handle_pane_send_keys(request.id, params),
            _ => {
                return responses::encode_error(
                    request.id,
                    "not_implemented",
                    "method not implemented yet",
                );
            }
        };

        serde_json::to_string(&response).unwrap()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use ratatui::layout::Direction;

    #[tokio::test]
    async fn pane_died_respawns_shell_and_clears_launch_metadata() {
        let (_api_tx, api_rx) = tokio::sync::mpsc::unbounded_channel();
        let mut app = App::new(
            &crate::config::Config::default(),
            true,
            None,
            api_rx,
            crate::api::EventHub::default(),
        );
        let mut workspace = crate::workspace::Workspace::test_new("restored");
        let pane_id = workspace.tabs[0].root_pane;
        workspace.test_split(Direction::Horizontal);
        let terminal_id = workspace.terminal_id(pane_id).cloned().unwrap();
        app.state.sessions = vec![workspace];
        app.state.ensure_test_terminals();
        let terminal = app
            .state
            .terminals
            .get_mut(&terminal_id)
            .expect("test terminal should exist");
        terminal.respawn_shell_on_exit = true;

        app.handle_internal_event(AppEvent::PaneDied { pane_id });

        assert!(
            app.find_pane(pane_id).is_some(),
            "respawnable pane should stay attached after the launch command exits"
        );
        let terminal = app
            .state
            .terminals
            .get(&terminal_id)
            .expect("terminal should survive respawn");
        assert!(!terminal.respawn_shell_on_exit);

        for (_, runtime) in app.terminal_runtimes.drain() {
            runtime.shutdown();
        }
    }

    #[tokio::test]
    async fn pane_died_last_respawnable_pane_respawns_shell() {
        let (_api_tx, api_rx) = tokio::sync::mpsc::unbounded_channel();
        let mut app = App::new(
            &crate::config::Config::default(),
            true,
            None,
            api_rx,
            crate::api::EventHub::default(),
        );
        let workspace = crate::workspace::Workspace::test_new("restored");
        let pane_id = workspace.tabs[0].root_pane;
        let terminal_id = workspace.terminal_id(pane_id).cloned().unwrap();
        app.state.sessions = vec![workspace];
        app.state.active_session = Some(0);
        app.state.ensure_test_terminals();
        let terminal = app
            .state
            .terminals
            .get_mut(&terminal_id)
            .expect("test terminal should exist");
        terminal.respawn_shell_on_exit = true;

        app.handle_internal_event(AppEvent::PaneDied { pane_id });

        assert!(
            app.find_pane(pane_id).is_some(),
            "last restored launch pane should recover into a shell"
        );
        assert!(!app.state.should_quit);

        for (_, runtime) in app.terminal_runtimes.drain() {
            runtime.shutdown();
        }
    }
}
