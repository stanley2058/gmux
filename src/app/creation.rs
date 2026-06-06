use std::path::PathBuf;

use tracing::error;

use super::{App, Mode};
use crate::{config::NewTerminalCwdConfig, workspace::SessionUiState};

pub(crate) fn resolve_new_terminal_cwd(
    policy: &NewTerminalCwdConfig,
    follow_cwd: Option<PathBuf>,
) -> PathBuf {
    match policy {
        NewTerminalCwdConfig::Follow => follow_cwd
            .or_else(|| std::env::var_os("HOME").map(PathBuf::from))
            .or_else(|| std::env::current_dir().ok())
            .unwrap_or_else(|| PathBuf::from("/")),
        NewTerminalCwdConfig::Home => std::env::var_os("HOME")
            .map(PathBuf::from)
            .or_else(|| std::env::current_dir().ok())
            .unwrap_or_else(|| PathBuf::from("/")),
        NewTerminalCwdConfig::Current => {
            std::env::current_dir().unwrap_or_else(|_| PathBuf::from("/"))
        }
        NewTerminalCwdConfig::Path(path) => expand_tilde_path(path),
    }
}

fn expand_tilde_path(path: &str) -> PathBuf {
    if path == "~" {
        return std::env::var_os("HOME")
            .map(PathBuf::from)
            .unwrap_or_else(|| PathBuf::from(path));
    }
    if let Some(rest) = path.strip_prefix("~/") {
        if let Some(home) = std::env::var_os("HOME") {
            return PathBuf::from(home).join(rest);
        }
    }
    PathBuf::from(path)
}

impl App {
    pub(super) fn seed_cwd_from_session(&self) -> Option<PathBuf> {
        self.state
            .session()?
            .resolved_identity_cwd_from(&self.state.terminals, &self.terminal_runtimes)
    }

    pub(super) fn resolve_new_terminal_cwd(&self, follow_cwd: Option<PathBuf>) -> PathBuf {
        resolve_new_terminal_cwd(&self.state.new_terminal_cwd, follow_cwd)
    }

    pub(crate) fn create_tab(&mut self) {
        let custom_name = self.state.requested_new_tab_name.take();
        let follow_cwd = self.seed_cwd_from_session();
        let initial_cwd = self.resolve_new_terminal_cwd(follow_cwd);
        match self.create_tab_with_options(initial_cwd, true) {
            Ok(tab_idx) => {
                if let Some(name) = custom_name {
                    if let Some(ws) = self.state.session_mut() {
                        if let Some(tab) = ws.tabs.get_mut(tab_idx) {
                            tab.set_custom_name(name);
                        }
                        self.schedule_session_save();
                    }
                }
            }
            Err(e) => {
                error!(err = %e, "failed to create tab");
            }
        }
    }

    pub(super) fn create_tab_with_options(
        &mut self,
        initial_cwd: PathBuf,
        focus: bool,
    ) -> std::io::Result<usize> {
        if self.state.has_session() {
            self.state.collapse_to_single_session();
        }
        let Some(session_idx) = self.state.session_index() else {
            return self.create_session_with_options(initial_cwd, focus);
        };
        let (rows, cols) = self.state.estimate_pane_size();
        let scrollback_limit_bytes = self.state.pane_scrollback_limit_bytes;
        let host_terminal_theme = self.state.host_terminal_theme;
        let default_shell = self.state.default_shell.clone();
        let shell_mode = self.state.shell_mode;
        let (idx, terminal, runtime, session_id, root_pane) = {
            let session = &mut self.state.sessions_mut()[session_idx];
            let (idx, terminal, runtime) = session.create_tab(
                rows,
                cols,
                initial_cwd,
                scrollback_limit_bytes,
                host_terminal_theme,
                crate::pane::PaneShellConfig::new(&default_shell, shell_mode),
            )?;
            let root_pane = session.tabs[idx].root_pane;
            (idx, terminal, runtime, session.id.clone(), root_pane)
        };
        self.terminal_runtimes.insert(terminal.id.clone(), runtime);
        self.state.terminals.insert(terminal.id.clone(), terminal);
        self.state.remove_alias_shadowed_by_new_pane(root_pane);
        if focus {
            self.state.focus_session_tab(session_idx, idx);
            self.state.mode = Mode::Terminal;
        }
        let tab_id = self
            .public_tab_id(session_idx, idx)
            .unwrap_or_else(|| format!("{}:{}", session_id, idx + 1));
        crate::logging::tab_created(&session_id, &tab_id, root_pane.raw());
        self.schedule_session_save();
        Ok(idx)
    }

    pub(crate) fn create_session_with_options(
        &mut self,
        initial_cwd: PathBuf,
        focus: bool,
    ) -> std::io::Result<usize> {
        if self.state.has_session() {
            self.state.collapse_to_single_session();
            return Ok(self.state.session_index().unwrap_or(0));
        }

        let should_focus = focus || self.state.session_index().is_none();
        let (rows, cols) = self.state.estimate_pane_size();
        let (ws, terminal, runtime) = SessionUiState::new(
            initial_cwd,
            rows,
            cols,
            self.state.pane_scrollback_limit_bytes,
            self.state.host_terminal_theme,
            crate::pane::PaneShellConfig::new(&self.state.default_shell, self.state.shell_mode),
            self.event_tx.clone(),
            self.render_notify.clone(),
            self.render_dirty.clone(),
        )?;
        self.terminal_runtimes.insert(terminal.id.clone(), runtime);
        self.state.terminals.insert(terminal.id.clone(), terminal);
        self.state.set_session(ws);
        let idx = 0;
        let root_pane = self.state.sessions()[idx].tabs[0].root_pane;
        self.state.remove_alias_shadowed_by_new_pane(root_pane);
        let session_id = self.state.sessions()[idx].id.clone();
        crate::logging::session_created(&session_id, root_pane.raw());
        if should_focus {
            self.state.focus_session(idx);
            self.state.mode = Mode::Terminal;
        }
        self.schedule_session_save();
        Ok(idx)
    }

    pub(super) fn collect_panes(&self) -> Vec<crate::api::schema::PaneInfo> {
        self.state
            .sessions()
            .iter()
            .enumerate()
            .flat_map(|(session_idx, session)| {
                session
                    .tabs
                    .iter()
                    .flat_map(|tab| tab.layout.pane_ids().into_iter())
                    .filter_map(move |pane_id| self.pane_info(session_idx, pane_id))
            })
            .collect()
    }

    pub(super) fn tab_info(
        &self,
        ws_idx: usize,
        tab_idx: usize,
    ) -> Option<crate::api::schema::TabInfo> {
        let session = self.state.sessions().get(ws_idx)?;
        let tab = session.tabs.get(tab_idx)?;
        Some(crate::api::schema::TabInfo {
            tab_id: self.public_tab_id(ws_idx, tab_idx)?,
            number: tab_idx + 1,
            label: tab.display_name(),
            focused: self.state.session_index() == Some(ws_idx) && session.active_tab == tab_idx,
            pane_count: tab.panes.len(),
        })
    }

    pub(super) fn tab_created_result(
        &self,
        ws_idx: usize,
        tab_idx: usize,
    ) -> Option<crate::api::schema::ResponseResult> {
        Some(crate::api::schema::ResponseResult::TabCreated {
            tab: self.tab_info(ws_idx, tab_idx)?,
            root_pane: self.root_pane_info(ws_idx, tab_idx)?,
        })
    }

    pub(super) fn root_pane_info(
        &self,
        ws_idx: usize,
        tab_idx: usize,
    ) -> Option<crate::api::schema::PaneInfo> {
        let session = self.state.sessions().get(ws_idx)?;
        let tab = session.tabs.get(tab_idx)?;
        self.pane_info(ws_idx, tab.root_pane)
    }

    pub(super) fn pane_info(
        &self,
        ws_idx: usize,
        pane_id: crate::layout::PaneId,
    ) -> Option<crate::api::schema::PaneInfo> {
        let session = self.state.sessions().get(ws_idx)?;
        let pane = session.pane_state(pane_id)?;
        let terminal = self.state.terminals.get(&pane.attached_terminal_id)?;
        let tab_idx = session.find_tab_index_for_pane(pane_id)?;
        let focused = self.state.is_active_pane(ws_idx, tab_idx, pane_id);
        Some(crate::api::schema::PaneInfo {
            pane_id: self.public_pane_id(ws_idx, pane_id)?,
            terminal_id: terminal.id.to_string(),
            tab_id: self.public_tab_id(ws_idx, tab_idx)?,
            focused,
            cwd: session.tabs[tab_idx]
                .cwd_for_pane(pane_id, &self.state.terminals, &self.terminal_runtimes)
                .map(|cwd| cwd.display().to_string()),
            foreground_cwd: session.tabs[tab_idx]
                .foreground_cwd_for_pane(pane_id, &self.terminal_runtimes)
                .map(|cwd| cwd.display().to_string()),
            label: terminal.manual_label.clone(),
            title: terminal.effective_title(),
            revision: terminal.revision,
        })
    }

    pub(super) fn lookup_runtime(
        &self,
        ws_idx: usize,
        pane_id: crate::layout::PaneId,
    ) -> Option<&crate::terminal::TerminalRuntime> {
        self.state
            .runtime_for_pane_in_session_at(&self.terminal_runtimes, ws_idx, pane_id)
    }

    pub(super) fn lookup_runtime_sender(
        &self,
        ws_idx: usize,
        pane_id: crate::layout::PaneId,
    ) -> Option<&crate::terminal::TerminalRuntime> {
        self.state
            .runtime_for_pane_in_session_at(&self.terminal_runtimes, ws_idx, pane_id)
    }
}
