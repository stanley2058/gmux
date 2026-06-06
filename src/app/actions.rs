//! Pure state mutations on AppState.
//! These don't need channels, async, or PTY runtime.

use tracing::{info, warn};

use crate::events::AppEvent;
use crate::layout::{find_in_direction, NavDirection, PaneId};
use crate::selection::Selection;
use unicode_width::UnicodeWidthChar;

use super::state::{
    text_matches_query, AppState, Mode, NavigatorRow, NavigatorTarget, PaneFocusTarget, ToastKind,
    ToastNotification, ViewLayout,
};
// ---------------------------------------------------------------------------
// Navigator operations
// ---------------------------------------------------------------------------

impl AppState {
    pub(crate) fn current_pane_focus_target(&self) -> Option<PaneFocusTarget> {
        let ws = self.session_container()?;
        let pane_id = ws.focused_pane_id()?;
        Some(PaneFocusTarget {
            workspace_id: ws.id.clone(),
            pane_id,
        })
    }

    fn pane_focus_target_indices(&self, target: &PaneFocusTarget) -> Option<(usize, usize)> {
        if let Some(ws_idx) = self
            .session_containers()
            .iter()
            .position(|ws| ws.id == target.workspace_id)
        {
            if let Some(tab_idx) =
                self.session_containers()[ws_idx].find_tab_index_for_pane(target.pane_id)
            {
                return Some((ws_idx, tab_idx));
            }
        }

        self.session_containers()
            .iter()
            .enumerate()
            .find_map(|(ws_idx, ws)| {
                ws.find_tab_index_for_pane(target.pane_id)
                    .map(|tab_idx| (ws_idx, tab_idx))
            })
    }

    pub(crate) fn flattened_tab_index(&self, ws_idx: usize, tab_idx: usize) -> Option<usize> {
        self.session_tab_entries()
            .position(|(entry_ws_idx, entry_tab_idx, _, _)| {
                entry_ws_idx == ws_idx && entry_tab_idx == tab_idx
            })
    }

    pub(crate) fn record_pane_focus_change(
        &mut self,
        previous: Option<PaneFocusTarget>,
        ws_idx: usize,
        pane_id: PaneId,
    ) {
        let Some(ws) = self.session_containers().get(ws_idx) else {
            return;
        };
        let target = PaneFocusTarget {
            workspace_id: ws.id.clone(),
            pane_id,
        };
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

    pub(crate) fn focus_pane_in_session_container(
        &mut self,
        ws_idx: usize,
        pane_id: PaneId,
    ) -> bool {
        let Some(ws) = self.session_containers().get(ws_idx) else {
            return false;
        };
        let Some(tab_idx) = ws.find_tab_index_for_pane(pane_id) else {
            return false;
        };
        let previous = self.current_pane_focus_target();
        let target = PaneFocusTarget {
            workspace_id: ws.id.clone(),
            pane_id,
        };
        if self.session_containers().len() == 1
            && self.active == Some(ws_idx)
            && previous.as_ref() == Some(&target)
        {
            return false;
        }

        if !self.focus_session_tab(ws_idx, tab_idx) {
            return false;
        }
        let Some((ws_idx, tab_idx)) = self.pane_focus_target_indices(&target) else {
            return false;
        };
        if let Some(tab) = self
            .session_containers_mut()
            .get_mut(ws_idx)
            .and_then(|ws| ws.tabs.get_mut(tab_idx))
        {
            tab.layout.focus_pane(pane_id);
            self.previous_pane_focus = previous;
            self.mark_session_dirty();
            return true;
        }
        false
    }

    pub(crate) fn focus_session_pane(&mut self, pane_id: PaneId) -> bool {
        let Some(ws_idx) = self
            .session_containers()
            .iter()
            .position(|ws| ws.find_tab_index_for_pane(pane_id).is_some())
        else {
            return false;
        };

        if self.focus_pane_in_session_container(ws_idx, pane_id) {
            return true;
        }

        self.collapse_to_single_session_workspace();
        self.session_container()
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
        self.navigator.search_focused = false;
        self.navigator.scroll = 0;

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
        let session_matches = self.session_container().is_some_and(|ws| {
            let session_label = ws.display_name_from(&self.terminals, terminal_runtimes);
            let session_search_text = session_label.to_lowercase();
            match query_kind {
                NavigatorQueryKind::Empty => true,
                NavigatorQueryKind::Text => navigator_matches(&query, &session_search_text),
            }
        });
        let multi_tab = self.session_tab_count() > 1;

        for ws_idx in 0..self.session_containers().len() {
            let child_query_kind = if session_matches {
                NavigatorQueryKind::Empty
            } else {
                query_kind
            };
            let child_rows = self.navigator_child_rows(ws_idx, child_query_kind, &query, multi_tab);
            if !session_matches && child_rows.is_empty() {
                continue;
            }

            rows.extend(child_rows);
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
        let Some(ws) = self.session_containers().get(ws_idx) else {
            return Vec::new();
        };
        let mut rows = Vec::new();
        for tab_idx in 0..ws.tabs.len() {
            let tab_row = multi_tab.then(|| self.navigator_tab_row(ws_idx, tab_idx));
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

    fn navigator_tab_row(&self, ws_idx: usize, tab_idx: usize) -> NavigatorRow {
        let ws = &self.session_containers()[ws_idx];
        let tab = &ws.tabs[tab_idx];
        let label = crate::workspace::session_tab_display_name(ws_idx, ws, tab_idx, tab);
        let pane_count = tab.panes.len();
        let meta = format!("{pane_count} panes");
        let search_text = format!("{label} {meta}").to_lowercase();
        NavigatorRow {
            target: NavigatorTarget::Tab { ws_idx, tab_idx },
            depth: 0,
            label,
            meta,
            seen: true,
            is_current: false,
            is_tab: true,
            search_text,
        }
    }

    fn navigator_pane_rows_for_tab(
        &self,
        ws_idx: usize,
        tab_idx: usize,
        multi_tab: bool,
    ) -> Vec<NavigatorRow> {
        let Some(ws) = self.session_containers().get(ws_idx) else {
            return Vec::new();
        };
        let Some(tab) = ws.tabs.get(tab_idx) else {
            return Vec::new();
        };
        let mut rows = Vec::new();
        for pane_id in tab.layout.pane_ids() {
            let Some(pane) = tab.panes.get(&pane_id) else {
                continue;
            };
            let terminal = self.terminals.get(&pane.attached_terminal_id);
            let pane_number = ws.public_pane_number(pane_id).unwrap_or(0);
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
            NavigatorTarget::Tab { ws_idx, tab_idx } => {
                if ws_idx >= self.session_containers().len() {
                    return false;
                }
                let tab_exists = self
                    .session_containers()
                    .get(ws_idx)
                    .is_some_and(|ws| tab_idx < ws.tabs.len());
                if !tab_exists {
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
                if ws_idx >= self.session_containers().len() {
                    return false;
                }
                if self
                    .session_containers()
                    .get(ws_idx)
                    .and_then(|ws| ws.tabs.get(tab_idx))
                    .is_some_and(|tab| tab.panes.contains_key(&pane_id))
                {
                    self.focus_pane_in_session_container(ws_idx, pane_id);
                    self.mode = Mode::Terminal;
                    return true;
                }
                false
            }
        }
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
// Session container operations
// ---------------------------------------------------------------------------

impl AppState {
    pub(crate) fn session_container_index(&self) -> Option<usize> {
        self.active
            .filter(|idx| self.session_containers().get(*idx).is_some())
            .or_else(|| (!self.session_containers().is_empty()).then_some(0))
    }

    pub(crate) fn session_container(&self) -> Option<&crate::workspace::Workspace> {
        self.session_container_index()
            .and_then(|idx| self.session_containers().get(idx))
    }

    pub(crate) fn session_container_mut(&mut self) -> Option<&mut crate::workspace::Workspace> {
        let idx = self.session_container_index()?;
        self.session_containers_mut().get_mut(idx)
    }

    pub(crate) fn collapse_to_single_session_workspace(&mut self) -> bool {
        match self.session_containers().len() {
            0 => {
                let changed = self.active.take().is_some() || self.selected != 0;
                self.selected = 0;
                self.tab_scroll = 0;
                self.tab_scroll_follow_active = true;
                changed
            }
            1 => {
                let changed = self.active != Some(0) || self.selected != 0;
                self.active = Some(0);
                self.selected = 0;
                changed
            }
            _ => {
                let active_ws_idx = self
                    .active
                    .unwrap_or(self.selected)
                    .min(self.session_containers().len().saturating_sub(1));
                let active_tab = self
                    .session_containers()
                    .iter()
                    .enumerate()
                    .take(active_ws_idx + 1)
                    .fold(0, |offset, (idx, ws)| {
                        if idx == active_ws_idx {
                            offset + ws.active_tab.min(ws.tabs.len().saturating_sub(1))
                        } else {
                            offset + ws.tabs.len()
                        }
                    });

                let extras = self.session_containers_mut().split_off(1);
                let primary = &mut self.session_containers_mut()[0];
                for mut workspace in extras {
                    if let (Some(name), Some(first_tab)) =
                        (workspace.custom_name.take(), workspace.tabs.first_mut())
                    {
                        if first_tab.custom_name.is_none() {
                            first_tab.custom_name = Some(name);
                        }
                    }
                    primary.tabs.append(&mut workspace.tabs);
                }

                if primary.tabs.is_empty() {
                    self.session_containers_mut().clear();
                    self.active = None;
                    self.selected = 0;
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

                self.active = Some(0);
                self.selected = 0;
                self.mobile_switcher_scroll = 0;
                self.pane_panel_scroll = 0;
                self.tab_scroll_follow_active = true;
                self.refresh_tab_bar_view();
                true
            }
        }
    }

    pub fn focus_session_container(&mut self, idx: usize) {
        let Some(active_tab) = self.session_containers().get(idx).and_then(|ws| {
            (!ws.tabs.is_empty()).then_some(ws.active_tab.min(ws.tabs.len().saturating_sub(1)))
        }) else {
            return;
        };

        self.focus_session_tab(idx, active_tab);
    }

    pub(crate) fn focus_session_tab(&mut self, ws_idx: usize, tab_idx: usize) -> bool {
        let Some(flat_tab_idx) = self.flattened_tab_index(ws_idx, tab_idx) else {
            return false;
        };

        let previous_focus = self.current_pane_focus_target();
        let workspace_changed = self.active != Some(ws_idx) || self.session_containers().len() > 1;
        self.selection = None;
        self.selection_autoscroll = None;

        self.collapse_to_single_session_workspace();
        self.active = Some(0);
        self.selected = 0;
        let workspace_id = self.session_containers()[0].id.clone();
        if workspace_changed {
            crate::logging::session_focused(&workspace_id);
        }
        self.mark_session_dirty();
        if workspace_changed
            && matches!(
                self.pane_panel_scope,
                crate::app::state::PanePanelScope::Current
            )
        {
            self.pane_panel_scroll = 0;
        }
        self.ensure_session_container_visible(0);
        if let Some(ws) = self.session_containers_mut().get_mut(0) {
            ws.switch_tab(flat_tab_idx);
            let tab_id = format!("{}:{}", workspace_id, flat_tab_idx + 1);
            crate::logging::tab_focused(&workspace_id, &tab_id);
        }
        self.tab_scroll_follow_active = true;
        self.refresh_tab_bar_view();
        self.record_pane_focus_after_navigation(previous_focus);
        true
    }

    pub(crate) fn ensure_session_container_visible(&mut self, idx: usize) {
        if idx >= self.session_containers().len() {
            return;
        }

        if self.view.layout == ViewLayout::Mobile && self.mode == Mode::Navigate {
            self.mobile_switcher_scroll = self
                .mobile_switcher_scroll
                .min(crate::ui::mobile_switcher_max_scroll(self));
            return;
        }

        if self.sidebar_collapsed {
            return;
        }
    }

    pub fn switch_tab(&mut self, idx: usize) {
        if self
            .session_container()
            .is_none_or(|ws| idx >= ws.tabs.len())
        {
            return;
        }
        let previous_focus = self.current_pane_focus_target();
        self.selection = None;
        self.selection_autoscroll = None;
        let Some(ws) = self.session_container_mut() else {
            return;
        };
        ws.switch_tab(idx);
        let workspace_id = ws.id.clone();
        let tab_id = format!("{}:{}", workspace_id, idx + 1);
        crate::logging::tab_focused(&workspace_id, &tab_id);
        self.mark_session_dirty();
        self.tab_scroll_follow_active = true;
        self.refresh_tab_bar_view();
        self.record_pane_focus_after_navigation(previous_focus);
    }

    pub(crate) fn mark_active_tab_seen(&mut self) -> bool {
        let Some(tab) = self
            .session_container_mut()
            .and_then(crate::workspace::Workspace::active_tab_mut)
        else {
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
        if let Some(ws) = self.session_container_mut() {
            if ws.move_tab(source_idx, insert_idx) {
                self.mark_session_dirty();
                self.tab_scroll_follow_active = true;
                self.refresh_tab_bar_view();
            }
        }
    }

    pub fn next_tab(&mut self) {
        if let Some(ws) = self.session_container() {
            if !ws.tabs.is_empty() {
                let next = (ws.active_tab + 1) % ws.tabs.len();
                self.switch_tab(next);
            }
        }
    }

    pub fn previous_tab(&mut self) {
        if let Some(ws) = self.session_container() {
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

        if self.session_container_index() == Some(ws_idx)
            && self
                .session_containers()
                .get(ws_idx)
                .and_then(crate::workspace::Workspace::focused_pane_id)
                == Some(pane_id)
        {
            self.ensure_pane_panel_entry_visible(idx);
            return true;
        }

        if self.focus_pane_in_session_container(ws_idx, pane_id) {
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

        let focused = self
            .session_container()
            .and_then(crate::workspace::Workspace::focused_pane_id);
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

    pub(crate) fn terminal_ids_for_session_container(
        &self,
        ws_idx: usize,
    ) -> Vec<crate::terminal::TerminalId> {
        self.session_containers()
            .get(ws_idx)
            .into_iter()
            .flat_map(|ws| &ws.tabs)
            .flat_map(|tab| tab.panes.values())
            .map(|pane| pane.attached_terminal_id.clone())
            .collect()
    }

    pub(crate) fn terminal_ids_for_tab(
        &self,
        ws_idx: usize,
        tab_idx: usize,
    ) -> Vec<crate::terminal::TerminalId> {
        self.session_containers()
            .get(ws_idx)
            .and_then(|ws| ws.tabs.get(tab_idx))
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
        self.session_containers()
            .get(ws_idx)?
            .pane_state(pane_id)
            .map(|pane| pane.attached_terminal_id.clone())
    }

    pub(crate) fn remove_unattached_terminal_ids(
        &mut self,
        terminal_ids: impl IntoIterator<Item = crate::terminal::TerminalId>,
    ) {
        for terminal_id in terminal_ids {
            let still_attached = self.session_containers().iter().any(|ws| {
                ws.tabs.iter().any(|tab| {
                    tab.panes
                        .values()
                        .any(|pane| pane.attached_terminal_id == terminal_id)
                })
            });
            if !still_attached
                && self.terminals.remove(&terminal_id).is_some()
                && !self.terminal_runtime_shutdowns.contains(&terminal_id)
            {
                self.terminal_runtime_shutdowns.push(terminal_id);
            }
        }
    }

    pub fn close_session_container(&mut self) {
        if self.session_containers().is_empty() {
            return;
        }
        self.collapse_to_single_session_workspace();
        let Some(close_idx) = self.session_container_index() else {
            return;
        };
        self.selection = None;
        self.selection_autoscroll = None;
        self.mark_session_dirty();

        let mut terminal_ids = Vec::new();
        terminal_ids.extend(self.terminal_ids_for_session_container(close_idx));
        if let Some(workspace_id) = self
            .session_containers()
            .get(close_idx)
            .map(|ws| ws.id.clone())
        {
            crate::logging::session_closed(&workspace_id);
        }
        self.session_containers_mut().remove(close_idx);
        self.remove_unattached_terminal_ids(terminal_ids);
        if self.session_containers().is_empty() {
            self.active = None;
            self.selected = 0;
            self.tab_scroll = 0;
            self.tab_scroll_follow_active = true;
        } else {
            if self.selected >= self.session_containers().len() {
                self.selected = self.session_containers().len() - 1;
            }
            self.active = Some(self.selected);
            self.ensure_session_container_visible(self.selected);
            self.tab_scroll_follow_active = true;
            self.refresh_tab_bar_view();
        }
    }

    fn refresh_tab_bar_view(&mut self) {
        let area = self.view.tab_bar_rect;
        let Some(ws) = self.session_container() else {
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
        let Some(ws_idx) = self.session_container_index() else {
            return false;
        };
        let Some(tab) = self.session_container().and_then(|ws| ws.active_tab()) else {
            return false;
        };
        let panes = if tab.zoomed {
            tab.layout.panes(self.view.terminal_area)
        } else {
            self.view.pane_infos.clone()
        };

        if let Some(focused) = panes.iter().find(|p| p.is_focused) {
            if let Some(target) = find_in_direction(focused, direction, &panes) {
                return self.focus_pane_in_session_container(ws_idx, target);
            }
        }
        false
    }

    pub fn resize_pane(&mut self, direction: NavDirection) -> bool {
        if let Some(first) = self.view.pane_infos.first() {
            let area = self
                .view
                .pane_infos
                .iter()
                .fold(first.rect, |acc, p| acc.union(p.rect));
            if let Some(tab) = self
                .session_container_mut()
                .and_then(|ws| ws.active_tab_mut())
            {
                tab.layout.resize_focused(direction, 0.05, area);
                self.mark_session_dirty();
                return true;
            }
        }
        false
    }

    pub fn cycle_pane(&mut self, reverse: bool) {
        let Some(ws_idx) = self.session_container_index() else {
            return;
        };
        let Some(tab) = self.session_container().and_then(|ws| ws.active_tab()) else {
            return;
        };
        let ids = tab.layout.pane_ids();
        if let Some(pos) = ids.iter().position(|id| *id == tab.layout.focused()) {
            let target = if reverse {
                ids[(pos + ids.len() - 1) % ids.len()]
            } else {
                ids[(pos + 1) % ids.len()]
            };
            self.focus_pane_in_session_container(ws_idx, target);
        }
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
        if let Some(tab) = self
            .session_containers_mut()
            .get_mut(ws_idx)
            .and_then(|ws| ws.tabs.get_mut(tab_idx))
        {
            tab.layout.focus_pane(target.pane_id);
            self.previous_pane_focus = current;
            self.mark_session_dirty();
        }
    }

    pub fn toggle_zoom(&mut self) {
        if let Some(tab) = self
            .session_container_mut()
            .and_then(|ws| ws.active_tab_mut())
        {
            if tab.layout.pane_count() > 1 {
                tab.zoomed = !tab.zoomed;
                self.mark_session_dirty();
            }
        }
    }

    /// Close the focused pane. Returns true when the close was deferred to confirmation.
    pub fn close_pane(&mut self) -> bool {
        self.collapse_to_single_session_workspace();
        let active = self.session_container_index();
        self.selection = None;
        self.selection_autoscroll = None;
        self.mark_session_dirty();
        let terminal_ids = active
            .and_then(|i| {
                self.session_containers()
                    .get(i)
                    .and_then(|ws| ws.focused_pane_id().map(|pane_id| (i, pane_id)))
            })
            .and_then(|(i, pane_id)| self.terminal_id_for_pane(i, pane_id))
            .into_iter()
            .collect::<Vec<_>>();
        let should_close_workspace = active
            .and_then(|i| self.session_containers_mut().get_mut(i))
            .is_some_and(|ws| ws.close_focused());
        if should_close_workspace {
            self.close_session_container();
        } else {
            self.remove_unattached_terminal_ids(terminal_ids);
        }
        false
    }

    /// Close the active tab. Returns true when the close was deferred to confirmation.
    pub fn close_tab(&mut self) -> bool {
        self.collapse_to_single_session_workspace();
        self.selection = None;
        self.selection_autoscroll = None;
        self.mark_session_dirty();
        let should_close_workspace = self
            .session_container()
            .is_some_and(|ws| ws.tabs.len() <= 1);
        if should_close_workspace {
            self.close_session_container();
            return false;
        }
        if let Some(ws_idx) = self.session_container_index() {
            let terminal_ids = self
                .session_containers()
                .get(ws_idx)
                .map(|ws| self.terminal_ids_for_tab(ws_idx, ws.active_tab))
                .unwrap_or_default();
            let Some(ws) = self.session_containers_mut().get_mut(ws_idx) else {
                return false;
            };
            let workspace_id = ws.id.clone();
            let closing_tab_id = format!("{}:{}", workspace_id, ws.active_tab + 1);
            ws.close_active_tab();
            self.remove_unattached_terminal_ids(terminal_ids);
            crate::logging::tab_closed(&workspace_id, &closing_tab_id);
            self.tab_scroll_follow_active = true;
            self.refresh_tab_bar_view();
        }
        false
    }
}

// ---------------------------------------------------------------------------
// Selection
// ---------------------------------------------------------------------------

impl AppState {
    pub fn clear_selection(&mut self) {
        self.selection = None;
        self.selection_autoscroll = None;
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
        let Some(ws_idx) = self.session_container_index() else {
            return false;
        };

        let Some(info) = self.pane_info_by_id(pane_id) else {
            return false;
        };
        if viewport_row >= info.inner_rect.height || col >= info.inner_rect.width {
            return false;
        }

        // Leave mouse input to terminal apps that requested it.
        let Some(rt) =
            self.runtime_for_pane_in_session_container(terminal_runtimes, ws_idx, pane_id)
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
        let ws_idx = self.session_container_index()?;
        let info = self.pane_info_by_id(pane_id)?;
        if viewport_row >= info.inner_rect.height || col >= info.inner_rect.width {
            return None;
        }

        let rt = self.runtime_for_pane_in_session_container(terminal_runtimes, ws_idx, pane_id)?;
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

        let Some(ws_idx) = self.session_container_index() else {
            return;
        };

        let text = self
            .runtime_for_pane_in_session_container(terminal_runtimes, ws_idx, sel.pane_id)
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
            AppEvent::UpdateReady {
                version,
                install_command,
            } => {
                self.update_available = Some(version.clone());
                self.update_install_command = install_command.clone();
                self.latest_release_notes_available = true;
                self.update_dismissed = true;
                if matches!(
                    self.toast_config.delivery,
                    crate::config::ToastDelivery::Gmux
                ) {
                    self.toast = Some(ToastNotification {
                        kind: ToastKind::UpdateInstalled,
                        title: format!("v{version} available"),
                        context: crate::update::update_install_instruction(&install_command),
                        target: None,
                    });
                }
            }
            // Intercepted in App::handle_internal_event before reaching this
            // dispatch; never touches AppState.
            AppEvent::ClipboardWrite { .. } => {}
        }
    }

    fn handle_pane_died(&mut self, pane_id: PaneId) {
        self.collapse_to_single_session_workspace();
        let ws_idx = self
            .session_containers()
            .iter()
            .position(|ws| ws.find_tab_index_for_pane(pane_id).is_some());

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
        let workspace_terminal_ids = self.terminal_ids_for_session_container(ws_idx);
        self.pane_id_aliases.retain(|_, alias| *alias != pane_id);
        let should_close_workspace = {
            let ws = &mut self.session_containers_mut()[ws_idx];
            ws.remove_pane(pane_id)
        };
        self.mark_session_dirty();

        if should_close_workspace {
            self.session_containers_mut().remove(ws_idx);
            self.remove_unattached_terminal_ids(workspace_terminal_ids);
            self.active = None;
            self.selected = 0;
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
            state.session_containers.push(ws);
        }
        state.ensure_test_terminals();
        if !state.session_containers.is_empty() {
            state.active = Some(0);
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
        state.session_containers[1].test_add_tab(Some("tests"));
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
        state.session_containers = vec![workspace];
        state.ensure_test_terminals();
        let terminal_id = state.session_containers[0]
            .terminal_id(pane)
            .cloned()
            .unwrap();
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
        let shell = state.session_containers[0].tabs[0].root_pane;
        let split = state.session_containers[0].test_split(Direction::Horizontal);
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
        let target = state.session_containers[1].tabs[0].root_pane;
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

        assert_eq!(state.session_containers.len(), 1);
        assert_eq!(state.active, Some(0));
        assert_eq!(state.session_containers[0].active_tab, 1);
        assert_eq!(state.session_containers[0].focused_pane_id(), Some(target));
        assert_eq!(state.mode, Mode::Terminal);
    }

    #[test]
    fn navigator_search_by_session_label_shows_child_rows() {
        let mut state = app_with_workspaces(&["issue-work"]);
        let root = state.session_containers[0].tabs[0].root_pane;

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
        let target = state.session_containers[1].tabs[0].root_pane;

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
        let shell = state.session_containers[0].tabs[0].root_pane;
        let split = state.session_containers[0].test_split(Direction::Horizontal);
        state.ensure_test_terminals();

        let shell_terminal_id = state.session_containers[0]
            .terminal_id(shell)
            .cloned()
            .unwrap();
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
        let root = state.session_containers[0].tabs[0].root_pane;
        let terminal_id = state.session_containers[0]
            .terminal_id(root)
            .cloned()
            .unwrap();
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
    fn update_ready_sets_explicit_upgrade_toast() {
        let mut state = AppState::test_new();
        state.toast_config.delivery = crate::config::ToastDelivery::Gmux;

        state.handle_app_event(crate::events::AppEvent::UpdateReady {
            version: "0.5.0".into(),
            install_command: "gmux update".into(),
        });

        assert_eq!(state.update_available.as_deref(), Some("0.5.0"));
        assert!(state.latest_release_notes_available);
        let toast = state.toast.as_ref().expect("update toast");
        assert_eq!(toast.title, "v0.5.0 available");
        assert_eq!(
            toast.context,
            "detach, run `gmux update`, then follow its restart guidance"
        );
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
        state.session_containers = vec![first, second];
        state.ensure_test_terminals();
        state.active = Some(0);
        state.selected = 0;
        state.mode = Mode::Terminal;
        state.pane_panel_scope = crate::app::state::PanePanelScope::All;

        state.next_pane_panel_entry();
        assert_eq!(state.active, Some(0));
        assert_eq!(state.session_containers.len(), 1);
        assert_eq!(state.session_containers[0].active_tab, 0);
        assert_eq!(
            state.session_containers[0].focused_pane_id(),
            Some(first_second)
        );

        state.next_pane_panel_entry();
        assert_eq!(state.active, Some(0));
        assert_eq!(state.session_containers[0].active_tab, 1);
        assert_eq!(
            state.session_containers[0].focused_pane_id(),
            Some(second_root)
        );

        state.previous_pane_panel_entry();
        assert_eq!(state.active, Some(0));
        assert_eq!(state.session_containers[0].active_tab, 0);
        assert_eq!(
            state.session_containers[0].focused_pane_id(),
            Some(first_second)
        );
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
        state.session_containers = vec![first, second];
        state.ensure_test_terminals();
        state.active = Some(0);
        state.selected = 0;
        state.mode = Mode::Terminal;
        state.pane_panel_scope = crate::app::state::PanePanelScope::All;

        assert!(state.focus_pane_panel_entry(2));

        assert_eq!(state.session_containers.len(), 1);
        assert_eq!(state.active, Some(0));
        assert_eq!(state.session_containers[0].active_tab, 1);
        assert_eq!(
            state.session_containers[0].focused_pane_id(),
            Some(second_root)
        );
    }

    #[test]
    fn focus_pane_panel_entry_succeeds_for_already_focused_pane() {
        let mut state = app_with_workspaces(&["one"]);
        let root = state.session_containers[0].tabs[0].root_pane;
        state.pane_panel_scope = crate::app::state::PanePanelScope::All;

        assert!(state.focus_pane_panel_entry(0));
        assert_eq!(state.active, Some(0));
        assert_eq!(state.session_containers[0].focused_pane_id(), Some(root));
    }

    #[test]
    fn next_pane_panel_entry_cycles_only_current_scope_entries() {
        let mut first = Workspace::test_new("one");
        let first_root = first.tabs[0].root_pane;
        let first_second = first.test_split(Direction::Horizontal);
        first.tabs[0].layout.focus_pane(first_second);
        let second = Workspace::test_new("two");

        let mut state = AppState::test_new();
        state.session_containers = vec![first, second];
        state.ensure_test_terminals();
        state.active = Some(0);
        state.selected = 0;
        state.mode = Mode::Terminal;
        state.pane_panel_scope = crate::app::state::PanePanelScope::Current;

        state.next_pane_panel_entry();

        assert_eq!(state.active, Some(0));
        assert_eq!(
            state.session_containers[0].focused_pane_id(),
            Some(first_root)
        );
    }

    #[test]
    fn previous_pane_panel_entry_keeps_wrapped_target_visible_in_pane_panel() {
        let mut workspace = Workspace::test_new("one");
        let root = workspace.tabs[0].root_pane;
        for idx in 1..8 {
            workspace.test_add_tab(Some(&format!("tab-{idx}")));
        }

        let mut state = AppState::test_new();
        state.session_containers = vec![workspace];
        state.ensure_test_terminals();
        state.active = Some(0);
        state.selected = 0;
        state.mode = Mode::Terminal;
        state.pane_panel_scope = crate::app::state::PanePanelScope::Current;
        state.session_containers[0].tabs[0].layout.focus_pane(root);
        crate::ui::compute_view(&mut state, ratatui::layout::Rect::new(0, 0, 80, 14));

        state.previous_pane_panel_entry();

        let last_idx = state.session_containers[0].tabs.len() - 1;
        assert_eq!(state.session_containers[0].active_tab, last_idx);
        assert!(state.pane_panel_scroll > 0);
    }

    #[test]
    fn focus_session_container_flattens_to_session_tab() {
        let mut state = app_with_workspaces(&["a", "b", "c"]);
        state.focus_session_container(2);
        assert_eq!(state.session_containers.len(), 1);
        assert_eq!(state.active, Some(0));
        assert_eq!(state.selected, 0);
        assert_eq!(state.session_containers[0].active_tab, 2);
    }

    #[test]
    fn session_container_tab_switch_works_without_active_index() {
        let mut state = app_with_workspaces(&["one"]);
        let second_tab = state.session_containers[0].test_add_tab(Some("logs"));
        state.active = None;
        state.selected = 0;

        state.switch_tab(second_tab);

        assert_eq!(state.session_containers[0].active_tab, second_tab);
        assert_eq!(state.active, None);
    }

    #[test]
    fn collapse_to_single_session_workspace_merges_tabs_and_focus() {
        let mut state = app_with_workspaces(&["one", "two"]);
        let second_first_root = state.session_containers[1].tabs[0].root_pane;
        let second_tab = state.session_containers[1].test_add_tab(Some("logs"));
        let second_tab_root = state.session_containers[1].tabs[second_tab].root_pane;
        state.session_containers[1].switch_tab(second_tab);
        state.active = Some(1);
        state.selected = 1;

        assert!(state.collapse_to_single_session_workspace());

        assert_eq!(state.session_containers.len(), 1);
        assert_eq!(state.active, Some(0));
        assert_eq!(state.selected, 0);
        assert_eq!(state.session_containers[0].active_tab, 2);
        let tab_labels: Vec<_> = state.session_containers[0]
            .tabs
            .iter()
            .map(|tab| tab.custom_name.as_deref())
            .collect();
        assert_eq!(tab_labels, vec![None, Some("two"), Some("logs")]);
        let tab_numbers: Vec<_> = state.session_containers[0]
            .tabs
            .iter()
            .map(|tab| tab.number)
            .collect();
        assert_eq!(tab_numbers, vec![1, 2, 3]);
        assert_eq!(
            state.session_containers[0].public_pane_number(second_first_root),
            Some(2)
        );
        assert_eq!(
            state.session_containers[0].public_pane_number(second_tab_root),
            Some(3)
        );
    }

    #[test]
    fn last_pane_toggles_to_previous_focus_in_active_tab() {
        let mut state = app_with_workspaces(&["test"]);
        let root = state.session_containers[0].tabs[0].root_pane;
        let right = state.session_containers[0].test_split(Direction::Horizontal);

        state.focus_pane_in_session_container(0, root);
        state.focus_pane_in_session_container(0, right);
        state.last_pane();

        assert_eq!(state.session_containers[0].focused_pane_id(), Some(root));

        state.last_pane();

        assert_eq!(state.session_containers[0].focused_pane_id(), Some(right));
    }

    #[test]
    fn removing_background_pane_preserves_last_pane_history() {
        let mut state = app_with_workspaces(&["test"]);
        let root = state.session_containers[0].tabs[0].root_pane;
        let right = state.session_containers[0].test_split(Direction::Horizontal);
        let background = state.session_containers[0].test_split(Direction::Horizontal);

        state.focus_pane_in_session_container(0, root);
        state.focus_pane_in_session_container(0, right);
        state.session_containers[0].remove_pane(background);
        state.last_pane();

        assert_eq!(state.session_containers[0].focused_pane_id(), Some(root));
    }

    #[test]
    fn last_pane_jumps_across_workspaces_and_tabs() {
        let mut state = app_with_workspaces(&["one", "two"]);
        let first_root = state.session_containers[0].tabs[0].root_pane;
        let second_tab = state.session_containers[1].test_add_tab(Some("logs"));
        let second_tab_root = state.session_containers[1].tabs[second_tab].root_pane;

        state.focus_pane_in_session_container(1, second_tab_root);
        state.last_pane();

        assert_eq!(state.active, Some(0));
        assert_eq!(state.session_containers[0].active_tab, 0);
        assert_eq!(
            state.session_containers[0].focused_pane_id(),
            Some(first_root)
        );

        state.last_pane();

        assert_eq!(state.session_containers.len(), 1);
        assert_eq!(state.active, Some(0));
        assert_eq!(state.session_containers[0].active_tab, 2);
        assert_eq!(
            state.session_containers[0].focused_pane_id(),
            Some(second_tab_root)
        );
    }

    #[test]
    fn last_pane_tracks_tab_and_session_tab_switches() {
        let mut state = app_with_workspaces(&["one", "two"]);
        let first_root = state.session_containers[0].tabs[0].root_pane;
        let first_second_tab = state.session_containers[0].test_add_tab(Some("logs"));
        let first_second_root = state.session_containers[0].tabs[first_second_tab].root_pane;
        let second_root = state.session_containers[1].tabs[0].root_pane;

        state.switch_tab(first_second_tab);
        state.last_pane();

        assert_eq!(state.active, Some(0));
        assert_eq!(state.session_containers[0].active_tab, 0);
        assert_eq!(
            state.session_containers[0].focused_pane_id(),
            Some(first_root)
        );

        state.last_pane();

        assert_eq!(state.active, Some(0));
        assert_eq!(state.session_containers[0].active_tab, first_second_tab);
        assert_eq!(
            state.session_containers[0].focused_pane_id(),
            Some(first_second_root)
        );

        assert_eq!(state.session_containers.len(), 1);

        state.focus_session_tab(0, 2);
        state.last_pane();

        assert_eq!(state.active, Some(0));
        assert_eq!(state.session_containers[0].active_tab, first_second_tab);
        assert_eq!(
            state.session_containers[0].focused_pane_id(),
            Some(first_second_root)
        );

        state.last_pane();

        assert_eq!(state.session_containers.len(), 1);
        assert_eq!(state.active, Some(0));
        assert_eq!(state.session_containers[0].active_tab, 2);
        assert_eq!(
            state.session_containers[0].focused_pane_id(),
            Some(second_root)
        );
    }

    #[test]
    fn last_pane_tracks_cross_workspace_tab_selection() {
        let mut state = app_with_workspaces(&["one", "two"]);
        let first_root = state.session_containers[0].tabs[0].root_pane;
        let second_first_root = state.session_containers[1].tabs[0].root_pane;
        let second_tab = state.session_containers[1].test_add_tab(Some("logs"));
        let second_tab_root = state.session_containers[1].tabs[second_tab].root_pane;

        state.focus_session_tab(1, second_tab);
        state.last_pane();

        assert_eq!(state.active, Some(0));
        assert_eq!(
            state.session_containers[0].focused_pane_id(),
            Some(first_root)
        );

        state.last_pane();

        assert_eq!(state.session_containers.len(), 1);
        assert_eq!(state.active, Some(0));
        assert_eq!(state.session_containers[0].active_tab, 2);
        assert_eq!(
            state.session_containers[0].focused_pane_id(),
            Some(second_tab_root)
        );
        assert_ne!(second_first_root, second_tab_root);
    }

    #[test]
    fn focus_session_container_updates_active_after_sidebar_trim() {
        let mut state = app_with_workspaces(&["a", "b", "c", "d", "e", "f", "g", "h"]);
        crate::ui::compute_view(&mut state, ratatui::layout::Rect::new(0, 0, 80, 14));

        state.focus_session_container(7);
        crate::ui::compute_view(&mut state, ratatui::layout::Rect::new(0, 0, 80, 14));

        assert_eq!(state.session_containers.len(), 1);
        assert_eq!(state.active, Some(0));
        assert_eq!(state.selected, 0);
        assert_eq!(state.session_containers[0].active_tab, 7);
    }

    #[test]
    fn focus_session_container_marks_panes_seen() {
        let mut state = app_with_workspaces(&["a", "b"]);
        // Mark a pane in workspace 1 as unseen
        let id = *state.session_containers[1].panes.keys().next().unwrap();
        state.session_containers[1].panes.get_mut(&id).unwrap().seen = false;

        state.focus_session_container(1);
        assert!(
            state.session_containers[0].tabs[1]
                .panes
                .get(&id)
                .unwrap()
                .seen
        );
    }

    #[test]
    fn focus_session_container_out_of_bounds_is_noop() {
        let mut state = app_with_workspaces(&["a"]);
        state.focus_session_container(5);
        assert_eq!(state.active, Some(0));
    }

    #[test]
    fn close_session_container_closes_canonical_session() {
        let mut state = app_with_workspaces(&["a", "b", "c"]);
        state.selected = 1;
        state.active = Some(1);

        state.close_session_container();

        assert!(state.session_containers.is_empty());
        assert_eq!(state.selected, 0);
        assert_eq!(state.active, None);
    }

    #[test]
    fn close_last_session_container_clears_active() {
        let mut state = app_with_workspaces(&["only"]);
        state.selected = 0;
        state.close_session_container();

        assert!(state.session_containers.is_empty());
        assert_eq!(state.active, None);
        assert_eq!(state.selected, 0);
    }

    #[test]
    fn close_session_container_ignores_stale_selected_workspace() {
        let mut state = app_with_workspaces(&["a", "b"]);
        state.selected = 99;
        state.active = Some(0);

        state.close_session_container();

        assert!(state.session_containers.is_empty());
        assert_eq!(state.selected, 0);
        assert_eq!(state.active, None);
    }

    #[test]
    fn pane_died_last_pane_in_tab_removes_tab_after_session_collapse() {
        let mut state = app_with_workspaces(&["a", "b"]);
        let pane_id = *state.session_containers[0].panes.keys().next().unwrap();

        state.handle_pane_died(pane_id);

        assert_eq!(state.session_containers.len(), 1);
        assert_eq!(state.session_containers[0].tabs.len(), 1);
        assert_eq!(
            state.session_containers[0].tabs[0].custom_name.as_deref(),
            Some("b")
        );
    }

    #[test]
    fn pane_died_last_workspace_enters_navigate() {
        let mut state = app_with_workspaces(&["only"]);
        state.mode = Mode::Terminal;
        let pane_id = *state.session_containers[0].panes.keys().next().unwrap();

        state.handle_pane_died(pane_id);

        assert!(state.session_containers.is_empty());
        assert_eq!(state.mode, Mode::Navigate);
    }

    #[test]
    fn pane_died_multi_pane_keeps_workspace() {
        let mut state = app_with_workspaces(&["test"]);
        let second_id = state.session_containers[0].test_split(Direction::Horizontal);

        state.handle_pane_died(second_id);

        assert_eq!(state.session_containers.len(), 1);
        assert_eq!(state.session_containers[0].panes.len(), 1);
    }

    #[test]
    fn pane_died_unknown_pane_is_noop() {
        let mut state = app_with_workspaces(&["test"]);
        let fake_id = PaneId::from_raw(9999);

        state.handle_pane_died(fake_id);

        assert_eq!(state.session_containers.len(), 1);
    }

    #[test]
    fn pane_died_unrelated_pane_preserves_selection() {
        // Two workspaces; user is selecting text in workspace 0.
        // A pane in workspace 1 dies — selection must be preserved.
        let mut state = app_with_workspaces(&["active", "bg"]);
        let active_pane = *state.session_containers[0].panes.keys().next().unwrap();
        let bg_pane = *state.session_containers[1].panes.keys().next().unwrap();

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
        let first_id = state.session_containers[0].tabs[0].root_pane;
        let second_id = state.session_containers[0].test_split(Direction::Horizontal);

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
        assert_eq!(state.session_containers[0].panes.len(), 1);
        assert_eq!(
            state.session_containers[0].panes.keys().next().unwrap(),
            &first_id
        );
    }

    #[test]
    fn update_ready_sets_manual_update_toast() {
        let mut state = AppState::test_new();
        state.toast_config.delivery = crate::config::ToastDelivery::Gmux;

        state.handle_app_event(AppEvent::UpdateReady {
            version: "0.5.0".into(),
            install_command: "gmux update".into(),
        });

        assert_eq!(state.update_available.as_deref(), Some("0.5.0"));
        assert!(state.latest_release_notes_available);
        assert!(state.update_dismissed);
        let toast = state.toast.as_ref().expect("update toast");
        assert_eq!(toast.kind, ToastKind::UpdateInstalled);
        assert_eq!(toast.title, "v0.5.0 available");
        assert_eq!(
            toast.context,
            "detach, run `gmux update`, then follow its restart guidance"
        );
    }

    #[test]
    fn update_ready_uses_event_install_command_in_toast() {
        let mut state = AppState::test_new();
        state.toast_config.delivery = crate::config::ToastDelivery::Gmux;

        state.handle_app_event(AppEvent::UpdateReady {
            version: "0.5.0".into(),
            install_command: "brew update && brew upgrade gmux".into(),
        });

        assert_eq!(
            state.update_install_command,
            "brew update && brew upgrade gmux"
        );
        let toast = state.toast.as_ref().expect("update toast");
        assert_eq!(
            toast.context,
            "detach, run `brew update && brew upgrade gmux`, then restart this Gmux session when ready"
        );
    }

    #[test]
    fn toggle_zoom_works() {
        let mut state = app_with_workspaces(&["test"]);
        state.session_containers[0].test_split(Direction::Horizontal);

        assert!(!state.session_containers[0].zoomed);
        state.toggle_zoom();
        assert!(state.session_containers[0].zoomed);
        state.toggle_zoom();
        assert!(!state.session_containers[0].zoomed);
    }

    #[test]
    fn toggle_zoom_single_pane_noop() {
        let mut state = app_with_workspaces(&["test"]);
        state.toggle_zoom();
        assert!(!state.session_containers[0].zoomed);
    }

    #[test]
    fn navigate_pane_changes_focus_while_zoomed() {
        let mut state = app_with_workspaces(&["test"]);
        let root = state.session_containers[0].tabs[0].root_pane;
        let right = state.session_containers[0].test_split(Direction::Horizontal);
        state.session_containers[0].layout.focus_pane(root);
        state.session_containers[0].zoomed = true;
        crate::ui::compute_view(&mut state, ratatui::layout::Rect::new(0, 0, 100, 20));

        assert_eq!(state.view.pane_infos.len(), 1);
        assert_eq!(state.view.pane_infos[0].id, root);

        state.navigate_pane(NavDirection::Right);
        crate::ui::compute_view(&mut state, ratatui::layout::Rect::new(0, 0, 100, 20));

        assert!(state.session_containers[0].zoomed);
        assert_eq!(state.session_containers[0].focused_pane_id(), Some(right));
        assert_eq!(state.view.pane_infos.len(), 1);
        assert_eq!(state.view.pane_infos[0].id, right);
        assert!(state.view.pane_infos[0].inner_rect.x > state.view.pane_infos[0].rect.x);
    }

    #[test]
    fn close_pane_removes_from_workspace() {
        let mut state = app_with_workspaces(&["test"]);
        state.session_containers[0].test_split(Direction::Horizontal);
        assert_eq!(state.session_containers[0].panes.len(), 2);

        state.close_pane();
        assert_eq!(state.session_containers[0].panes.len(), 1);
    }

    #[test]
    fn close_pane_removes_unattached_terminal_state() {
        let mut state = app_with_workspaces(&["test"]);
        let pane_id = state.session_containers[0].test_split(Direction::Horizontal);
        state.ensure_test_terminals();
        let terminal_id = state.terminal_id_for_pane(0, pane_id).unwrap();

        state.close_pane();

        assert!(!state.terminals.contains_key(&terminal_id));
    }

    #[test]
    fn close_tab_removes_unattached_terminal_states() {
        let mut state = app_with_workspaces(&["test"]);
        let tab_idx = state.session_containers[0].test_add_tab(Some("logs"));
        state.ensure_test_terminals();
        state.session_containers[0].switch_tab(tab_idx);
        let pane_id = state.session_containers[0].tabs[tab_idx].root_pane;
        let terminal_id = state.terminal_id_for_pane(0, pane_id).unwrap();

        state.close_tab();

        assert!(!state.terminals.contains_key(&terminal_id));
    }

    #[test]
    fn close_session_container_removes_unattached_terminal_states() {
        let mut state = app_with_workspaces(&["one", "two"]);
        let terminal_id = state
            .terminal_id_for_pane(0, state.session_containers[0].tabs[0].root_pane)
            .unwrap();

        state.close_session_container();

        assert!(!state.terminals.contains_key(&terminal_id));
    }

    #[test]
    fn close_tab_closes_active_workspace_not_selected_workspace() {
        let mut state = app_with_workspaces(&["selected", "active"]);
        let active_terminal_id = state
            .terminal_id_for_pane(1, state.session_containers[1].tabs[0].root_pane)
            .unwrap();
        state.active = Some(1);
        state.selected = 0;

        state.close_tab();

        assert_eq!(state.session_containers.len(), 1);
        assert_eq!(state.session_containers[0].display_name(), "selected");
        assert!(!state.terminals.contains_key(&active_terminal_id));
    }

    #[test]
    fn close_pane_last_pane_closes_active_workspace_not_selected_workspace() {
        let mut state = app_with_workspaces(&["selected", "active"]);
        let active_terminal_id = state
            .terminal_id_for_pane(1, state.session_containers[1].tabs[0].root_pane)
            .unwrap();
        state.active = Some(1);
        state.selected = 0;

        state.close_pane();

        assert_eq!(state.session_containers.len(), 1);
        assert_eq!(state.session_containers[0].display_name(), "selected");
        assert!(!state.terminals.contains_key(&active_terminal_id));
    }
}
