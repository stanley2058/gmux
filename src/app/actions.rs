//! Pure state mutations on AppState.
//! These don't need channels, async, or PTY runtime.

use tracing::{info, warn};

use crate::events::AppEvent;
use crate::layout::{find_in_direction_with_bias, NavDirection, PaneId, PaneInfo};
use crate::selection::Selection;
use crate::workspace::SessionUiState;
use unicode_width::UnicodeWidthChar;

use super::state::{
    text_matches_query, AppState, Mode, NavigatorRow, NavigatorTarget, PaneFocusTarget,
    PaneNavigationAxis, ViewLayout,
};
// ---------------------------------------------------------------------------
// Navigator operations
// ---------------------------------------------------------------------------

impl AppState {
    pub(crate) fn current_pane_focus_target(&self) -> Option<PaneFocusTarget> {
        let ws = self.session()?;
        let pane_id = ws.focused_pane_id()?;
        Some(PaneFocusTarget {
            session_id: ws.id.clone(),
            pane_id,
        })
    }

    fn pane_focus_target_indices(&self, target: &PaneFocusTarget) -> Option<(usize, usize)> {
        if let Some(entry) = self
            .session_entries()
            .find(|entry| entry.session.id == target.session_id)
        {
            if let Some(tab_idx) = entry.session.find_tab_index_for_pane(target.pane_id) {
                return Some((entry.session_idx, tab_idx));
            }
        }

        self.session_tab_entries().find_map(|entry| {
            entry
                .tab
                .panes
                .contains_key(&target.pane_id)
                .then_some((entry.session_idx, entry.tab_idx))
        })
    }

    pub(crate) fn flattened_tab_index(&self, ws_idx: usize, tab_idx: usize) -> Option<usize> {
        self.session_tab_entries()
            .position(|entry| entry.session_idx == ws_idx && entry.tab_idx == tab_idx)
    }

    pub(crate) fn record_pane_focus_change(
        &mut self,
        previous: Option<PaneFocusTarget>,
        ws_idx: usize,
        pane_id: PaneId,
    ) {
        let Some(session_id) = self
            .session_entries()
            .find(|entry| entry.session_idx == ws_idx)
            .map(|entry| entry.session.id.clone())
        else {
            return;
        };
        let target = PaneFocusTarget {
            session_id,
            pane_id,
        };
        self.pane_navigation_bias = None;
        if previous.as_ref() != Some(&target) {
            self.previous_pane_focus = previous;
        }
    }

    fn record_pane_focus_after_navigation(&mut self, previous: Option<PaneFocusTarget>) {
        let current = self.current_pane_focus_target();
        if previous != current {
            self.previous_pane_focus = previous;
        }
    }

    pub(crate) fn focus_pane_in_session_at(&mut self, ws_idx: usize, pane_id: PaneId) -> bool {
        let Some((tab_idx, session_id)) = self
            .session_tab_entries()
            .find(|entry| entry.session_idx == ws_idx && entry.tab.panes.contains_key(&pane_id))
            .map(|entry| (entry.tab_idx, entry.session.id.clone()))
        else {
            return false;
        };
        let previous = self.current_pane_focus_target();
        let target = PaneFocusTarget {
            session_id,
            pane_id,
        };
        let has_one_session = self.session_entries().nth(1).is_none();
        if has_one_session
            && self.active_session == Some(ws_idx)
            && previous.as_ref() == Some(&target)
        {
            self.pane_navigation_bias = None;
            return false;
        }

        if !self.focus_session_tab(ws_idx, tab_idx) {
            return false;
        }
        let Some((_ws_idx, tab_idx)) = self.pane_focus_target_indices(&target) else {
            return false;
        };
        self.pane_navigation_bias = None;
        if let Some(tab) = self.session_mut().and_then(|ws| ws.tabs.get_mut(tab_idx)) {
            tab.layout.focus_pane(pane_id);
            self.previous_pane_focus = previous;
            self.mark_session_dirty();
            return true;
        }
        false
    }

    pub(crate) fn focus_session_pane(&mut self, pane_id: PaneId) -> bool {
        let Some(ws_idx) = self
            .session_tab_entries()
            .find(|entry| entry.tab.panes.contains_key(&pane_id))
            .map(|entry| entry.session_idx)
        else {
            return false;
        };

        if self.focus_pane_in_session_at(ws_idx, pane_id) {
            return true;
        }

        self.collapse_to_single_session();
        self.session()
            .and_then(|ws| ws.pane_state(pane_id))
            .is_some()
    }

    #[cfg(test)]
    pub(crate) fn open_navigator(&mut self) {
        let terminal_runtimes = crate::terminal::TerminalRuntimeRegistry::new();
        self.open_navigator_from(&terminal_runtimes);
    }

    pub(crate) fn open_navigator_from(
        &mut self,
        terminal_runtimes: &crate::terminal::TerminalRuntimeRegistry,
    ) {
        self.navigator.query.clear();
        self.navigator.search_focused = true;
        self.navigator.scroll = 0;
        self.navigator.directory_candidates = zoxide_directory_candidates();

        self.mode = Mode::Navigator;
        self.navigator.selected = self
            .current_navigator_row_index_from(terminal_runtimes)
            .unwrap_or(0);
        self.ensure_navigator_selection_visible_from(terminal_runtimes);
    }

    #[cfg(test)]
    pub(crate) fn navigator_rows(&self) -> Vec<NavigatorRow> {
        let terminal_runtimes = crate::terminal::TerminalRuntimeRegistry::new();
        self.navigator_rows_from(&terminal_runtimes)
    }

    pub(crate) fn navigator_rows_from(
        &self,
        terminal_runtimes: &crate::terminal::TerminalRuntimeRegistry,
    ) -> Vec<NavigatorRow> {
        let query = self.navigator.query.trim().to_lowercase();
        let query_kind = navigator_query_kind(&query);
        let mut rows = Vec::new();
        let multi_tab = self.session_tab_count() > 1;

        for entry in self.session_entries() {
            let session_label = entry
                .session
                .display_name_from(&self.terminals, terminal_runtimes);
            let session_meta = entry
                .session
                .resolved_identity_cwd_from(&self.terminals, terminal_runtimes)
                .map(|cwd| cwd.display().to_string())
                .unwrap_or_else(|| "session".to_string());
            let session_search_text = format!("{session_label} {session_meta}").to_lowercase();
            let session_matches = match query_kind {
                NavigatorQueryKind::Empty => true,
                NavigatorQueryKind::Text => navigator_matches(&query, &session_search_text),
            };
            let child_query_kind = if session_matches {
                NavigatorQueryKind::Empty
            } else {
                query_kind
            };
            let child_rows =
                self.navigator_child_rows(entry.session_idx, child_query_kind, &query, multi_tab);
            if !session_matches && child_rows.is_empty() {
                continue;
            }

            if session_matches && query_kind == NavigatorQueryKind::Empty {
                rows.push(NavigatorRow {
                    target: NavigatorTarget::Session {
                        ws_idx: entry.session_idx,
                    },
                    depth: 0,
                    label: session_label,
                    meta: session_meta,
                    seen: true,
                    is_current: self.session_index() == Some(entry.session_idx),
                    is_tab: true,
                    search_text: session_search_text,
                });
            }
            rows.extend(child_rows);
        }

        for cwd in &self.navigator.directory_candidates {
            let label = crate::workspace::derive_label_from_cwd(cwd);
            let meta = cwd.display().to_string();
            let search_text = format!("{label} {meta}").to_lowercase();
            if query_kind == NavigatorQueryKind::Text && !navigator_matches(&query, &search_text) {
                continue;
            }
            rows.push(NavigatorRow {
                target: NavigatorTarget::Directory { cwd: cwd.clone() },
                depth: 0,
                label,
                meta,
                seen: true,
                is_current: false,
                is_tab: true,
                search_text,
            });
        }
        rows
    }

    fn navigator_child_rows(
        &self,
        ws_idx: usize,
        query_kind: NavigatorQueryKind,
        query: &str,
        multi_tab: bool,
    ) -> Vec<NavigatorRow> {
        let Some(session) = self
            .session_entries()
            .find(|entry| entry.session_idx == ws_idx)
            .map(|entry| entry.session)
        else {
            return Vec::new();
        };
        let mut rows = Vec::new();
        for tab_idx in 0..session.tabs.len() {
            let tab_row = multi_tab
                .then(|| self.navigator_tab_row(ws_idx, tab_idx))
                .flatten();
            let tab_matches = tab_row.as_ref().is_some_and(|row| match query_kind {
                NavigatorQueryKind::Empty => true,
                NavigatorQueryKind::Text => navigator_matches(query, &row.search_text),
            });
            let pane_rows = self.navigator_pane_rows_for_tab(ws_idx, tab_idx, multi_tab);
            let filtered_panes = match query_kind {
                NavigatorQueryKind::Empty => pane_rows,
                NavigatorQueryKind::Text if tab_matches => pane_rows,
                NavigatorQueryKind::Text => pane_rows
                    .into_iter()
                    .filter(|row| navigator_matches(query, &row.search_text))
                    .collect::<Vec<_>>(),
            };

            if let Some(tab_row) = tab_row {
                if tab_matches || !filtered_panes.is_empty() {
                    rows.push(tab_row);
                }
            }
            rows.extend(filtered_panes);
        }
        rows
    }

    fn navigator_tab_row(&self, ws_idx: usize, tab_idx: usize) -> Option<NavigatorRow> {
        let entry = self
            .session_tab_entries()
            .find(|entry| entry.session_idx == ws_idx && entry.tab_idx == tab_idx)?;
        let label =
            crate::workspace::session_tab_display_name(ws_idx, entry.session, tab_idx, entry.tab);
        let pane_count = entry.tab.panes.len();
        let meta = format!("{pane_count} panes");
        let search_text = format!("{label} {meta}").to_lowercase();
        Some(NavigatorRow {
            target: NavigatorTarget::Tab { ws_idx, tab_idx },
            depth: 0,
            label,
            meta,
            seen: true,
            is_current: false,
            is_tab: true,
            search_text,
        })
    }

    fn navigator_pane_rows_for_tab(
        &self,
        ws_idx: usize,
        tab_idx: usize,
        multi_tab: bool,
    ) -> Vec<NavigatorRow> {
        let Some(entry) = self
            .session_tab_entries()
            .find(|entry| entry.session_idx == ws_idx && entry.tab_idx == tab_idx)
        else {
            return Vec::new();
        };
        let mut rows = Vec::new();
        for pane_id in entry.tab.layout.pane_ids() {
            let Some(pane) = entry.tab.panes.get(&pane_id) else {
                continue;
            };
            let terminal = self.terminals.get(&pane.attached_terminal_id);
            let pane_number = entry.session.public_pane_number(pane_id).unwrap_or(0);
            let label = terminal
                .and_then(|terminal| terminal.effective_title())
                .or_else(|| {
                    terminal
                        .and_then(|terminal| terminal.manual_label.as_deref().map(str::to_string))
                })
                .or_else(|| {
                    launch_label(terminal.and_then(|terminal| terminal.launch_argv.as_ref()))
                })
                .unwrap_or_else(|| format!("pane {pane_number}"));
            let meta = "shell".to_string();
            let is_current = self.is_active_pane(ws_idx, tab_idx, pane_id);
            let search_text = format!("{label} {meta}").to_lowercase();
            rows.push(NavigatorRow {
                target: NavigatorTarget::Pane {
                    ws_idx,
                    tab_idx,
                    pane_id,
                },
                depth: if multi_tab { 1 } else { 0 },
                label,
                meta,
                seen: pane.seen,
                is_current,
                is_tab: false,
                search_text,
            });
        }
        rows
    }

    fn current_navigator_row_index_from(
        &self,
        terminal_runtimes: &crate::terminal::TerminalRuntimeRegistry,
    ) -> Option<usize> {
        let rows = self.navigator_rows_from(terminal_runtimes);
        rows.iter()
            .position(|row| matches!(row.target, NavigatorTarget::Pane { .. }) && row.is_current)
            .or_else(|| rows.iter().position(|row| row.is_current))
    }

    pub(crate) fn ensure_navigator_selection_visible_from(
        &mut self,
        terminal_runtimes: &crate::terminal::TerminalRuntimeRegistry,
    ) {
        let body = self.navigator_body_rect();
        let viewport = body.height as usize;
        if viewport == 0 {
            self.navigator.scroll = 0;
            return;
        }
        let max_scroll = self.navigator_max_scroll_from(terminal_runtimes, viewport);
        if self.navigator.selected < self.navigator.scroll {
            self.navigator.scroll = self.navigator.selected;
        } else if self.navigator.selected >= self.navigator.scroll.saturating_add(viewport) {
            self.navigator.scroll = self
                .navigator
                .selected
                .saturating_add(1)
                .saturating_sub(viewport);
        }
        self.navigator.scroll = self.navigator.scroll.min(max_scroll);
    }

    pub(crate) fn navigator_max_scroll_from(
        &self,
        terminal_runtimes: &crate::terminal::TerminalRuntimeRegistry,
        viewport: usize,
    ) -> usize {
        if viewport == 0 {
            return 0;
        }
        self.navigator_rows_from(terminal_runtimes)
            .len()
            .saturating_sub(viewport)
    }

    pub(crate) fn move_navigator_selection_from(
        &mut self,
        terminal_runtimes: &crate::terminal::TerminalRuntimeRegistry,
        delta: isize,
    ) {
        let count = self.navigator_rows_from(terminal_runtimes).len();
        if count == 0 {
            self.navigator.selected = 0;
            self.navigator.scroll = 0;
            return;
        }
        let current = self.navigator.selected.min(count - 1) as isize;
        self.navigator.selected = (current + delta).clamp(0, count as isize - 1) as usize;
        self.ensure_navigator_selection_visible_from(terminal_runtimes);
    }

    pub(crate) fn clamp_navigator_selection_from(
        &mut self,
        terminal_runtimes: &crate::terminal::TerminalRuntimeRegistry,
    ) {
        let count = self.navigator_rows_from(terminal_runtimes).len();
        self.navigator.selected = self.navigator.selected.min(count.saturating_sub(1));
        self.ensure_navigator_selection_visible_from(terminal_runtimes);
    }

    #[cfg(test)]
    pub(crate) fn accept_navigator_selection(&mut self) -> bool {
        let terminal_runtimes = crate::terminal::TerminalRuntimeRegistry::new();
        self.accept_navigator_selection_from(&terminal_runtimes)
    }

    pub(crate) fn accept_navigator_selection_from(
        &mut self,
        terminal_runtimes: &crate::terminal::TerminalRuntimeRegistry,
    ) -> bool {
        let Some(row) = self
            .navigator_rows_from(terminal_runtimes)
            .get(self.navigator.selected)
            .cloned()
        else {
            return false;
        };
        self.focus_navigator_target(row.target)
    }

    pub(crate) fn focus_navigator_target(&mut self, target: NavigatorTarget) -> bool {
        match target {
            NavigatorTarget::Session { ws_idx } => {
                if self.sessions.get(ws_idx).is_none() {
                    return false;
                }
                self.focus_session(ws_idx);
                self.mode = Mode::Terminal;
                true
            }
            NavigatorTarget::Tab { ws_idx, tab_idx } => {
                if !self
                    .session_tab_entries()
                    .any(|entry| entry.session_idx == ws_idx && entry.tab_idx == tab_idx)
                {
                    return false;
                }
                self.focus_session_tab(ws_idx, tab_idx);
                self.mode = Mode::Terminal;
                true
            }
            NavigatorTarget::Pane {
                ws_idx,
                tab_idx,
                pane_id,
            } => {
                if self.session_tab_entries().any(|entry| {
                    entry.session_idx == ws_idx
                        && entry.tab_idx == tab_idx
                        && entry.tab.panes.contains_key(&pane_id)
                }) {
                    self.focus_pane_in_session_at(ws_idx, pane_id);
                    self.mode = Mode::Terminal;
                    return true;
                }
                false
            }
            NavigatorTarget::Directory { cwd } => {
                self.request_new_session_cwd = Some(cwd);
                self.mode = Mode::Terminal;
                true
            }
        }
    }
}

fn zoxide_directory_candidates() -> Vec<std::path::PathBuf> {
    #[cfg(test)]
    {
        return Vec::new();
    }
    #[cfg(not(test))]
    {
        let Ok(output) = std::process::Command::new("zoxide")
            .args(["query", "-l"])
            .output()
        else {
            return Vec::new();
        };
        if !output.status.success() {
            return Vec::new();
        }
        String::from_utf8_lossy(&output.stdout)
            .lines()
            .filter_map(|line| {
                let path = std::path::PathBuf::from(line.trim());
                (path.is_absolute() && path.is_dir()).then_some(path)
            })
            .take(100)
            .collect()
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum NavigatorQueryKind {
    Empty,
    Text,
}

fn navigator_query_kind(query: &str) -> NavigatorQueryKind {
    if query.is_empty() {
        NavigatorQueryKind::Empty
    } else {
        NavigatorQueryKind::Text
    }
}

fn navigator_matches(query: &str, text: &str) -> bool {
    text_matches_query(query, text)
}

fn launch_label(argv: Option<&Vec<String>>) -> Option<String> {
    let argv = argv?;
    let command = argv.first()?;
    std::path::Path::new(command)
        .file_name()
        .and_then(|name| name.to_str())
        .map(str::to_string)
        .or_else(|| Some(command.clone()))
}

// ---------------------------------------------------------------------------
// Session operations
// ---------------------------------------------------------------------------

impl AppState {
    pub(crate) fn session_index(&self) -> Option<usize> {
        self.active_session
            .filter(|idx| {
                self.session_entries()
                    .any(|entry| entry.session_idx == *idx)
            })
            .or_else(|| self.session_entries().next().map(|entry| entry.session_idx))
    }

    pub(crate) fn has_session(&self) -> bool {
        self.session_index().is_some()
    }

    pub(crate) fn session(&self) -> Option<&SessionUiState> {
        self.session_index().and_then(|idx| {
            self.session_entries()
                .find(|entry| entry.session_idx == idx)
                .map(|entry| entry.session)
        })
    }

    pub(crate) fn session_mut(&mut self) -> Option<&mut SessionUiState> {
        let idx = self.session_index()?;
        self.sessions.get_mut(idx)
    }

    pub(crate) fn collapse_to_single_session(&mut self) -> bool {
        let session_count = self.session_entries().count();
        match session_count {
            0 => {
                let changed = self.active_session.take().is_some() || self.selected_session != 0;
                self.selected_session = 0;
                self.tab_scroll = 0;
                self.tab_scroll_follow_active = true;
                changed
            }
            1 => {
                let changed = self.active_session != Some(0) || self.selected_session != 0;
                self.active_session = Some(0);
                self.selected_session = 0;
                changed
            }
            _ => {
                let active_ws_idx = self
                    .active_session
                    .unwrap_or(self.selected_session)
                    .min(session_count.saturating_sub(1));
                let active_tab =
                    self.session_entries()
                        .take(active_ws_idx + 1)
                        .fold(0, |offset, entry| {
                            if entry.session_idx == active_ws_idx {
                                offset
                                    + entry
                                        .session
                                        .active_tab
                                        .min(entry.session.tabs.len().saturating_sub(1))
                            } else {
                                offset + entry.session.tabs.len()
                            }
                        });

                let mut sessions = std::mem::take(self.sessions_mut());
                let mut primary = sessions.remove(0);
                for mut session in sessions {
                    if let (Some(name), Some(first_tab)) =
                        (session.custom_name.take(), session.tabs.first_mut())
                    {
                        if first_tab.custom_name.is_none() {
                            first_tab.custom_name = Some(name);
                        }
                    }
                    primary.tabs.append(&mut session.tabs);
                }

                if primary.tabs.is_empty() {
                    self.clear_session();
                    self.tab_scroll = 0;
                    self.tab_scroll_follow_active = true;
                    return true;
                }

                for (idx, tab) in primary.tabs.iter_mut().enumerate() {
                    tab.number = idx + 1;
                }
                primary.active_tab = active_tab.min(primary.tabs.len().saturating_sub(1));
                primary.public_pane_numbers.clear();
                let mut next_public_pane_number = 1;
                for pane_id in primary
                    .tabs
                    .iter()
                    .flat_map(|tab| tab.layout.pane_ids().into_iter())
                {
                    primary
                        .public_pane_numbers
                        .insert(pane_id, next_public_pane_number);
                    next_public_pane_number += 1;
                }
                primary.next_public_pane_number = next_public_pane_number;

                self.set_session(primary);
                self.mobile_switcher_scroll = 0;
                self.pane_panel_scroll = 0;
                self.tab_scroll_follow_active = true;
                self.refresh_tab_bar_view();
                true
            }
        }
    }

    pub fn focus_session(&mut self, idx: usize) {
        let Some(active_tab) = self
            .session_entries()
            .find(|entry| entry.session_idx == idx)
            .and_then(|entry| {
                (!entry.session.tabs.is_empty()).then_some(
                    entry
                        .session
                        .active_tab
                        .min(entry.session.tabs.len().saturating_sub(1)),
                )
            })
        else {
            return;
        };

        self.focus_session_tab(idx, active_tab);
    }

    pub(crate) fn focus_session_tab(&mut self, ws_idx: usize, tab_idx: usize) -> bool {
        let Some(flat_tab_idx) = self.flattened_tab_index(ws_idx, tab_idx) else {
            return false;
        };

        let previous_focus = self.current_pane_focus_target();
        let session_changed =
            self.active_session != Some(ws_idx) || self.session_entries().nth(1).is_some();
        self.selection = None;
        self.selection_autoscroll = None;
        self.pane_navigation_bias = None;

        self.collapse_to_single_session();
        self.active_session = Some(0);
        self.selected_session = 0;
        let Some(session_id) = self.session().map(|session| session.id.clone()) else {
            return false;
        };
        if session_changed {
            crate::logging::session_focused(&session_id);
        }
        self.mark_session_dirty();
        if session_changed
            && matches!(
                self.pane_panel_scope,
                crate::app::state::PanePanelScope::Current
            )
        {
            self.pane_panel_scroll = 0;
        }
        self.ensure_session_visible(0);
        if let Some(ws) = self.session_mut() {
            ws.switch_tab(flat_tab_idx);
            let tab_id = format!("{}:{}", session_id, flat_tab_idx + 1);
            crate::logging::tab_focused(&session_id, &tab_id);
        }
        self.tab_scroll_follow_active = true;
        self.refresh_tab_bar_view();
        self.record_pane_focus_after_navigation(previous_focus);
        true
    }

    pub(crate) fn ensure_session_visible(&mut self, idx: usize) {
        if !self.session_entries().any(|entry| entry.session_idx == idx) {
            return;
        }

        if self.view.layout == ViewLayout::Mobile && self.mode == Mode::Navigate {
            self.mobile_switcher_scroll = self
                .mobile_switcher_scroll
                .min(crate::ui::mobile_switcher_max_scroll(self));
        }
    }

    pub fn switch_tab(&mut self, idx: usize) {
        if self.session().is_none_or(|ws| idx >= ws.tabs.len()) {
            return;
        }
        let previous_focus = self.current_pane_focus_target();
        self.selection = None;
        self.selection_autoscroll = None;
        self.pane_navigation_bias = None;
        let Some(ws) = self.session_mut() else {
            return;
        };
        ws.switch_tab(idx);
        let session_id = ws.id.clone();
        let tab_id = format!("{}:{}", session_id, idx + 1);
        crate::logging::tab_focused(&session_id, &tab_id);
        self.mark_session_dirty();
        self.tab_scroll_follow_active = true;
        self.refresh_tab_bar_view();
        self.record_pane_focus_after_navigation(previous_focus);
    }

    pub(crate) fn mark_active_tab_seen(&mut self) -> bool {
        let Some(tab) = self.session_mut().and_then(SessionUiState::active_tab_mut) else {
            return false;
        };

        let mut changed = false;
        for pane in tab.panes.values_mut() {
            if !pane.seen {
                pane.seen = true;
                changed = true;
            }
        }
        changed
    }

    pub fn scroll_tabs_left(&mut self) {
        self.tab_scroll_follow_active = false;
        self.tab_scroll = self.tab_scroll.saturating_sub(1);
        self.refresh_tab_bar_view();
    }

    pub fn scroll_tabs_right(&mut self) {
        self.tab_scroll_follow_active = false;
        self.tab_scroll = self.tab_scroll.saturating_add(1);
        self.refresh_tab_bar_view();
    }

    pub fn move_tab(&mut self, source_idx: usize, insert_idx: usize) {
        if let Some(ws) = self.session_mut() {
            if ws.move_tab(source_idx, insert_idx) {
                self.mark_session_dirty();
                self.tab_scroll_follow_active = true;
                self.refresh_tab_bar_view();
            }
        }
    }

    pub fn next_tab(&mut self) {
        if let Some(ws) = self.session() {
            if !ws.tabs.is_empty() {
                let next = (ws.active_tab + 1) % ws.tabs.len();
                self.switch_tab(next);
            }
        }
    }

    pub fn last_tab(&mut self) {
        if let Some(ws) = self.session() {
            if let Some(last) = ws.tabs.len().checked_sub(1) {
                self.switch_tab(last);
            }
        }
    }

    pub fn previous_tab(&mut self) {
        if let Some(ws) = self.session() {
            if !ws.tabs.is_empty() {
                let prev = if ws.active_tab == 0 {
                    ws.tabs.len() - 1
                } else {
                    ws.active_tab - 1
                };
                self.switch_tab(prev);
            }
        }
    }

    pub fn next_pane_panel_entry(&mut self) {
        self.cycle_pane_panel_entry(true);
    }

    pub fn previous_pane_panel_entry(&mut self) {
        self.cycle_pane_panel_entry(false);
    }

    pub fn focus_pane_panel_entry(&mut self, idx: usize) -> bool {
        let entries = crate::ui::pane_panel_entries(self);
        let Some(target) = entries.get(idx) else {
            return false;
        };
        let ws_idx = target.ws_idx;
        let pane_id = target.pane_id;

        if self.session_index() == Some(ws_idx)
            && self.session().and_then(SessionUiState::focused_pane_id) == Some(pane_id)
        {
            self.ensure_pane_panel_entry_visible(idx);
            return true;
        }

        if self.focus_pane_in_session_at(ws_idx, pane_id) {
            self.ensure_pane_panel_entry_visible(idx);
            return true;
        }
        false
    }

    fn cycle_pane_panel_entry(&mut self, forward: bool) {
        let entries = crate::ui::pane_panel_entries(self);
        if entries.is_empty() {
            return;
        }

        let focused = self.session().and_then(SessionUiState::focused_pane_id);
        let current_idx =
            focused.and_then(|pane_id| entries.iter().position(|entry| entry.pane_id == pane_id));
        let target_idx = match (current_idx, forward) {
            (Some(idx), true) => (idx + 1) % entries.len(),
            (Some(0), false) => entries.len() - 1,
            (Some(idx), false) => idx - 1,
            (None, true) => 0,
            (None, false) => entries.len() - 1,
        };

        self.focus_pane_panel_entry(target_idx);
    }

    fn ensure_pane_panel_entry_visible(&mut self, idx: usize) {
        if self.sidebar_collapsed {
            return;
        }

        let detail_area = crate::ui::expanded_pane_panel_rect(self.view.sidebar_rect);
        let metrics = crate::ui::pane_panel_scroll_metrics(self, detail_area);
        let visible = metrics.viewport_rows;
        if visible == 0 {
            return;
        }

        if idx < self.pane_panel_scroll {
            self.pane_panel_scroll = idx;
        } else if idx >= self.pane_panel_scroll.saturating_add(visible) {
            self.pane_panel_scroll = idx.saturating_add(1).saturating_sub(visible);
        }

        let max_scroll =
            crate::ui::pane_panel_scroll_metrics(self, detail_area).max_offset_from_bottom;
        self.pane_panel_scroll = self.pane_panel_scroll.min(max_scroll);
    }

    pub(crate) fn terminal_ids_for_session_at(
        &self,
        ws_idx: usize,
    ) -> Vec<crate::terminal::TerminalId> {
        self.session_tab_entries()
            .filter(|entry| entry.session_idx == ws_idx)
            .flat_map(|entry| entry.tab.panes.values())
            .map(|pane| pane.attached_terminal_id.clone())
            .collect()
    }

    pub(crate) fn terminal_ids_for_tab(
        &self,
        ws_idx: usize,
        tab_idx: usize,
    ) -> Vec<crate::terminal::TerminalId> {
        self.session_tab_entries()
            .find(|entry| entry.session_idx == ws_idx && entry.tab_idx == tab_idx)
            .map(|entry| entry.tab)
            .into_iter()
            .flat_map(|tab| tab.panes.values())
            .map(|pane| pane.attached_terminal_id.clone())
            .collect()
    }

    pub(crate) fn terminal_id_for_pane(
        &self,
        ws_idx: usize,
        pane_id: PaneId,
    ) -> Option<crate::terminal::TerminalId> {
        self.session_tab_entries()
            .find(|entry| entry.session_idx == ws_idx && entry.tab.panes.contains_key(&pane_id))?
            .tab
            .panes
            .get(&pane_id)
            .map(|pane| pane.attached_terminal_id.clone())
    }

    pub(crate) fn remove_unattached_terminal_ids(
        &mut self,
        terminal_ids: impl IntoIterator<Item = crate::terminal::TerminalId>,
    ) {
        for terminal_id in terminal_ids {
            let still_attached = self.session_tab_entries().any(|entry| {
                entry
                    .tab
                    .panes
                    .values()
                    .any(|pane| pane.attached_terminal_id == terminal_id)
            });
            if !still_attached
                && self.terminals.remove(&terminal_id).is_some()
                && !self.terminal_runtime_shutdowns.contains(&terminal_id)
            {
                self.terminal_runtime_shutdowns.push(terminal_id);
            }
        }
    }

    pub fn close_session(&mut self) {
        if !self.has_session() {
            return;
        }
        self.collapse_to_single_session();
        let Some(close_idx) = self.session_index() else {
            return;
        };
        self.selection = None;
        self.selection_autoscroll = None;
        self.mark_session_dirty();

        let mut terminal_ids = Vec::new();
        terminal_ids.extend(self.terminal_ids_for_session_at(close_idx));
        if let Some(session_id) = self
            .session_entries()
            .find(|entry| entry.session_idx == close_idx)
            .map(|entry| entry.session.id.clone())
        {
            crate::logging::session_closed(&session_id);
        }
        self.clear_session();
        self.should_quit = true;
        self.remove_unattached_terminal_ids(terminal_ids);
        self.tab_scroll = 0;
        self.tab_scroll_follow_active = true;
    }

    fn refresh_tab_bar_view(&mut self) {
        let terminal_runtimes = crate::terminal::TerminalRuntimeRegistry::new();
        let area = crate::ui::top_bar_tab_area(self, &terminal_runtimes, self.view.tab_bar_rect);
        let Some(ws) = self.session() else {
            self.tab_scroll = 0;
            self.view.tab_hit_areas.clear();
            self.view.tab_scroll_left_hit_area = ratatui::layout::Rect::default();
            self.view.tab_scroll_right_hit_area = ratatui::layout::Rect::default();
            self.view.new_tab_hit_area = ratatui::layout::Rect::default();
            return;
        };

        let layout = crate::ui::compute_tab_bar_view(
            ws,
            area,
            self.tab_scroll,
            self.tab_scroll_follow_active,
            self.mouse_capture,
        );
        self.tab_scroll = layout.scroll;
        self.view.tab_hit_areas = layout.tab_hit_areas;
        self.view.tab_scroll_left_hit_area = layout.scroll_left_hit_area;
        self.view.tab_scroll_right_hit_area = layout.scroll_right_hit_area;
        self.view.new_tab_hit_area = layout.new_tab_hit_area;
    }
}

// ---------------------------------------------------------------------------
// Pane operations
// ---------------------------------------------------------------------------

impl AppState {
    pub fn navigate_pane(&mut self, direction: NavDirection) -> bool {
        let Some(ws_idx) = self.session_index() else {
            return false;
        };
        let Some(session_id) = self.session().map(|ws| ws.id.clone()) else {
            return false;
        };
        let Some(tab) = self.session().and_then(|ws| ws.active_tab()) else {
            return false;
        };
        let panes = if tab.zoomed {
            tab.layout.panes(self.view.terminal_area)
        } else {
            self.view.pane_infos.clone()
        };

        if let Some(focused) = panes.iter().find(|p| p.is_focused) {
            let bias = self.navigation_bias_for(&session_id, focused, direction);
            if let Some(target) = find_in_direction_with_bias(focused, direction, &panes, bias) {
                let source = focused.clone();
                if self.focus_pane_in_session_at(ws_idx, target) {
                    self.pane_navigation_bias = Some(crate::app::state::PaneNavigationBias {
                        session_id,
                        pane_id: target,
                        axis: navigation_axis(direction),
                        perpendicular_coord: perpendicular_center(&source, direction),
                    });
                    return true;
                }
            }
        }
        self.pane_navigation_bias = None;
        false
    }

    fn navigation_bias_for(
        &self,
        session_id: &str,
        focused: &PaneInfo,
        direction: NavDirection,
    ) -> Option<u16> {
        let bias = self.pane_navigation_bias.as_ref()?;
        (bias.session_id == session_id
            && bias.pane_id == focused.id
            && bias.axis == navigation_axis(direction))
        .then_some(bias.perpendicular_coord)
    }

    pub fn resize_pane(&mut self, direction: NavDirection) -> bool {
        if let Some(first) = self.view.pane_infos.first() {
            let area = self
                .view
                .pane_infos
                .iter()
                .fold(first.rect, |acc, p| acc.union(p.rect));
            self.pane_navigation_bias = None;
            if let Some(tab) = self.session_mut().and_then(|ws| ws.active_tab_mut()) {
                tab.layout.resize_focused(direction, 0.05, area);
                self.mark_session_dirty();
                return true;
            }
        }
        false
    }

    pub fn cycle_pane(&mut self, reverse: bool) {
        let Some(ws_idx) = self.session_index() else {
            return;
        };
        let Some(tab) = self.session().and_then(|ws| ws.active_tab()) else {
            return;
        };
        let ids = tab.layout.pane_ids();
        if let Some(pos) = ids.iter().position(|id| *id == tab.layout.focused()) {
            let target = if reverse {
                ids[(pos + ids.len() - 1) % ids.len()]
            } else {
                ids[(pos + 1) % ids.len()]
            };
            self.focus_pane_in_session_at(ws_idx, target);
        }
    }

    pub fn swap_pane(&mut self, reverse: bool) -> bool {
        self.selection = None;
        self.selection_autoscroll = None;
        self.pane_navigation_bias = None;
        let Some(tab) = self.session_mut().and_then(|ws| ws.active_tab_mut()) else {
            return false;
        };
        if !tab.swap_focused_pane(reverse) {
            return false;
        }
        self.mark_session_dirty();
        true
    }

    pub fn last_pane(&mut self) {
        let Some(target) = self.previous_pane_focus.clone() else {
            return;
        };
        let Some((ws_idx, tab_idx)) = self.pane_focus_target_indices(&target) else {
            self.previous_pane_focus = None;
            return;
        };
        let current = self.current_pane_focus_target();
        if current.as_ref() == Some(&target) {
            self.previous_pane_focus = None;
            return;
        }

        if !self.focus_session_tab(ws_idx, tab_idx) {
            self.previous_pane_focus = None;
            return;
        }
        let Some((ws_idx, tab_idx)) = self.pane_focus_target_indices(&target) else {
            self.previous_pane_focus = None;
            return;
        };
        if self.session_index() != Some(ws_idx) {
            self.previous_pane_focus = None;
            return;
        }
        self.pane_navigation_bias = None;
        if let Some(tab) = self.session_mut().and_then(|ws| ws.tabs.get_mut(tab_idx)) {
            tab.layout.focus_pane(target.pane_id);
            self.previous_pane_focus = current;
            self.mark_session_dirty();
        }
    }

    pub fn toggle_zoom(&mut self) {
        self.pane_navigation_bias = None;
        if let Some(tab) = self.session_mut().and_then(|ws| ws.active_tab_mut()) {
            if tab.layout.pane_count() > 1 {
                tab.zoomed = !tab.zoomed;
                self.mark_session_dirty();
            }
        }
    }

    /// Close the focused pane. Returns true when the close was deferred to confirmation.
    pub fn close_pane(&mut self) -> bool {
        self.collapse_to_single_session();
        let active = self.session_index();
        self.selection = None;
        self.selection_autoscroll = None;
        self.pane_navigation_bias = None;
        self.mark_session_dirty();
        let terminal_ids = active
            .and_then(|i| {
                self.session()
                    .and_then(|ws| ws.focused_pane_id().map(|pane_id| (i, pane_id)))
            })
            .and_then(|(i, pane_id)| self.terminal_id_for_pane(i, pane_id))
            .into_iter()
            .collect::<Vec<_>>();
        let should_close_session = active
            .and_then(|_| self.session_mut())
            .is_some_and(|ws| ws.close_focused());
        if should_close_session {
            self.close_session();
        } else {
            self.remove_unattached_terminal_ids(terminal_ids);
        }
        false
    }

    /// Close the active tab. Returns true when the close was deferred to confirmation.
    pub fn close_tab(&mut self) -> bool {
        self.collapse_to_single_session();
        self.selection = None;
        self.selection_autoscroll = None;
        self.mark_session_dirty();
        let should_close_session = self.session().is_some_and(|ws| ws.tabs.len() <= 1);
        if should_close_session {
            self.close_session();
            return false;
        }
        if let Some(ws_idx) = self.session_index() {
            let terminal_ids = self
                .session()
                .map(|ws| self.terminal_ids_for_tab(ws_idx, ws.active_tab))
                .unwrap_or_default();
            let Some(ws) = self.session_mut() else {
                return false;
            };
            let session_id = ws.id.clone();
            let closing_tab_id = format!("{}:{}", session_id, ws.active_tab + 1);
            ws.close_active_tab();
            self.remove_unattached_terminal_ids(terminal_ids);
            crate::logging::tab_closed(&session_id, &closing_tab_id);
            self.tab_scroll_follow_active = true;
            self.refresh_tab_bar_view();
        }
        false
    }
}

fn navigation_axis(direction: NavDirection) -> PaneNavigationAxis {
    match direction {
        NavDirection::Left | NavDirection::Right => PaneNavigationAxis::Horizontal,
        NavDirection::Up | NavDirection::Down => PaneNavigationAxis::Vertical,
    }
}

fn perpendicular_center(info: &PaneInfo, direction: NavDirection) -> u16 {
    match direction {
        NavDirection::Left | NavDirection::Right => {
            info.rect.y.saturating_add(info.rect.height / 2)
        }
        NavDirection::Up | NavDirection::Down => info.rect.x.saturating_add(info.rect.width / 2),
    }
}

// ---------------------------------------------------------------------------
// Selection
// ---------------------------------------------------------------------------

impl AppState {
    pub fn clear_selection(&mut self) {
        self.selection = None;
        self.selection_autoscroll = None;
        self.selection_viewport_pin = None;
    }

    pub(crate) fn stop_selection_autoscroll_state(&mut self) {
        self.selection_autoscroll = None;
    }

    pub(crate) fn copy_word_at_pane_cell(
        &mut self,
        terminal_runtimes: &crate::terminal::TerminalRuntimeRegistry,
        pane_id: crate::layout::PaneId,
        viewport_row: u16,
        col: u16,
    ) -> bool {
        // Resolve the active pane cell the double-click landed on.
        let Some(ws_idx) = self.session_index() else {
            return false;
        };

        let Some(info) = self.pane_info_by_id(pane_id) else {
            return false;
        };
        if viewport_row >= info.inner_rect.height || col >= info.inner_rect.width {
            return false;
        }

        // Leave mouse input to terminal apps that requested it.
        let Some(rt) = self.runtime_for_pane_in_session_at(terminal_runtimes, ws_idx, pane_id)
        else {
            return false;
        };
        if rt
            .input_state()
            .is_some_and(crate::pane::InputState::mouse_reporting_enabled)
        {
            return false;
        }

        // Read the visible row and identify the clicked token bounds.
        let metrics = self.pane_scroll_metrics(terminal_runtimes, pane_id);
        let row_selection = Selection::range(
            pane_id,
            viewport_row,
            0,
            info.inner_rect.width.saturating_sub(1),
            metrics,
        );
        let Some(row_text) = rt.extract_selection(&row_selection) else {
            return false;
        };
        let Some((start_col, end_col)) = word_bounds_at_column(&row_text, col) else {
            return false;
        };

        // Copy the token and keep its selection visible as short-lived feedback.
        let mut selection = Selection::range(pane_id, viewport_row, start_col, end_col, metrics);
        if !selection.finish() {
            return false;
        }

        let Some(text) = rt
            .extract_selection(&selection)
            .filter(|text| !text.is_empty())
        else {
            self.clear_selection();
            return false;
        };
        self.request_clipboard_write = Some(text.into_bytes());
        self.selection = Some(selection);
        self.selection_autoscroll = None;
        info!("copied double-clicked token to clipboard");
        true
    }

    pub(crate) fn url_at_pane_cell(
        &self,
        terminal_runtimes: &crate::terminal::TerminalRuntimeRegistry,
        pane_id: crate::layout::PaneId,
        viewport_row: u16,
        col: u16,
    ) -> Option<String> {
        let ws_idx = self.session_index()?;
        let info = self.pane_info_by_id(pane_id)?;
        if viewport_row >= info.inner_rect.height || col >= info.inner_rect.width {
            return None;
        }

        let rt = self.runtime_for_pane_in_session_at(terminal_runtimes, ws_idx, pane_id)?;
        let screen_col = info.inner_rect.x.saturating_add(col);
        let screen_row = info.inner_rect.y.saturating_add(viewport_row);
        if let Some((_, _, uri)) = rt
            .visible_hyperlinks(info.inner_rect)
            .into_iter()
            .find(|((x, y), _, _)| *x == screen_col && *y == screen_row)
        {
            return safe_web_url(&uri).map(str::to_owned);
        }

        let metrics = self.pane_scroll_metrics(terminal_runtimes, pane_id);
        let row_selection = Selection::range(
            pane_id,
            viewport_row,
            0,
            info.inner_rect.width.saturating_sub(1),
            metrics,
        );
        let row_text = rt.extract_selection(&row_selection)?;
        url_at_column(&row_text, col).map(str::to_owned)
    }

    pub fn copy_selection(&mut self, terminal_runtimes: &crate::terminal::TerminalRuntimeRegistry) {
        let mut sel = match self.selection.take() {
            Some(sel) => sel,
            None => return,
        };
        if !sel.finish() {
            return;
        }

        let Some(ws_idx) = self.session_index() else {
            return;
        };

        let text = self
            .runtime_for_pane_in_session_at(terminal_runtimes, ws_idx, sel.pane_id)
            .and_then(|rt| rt.extract_selection(&sel));
        if let Some(text) = text {
            if !text.is_empty() {
                self.request_clipboard_write = Some(text.into_bytes());
                info!("copied selection to clipboard");
            }
        }

        self.clear_selection();
    }
}

pub(crate) fn safe_web_url(url: &str) -> Option<&str> {
    (url.starts_with("http://") || url.starts_with("https://")).then_some(url)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct TextCell {
    ch: char,
    start_col: u16,
    end_col: u16,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct CellSpan {
    start: usize,
    end: usize,
}

impl CellSpan {
    fn contains(self, idx: usize) -> bool {
        idx >= self.start && idx <= self.end
    }

    fn columns(self, cells: &[TextCell]) -> (u16, u16) {
        (cells[self.start].start_col, cells[self.end].end_col)
    }
}

/// Finds the terminal display-column bounds for the token under a double-click.
///
/// The algorithm first maps text to terminal cells so wide characters and
/// zero-width marks use display columns, then prefers structured spans that
/// users expect to copy whole (URLs and quoted paths), and finally falls back
/// to a separator-delimited token.
fn word_bounds_at_column(row: &str, col: u16) -> Option<(u16, u16)> {
    // Map the row into display cells before doing any word-boundary work.
    let cells = text_cells(row);
    let clicked_idx = cell_index_at_column(&cells, col)?;

    // Prefer spans that can legally include punctuation or spaces.
    let span = url_span_at_column(&cells, clicked_idx)
        .or_else(|| quoted_path_span_at_column(&cells, clicked_idx))
        .or_else(|| token_span_at_column(&cells, clicked_idx))?;

    // Convert the internal cell span back to inclusive terminal columns.
    Some(span.columns(&cells))
}

pub(crate) fn url_at_column(row: &str, col: u16) -> Option<&str> {
    let cells = text_cells(row);
    let clicked_idx = cell_index_at_column(&cells, col)?;
    let span = url_span_at_column(&cells, clicked_idx)?;
    let start_byte = byte_index_for_cell(row, span.start);
    let end_byte = byte_index_after_cell(row, span.end);
    safe_web_url(row.get(start_byte..end_byte)?)
}

fn token_span_at_column(cells: &[TextCell], clicked_idx: usize) -> Option<CellSpan> {
    if is_word_separator(cells[clicked_idx].ch) {
        return None;
    }

    let mut start = clicked_idx;
    while start > 0 && !is_word_separator(cells[start - 1].ch) {
        start -= 1;
    }

    let mut end = clicked_idx;
    while end + 1 < cells.len() && !is_word_separator(cells[end + 1].ch) {
        end += 1;
    }

    trim_token_edges(cells, CellSpan { start, end }).filter(|span| span.contains(clicked_idx))
}

fn text_cells(row: &str) -> Vec<TextCell> {
    let mut next_col = 0u16;
    row.chars()
        .map(|ch| {
            let width = UnicodeWidthChar::width(ch).unwrap_or(0) as u16;
            let start_col = if width == 0 {
                next_col.saturating_sub(1)
            } else {
                next_col
            };
            if width > 0 {
                next_col = next_col.saturating_add(width);
            }
            TextCell {
                ch,
                start_col,
                end_col: next_col.saturating_sub(1),
            }
        })
        .collect()
}

fn cell_index_at_column(cells: &[TextCell], col: u16) -> Option<usize> {
    cells
        .iter()
        .position(|cell| cell.start_col <= col && col <= cell.end_col)
}

fn byte_index_for_cell(row: &str, cell_idx: usize) -> usize {
    row.char_indices()
        .nth(cell_idx)
        .map(|(idx, _)| idx)
        .unwrap_or(row.len())
}

fn byte_index_after_cell(row: &str, cell_idx: usize) -> usize {
    row.char_indices()
        .nth(cell_idx.saturating_add(1))
        .map(|(idx, _)| idx)
        .unwrap_or(row.len())
}

fn url_span_at_column(cells: &[TextCell], clicked_idx: usize) -> Option<CellSpan> {
    let mut start = 0;
    while start < cells.len() {
        if starts_with_chars(&cells[start..], "http://")
            || starts_with_chars(&cells[start..], "https://")
        {
            let mut end = start;
            while end + 1 < cells.len() && !cells[end + 1].ch.is_whitespace() {
                end += 1;
            }
            if clicked_idx >= start && clicked_idx <= end {
                let span = trim_url_edges(cells, CellSpan { start, end })?;
                return span.contains(clicked_idx).then_some(span);
            }
            start = end + 1;
        } else {
            start += 1;
        }
    }
    None
}

fn trim_url_edges(cells: &[TextCell], span: CellSpan) -> Option<CellSpan> {
    let start = span.start;
    let mut end = span.end;
    while start <= end && should_trim_trailing_url_cell(cells, start, end) {
        if end == 0 {
            return None;
        }
        end -= 1;
    }
    (start <= end).then_some(CellSpan { start, end })
}

fn should_trim_trailing_url_cell(cells: &[TextCell], start: usize, end: usize) -> bool {
    match cells[end].ch {
        '"' | '\'' | '`' | '.' | ',' | ';' | ':' | '!' | '?' => true,
        ')' => !trailing_url_closer_is_balanced(cells, start, end, '(', ')'),
        ']' => !trailing_url_closer_is_balanced(cells, start, end, '[', ']'),
        '}' => !trailing_url_closer_is_balanced(cells, start, end, '{', '}'),
        _ => false,
    }
}

fn trailing_url_closer_is_balanced(
    cells: &[TextCell],
    start: usize,
    end: usize,
    open: char,
    close: char,
) -> bool {
    let mut balance = 0i32;
    for cell in &cells[start..end] {
        if cell.ch == open {
            balance += 1;
        } else if cell.ch == close {
            balance -= 1;
        }
    }
    balance > 0
}

fn quoted_path_span_at_column(cells: &[TextCell], clicked_idx: usize) -> Option<CellSpan> {
    let clicked = cells.get(clicked_idx)?.ch;
    if clicked == '"' || clicked == '\'' || clicked == '`' {
        return None;
    }

    for quote in ['"', '\'', '`'] {
        let mut start = None;
        for (idx, cell) in cells.iter().copied().enumerate() {
            let ch = cell.ch;
            if ch != quote || is_escaped(cells, idx) {
                continue;
            }
            if let Some(open) = start {
                if clicked_idx > open
                    && clicked_idx < idx
                    && cells[open + 1..idx].iter().any(|cell| cell.ch == '/')
                {
                    return Some(CellSpan {
                        start: open + 1,
                        end: idx - 1,
                    });
                }
                start = None;
            } else {
                start = Some(idx);
            }
        }
    }
    None
}

fn is_escaped(cells: &[TextCell], idx: usize) -> bool {
    let mut slashes = 0;
    let mut cursor = idx;
    while cursor > 0 && cells[cursor - 1].ch == '\\' {
        slashes += 1;
        cursor -= 1;
    }
    slashes % 2 == 1
}

fn starts_with_chars(cells: &[TextCell], prefix: &str) -> bool {
    prefix
        .chars()
        .enumerate()
        .all(|(idx, expected)| cells.get(idx).is_some_and(|cell| cell.ch == expected))
}

fn is_word_separator(ch: char) -> bool {
    ch.is_whitespace()
        || matches!(
            ch,
            '|' | '(' | ')' | '[' | ']' | '{' | '}' | ',' | ';' | '!'
        )
}

fn trim_token_edges(cells: &[TextCell], span: CellSpan) -> Option<CellSpan> {
    let mut start = span.start;
    let mut end = span.end;
    while start <= end && is_leading_token_wrapper(cells[start].ch) {
        start += 1;
    }
    if start < end && cells[end].ch == '$' && is_trailing_token_wrapper(cells[end - 1].ch) {
        end -= 1;
    }
    while start <= end && is_trailing_token_wrapper(cells[end].ch) {
        if end == 0 {
            return None;
        }
        end -= 1;
    }
    (start <= end).then_some(CellSpan { start, end })
}

fn is_leading_token_wrapper(ch: char) -> bool {
    matches!(ch, '(' | '[' | '{' | '<' | '"' | '\'' | '`')
}

fn is_trailing_token_wrapper(ch: char) -> bool {
    matches!(
        ch,
        ')' | ']' | '}' | '>' | '"' | '\'' | '`' | '.' | ',' | ';' | ':' | '!' | '?'
    )
}

// ---------------------------------------------------------------------------
// Event handling
// ---------------------------------------------------------------------------

impl AppState {
    pub fn handle_app_event(&mut self, event: AppEvent) {
        match event {
            AppEvent::PaneDied { pane_id } => {
                self.handle_pane_died(pane_id);
            }
            // Intercepted in App::handle_internal_event before reaching this
            // dispatch; never touches AppState.
            AppEvent::ClipboardWrite { .. } => {}
            AppEvent::UpdateCheckFinished(result) => {
                self.update.checking = false;
                if result.release.is_some() {
                    self.update.available = result.release;
                }
                if let Some(error) = result.error {
                    tracing::debug!(error, "update check failed");
                }
            }
            AppEvent::UpdateInstallFinished(result) => {
                self.update.installing = false;
                match result {
                    crate::update::UpdateInstallResult::Success(success) => {
                        self.update.message = Some(format!(
                            "updated to {}; relaunching current session",
                            success.version
                        ));
                        self.mode = Mode::UpdateMessage;
                    }
                    crate::update::UpdateInstallResult::Failed { message } => {
                        self.update.message = Some(format!("update failed: {message}"));
                        self.mode = Mode::UpdateMessage;
                    }
                }
            }
        }
    }

    fn handle_pane_died(&mut self, pane_id: PaneId) {
        self.collapse_to_single_session();
        let ws_idx = self
            .session_tab_entries()
            .find(|entry| entry.tab.panes.contains_key(&pane_id))
            .map(|entry| entry.session_idx);

        let Some(ws_idx) = ws_idx else {
            warn!(pane = pane_id.raw(), "PaneDied for unknown pane");
            return;
        };

        if self
            .selection
            .as_ref()
            .is_some_and(|s| s.pane_id == pane_id)
        {
            self.selection = None;
            self.selection_autoscroll = None;
        }

        let pane_terminal_id = self.terminal_id_for_pane(ws_idx, pane_id);
        let session_terminal_ids = self.terminal_ids_for_session_at(ws_idx);
        self.pane_id_aliases.retain(|_, alias| *alias != pane_id);
        let should_close_session = {
            if self.session_index() != Some(ws_idx) {
                warn!(
                    pane = pane_id.raw(),
                    session = ws_idx,
                    "PaneDied target session disappeared"
                );
                return;
            }
            let Some(ws) = self.session_mut() else {
                warn!(
                    pane = pane_id.raw(),
                    session = ws_idx,
                    "PaneDied target session disappeared"
                );
                return;
            };
            ws.remove_pane(pane_id)
        };
        self.mark_session_dirty();

        if should_close_session {
            self.clear_session();
            self.should_quit = true;
            self.remove_unattached_terminal_ids(session_terminal_ids);
            if self.mode == Mode::Terminal {
                self.mode = Mode::Navigate;
            }
        } else {
            self.remove_unattached_terminal_ids(pane_terminal_id);
        }
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use crate::workspace::Workspace;
    use ratatui::layout::Direction;

    fn app_with_workspaces(names: &[&str]) -> AppState {
        let mut state = AppState::test_new();
        for name in names {
            let ws = Workspace::test_new(name);
            state.sessions.push(ws);
        }
        state.ensure_test_terminals();
        if !state.sessions.is_empty() {
            state.active_session = Some(0);
            state.mode = Mode::Terminal;
        }
        state
    }

    fn selected_word(row: &str, col: u16) -> Option<String> {
        let (start, end) = word_bounds_at_column(row, col)?;
        Some(text_in_cell_range(row, start, end))
    }

    fn selected_url<'a>(row: &'a str, click: &str) -> Option<&'a str> {
        url_at_column(row, col_of(row, click))
    }

    fn text_in_cell_range(row: &str, start_col: u16, end_col: u16) -> String {
        text_cells(row)
            .into_iter()
            .filter(|cell| cell.start_col >= start_col && cell.end_col <= end_col)
            .map(|cell| cell.ch)
            .collect()
    }

    fn col_of(row: &str, needle: &str) -> u16 {
        let byte_idx = row
            .find(needle)
            .unwrap_or_else(|| panic!("{needle:?} not found in {row:?}"));
        let prefix = &row[..byte_idx];
        prefix
            .chars()
            .map(|ch| UnicodeWidthChar::width(ch).unwrap_or(0) as u16)
            .sum()
    }

    fn assert_selects(row: &str, click: &str, expected: &str) {
        assert_eq!(
            selected_word(row, col_of(row, click)).as_deref(),
            Some(expected),
            "row={row:?}, click={click:?}"
        );
    }

    fn assert_selects_nothing(row: &str, click: &str) {
        assert_eq!(
            selected_word(row, col_of(row, click)),
            None,
            "row={row:?}, click={click:?}"
        );
    }

    #[test]
    fn double_click_word_bounds_cover_terminal_text() {
        let cases = [
            (
                "see https://example.com/a-b_c?q=x@y.",
                "example.com",
                "https://example.com/a-b_c?q=x@y",
            ),
            (
                "open \"https://example.com/a,b;c?q=x\";",
                "example.com",
                "https://example.com/a,b;c?q=x",
            ),
            (
                "see https://en.wikipedia.org/wiki/Foo_(bar_(baz)),",
                "wikipedia",
                "https://en.wikipedia.org/wiki/Foo_(bar_(baz))",
            ),
            (
                "see https://example.com/a(b[c{d}e]f),",
                "example.com",
                "https://example.com/a(b[c{d}e]f)",
            ),
            (
                "see (https://example.com/a(b(c)d)))",
                "example.com",
                "https://example.com/a(b(c)d)",
            ),
            (
                "open /tmp/foo-bar/baz_qux/",
                "foo-bar",
                "/tmp/foo-bar/baz_qux/",
            ),
            (
                "open ./src/app/actions.rs:795",
                "actions",
                "./src/app/actions.rs:795",
            ),
            (
                "open ../gmux-projects/issue-1",
                "gmux",
                "../gmux-projects/issue-1",
            ),
            (
                "edit src/app/actions.rs,then",
                "actions",
                "src/app/actions.rs",
            ),
            (
                "cat \"/tmp/build output/log.txt\"",
                "output",
                "/tmp/build output/log.txt",
            ),
            (
                "cat '/Users/me/Library/Application Support/app/config.json'",
                "Support",
                "/Users/me/Library/Application Support/app/config.json",
            ),
            ("echo 你好-world done", "好", "你好-world"),
            ("先跑 cargo test", "cargo", "cargo"),
            (
                "export PATH=$HOME/.cargo/bin:$PATH",
                "$HOME",
                "PATH=$HOME/.cargo/bin:$PATH",
            ),
            (
                "git checkout feature/foo-bar_baz",
                "foo",
                "feature/foo-bar_baz",
            ),
            ("refs #123 and @owner/name", "#123", "#123"),
            ("refs #123 and @owner/name", "owner", "@owner/name"),
            ("cargo test --package=gmux", "--package", "--package=gmux"),
            (
                "cargo test app::actions::tests",
                "app::",
                "app::actions::tests",
            ),
            (
                "image ghcr.io/org/app:latest",
                "ghcr",
                "ghcr.io/org/app:latest",
            ),
            ("ERROR [worker-1] request_id=abc-123", "worker", "worker-1"),
            (
                "tmux|newhoo|fixhoo|newmoo|notification|window_bell|gmux",
                "newhoo",
                "newhoo",
            ),
            (
                "render_status_line(app, area)",
                "render",
                "render_status_line",
            ),
            ("render_status_line(app, area)", "app", "app"),
            ("render_status_line(app, area)", "area", "area"),
            ("if !enabled {", "enabled", "enabled"),
            ("println!(\"hi\")", "println", "println"),
            ("( master)$", "master", "master"),
            ("regex foo$", "foo", "foo$"),
        ];

        for (row, click, expected) in cases {
            assert_selects(row, click, expected);
        }

        let row = "echo 你好-world done";
        assert_eq!(
            selected_word(row, col_of(row, "好") + 1).as_deref(),
            Some("你好-world")
        );
    }

    #[test]
    fn double_click_word_bounds_ignore_delimiters() {
        for (row, click) in [
            (
                "tmux|newhoo|fixhoo|newmoo|notification|window_bell|gmux",
                "|",
            ),
            ("alpha,beta;gamma", ","),
            ("alpha,beta;gamma", ";"),
            ("render_status_line(app, area)", "("),
            ("render_status_line(app, area)", ")"),
            ("if !enabled {", "!"),
            ("if !enabled {", "{"),
            ("(done).", "("),
            ("(done).", "."),
        ] {
            assert_selects_nothing(row, click);
        }
    }

    #[test]
    fn url_at_column_returns_safe_visible_url_only() {
        assert_eq!(
            selected_url("see https://example.com/a(b)c.", "example"),
            Some("https://example.com/a(b)c")
        );
        assert_eq!(
            selected_url("[docs](https://example.com/docs),", "example"),
            Some("https://example.com/docs")
        );
        assert_eq!(
            selected_url("[docs](https://example.com/docs)", "docs"),
            None
        );
        assert_eq!(selected_url("open file:///tmp/report", "file"), None);
    }

    #[test]
    fn navigator_rows_show_tab_nodes_for_flattened_session_tabs() {
        let mut state = app_with_workspaces(&["single", "multi"]);
        state.sessions[1].test_add_tab(Some("tests"));
        state.ensure_test_terminals();

        state.open_navigator();
        let rows = state.navigator_rows();

        assert!(rows.iter().any(|row| matches!(
            row.target,
            crate::app::state::NavigatorTarget::Tab {
                ws_idx: 0,
                tab_idx: 0
            }
        )));
        assert!(rows.iter().any(|row| matches!(
            row.target,
            crate::app::state::NavigatorTarget::Tab {
                ws_idx: 1,
                tab_idx: 0
            }
        )));
        assert!(rows.iter().any(|row| matches!(
            row.target,
            crate::app::state::NavigatorTarget::Tab {
                ws_idx: 1,
                tab_idx: 1
            }
        )));
        assert!(rows.iter().any(|row| {
            matches!(
                row.target,
                crate::app::state::NavigatorTarget::Tab {
                    ws_idx: 1,
                    tab_idx: 0
                }
            ) && row.label == "multi"
        }));
    }

    #[tokio::test]
    async fn navigator_rows_match_live_root_runtime_cwd_workspace_label() {
        let unique = format!(
            "gmux-navigator-runtime-cwd-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        );
        let root = std::env::temp_dir().join(unique);
        let stale_cwd = root.join("issue-264-nix-support");
        let live_cwd = root.join("gmux");
        std::fs::create_dir_all(stale_cwd.join(".git")).unwrap();
        std::fs::create_dir_all(live_cwd.join(".git")).unwrap();

        let mut state = AppState::test_new();
        let mut workspace = Workspace::test_new("stale-name");
        workspace.custom_name = None;
        workspace.identity_cwd = stale_cwd.clone();
        let pane = workspace.tabs[0].root_pane;
        state.sessions = vec![workspace];
        state.ensure_test_terminals();
        let terminal_id = state.sessions[0].terminal_id(pane).cloned().unwrap();
        state.terminals.get_mut(&terminal_id).unwrap().cwd = stale_cwd;

        let (events, _) = tokio::sync::mpsc::channel(4);
        let runtime = crate::terminal::TerminalRuntime::spawn(
            pane,
            24,
            80,
            live_cwd.clone(),
            0,
            crate::terminal_theme::TerminalTheme::default(),
            crate::pane::PaneShellConfig::new("/bin/sh", crate::config::ShellModeConfig::NonLogin),
            events,
            std::sync::Arc::new(tokio::sync::Notify::new()),
            std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false)),
        )
        .unwrap();

        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(2);
        while runtime.cwd() != Some(live_cwd.clone()) && std::time::Instant::now() < deadline {
            tokio::time::sleep(std::time::Duration::from_millis(10)).await;
        }

        let mut runtime_registry = crate::terminal::TerminalRuntimeRegistry::new();
        runtime_registry.insert(terminal_id, runtime);
        state.open_navigator_from(&runtime_registry);
        state.navigator.query = "gmux".into();
        let rows = state.navigator_rows_from(&runtime_registry);

        for (_, runtime) in runtime_registry.drain() {
            runtime.shutdown();
        }
        let _ = std::fs::remove_dir_all(root);

        assert_eq!(rows.len(), 1);
        assert!(matches!(
            rows[0].target,
            crate::app::state::NavigatorTarget::Pane { .. }
        ));
    }

    #[test]
    fn navigator_rows_include_plain_panes_with_generic_meta() {
        let mut state = app_with_workspaces(&["one"]);
        let shell = state.sessions[0].tabs[0].root_pane;
        let split = state.sessions[0].test_split(Direction::Horizontal);
        state.ensure_test_terminals();

        state.open_navigator();
        let rows = state.navigator_rows();

        assert!(rows.iter().any(|row| matches!(
            row.target,
            crate::app::state::NavigatorTarget::Pane { pane_id, .. } if pane_id == shell
        )));
        let pane_row = rows
            .iter()
            .find(|row| {
                matches!(
                    row.target,
                    crate::app::state::NavigatorTarget::Pane { pane_id, .. } if pane_id == split
                )
            })
            .expect("split pane should appear in the navigator");
        assert_eq!(pane_row.meta, "shell");
    }

    #[test]
    fn opening_navigator_selects_current_pane_without_workspace_rows() {
        let mut state = app_with_workspaces(&["one", "two"]);

        state.open_navigator();
        let selected = state.navigator_rows()[state.navigator.selected].clone();

        assert!(selected.is_current);
        assert!(matches!(
            selected.target,
            crate::app::state::NavigatorTarget::Pane { .. }
        ));
    }

    #[test]
    fn accepting_navigator_pane_collapses_to_session_tab_and_focus() {
        let mut state = app_with_workspaces(&["one", "two"]);
        let target = state.sessions[1].tabs[0].root_pane;
        state.open_navigator();
        state.navigator.selected = state
            .navigator_rows()
            .iter()
            .position(|row| {
                matches!(
                    row.target,
                    crate::app::state::NavigatorTarget::Pane { pane_id, .. } if pane_id == target
                )
            })
            .unwrap();

        assert!(state.accept_navigator_selection());

        assert_eq!(state.sessions.len(), 1);
        assert_eq!(state.active_session, Some(0));
        assert_eq!(state.sessions[0].active_tab, 1);
        assert_eq!(state.sessions[0].focused_pane_id(), Some(target));
        assert_eq!(state.mode, Mode::Terminal);
    }

    #[test]
    fn navigator_search_by_session_label_shows_child_rows() {
        let mut state = app_with_workspaces(&["issue-work"]);
        let root = state.sessions[0].tabs[0].root_pane;

        state.open_navigator();
        state.navigator.query = "work".into();

        assert!(state.navigator_rows().iter().any(|row| matches!(
            row.target,
            crate::app::state::NavigatorTarget::Pane { pane_id, .. } if pane_id == root
        )));
    }

    #[test]
    fn navigator_search_by_inherited_tab_label_shows_child_rows() {
        let mut state = app_with_workspaces(&["one", "two"]);
        let target = state.sessions[1].tabs[0].root_pane;

        state.open_navigator();
        state.navigator.query = "two".into();
        let rows = state.navigator_rows();

        assert!(rows.iter().any(|row| {
            matches!(
                row.target,
                crate::app::state::NavigatorTarget::Tab {
                    ws_idx: 1,
                    tab_idx: 0
                }
            ) && row.label == "two"
        }));
        assert!(rows.iter().any(|row| matches!(
            row.target,
            crate::app::state::NavigatorTarget::Pane { pane_id, .. } if pane_id == target
        )));
    }

    #[test]
    fn navigator_search_ignores_hidden_status_metadata() {
        let mut state = app_with_workspaces(&["one"]);
        let shell = state.sessions[0].tabs[0].root_pane;
        let split = state.sessions[0].test_split(Direction::Horizontal);
        state.ensure_test_terminals();

        let shell_terminal_id = state.sessions[0].terminal_id(shell).cloned().unwrap();
        state
            .terminals
            .get_mut(&shell_terminal_id)
            .unwrap()
            .set_manual_label("wheel notes".into());
        state.open_navigator();
        state.navigator.query = "working".into();
        let state_rows = state.navigator_rows();

        assert!(state_rows.is_empty());

        state.navigator.query = "wheel".into();
        let text_rows = state.navigator_rows();

        assert!(text_rows.iter().any(|row| matches!(
            row.target,
            crate::app::state::NavigatorTarget::Pane { pane_id, .. } if pane_id == shell
        )));
        assert!(!state_rows.iter().any(|row| matches!(
            row.target,
            crate::app::state::NavigatorTarget::Pane { pane_id, .. } if pane_id == shell
        )));
        assert!(!text_rows.iter().any(|row| matches!(
            row.target,
            crate::app::state::NavigatorTarget::Pane { pane_id, .. } if pane_id == split
        )));
    }

    #[test]
    fn navigator_search_filters_panes_without_workspace_context_rows() {
        let mut state = app_with_workspaces(&["one"]);
        let root = state.sessions[0].tabs[0].root_pane;
        let terminal_id = state.sessions[0].terminal_id(root).cloned().unwrap();
        state
            .terminals
            .get_mut(&terminal_id)
            .unwrap()
            .set_manual_label("weekly review".into());
        state.open_navigator();
        state.navigator.query = "weekly".into();

        let rows = state.navigator_rows();

        assert!(rows.iter().any(|row| matches!(
            row.target,
            crate::app::state::NavigatorTarget::Pane { pane_id, .. } if pane_id == root
        )));
        assert!(rows.iter().any(|row| row.label.contains("weekly")));
    }

    #[test]
    fn next_pane_panel_entry_cycles_pane_panel_entries_in_all_scope() {
        let mut first = Workspace::test_new("one");
        let first_root = first.tabs[0].root_pane;
        let first_second = first.test_split(Direction::Horizontal);
        first.tabs[0].layout.focus_pane(first_root);
        let second = Workspace::test_new("two");
        let second_root = second.tabs[0].root_pane;

        let mut state = AppState::test_new();
        state.sessions = vec![first, second];
        state.ensure_test_terminals();
        state.active_session = Some(0);
        state.selected_session = 0;
        state.mode = Mode::Terminal;
        state.pane_panel_scope = crate::app::state::PanePanelScope::All;

        state.next_pane_panel_entry();
        assert_eq!(state.active_session, Some(0));
        assert_eq!(state.sessions.len(), 1);
        assert_eq!(state.sessions[0].active_tab, 0);
        assert_eq!(state.sessions[0].focused_pane_id(), Some(first_second));

        state.next_pane_panel_entry();
        assert_eq!(state.active_session, Some(0));
        assert_eq!(state.sessions[0].active_tab, 1);
        assert_eq!(state.sessions[0].focused_pane_id(), Some(second_root));

        state.previous_pane_panel_entry();
        assert_eq!(state.active_session, Some(0));
        assert_eq!(state.sessions[0].active_tab, 0);
        assert_eq!(state.sessions[0].focused_pane_id(), Some(first_second));
    }

    #[test]
    fn focus_pane_panel_entry_uses_pane_panel_order() {
        let mut first = Workspace::test_new("one");
        let first_root = first.tabs[0].root_pane;
        let _first_second = first.test_split(Direction::Horizontal);
        first.tabs[0].layout.focus_pane(first_root);
        let second = Workspace::test_new("two");
        let second_root = second.tabs[0].root_pane;

        let mut state = AppState::test_new();
        state.sessions = vec![first, second];
        state.ensure_test_terminals();
        state.active_session = Some(0);
        state.selected_session = 0;
        state.mode = Mode::Terminal;
        state.pane_panel_scope = crate::app::state::PanePanelScope::All;

        assert!(state.focus_pane_panel_entry(2));

        assert_eq!(state.sessions.len(), 1);
        assert_eq!(state.active_session, Some(0));
        assert_eq!(state.sessions[0].active_tab, 1);
        assert_eq!(state.sessions[0].focused_pane_id(), Some(second_root));
    }

    #[test]
    fn focus_pane_panel_entry_succeeds_for_already_focused_pane() {
        let mut state = app_with_workspaces(&["one"]);
        let root = state.sessions[0].tabs[0].root_pane;
        state.pane_panel_scope = crate::app::state::PanePanelScope::All;

        assert!(state.focus_pane_panel_entry(0));
        assert_eq!(state.active_session, Some(0));
        assert_eq!(state.sessions[0].focused_pane_id(), Some(root));
    }

    #[test]
    fn next_pane_panel_entry_cycles_only_current_scope_entries() {
        let mut first = Workspace::test_new("one");
        let first_root = first.tabs[0].root_pane;
        let first_second = first.test_split(Direction::Horizontal);
        first.tabs[0].layout.focus_pane(first_second);
        let second = Workspace::test_new("two");

        let mut state = AppState::test_new();
        state.sessions = vec![first, second];
        state.ensure_test_terminals();
        state.active_session = Some(0);
        state.selected_session = 0;
        state.mode = Mode::Terminal;
        state.pane_panel_scope = crate::app::state::PanePanelScope::Current;

        state.next_pane_panel_entry();

        assert_eq!(state.active_session, Some(0));
        assert_eq!(state.sessions[0].focused_pane_id(), Some(first_root));
    }

    #[test]
    fn previous_pane_panel_entry_wraps_to_last_entry() {
        let mut workspace = Workspace::test_new("one");
        let root = workspace.tabs[0].root_pane;
        for idx in 1..8 {
            workspace.test_add_tab(Some(&format!("tab-{idx}")));
        }

        let mut state = AppState::test_new();
        state.sessions = vec![workspace];
        state.ensure_test_terminals();
        state.active_session = Some(0);
        state.selected_session = 0;
        state.mode = Mode::Terminal;
        state.pane_panel_scope = crate::app::state::PanePanelScope::Current;
        state.sessions[0].tabs[0].layout.focus_pane(root);
        crate::ui::compute_view(&mut state, ratatui::layout::Rect::new(0, 0, 80, 14));

        state.previous_pane_panel_entry();

        let last_idx = state.sessions[0].tabs.len() - 1;
        assert_eq!(state.sessions[0].active_tab, last_idx);
        assert_eq!(state.pane_panel_scroll, 0);
    }

    #[test]
    fn focus_session_flattens_to_session_tab() {
        let mut state = app_with_workspaces(&["a", "b", "c"]);
        state.focus_session(2);
        assert_eq!(state.sessions.len(), 1);
        assert_eq!(state.active_session, Some(0));
        assert_eq!(state.selected_session, 0);
        assert_eq!(state.sessions[0].active_tab, 2);
    }

    #[test]
    fn session_tab_switch_works_without_active_index() {
        let mut state = app_with_workspaces(&["one"]);
        let second_tab = state.sessions[0].test_add_tab(Some("logs"));
        state.active_session = None;
        state.selected_session = 0;

        state.switch_tab(second_tab);

        assert_eq!(state.sessions[0].active_tab, second_tab);
        assert_eq!(state.active_session, None);
    }

    #[test]
    fn collapse_to_single_session_merges_tabs_and_focus() {
        let mut state = app_with_workspaces(&["one", "two"]);
        let second_first_root = state.sessions[1].tabs[0].root_pane;
        let second_tab = state.sessions[1].test_add_tab(Some("logs"));
        let second_tab_root = state.sessions[1].tabs[second_tab].root_pane;
        state.sessions[1].switch_tab(second_tab);
        state.active_session = Some(1);
        state.selected_session = 1;

        assert!(state.collapse_to_single_session());

        assert_eq!(state.sessions.len(), 1);
        assert_eq!(state.active_session, Some(0));
        assert_eq!(state.selected_session, 0);
        assert_eq!(state.sessions[0].active_tab, 2);
        let tab_labels: Vec<_> = state.sessions[0]
            .tabs
            .iter()
            .map(|tab| tab.custom_name.as_deref())
            .collect();
        assert_eq!(tab_labels, vec![None, Some("two"), Some("logs")]);
        let tab_numbers: Vec<_> = state.sessions[0]
            .tabs
            .iter()
            .map(|tab| tab.number)
            .collect();
        assert_eq!(tab_numbers, vec![1, 2, 3]);
        assert_eq!(
            state.sessions[0].public_pane_number(second_first_root),
            Some(2)
        );
        assert_eq!(
            state.sessions[0].public_pane_number(second_tab_root),
            Some(3)
        );
    }

    #[test]
    fn last_pane_toggles_to_previous_focus_in_active_tab() {
        let mut state = app_with_workspaces(&["test"]);
        let root = state.sessions[0].tabs[0].root_pane;
        let right = state.sessions[0].test_split(Direction::Horizontal);

        state.focus_pane_in_session_at(0, root);
        state.focus_pane_in_session_at(0, right);
        state.last_pane();

        assert_eq!(state.sessions[0].focused_pane_id(), Some(root));

        state.last_pane();

        assert_eq!(state.sessions[0].focused_pane_id(), Some(right));
    }

    #[test]
    fn removing_background_pane_preserves_last_pane_history() {
        let mut state = app_with_workspaces(&["test"]);
        let root = state.sessions[0].tabs[0].root_pane;
        let right = state.sessions[0].test_split(Direction::Horizontal);
        let background = state.sessions[0].test_split(Direction::Horizontal);

        state.focus_pane_in_session_at(0, root);
        state.focus_pane_in_session_at(0, right);
        state.sessions[0].remove_pane(background);
        state.last_pane();

        assert_eq!(state.sessions[0].focused_pane_id(), Some(root));
    }

    #[test]
    fn last_pane_jumps_across_workspaces_and_tabs() {
        let mut state = app_with_workspaces(&["one", "two"]);
        let first_root = state.sessions[0].tabs[0].root_pane;
        let second_tab = state.sessions[1].test_add_tab(Some("logs"));
        let second_tab_root = state.sessions[1].tabs[second_tab].root_pane;

        state.focus_pane_in_session_at(1, second_tab_root);
        state.last_pane();

        assert_eq!(state.active_session, Some(0));
        assert_eq!(state.sessions[0].active_tab, 0);
        assert_eq!(state.sessions[0].focused_pane_id(), Some(first_root));

        state.last_pane();

        assert_eq!(state.sessions.len(), 1);
        assert_eq!(state.active_session, Some(0));
        assert_eq!(state.sessions[0].active_tab, 2);
        assert_eq!(state.sessions[0].focused_pane_id(), Some(second_tab_root));
    }

    #[test]
    fn last_pane_tracks_tab_and_session_tab_switches() {
        let mut state = app_with_workspaces(&["one", "two"]);
        let first_root = state.sessions[0].tabs[0].root_pane;
        let first_second_tab = state.sessions[0].test_add_tab(Some("logs"));
        let first_second_root = state.sessions[0].tabs[first_second_tab].root_pane;
        let second_root = state.sessions[1].tabs[0].root_pane;

        state.switch_tab(first_second_tab);
        state.last_pane();

        assert_eq!(state.active_session, Some(0));
        assert_eq!(state.sessions[0].active_tab, 0);
        assert_eq!(state.sessions[0].focused_pane_id(), Some(first_root));

        state.last_pane();

        assert_eq!(state.active_session, Some(0));
        assert_eq!(state.sessions[0].active_tab, first_second_tab);
        assert_eq!(state.sessions[0].focused_pane_id(), Some(first_second_root));

        assert_eq!(state.sessions.len(), 1);

        state.focus_session_tab(0, 2);
        state.last_pane();

        assert_eq!(state.active_session, Some(0));
        assert_eq!(state.sessions[0].active_tab, first_second_tab);
        assert_eq!(state.sessions[0].focused_pane_id(), Some(first_second_root));

        state.last_pane();

        assert_eq!(state.sessions.len(), 1);
        assert_eq!(state.active_session, Some(0));
        assert_eq!(state.sessions[0].active_tab, 2);
        assert_eq!(state.sessions[0].focused_pane_id(), Some(second_root));
    }

    #[test]
    fn last_pane_tracks_cross_workspace_tab_selection() {
        let mut state = app_with_workspaces(&["one", "two"]);
        let first_root = state.sessions[0].tabs[0].root_pane;
        let second_first_root = state.sessions[1].tabs[0].root_pane;
        let second_tab = state.sessions[1].test_add_tab(Some("logs"));
        let second_tab_root = state.sessions[1].tabs[second_tab].root_pane;

        state.focus_session_tab(1, second_tab);
        state.last_pane();

        assert_eq!(state.active_session, Some(0));
        assert_eq!(state.sessions[0].focused_pane_id(), Some(first_root));

        state.last_pane();

        assert_eq!(state.sessions.len(), 1);
        assert_eq!(state.active_session, Some(0));
        assert_eq!(state.sessions[0].active_tab, 2);
        assert_eq!(state.sessions[0].focused_pane_id(), Some(second_tab_root));
        assert_ne!(second_first_root, second_tab_root);
    }

    #[test]
    fn focus_session_updates_active_after_sidebar_trim() {
        let mut state = app_with_workspaces(&["a", "b", "c", "d", "e", "f", "g", "h"]);
        crate::ui::compute_view(&mut state, ratatui::layout::Rect::new(0, 0, 80, 14));

        state.focus_session(7);
        crate::ui::compute_view(&mut state, ratatui::layout::Rect::new(0, 0, 80, 14));

        assert_eq!(state.sessions.len(), 1);
        assert_eq!(state.active_session, Some(0));
        assert_eq!(state.selected_session, 0);
        assert_eq!(state.sessions[0].active_tab, 7);
    }

    #[test]
    fn focus_session_marks_panes_seen() {
        let mut state = app_with_workspaces(&["a", "b"]);
        // Mark a pane in workspace 1 as unseen
        let id = *state.sessions[1].panes.keys().next().unwrap();
        state.sessions[1].panes.get_mut(&id).unwrap().seen = false;

        state.focus_session(1);
        assert!(state.sessions[0].tabs[1].panes.get(&id).unwrap().seen);
    }

    #[test]
    fn focus_session_out_of_bounds_is_noop() {
        let mut state = app_with_workspaces(&["a"]);
        state.focus_session(5);
        assert_eq!(state.active_session, Some(0));
    }

    #[test]
    fn close_session_closes_canonical_session() {
        let mut state = app_with_workspaces(&["a", "b", "c"]);
        state.selected_session = 1;
        state.active_session = Some(1);

        state.close_session();

        assert!(state.sessions.is_empty());
        assert_eq!(state.selected_session, 0);
        assert_eq!(state.active_session, None);
        assert!(state.should_quit);
    }

    #[test]
    fn close_last_session_clears_active() {
        let mut state = app_with_workspaces(&["only"]);
        state.selected_session = 0;
        state.close_session();

        assert!(state.sessions.is_empty());
        assert_eq!(state.active_session, None);
        assert_eq!(state.selected_session, 0);
        assert!(state.should_quit);
    }

    #[test]
    fn close_session_ignores_stale_selected_workspace() {
        let mut state = app_with_workspaces(&["a", "b"]);
        state.selected_session = 99;
        state.active_session = Some(0);

        state.close_session();

        assert!(state.sessions.is_empty());
        assert_eq!(state.selected_session, 0);
        assert_eq!(state.active_session, None);
        assert!(state.should_quit);
    }

    #[test]
    fn pane_died_last_pane_in_tab_removes_tab_after_session_collapse() {
        let mut state = app_with_workspaces(&["a", "b"]);
        let pane_id = *state.sessions[0].panes.keys().next().unwrap();

        state.handle_pane_died(pane_id);

        assert_eq!(state.sessions.len(), 1);
        assert_eq!(state.sessions[0].tabs.len(), 1);
        assert_eq!(state.sessions[0].tabs[0].custom_name.as_deref(), Some("b"));
    }

    #[test]
    fn pane_died_last_workspace_enters_navigate() {
        let mut state = app_with_workspaces(&["only"]);
        state.mode = Mode::Terminal;
        let pane_id = *state.sessions[0].panes.keys().next().unwrap();

        state.handle_pane_died(pane_id);

        assert!(state.sessions.is_empty());
        assert_eq!(state.mode, Mode::Navigate);
        assert!(state.should_quit);
    }

    #[test]
    fn pane_died_multi_pane_keeps_workspace() {
        let mut state = app_with_workspaces(&["test"]);
        let second_id = state.sessions[0].test_split(Direction::Horizontal);

        state.handle_pane_died(second_id);

        assert_eq!(state.sessions.len(), 1);
        assert_eq!(state.sessions[0].panes.len(), 1);
    }

    #[test]
    fn pane_died_unknown_pane_is_noop() {
        let mut state = app_with_workspaces(&["test"]);
        let fake_id = PaneId::from_raw(9999);

        state.handle_pane_died(fake_id);

        assert_eq!(state.sessions.len(), 1);
    }

    #[test]
    fn pane_died_unrelated_pane_preserves_selection() {
        // Two workspaces; user is selecting text in workspace 0.
        // A pane in workspace 1 dies — selection must be preserved.
        let mut state = app_with_workspaces(&["active", "bg"]);
        let active_pane = *state.sessions[0].panes.keys().next().unwrap();
        let bg_pane = *state.sessions[1].panes.keys().next().unwrap();

        state.selection = Some(crate::selection::Selection::anchor(active_pane, 0, 0, None));
        state.selection_autoscroll = Some(crate::app::state::SelectionAutoscroll {
            direction: crate::app::state::SelectionAutoscrollDirection::Down,
            last_mouse_screen_col: 0,
            last_mouse_screen_row: 23,
            inner_rect: ratatui::layout::Rect::new(0, 0, 80, 24),
        });

        state.handle_pane_died(bg_pane);

        assert!(state.selection.is_some());
        assert!(state.selection_autoscroll.is_some());
    }

    #[test]
    fn pane_died_same_pane_clears_selection() {
        let mut state = app_with_workspaces(&["test"]);
        let first_id = state.sessions[0].tabs[0].root_pane;
        let second_id = state.sessions[0].test_split(Direction::Horizontal);

        state.selection = Some(crate::selection::Selection::anchor(second_id, 0, 0, None));
        state.selection_autoscroll = Some(crate::app::state::SelectionAutoscroll {
            direction: crate::app::state::SelectionAutoscrollDirection::Down,
            last_mouse_screen_col: 0,
            last_mouse_screen_row: 23,
            inner_rect: ratatui::layout::Rect::new(0, 0, 80, 24),
        });

        state.handle_pane_died(second_id);

        // first_id still alive, workspace stays, but selection was on the dying pane
        assert!(state.selection.is_none());
        assert!(state.selection_autoscroll.is_none());
        assert_eq!(state.sessions[0].panes.len(), 1);
        assert_eq!(state.sessions[0].panes.keys().next().unwrap(), &first_id);
    }

    #[test]
    fn toggle_zoom_works() {
        let mut state = app_with_workspaces(&["test"]);
        state.sessions[0].test_split(Direction::Horizontal);

        assert!(!state.sessions[0].zoomed);
        state.toggle_zoom();
        assert!(state.sessions[0].zoomed);
        state.toggle_zoom();
        assert!(!state.sessions[0].zoomed);
    }

    #[test]
    fn toggle_zoom_single_pane_noop() {
        let mut state = app_with_workspaces(&["test"]);
        state.toggle_zoom();
        assert!(!state.sessions[0].zoomed);
    }

    #[test]
    fn swap_pane_next_moves_focused_pane_forward_and_preserves_focus() {
        let mut state = app_with_workspaces(&["test"]);
        let first = state.sessions[0].tabs[0].root_pane;
        let second = state.sessions[0].test_split(Direction::Horizontal);
        let third = state.sessions[0].test_split(Direction::Horizontal);
        state.sessions[0].layout.focus_pane(second);

        assert!(state.swap_pane(false));

        assert_eq!(
            state.sessions[0].tabs[0].layout.pane_ids(),
            vec![first, third, second]
        );
        assert_eq!(state.sessions[0].focused_pane_id(), Some(second));
        assert!(state.session_dirty);
    }

    #[test]
    fn swap_pane_previous_wraps_to_last_pane() {
        let mut state = app_with_workspaces(&["test"]);
        let first = state.sessions[0].tabs[0].root_pane;
        let second = state.sessions[0].test_split(Direction::Horizontal);
        let third = state.sessions[0].test_split(Direction::Horizontal);
        state.sessions[0].layout.focus_pane(first);

        assert!(state.swap_pane(true));

        assert_eq!(
            state.sessions[0].tabs[0].layout.pane_ids(),
            vec![third, second, first]
        );
        assert_eq!(state.sessions[0].focused_pane_id(), Some(first));
    }

    #[test]
    fn swap_pane_single_pane_noop() {
        let mut state = app_with_workspaces(&["test"]);
        let first = state.sessions[0].tabs[0].root_pane;

        assert!(!state.swap_pane(false));

        assert_eq!(state.sessions[0].tabs[0].layout.pane_ids(), vec![first]);
        assert!(!state.session_dirty);
    }

    #[test]
    fn navigate_pane_without_bias_uses_existing_order() {
        let mut state = app_with_workspaces(&["test"]);
        let left = state.sessions[0].tabs[0].root_pane;
        let right_top = state.sessions[0].test_split(Direction::Horizontal);
        state.sessions[0].test_split(Direction::Vertical);
        state.sessions[0].test_split(Direction::Vertical);
        state.sessions[0].layout.focus_pane(left);
        crate::ui::compute_view(&mut state, ratatui::layout::Rect::new(0, 0, 100, 30));

        assert!(state.navigate_pane(NavDirection::Right));

        assert_eq!(state.sessions[0].focused_pane_id(), Some(right_top));
    }

    #[test]
    fn navigate_pane_returns_to_previous_edge_position() {
        let mut state = app_with_workspaces(&["test"]);
        let left = state.sessions[0].tabs[0].root_pane;
        state.sessions[0].test_split(Direction::Horizontal);
        state.sessions[0].test_split(Direction::Vertical);
        let right_bottom = state.sessions[0].test_split(Direction::Vertical);
        state.sessions[0].layout.focus_pane(right_bottom);
        crate::ui::compute_view(&mut state, ratatui::layout::Rect::new(0, 0, 100, 30));

        assert!(state.navigate_pane(NavDirection::Left));
        assert_eq!(state.sessions[0].focused_pane_id(), Some(left));
        crate::ui::compute_view(&mut state, ratatui::layout::Rect::new(0, 0, 100, 30));

        assert!(state.navigate_pane(NavDirection::Right));

        assert_eq!(state.sessions[0].focused_pane_id(), Some(right_bottom));
    }

    #[test]
    fn direct_focus_clears_pane_navigation_bias() {
        let mut state = app_with_workspaces(&["test"]);
        let left = state.sessions[0].tabs[0].root_pane;
        let right_top = state.sessions[0].test_split(Direction::Horizontal);
        state.sessions[0].test_split(Direction::Vertical);
        let right_bottom = state.sessions[0].test_split(Direction::Vertical);
        state.sessions[0].layout.focus_pane(right_bottom);
        crate::ui::compute_view(&mut state, ratatui::layout::Rect::new(0, 0, 100, 30));

        assert!(state.navigate_pane(NavDirection::Left));
        assert_eq!(state.sessions[0].focused_pane_id(), Some(left));
        assert!(!state.focus_pane_in_session_at(0, left));
        crate::ui::compute_view(&mut state, ratatui::layout::Rect::new(0, 0, 100, 30));

        assert!(state.navigate_pane(NavDirection::Right));

        assert_eq!(state.sessions[0].focused_pane_id(), Some(right_top));
    }

    #[test]
    fn navigate_pane_changes_focus_while_zoomed() {
        let mut state = app_with_workspaces(&["test"]);
        let root = state.sessions[0].tabs[0].root_pane;
        let right = state.sessions[0].test_split(Direction::Horizontal);
        state.sessions[0].layout.focus_pane(root);
        state.sessions[0].zoomed = true;
        crate::ui::compute_view(&mut state, ratatui::layout::Rect::new(0, 0, 100, 20));

        assert_eq!(state.view.pane_infos.len(), 1);
        assert_eq!(state.view.pane_infos[0].id, root);

        state.navigate_pane(NavDirection::Right);
        crate::ui::compute_view(&mut state, ratatui::layout::Rect::new(0, 0, 100, 20));

        assert!(state.sessions[0].zoomed);
        assert_eq!(state.sessions[0].focused_pane_id(), Some(right));
        assert_eq!(state.view.pane_infos.len(), 1);
        assert_eq!(state.view.pane_infos[0].id, right);
        assert_eq!(
            state.view.pane_infos[0].inner_rect.x,
            state.view.pane_infos[0].rect.x
        );
    }

    #[test]
    fn close_pane_removes_from_workspace() {
        let mut state = app_with_workspaces(&["test"]);
        state.sessions[0].test_split(Direction::Horizontal);
        assert_eq!(state.sessions[0].panes.len(), 2);

        state.close_pane();
        assert_eq!(state.sessions[0].panes.len(), 1);
    }

    #[test]
    fn close_pane_removes_unattached_terminal_state() {
        let mut state = app_with_workspaces(&["test"]);
        let pane_id = state.sessions[0].test_split(Direction::Horizontal);
        state.ensure_test_terminals();
        let terminal_id = state.terminal_id_for_pane(0, pane_id).unwrap();

        state.close_pane();

        assert!(!state.terminals.contains_key(&terminal_id));
    }

    #[test]
    fn close_pane_last_pane_requests_quit() {
        let mut state = app_with_workspaces(&["test"]);

        state.close_pane();

        assert!(state.sessions.is_empty());
        assert!(state.should_quit);
    }

    #[test]
    fn close_tab_removes_unattached_terminal_states() {
        let mut state = app_with_workspaces(&["test"]);
        let tab_idx = state.sessions[0].test_add_tab(Some("logs"));
        state.ensure_test_terminals();
        state.sessions[0].switch_tab(tab_idx);
        let pane_id = state.sessions[0].tabs[tab_idx].root_pane;
        let terminal_id = state.terminal_id_for_pane(0, pane_id).unwrap();

        state.close_tab();

        assert!(!state.terminals.contains_key(&terminal_id));
    }

    #[test]
    fn close_session_removes_unattached_terminal_states() {
        let mut state = app_with_workspaces(&["one", "two"]);
        let terminal_id = state
            .terminal_id_for_pane(0, state.sessions[0].tabs[0].root_pane)
            .unwrap();

        state.close_session();

        assert!(!state.terminals.contains_key(&terminal_id));
    }

    #[test]
    fn close_tab_closes_active_workspace_not_selected_workspace() {
        let mut state = app_with_workspaces(&["selected", "active"]);
        let active_terminal_id = state
            .terminal_id_for_pane(1, state.sessions[1].tabs[0].root_pane)
            .unwrap();
        state.active_session = Some(1);
        state.selected_session = 0;

        state.close_tab();

        assert_eq!(state.sessions.len(), 1);
        assert_eq!(state.sessions[0].display_name(), "selected");
        assert!(!state.terminals.contains_key(&active_terminal_id));
    }

    #[test]
    fn close_pane_last_pane_closes_active_workspace_not_selected_workspace() {
        let mut state = app_with_workspaces(&["selected", "active"]);
        let active_terminal_id = state
            .terminal_id_for_pane(1, state.sessions[1].tabs[0].root_pane)
            .unwrap();
        state.active_session = Some(1);
        state.selected_session = 0;

        state.close_pane();

        assert_eq!(state.sessions.len(), 1);
        assert_eq!(state.sessions[0].display_name(), "selected");
        assert!(!state.terminals.contains_key(&active_terminal_id));
        assert!(!state.should_quit);
    }
}
