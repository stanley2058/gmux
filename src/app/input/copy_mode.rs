use crossterm::event::{KeyCode, KeyEventKind, KeyModifiers};
use regex::Regex;
use unicode_width::UnicodeWidthChar;

use crate::{
    app::{
        state::{CopyModeSearchMatch, CopyModeSelection, CopyModeState},
        App, AppState, Mode,
    },
    input::TerminalKey,
    selection::Selection,
    terminal::TerminalRuntimeRegistry,
};

impl App {
    pub(crate) fn handle_copy_mode_key(&mut self, key: TerminalKey) {
        if key.kind == KeyEventKind::Release {
            return;
        }
        if !self.state.copy_mode_search.active {
            match copy_mode_command_char(key) {
                Some('o') => {
                    self.state.exit_copy_mode(&self.terminal_runtimes, false);
                    self.launch_focused_scrollback(super::navigate::ScrollbackOpener::Pager);
                    return;
                }
                Some('O') => {
                    self.state.exit_copy_mode(&self.terminal_runtimes, false);
                    self.launch_focused_scrollback(super::navigate::ScrollbackOpener::Editor);
                    return;
                }
                _ => {}
            }
        }
        self.state
            .handle_copy_mode_key(&self.terminal_runtimes, key);
        if let Some(content) = self.state.request_clipboard_write.take() {
            if self
                .event_tx
                .try_send(crate::events::AppEvent::ClipboardWrite { content })
                .is_err()
            {
                tracing::warn!("failed to queue clipboard write event");
            }
        }
    }

    pub(crate) fn handle_copy_mode_paste(&mut self, text: &str) -> bool {
        self.state.paste_copy_mode_search(text)
    }
}

impl AppState {
    pub(crate) fn enter_copy_mode(&mut self, terminal_runtimes: &TerminalRuntimeRegistry) {
        let Some(ws_idx) = self.session_index() else {
            return;
        };
        let Some(pane_id) = self.session().and_then(|ws| ws.focused_pane_id()) else {
            return;
        };
        let Some(info) = self.pane_info_by_id(pane_id).cloned() else {
            return;
        };
        if info.inner_rect.width == 0 || info.inner_rect.height == 0 {
            return;
        }

        let cursor = self
            .runtime_for_pane_in_session_at(terminal_runtimes, ws_idx, pane_id)
            .and_then(|rt| rt.cursor_state(info.inner_rect, true))
            .filter(|cursor| cursor.visible)
            .map(|cursor| {
                (
                    cursor.y.saturating_sub(info.inner_rect.y),
                    cursor.x.saturating_sub(info.inner_rect.x),
                )
            })
            .unwrap_or_else(|| (info.inner_rect.height.saturating_sub(1), 0));
        let entry_offset_from_bottom = self
            .pane_scroll_metrics(terminal_runtimes, pane_id)
            .map_or(0, |metrics| metrics.offset_from_bottom);

        self.clear_selection();
        self.copy_mode_search = Default::default();
        self.copy_mode = Some(CopyModeState {
            pane_id,
            cursor_row: cursor.0.min(info.inner_rect.height.saturating_sub(1)),
            cursor_col: cursor.1.min(info.inner_rect.width.saturating_sub(1)),
            entry_offset_from_bottom,
            selection: None,
        });
        self.mode = Mode::Copy;
    }

    pub(crate) fn handle_copy_mode_key(
        &mut self,
        terminal_runtimes: &TerminalRuntimeRegistry,
        key: TerminalKey,
    ) {
        if self.copy_mode_search.active {
            self.handle_copy_mode_search_key(terminal_runtimes, key);
            return;
        }

        match key.code {
            KeyCode::Esc => {
                self.exit_copy_mode(terminal_runtimes, false);
                return;
            }
            KeyCode::Enter => {
                self.exit_copy_mode(terminal_runtimes, true);
                return;
            }
            KeyCode::Left => {
                self.move_copy_cursor(terminal_runtimes, 0, -1);
                return;
            }
            KeyCode::Down => {
                self.move_copy_cursor(terminal_runtimes, 1, 0);
                return;
            }
            KeyCode::Up => {
                self.move_copy_cursor(terminal_runtimes, -1, 0);
                return;
            }
            KeyCode::Right => {
                self.move_copy_cursor(terminal_runtimes, 0, 1);
                return;
            }
            KeyCode::PageUp => {
                self.scroll_copy_mode_page(terminal_runtimes, -1, false);
                return;
            }
            KeyCode::PageDown => {
                self.scroll_copy_mode_page(terminal_runtimes, 1, false);
                return;
            }
            KeyCode::Home => {
                self.copy_mode_line_edge(terminal_runtimes, false);
                return;
            }
            KeyCode::End => {
                self.copy_mode_line_edge(terminal_runtimes, true);
                return;
            }
            _ => {}
        }

        match (key.code, key.modifiers) {
            (KeyCode::Char('b'), mods) if mods.contains(KeyModifiers::CONTROL) => {
                self.scroll_copy_mode_page(terminal_runtimes, -1, false)
            }
            (KeyCode::Char('f'), mods) if mods.contains(KeyModifiers::CONTROL) => {
                self.scroll_copy_mode_page(terminal_runtimes, 1, false)
            }
            (KeyCode::Char('u'), mods) if mods.contains(KeyModifiers::CONTROL) => {
                self.scroll_copy_mode_page(terminal_runtimes, -1, true)
            }
            (KeyCode::Char('d'), mods) if mods.contains(KeyModifiers::CONTROL) => {
                self.scroll_copy_mode_page(terminal_runtimes, 1, true)
            }
            (KeyCode::Char('y'), mods) if mods.contains(KeyModifiers::CONTROL) => {
                self.scroll_copy_mode_lines(terminal_runtimes, -1, 1)
            }
            (KeyCode::Char('e'), mods) if mods.contains(KeyModifiers::CONTROL) => {
                self.scroll_copy_mode_lines(terminal_runtimes, 1, 1)
            }
            _ => {}
        }

        let Some(ch) = copy_mode_command_char(key) else {
            return;
        };
        match ch {
            '/' => self.begin_copy_mode_search(),
            'n' => self.repeat_copy_mode_search(terminal_runtimes, SearchDirection::Forward),
            'N' => self.repeat_copy_mode_search(terminal_runtimes, SearchDirection::Backward),
            'q' => self.exit_copy_mode(terminal_runtimes, false),
            'y' => self.exit_copy_mode(terminal_runtimes, true),
            'v' | ' ' => self.begin_copy_mode_selection(terminal_runtimes),
            'V' => self.select_copy_mode_line(terminal_runtimes),
            'h' => self.move_copy_cursor(terminal_runtimes, 0, -1),
            'j' => self.move_copy_cursor(terminal_runtimes, 1, 0),
            'k' => self.move_copy_cursor(terminal_runtimes, -1, 0),
            'l' => self.move_copy_cursor(terminal_runtimes, 0, 1),
            'g' => self.copy_mode_history_top(terminal_runtimes),
            'G' => self.copy_mode_history_bottom(terminal_runtimes),
            '0' => self.copy_mode_line_edge(terminal_runtimes, false),
            '$' => self.copy_mode_line_edge(terminal_runtimes, true),
            '^' => self.copy_mode_first_non_blank(terminal_runtimes),
            'w' => self.copy_mode_word_motion(terminal_runtimes, WordMotion::NextStart),
            'b' => self.copy_mode_word_motion(terminal_runtimes, WordMotion::PreviousStart),
            'e' => self.copy_mode_word_motion(terminal_runtimes, WordMotion::NextEnd),
            '{' => self.copy_mode_paragraph(terminal_runtimes, -1),
            '}' => self.copy_mode_paragraph(terminal_runtimes, 1),
            _ => {}
        }
    }

    fn begin_copy_mode_search(&mut self) {
        self.copy_mode_search.query.clear();
        self.copy_mode_search.invalid_regex = None;
        self.copy_mode_search.active = true;
    }

    fn handle_copy_mode_search_key(
        &mut self,
        terminal_runtimes: &TerminalRuntimeRegistry,
        key: TerminalKey,
    ) {
        match key.code {
            KeyCode::Esc => {
                self.copy_mode_search.query.clear();
                self.copy_mode_search.active = false;
            }
            KeyCode::Enter => {
                let query = if self.copy_mode_search.query.is_empty() {
                    self.copy_mode_search.last_query.clone()
                } else {
                    self.copy_mode_search.query.clone()
                };
                self.copy_mode_search.query.clear();
                self.copy_mode_search.active = false;
                if !query.is_empty() {
                    self.copy_mode_search.last_query = query;
                    self.repeat_copy_mode_search(terminal_runtimes, SearchDirection::Forward);
                }
            }
            KeyCode::Backspace => {
                self.copy_mode_search.query.pop();
            }
            KeyCode::Char('u') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                self.copy_mode_search.query.clear();
            }
            _ => {
                if let Some(ch) = copy_mode_command_char(key) {
                    self.copy_mode_search.query.push(ch);
                }
            }
        }
    }

    fn paste_copy_mode_search(&mut self, text: &str) -> bool {
        if self.mode != Mode::Copy || !self.copy_mode_search.active {
            return false;
        }

        self.copy_mode_search
            .query
            .extend(text.chars().filter(|ch| *ch == '\t' || !ch.is_control()));
        true
    }

    fn exit_copy_mode(&mut self, terminal_runtimes: &TerminalRuntimeRegistry, copy: bool) {
        let restore_scroll = self
            .copy_mode
            .map(|copy_mode| (copy_mode.pane_id, copy_mode.entry_offset_from_bottom));
        if copy {
            self.copy_selection(terminal_runtimes);
        } else {
            self.clear_selection();
        }
        if let Some((pane_id, offset_from_bottom)) = restore_scroll {
            self.set_pane_scroll_offset(terminal_runtimes, pane_id, offset_from_bottom);
        }
        self.copy_mode = None;
        self.copy_mode_search = Default::default();
        self.mode = if self.session().is_some() {
            Mode::Terminal
        } else {
            Mode::Navigate
        };
    }

    fn begin_copy_mode_selection(&mut self, terminal_runtimes: &TerminalRuntimeRegistry) {
        let Some(copy_mode) = self.copy_mode else {
            return;
        };
        let Some(info) = self.pane_info_by_id(copy_mode.pane_id).cloned() else {
            return;
        };
        if copy_mode.cursor_row >= info.inner_rect.height
            || copy_mode.cursor_col >= info.inner_rect.width
        {
            return;
        }

        let metrics = self.pane_scroll_metrics(terminal_runtimes, copy_mode.pane_id);
        self.selection = Some(Selection::anchor(
            copy_mode.pane_id,
            copy_mode.cursor_row,
            copy_mode.cursor_col,
            metrics,
        ));
        if let Some(copy_mode) = self.copy_mode.as_mut() {
            copy_mode.selection = Some(CopyModeSelection::Character);
        }
    }

    fn select_copy_mode_line(&mut self, terminal_runtimes: &TerminalRuntimeRegistry) {
        let Some(mut copy_mode) = self.copy_mode else {
            return;
        };
        let Some(info) = self.pane_info_by_id(copy_mode.pane_id) else {
            return;
        };
        let end_col = info.inner_rect.width.saturating_sub(1);
        let metrics = self.pane_scroll_metrics(terminal_runtimes, copy_mode.pane_id);
        let anchor_row = Selection::absolute_row_for_viewport(copy_mode.cursor_row, metrics);
        self.selection = Some(Selection::line_range(
            copy_mode.pane_id,
            anchor_row,
            anchor_row,
            end_col,
        ));
        copy_mode.selection = Some(CopyModeSelection::Linewise { anchor_row });
        self.copy_mode = Some(copy_mode);
    }

    fn move_copy_cursor(
        &mut self,
        terminal_runtimes: &TerminalRuntimeRegistry,
        row_delta: i16,
        col_delta: i16,
    ) {
        let Some(mut copy_mode) = self.copy_mode else {
            return;
        };
        let Some(info) = self.pane_info_by_id(copy_mode.pane_id).cloned() else {
            self.exit_copy_mode(terminal_runtimes, false);
            return;
        };

        if col_delta < 0 {
            copy_mode.cursor_col = copy_mode
                .cursor_col
                .saturating_sub(col_delta.unsigned_abs());
        } else if col_delta > 0 {
            copy_mode.cursor_col = copy_mode
                .cursor_col
                .saturating_add(col_delta as u16)
                .min(info.inner_rect.width.saturating_sub(1));
        }

        if row_delta < 0 {
            let delta = row_delta.unsigned_abs();
            if copy_mode.cursor_row >= delta {
                copy_mode.cursor_row -= delta;
            } else {
                self.scroll_pane_up(terminal_runtimes, copy_mode.pane_id, usize::from(delta));
                copy_mode.cursor_row = 0;
            }
        } else if row_delta > 0 {
            let delta = row_delta as u16;
            let bottom = info.inner_rect.height.saturating_sub(1);
            if copy_mode.cursor_row.saturating_add(delta) <= bottom {
                copy_mode.cursor_row += delta;
            } else {
                self.scroll_pane_down(terminal_runtimes, copy_mode.pane_id, usize::from(delta));
                copy_mode.cursor_row = bottom;
            }
        }

        self.copy_mode = Some(copy_mode);
        self.sync_copy_mode_selection(terminal_runtimes);
    }

    fn scroll_copy_mode_page(
        &mut self,
        terminal_runtimes: &TerminalRuntimeRegistry,
        direction: i16,
        half_page: bool,
    ) {
        let Some(copy_mode) = self.copy_mode else {
            return;
        };
        let Some(info) = self.pane_info_by_id(copy_mode.pane_id).cloned() else {
            self.exit_copy_mode(terminal_runtimes, false);
            return;
        };
        let lines = copy_mode_page_lines(info.inner_rect.height, half_page);
        self.scroll_copy_mode_lines_with_state(
            terminal_runtimes,
            copy_mode,
            info,
            direction,
            lines,
        );
    }

    fn scroll_copy_mode_lines(
        &mut self,
        terminal_runtimes: &TerminalRuntimeRegistry,
        direction: i16,
        lines: usize,
    ) {
        let Some(copy_mode) = self.copy_mode else {
            return;
        };
        let Some(info) = self.pane_info_by_id(copy_mode.pane_id).cloned() else {
            self.exit_copy_mode(terminal_runtimes, false);
            return;
        };
        self.scroll_copy_mode_lines_with_state(
            terminal_runtimes,
            copy_mode,
            info,
            direction,
            lines,
        );
    }

    fn scroll_copy_mode_lines_with_state(
        &mut self,
        terminal_runtimes: &TerminalRuntimeRegistry,
        mut copy_mode: CopyModeState,
        info: crate::layout::PaneInfo,
        direction: i16,
        lines: usize,
    ) {
        if let Some(metrics) = self.pane_scroll_metrics(terminal_runtimes, copy_mode.pane_id) {
            if direction < 0 {
                let next_offset = metrics.offset_from_bottom.saturating_add(lines);
                if next_offset > metrics.max_offset_from_bottom {
                    let scrolled_lines = metrics
                        .max_offset_from_bottom
                        .saturating_sub(metrics.offset_from_bottom);
                    let cursor_lines = lines.saturating_sub(scrolled_lines);
                    self.set_pane_scroll_offset(
                        terminal_runtimes,
                        copy_mode.pane_id,
                        metrics.max_offset_from_bottom,
                    );
                    copy_mode.cursor_row = copy_mode
                        .cursor_row
                        .saturating_sub(cursor_lines.min(u16::MAX as usize) as u16);
                } else {
                    self.set_pane_scroll_offset(terminal_runtimes, copy_mode.pane_id, next_offset);
                }
            } else if metrics.offset_from_bottom < lines {
                let cursor_lines = lines.saturating_sub(metrics.offset_from_bottom);
                self.set_pane_scroll_offset(terminal_runtimes, copy_mode.pane_id, 0);
                copy_mode.cursor_row = copy_mode
                    .cursor_row
                    .saturating_add(cursor_lines.min(u16::MAX as usize) as u16)
                    .min(info.inner_rect.height.saturating_sub(1));
            } else {
                self.set_pane_scroll_offset(
                    terminal_runtimes,
                    copy_mode.pane_id,
                    metrics.offset_from_bottom - lines,
                );
            }
        } else if direction < 0 {
            self.scroll_pane_up(terminal_runtimes, copy_mode.pane_id, lines);
        } else {
            self.scroll_pane_down(terminal_runtimes, copy_mode.pane_id, lines);
        }
        self.copy_mode = Some(copy_mode);
        self.sync_copy_mode_selection(terminal_runtimes);
    }

    fn copy_mode_history_top(&mut self, terminal_runtimes: &TerminalRuntimeRegistry) {
        let Some(mut copy_mode) = self.copy_mode else {
            return;
        };
        let Some(metrics) = self.pane_scroll_metrics(terminal_runtimes, copy_mode.pane_id) else {
            return;
        };
        self.set_pane_scroll_offset(
            terminal_runtimes,
            copy_mode.pane_id,
            metrics.max_offset_from_bottom,
        );
        copy_mode.cursor_row = 0;
        self.copy_mode = Some(copy_mode);
        self.sync_copy_mode_selection(terminal_runtimes);
    }

    fn copy_mode_history_bottom(&mut self, terminal_runtimes: &TerminalRuntimeRegistry) {
        let Some(mut copy_mode) = self.copy_mode else {
            return;
        };
        let Some(info) = self.pane_info_by_id(copy_mode.pane_id) else {
            self.exit_copy_mode(terminal_runtimes, false);
            return;
        };
        self.set_pane_scroll_offset(terminal_runtimes, copy_mode.pane_id, 0);
        copy_mode.cursor_row = info.inner_rect.height.saturating_sub(1);
        self.copy_mode = Some(copy_mode);
        self.sync_copy_mode_selection(terminal_runtimes);
    }

    fn copy_mode_line_edge(&mut self, terminal_runtimes: &TerminalRuntimeRegistry, end: bool) {
        let Some(mut copy_mode) = self.copy_mode else {
            return;
        };
        let Some(info) = self.pane_info_by_id(copy_mode.pane_id) else {
            self.exit_copy_mode(terminal_runtimes, false);
            return;
        };
        copy_mode.cursor_col = if end {
            info.inner_rect.width.saturating_sub(1)
        } else {
            0
        };
        self.copy_mode = Some(copy_mode);
        self.sync_copy_mode_selection(terminal_runtimes);
    }

    fn copy_mode_first_non_blank(&mut self, terminal_runtimes: &TerminalRuntimeRegistry) {
        let Some(mut copy_mode) = self.copy_mode else {
            return;
        };
        let Some(text) = self.copy_mode_visible_row_text(terminal_runtimes, copy_mode.cursor_row)
        else {
            return;
        };
        copy_mode.cursor_col = first_non_blank_col(&text).unwrap_or(0);
        self.copy_mode = Some(copy_mode);
        self.sync_copy_mode_selection(terminal_runtimes);
    }

    fn copy_mode_word_motion(
        &mut self,
        terminal_runtimes: &TerminalRuntimeRegistry,
        motion: WordMotion,
    ) {
        let Some(mut copy_mode) = self.copy_mode else {
            return;
        };
        let Some(info) = self.pane_info_by_id(copy_mode.pane_id) else {
            self.exit_copy_mode(terminal_runtimes, false);
            return;
        };
        let Some(text) = self.copy_mode_visible_row_text(terminal_runtimes, copy_mode.cursor_row)
        else {
            return;
        };
        let Some(col) = word_motion_target(&text, copy_mode.cursor_col, motion) else {
            return;
        };
        copy_mode.cursor_col = col.min(info.inner_rect.width.saturating_sub(1));
        self.copy_mode = Some(copy_mode);
        self.sync_copy_mode_selection(terminal_runtimes);
    }

    fn copy_mode_paragraph(&mut self, terminal_runtimes: &TerminalRuntimeRegistry, direction: i16) {
        let Some(copy_mode) = self.copy_mode else {
            return;
        };
        let Some(pane_height) = self
            .pane_info_by_id(copy_mode.pane_id)
            .map(|info| info.inner_rect.height)
        else {
            self.exit_copy_mode(terminal_runtimes, false);
            return;
        };
        let limit = self
            .pane_scroll_metrics(terminal_runtimes, copy_mode.pane_id)
            .map(|metrics| metrics.max_offset_from_bottom + metrics.viewport_rows)
            .unwrap_or(pane_height as usize)
            .clamp(1, 1000);

        for _ in 0..limit {
            let before = self.copy_mode;
            let before_offset = self
                .pane_scroll_metrics(terminal_runtimes, copy_mode.pane_id)
                .map(|metrics| metrics.offset_from_bottom);

            self.move_copy_cursor(terminal_runtimes, direction, 0);

            let Some(after) = self.copy_mode else {
                return;
            };
            if self
                .copy_mode_visible_row_text(terminal_runtimes, after.cursor_row)
                .is_some_and(|text| text.trim().is_empty())
            {
                return;
            }

            let Some(after_metrics) = self.pane_scroll_metrics(terminal_runtimes, after.pane_id)
            else {
                continue;
            };
            let did_not_move =
                before == self.copy_mode && before_offset == Some(after_metrics.offset_from_bottom);
            let at_top = direction < 0
                && after.cursor_row == 0
                && after_metrics.offset_from_bottom == after_metrics.max_offset_from_bottom;
            let at_bottom = direction > 0
                && after.cursor_row + 1 >= pane_height
                && after_metrics.offset_from_bottom == 0;
            if did_not_move || at_top || at_bottom {
                return;
            }
        }
    }

    fn copy_mode_visible_row_text(
        &self,
        terminal_runtimes: &TerminalRuntimeRegistry,
        viewport_row: u16,
    ) -> Option<String> {
        let copy_mode = self.copy_mode?;
        let ws_idx = self.session_index()?;
        let info = self.pane_info_by_id(copy_mode.pane_id)?;
        if viewport_row >= info.inner_rect.height || info.inner_rect.width == 0 {
            return None;
        }
        let metrics = self.pane_scroll_metrics(terminal_runtimes, copy_mode.pane_id);
        let row_selection = Selection::range(
            copy_mode.pane_id,
            viewport_row,
            0,
            info.inner_rect.width.saturating_sub(1),
            metrics,
        );
        self.runtime_for_pane_in_session_at(terminal_runtimes, ws_idx, copy_mode.pane_id)?
            .extract_selection(&row_selection)
    }

    fn copy_mode_absolute_row_text(
        &self,
        terminal_runtimes: &TerminalRuntimeRegistry,
        copy_mode: CopyModeState,
        info: &crate::layout::PaneInfo,
        absolute_row: usize,
    ) -> Option<String> {
        let ws_idx = self.session_index()?;
        if info.inner_rect.width == 0 {
            return None;
        }
        let absolute_row: u32 = absolute_row.try_into().ok()?;
        let row_selection = Selection::line_range(
            copy_mode.pane_id,
            absolute_row,
            absolute_row,
            info.inner_rect.width.saturating_sub(1),
        );
        self.runtime_for_pane_in_session_at(terminal_runtimes, ws_idx, copy_mode.pane_id)?
            .extract_selection(&row_selection)
    }

    fn repeat_copy_mode_search(
        &mut self,
        terminal_runtimes: &TerminalRuntimeRegistry,
        direction: SearchDirection,
    ) {
        let query = self.copy_mode_search.last_query.clone();
        if query.is_empty() {
            return;
        }
        let Some(copy_mode) = self.copy_mode else {
            return;
        };
        let Some(info) = self.pane_info_by_id(copy_mode.pane_id).cloned() else {
            self.exit_copy_mode(terminal_runtimes, false);
            return;
        };
        if info.inner_rect.width == 0 || info.inner_rect.height == 0 {
            return;
        }

        let metrics = self.pane_scroll_metrics(terminal_runtimes, copy_mode.pane_id);
        let current_row = Selection::absolute_row_for_viewport(copy_mode.cursor_row, metrics);
        let total_rows = copy_mode_total_rows(metrics, info.inner_rect.height);
        let matches = self.collect_copy_mode_search_matches(
            terminal_runtimes,
            copy_mode,
            &info,
            total_rows,
            &query,
        );
        let matches = match matches {
            Ok(matches) => matches,
            Err(err) => {
                self.copy_mode_search.matches.clear();
                self.copy_mode_search.current_match = None;
                self.copy_mode_search.invalid_regex = Some(err.to_string());
                return;
            }
        };
        self.copy_mode_search.invalid_regex = None;
        let context = CopyModeSearchContext {
            matches: &matches,
            direction,
        };
        let Some(current_match) =
            self.find_copy_mode_search_match(&context, current_row as usize, copy_mode.cursor_col)
        else {
            self.copy_mode_search.matches = matches;
            self.copy_mode_search.current_match = None;
            return;
        };
        let search_match = matches[current_match];
        self.copy_mode_search.matches = matches;
        self.copy_mode_search.current_match = Some(current_match);

        self.move_copy_mode_cursor_to_absolute_row(
            terminal_runtimes,
            copy_mode,
            info,
            search_match.row as usize,
            search_match.start_col,
        );
    }

    fn find_copy_mode_search_match(
        &self,
        context: &CopyModeSearchContext<'_>,
        current_row: usize,
        cursor_col: u16,
    ) -> Option<usize> {
        if context.matches.is_empty() {
            return None;
        }
        match context.direction {
            SearchDirection::Forward => context
                .matches
                .iter()
                .position(|search_match| {
                    (search_match.row as usize) > current_row
                        || ((search_match.row as usize) == current_row
                            && search_match.start_col > cursor_col)
                })
                .or(Some(0)),
            SearchDirection::Backward => context
                .matches
                .iter()
                .rposition(|search_match| {
                    (search_match.row as usize) < current_row
                        || ((search_match.row as usize) == current_row
                            && search_match.start_col < cursor_col)
                })
                .or_else(|| context.matches.len().checked_sub(1)),
        }
    }

    fn collect_copy_mode_search_matches(
        &self,
        terminal_runtimes: &TerminalRuntimeRegistry,
        copy_mode: CopyModeState,
        info: &crate::layout::PaneInfo,
        total_rows: usize,
        query: &str,
    ) -> Result<Vec<CopyModeSearchMatch>, regex::Error> {
        let regex = Regex::new(query)?;
        let mut matches = Vec::new();
        for absolute_row in 0..total_rows {
            let Some(text) =
                self.copy_mode_absolute_row_text(terminal_runtimes, copy_mode, info, absolute_row)
            else {
                continue;
            };
            for regex_match in regex.find_iter(&text) {
                if regex_match.is_empty() {
                    continue;
                }
                let start_col = display_col_for_byte(&text, regex_match.start());
                let width = display_width_for_text(regex_match.as_str());
                if width == 0 {
                    continue;
                }
                matches.push(CopyModeSearchMatch {
                    row: absolute_row.try_into().unwrap_or(u32::MAX),
                    start_col,
                    end_col: start_col
                        .saturating_add(width.saturating_sub(1))
                        .min(info.inner_rect.width.saturating_sub(1)),
                });
            }
        }
        Ok(matches)
    }

    fn move_copy_mode_cursor_to_absolute_row(
        &mut self,
        terminal_runtimes: &TerminalRuntimeRegistry,
        mut copy_mode: CopyModeState,
        info: crate::layout::PaneInfo,
        absolute_row: usize,
        cursor_col: u16,
    ) {
        let pane_height = usize::from(info.inner_rect.height);
        let mut viewport_top = self
            .pane_scroll_metrics(terminal_runtimes, copy_mode.pane_id)
            .map(|metrics| {
                metrics
                    .max_offset_from_bottom
                    .saturating_sub(metrics.offset_from_bottom)
            })
            .unwrap_or(0);

        if let Some(metrics) = self.pane_scroll_metrics(terminal_runtimes, copy_mode.pane_id) {
            let viewport_bottom = viewport_top.saturating_add(pane_height).saturating_sub(1);
            let should_center = absolute_row < viewport_top || absolute_row >= viewport_bottom;
            if should_center {
                viewport_top = absolute_row.saturating_sub(pane_height / 2);
            }
            viewport_top = viewport_top.min(metrics.max_offset_from_bottom);
            self.set_pane_scroll_offset(
                terminal_runtimes,
                copy_mode.pane_id,
                metrics.max_offset_from_bottom.saturating_sub(viewport_top),
            );
        }

        copy_mode.cursor_row = absolute_row
            .saturating_sub(viewport_top)
            .min(usize::from(info.inner_rect.height.saturating_sub(1)))
            as u16;
        copy_mode.cursor_col = cursor_col.min(info.inner_rect.width.saturating_sub(1));
        self.copy_mode = Some(copy_mode);
        self.sync_copy_mode_selection(terminal_runtimes);
    }

    fn sync_copy_mode_selection(&mut self, terminal_runtimes: &TerminalRuntimeRegistry) {
        let Some(copy_mode) = self.copy_mode else {
            return;
        };
        let Some(selection) = copy_mode.selection else {
            return;
        };
        let Some(info) = self.pane_info_by_id(copy_mode.pane_id).cloned() else {
            return;
        };
        match selection {
            CopyModeSelection::Character => {
                let screen_col = info.inner_rect.x.saturating_add(copy_mode.cursor_col);
                let screen_row = info.inner_rect.y.saturating_add(copy_mode.cursor_row);
                self.update_selection_cursor(
                    terminal_runtimes,
                    copy_mode.pane_id,
                    screen_col,
                    screen_row,
                );
            }
            CopyModeSelection::Linewise { anchor_row } => {
                let metrics = self.pane_scroll_metrics(terminal_runtimes, copy_mode.pane_id);
                let cursor_row =
                    Selection::absolute_row_for_viewport(copy_mode.cursor_row, metrics);
                self.selection = Some(Selection::line_range(
                    copy_mode.pane_id,
                    anchor_row,
                    cursor_row,
                    info.inner_rect.width.saturating_sub(1),
                ));
            }
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum SearchDirection {
    Forward,
    Backward,
}

struct CopyModeSearchContext<'a> {
    matches: &'a [CopyModeSearchMatch],
    direction: SearchDirection,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum WordMotion {
    NextStart,
    PreviousStart,
    NextEnd,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct WordSpan {
    start: u16,
    end: u16,
}

fn first_non_blank_col(text: &str) -> Option<u16> {
    let mut col = 0u16;
    for ch in text.chars() {
        if !ch.is_whitespace() {
            return Some(col);
        }
        col = col.saturating_add(char_cell_width(ch));
    }
    None
}

fn word_motion_target(text: &str, cursor_col: u16, motion: WordMotion) -> Option<u16> {
    let spans = word_spans(text);
    match motion {
        WordMotion::NextStart => spans.iter().enumerate().find_map(|(idx, span)| {
            if cursor_col < span.start {
                Some(span.start)
            } else if cursor_col >= span.start && cursor_col <= span.end {
                spans.get(idx + 1).map(|next| next.start)
            } else {
                None
            }
        }),
        WordMotion::PreviousStart => spans
            .iter()
            .rev()
            .find(|span| span.start < cursor_col)
            .map(|span| span.start),
        WordMotion::NextEnd => spans.iter().find_map(|span| {
            if cursor_col < span.end {
                Some(span.end)
            } else {
                None
            }
        }),
    }
}

fn word_spans(text: &str) -> Vec<WordSpan> {
    let mut spans = Vec::new();
    let mut col = 0u16;
    let mut start = None;

    for ch in text.chars() {
        let width = char_cell_width(ch);
        if ch.is_whitespace() {
            if let Some(start_col) = start.take() {
                spans.push(WordSpan {
                    start: start_col,
                    end: col.saturating_sub(1),
                });
            }
        } else if start.is_none() {
            start = Some(col);
        }
        col = col.saturating_add(width);
    }

    if let Some(start_col) = start {
        spans.push(WordSpan {
            start: start_col,
            end: col.saturating_sub(1),
        });
    }
    spans
}

fn char_cell_width(ch: char) -> u16 {
    UnicodeWidthChar::width(ch).unwrap_or(1).max(1) as u16
}

fn display_col_for_byte(text: &str, byte_idx: usize) -> u16 {
    text[..byte_idx]
        .chars()
        .fold(0u16, |col, ch| col.saturating_add(char_cell_width(ch)))
}

fn display_width_for_text(text: &str) -> u16 {
    text.chars()
        .fold(0u16, |width, ch| width.saturating_add(char_cell_width(ch)))
}

fn copy_mode_total_rows(metrics: Option<crate::pane::ScrollMetrics>, pane_height: u16) -> usize {
    metrics
        .map(|metrics| metrics.max_offset_from_bottom + metrics.viewport_rows)
        .unwrap_or(usize::from(pane_height))
        .max(1)
}

fn copy_mode_page_lines(height: u16, half_page: bool) -> usize {
    if height <= 2 {
        1
    } else if half_page {
        usize::from(height / 2)
    } else {
        usize::from(height - 2)
    }
}

fn copy_mode_command_char(key: TerminalKey) -> Option<char> {
    if !key.modifiers.difference(KeyModifiers::SHIFT).is_empty() {
        return None;
    }

    if let Some(ch) = key.shifted_codepoint.and_then(char::from_u32) {
        return Some(ch);
    }

    let KeyCode::Char(ch) = key.code else {
        return None;
    };
    if key.modifiers.contains(KeyModifiers::SHIFT) {
        Some(shifted_ascii_char(ch).unwrap_or(ch))
    } else {
        Some(ch)
    }
}

fn shifted_ascii_char(ch: char) -> Option<char> {
    match ch {
        'a'..='z' => Some(ch.to_ascii_uppercase()),
        '1' => Some('!'),
        '2' => Some('@'),
        '3' => Some('#'),
        '4' => Some('$'),
        '5' => Some('%'),
        '6' => Some('^'),
        '7' => Some('&'),
        '8' => Some('*'),
        '9' => Some('('),
        '0' => Some(')'),
        '-' => Some('_'),
        '=' => Some('+'),
        '[' => Some('{'),
        ']' => Some('}'),
        '\\' => Some('|'),
        ';' => Some(':'),
        '\'' => Some('"'),
        ',' => Some('<'),
        '.' => Some('>'),
        '/' => Some('?'),
        '`' => Some('~'),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::super::{app_for_mouse_test, numbered_lines_bytes, unique_temp_path, wait_for_file};
    use super::*;
    use crate::{events::AppEvent, workspace::Workspace};
    use ratatui::layout::Rect;

    fn app_with_copy_runtime(
        runtime: impl FnOnce(u16, u16) -> crate::terminal::TerminalRuntime,
    ) -> (App, crate::layout::PaneId) {
        let mut app = app_for_mouse_test();
        let mut ws = Workspace::test_new("test");
        let pane_id = ws.tabs[0].root_pane;
        let pane_infos = ws.tabs[0].layout.panes(Rect::new(0, 0, 20, 5));
        let info = pane_infos[0].clone();
        ws.tabs[0].runtimes.insert(
            pane_id,
            runtime(info.inner_rect.width, info.inner_rect.height),
        );
        app.state.sessions = vec![ws];
        app.state.active_session = Some(0);
        app.state.selected_session = 0;
        app.state.mode = Mode::Terminal;
        app.state.view.pane_infos = pane_infos;
        (app, pane_id)
    }

    fn app_with_copy_screen(bytes: &[u8]) -> (App, crate::layout::PaneId) {
        app_with_copy_runtime(|cols, rows| {
            crate::terminal::TerminalRuntime::test_with_screen_bytes(cols, rows, bytes)
        })
    }

    fn app_with_copy_scrollback(bytes: &[u8]) -> (App, crate::layout::PaneId) {
        app_with_copy_runtime(|cols, rows| {
            crate::terminal::TerminalRuntime::test_with_scrollback_bytes(
                cols,
                rows,
                16 * 1024,
                bytes,
            )
        })
    }

    fn copy_mode_clipboard_text(app: &mut App) -> String {
        match app.event_rx.try_recv().expect("clipboard event") {
            AppEvent::ClipboardWrite { content } => {
                String::from_utf8(content).expect("utf8 clipboard")
            }
            other => panic!("unexpected event: {other:?}"),
        }
    }

    fn copy_mode_viewport_top_row(app: &App, pane_id: crate::layout::PaneId) -> usize {
        let metrics = app
            .state
            .runtime_for_pane_in_session_at(&app.terminal_runtimes, 0, pane_id)
            .and_then(crate::terminal::TerminalRuntime::scroll_metrics)
            .expect("copy mode scroll metrics");
        metrics
            .max_offset_from_bottom
            .saturating_sub(metrics.offset_from_bottom)
    }

    fn copy_mode_offset_from_bottom(app: &App, pane_id: crate::layout::PaneId) -> usize {
        app.state
            .runtime_for_pane_in_session_at(&app.terminal_runtimes, 0, pane_id)
            .and_then(crate::terminal::TerminalRuntime::scroll_metrics)
            .expect("copy mode scroll metrics")
            .offset_from_bottom
    }

    fn copy_mode_scroll_metrics(
        app: &App,
        pane_id: crate::layout::PaneId,
    ) -> crate::pane::ScrollMetrics {
        app.state
            .runtime_for_pane_in_session_at(&app.terminal_runtimes, 0, pane_id)
            .and_then(crate::terminal::TerminalRuntime::scroll_metrics)
            .expect("copy mode scroll metrics")
    }

    fn copy_mode_cursor_absolute_row(app: &App, pane_id: crate::layout::PaneId) -> usize {
        copy_mode_viewport_top_row(app, pane_id)
            + usize::from(app.state.copy_mode.expect("copy mode").cursor_row)
    }

    fn enter_copy_mode_search(app: &mut App, query: &str) {
        app.handle_copy_mode_key(TerminalKey::new(KeyCode::Char('/'), KeyModifiers::empty()));
        for ch in query.chars() {
            app.handle_copy_mode_key(TerminalKey::new(KeyCode::Char(ch), KeyModifiers::empty()));
        }
        app.handle_copy_mode_key(TerminalKey::new(KeyCode::Enter, KeyModifiers::empty()));
    }

    #[tokio::test]
    async fn enter_copy_mode_tracks_focused_pane() {
        let (mut app, pane_id) = app_with_copy_screen(b"alpha\nbeta\n");
        app.state.enter_copy_mode(&app.terminal_runtimes);
        assert_eq!(app.state.mode, Mode::Copy);
        assert_eq!(app.state.copy_mode.expect("copy mode").pane_id, pane_id);
    }

    #[tokio::test]
    async fn copy_mode_ctrl_b_and_ctrl_f_use_full_page_size() {
        let bytes = numbered_lines_bytes(64);
        let (mut app, pane_id) = app_with_copy_scrollback(&bytes);
        app.state.enter_copy_mode(&app.terminal_runtimes);
        let height = app.state.copy_mode.expect("copy mode").cursor_row + 1;
        let expected_lines = copy_mode_page_lines(height, false);

        app.handle_copy_mode_key(TerminalKey::new(KeyCode::Char('b'), KeyModifiers::CONTROL));
        assert_eq!(copy_mode_offset_from_bottom(&app, pane_id), expected_lines);

        app.handle_copy_mode_key(TerminalKey::new(KeyCode::Char('f'), KeyModifiers::CONTROL));
        assert_eq!(copy_mode_offset_from_bottom(&app, pane_id), 0);
        assert_eq!(app.state.mode, Mode::Copy);
    }

    #[tokio::test]
    async fn copy_mode_ctrl_y_and_ctrl_e_scroll_single_lines() {
        let bytes = numbered_lines_bytes(64);
        let (mut app, pane_id) = app_with_copy_scrollback(&bytes);
        app.state.enter_copy_mode(&app.terminal_runtimes);

        app.handle_copy_mode_key(TerminalKey::new(KeyCode::Char('y'), KeyModifiers::CONTROL));
        assert_eq!(copy_mode_offset_from_bottom(&app, pane_id), 1);

        app.handle_copy_mode_key(TerminalKey::new(KeyCode::Char('e'), KeyModifiers::CONTROL));
        assert_eq!(copy_mode_offset_from_bottom(&app, pane_id), 0);
        assert_eq!(app.state.mode, Mode::Copy);
    }

    #[tokio::test]
    async fn copy_mode_word_motions_use_visible_row_words() {
        let (mut app, _) = app_with_copy_screen(b"foo bar baz\n");
        app.state.enter_copy_mode(&app.terminal_runtimes);
        if let Some(copy_mode) = app.state.copy_mode.as_mut() {
            copy_mode.cursor_row = 0;
            copy_mode.cursor_col = 0;
        }

        app.handle_copy_mode_key(TerminalKey::new(KeyCode::Char('w'), KeyModifiers::empty()));
        assert_eq!(app.state.copy_mode.expect("copy mode").cursor_col, 4);

        app.handle_copy_mode_key(TerminalKey::new(KeyCode::Char('e'), KeyModifiers::empty()));
        assert_eq!(app.state.copy_mode.expect("copy mode").cursor_col, 6);

        app.handle_copy_mode_key(TerminalKey::new(KeyCode::Char('b'), KeyModifiers::empty()));
        assert_eq!(app.state.copy_mode.expect("copy mode").cursor_col, 4);
    }

    #[tokio::test]
    async fn copy_mode_slash_search_moves_to_visible_match() {
        let (mut app, _) = app_with_copy_screen(b"alpha\r\nbeta\r\ngamma\r\n");
        app.state.enter_copy_mode(&app.terminal_runtimes);
        if let Some(copy_mode) = app.state.copy_mode.as_mut() {
            copy_mode.cursor_row = 0;
            copy_mode.cursor_col = 0;
        }

        enter_copy_mode_search(&mut app, "beta");

        let copy_mode = app.state.copy_mode.expect("copy mode");
        assert_eq!(copy_mode.cursor_row, 1);
        assert_eq!(copy_mode.cursor_col, 0);
        assert_eq!(app.state.copy_mode_search.last_query, "beta");
        assert_eq!(app.state.copy_mode_search.matches.len(), 1);
        assert_eq!(app.state.copy_mode_search.current_match, Some(0));
        assert!(!app.state.copy_mode_search.active);
    }

    #[tokio::test]
    async fn copy_mode_search_accepts_paste() {
        let (mut app, _) = app_with_copy_screen(b"alpha\r\nbeta\r\ngamma\r\n");
        app.state.enter_copy_mode(&app.terminal_runtimes);
        if let Some(copy_mode) = app.state.copy_mode.as_mut() {
            copy_mode.cursor_row = 0;
            copy_mode.cursor_col = 0;
        }

        app.handle_copy_mode_key(TerminalKey::new(KeyCode::Char('/'), KeyModifiers::empty()));
        app.handle_paste("beta\n".to_string()).await;
        assert_eq!(app.state.copy_mode_search.query, "beta");
        app.handle_copy_mode_key(TerminalKey::new(KeyCode::Enter, KeyModifiers::empty()));

        let copy_mode = app.state.copy_mode.expect("copy mode");
        assert_eq!(copy_mode.cursor_row, 1);
        assert_eq!(copy_mode.cursor_col, 0);
        assert_eq!(app.state.copy_mode_search.last_query, "beta");
    }

    #[tokio::test]
    async fn copy_mode_slash_search_uses_regex() {
        let (mut app, _) = app_with_copy_screen(b"task-1\r\ntask-20\r\njob-3\r\n");
        app.state.enter_copy_mode(&app.terminal_runtimes);
        if let Some(copy_mode) = app.state.copy_mode.as_mut() {
            copy_mode.cursor_row = 0;
            copy_mode.cursor_col = 0;
        }

        enter_copy_mode_search(&mut app, r"task-\d+");

        let copy_mode = app.state.copy_mode.expect("copy mode");
        assert_eq!(copy_mode.cursor_row, 1);
        assert_eq!(copy_mode.cursor_col, 0);
        assert_eq!(app.state.copy_mode_search.matches.len(), 2);
        assert_eq!(app.state.copy_mode_search.current_match, Some(1));
    }

    #[tokio::test]
    async fn copy_mode_invalid_regex_records_error_without_moving() {
        let (mut app, _) = app_with_copy_screen(b"alpha\r\nbeta\r\n");
        app.state.enter_copy_mode(&app.terminal_runtimes);
        if let Some(copy_mode) = app.state.copy_mode.as_mut() {
            copy_mode.cursor_row = 0;
            copy_mode.cursor_col = 0;
        }

        enter_copy_mode_search(&mut app, "[");

        let copy_mode = app.state.copy_mode.expect("copy mode");
        assert_eq!(copy_mode.cursor_row, 0);
        assert_eq!(copy_mode.cursor_col, 0);
        assert_eq!(app.state.copy_mode_search.last_query, "[");
        assert!(app.state.copy_mode_search.matches.is_empty());
        assert_eq!(app.state.copy_mode_search.current_match, None);
        assert!(app.state.copy_mode_search.invalid_regex.is_some());
    }

    #[tokio::test]
    async fn copy_mode_slash_search_wraps_into_scrollback() {
        let mut text = String::new();
        for row in 0..32 {
            if row == 3 {
                text.push_str("needle row\r\n");
            } else {
                text.push_str(&format!("row {row:02}\r\n"));
            }
        }
        let (mut app, pane_id) = app_with_copy_scrollback(text.as_bytes());
        app.state.enter_copy_mode(&app.terminal_runtimes);

        enter_copy_mode_search(&mut app, "needle");

        assert_eq!(copy_mode_cursor_absolute_row(&app, pane_id), 3);
        assert_eq!(app.state.copy_mode.expect("copy mode").cursor_col, 0);
    }

    #[tokio::test]
    async fn copy_mode_search_centers_lower_match_instead_of_hint_row() {
        let mut text = String::new();
        for row in 0..32 {
            if row == 15 {
                text.push_str("needle row\r\n");
            } else {
                text.push_str(&format!("row {row:02}\r\n"));
            }
        }
        let (mut app, pane_id) = app_with_copy_scrollback(text.as_bytes());
        app.state.enter_copy_mode(&app.terminal_runtimes);

        let metrics = copy_mode_scroll_metrics(&app, pane_id);
        app.state.set_pane_scroll_offset(
            &app.terminal_runtimes,
            pane_id,
            metrics.max_offset_from_bottom.saturating_sub(5),
        );
        if let Some(copy_mode) = app.state.copy_mode.as_mut() {
            copy_mode.cursor_row = 0;
            copy_mode.cursor_col = 0;
        }

        enter_copy_mode_search(&mut app, "needle");

        let pane_height = app
            .state
            .pane_info_by_id(pane_id)
            .expect("pane info")
            .inner_rect
            .height;
        assert_eq!(copy_mode_cursor_absolute_row(&app, pane_id), 15);
        assert_eq!(
            app.state.copy_mode.expect("copy mode").cursor_row,
            pane_height / 2
        );
    }

    #[tokio::test]
    async fn copy_mode_n_and_shift_n_repeat_last_search() {
        let (mut app, _) = app_with_copy_screen(b"one needle\r\ntwo needle\r\nthree needle\r\n");
        app.state.enter_copy_mode(&app.terminal_runtimes);
        if let Some(copy_mode) = app.state.copy_mode.as_mut() {
            copy_mode.cursor_row = 0;
            copy_mode.cursor_col = 0;
        }

        enter_copy_mode_search(&mut app, "needle");
        assert_eq!(app.state.copy_mode.expect("copy mode").cursor_row, 0);
        assert_eq!(app.state.copy_mode.expect("copy mode").cursor_col, 4);
        assert_eq!(app.state.copy_mode_search.matches.len(), 3);
        assert_eq!(app.state.copy_mode_search.current_match, Some(0));

        app.handle_copy_mode_key(TerminalKey::new(KeyCode::Char('n'), KeyModifiers::empty()));
        assert_eq!(app.state.copy_mode.expect("copy mode").cursor_row, 1);
        assert_eq!(app.state.copy_mode.expect("copy mode").cursor_col, 4);
        assert_eq!(app.state.copy_mode_search.current_match, Some(1));

        app.handle_copy_mode_key(TerminalKey::new(KeyCode::Char('n'), KeyModifiers::SHIFT));
        assert_eq!(app.state.copy_mode.expect("copy mode").cursor_row, 0);
        assert_eq!(app.state.copy_mode.expect("copy mode").cursor_col, 4);
        assert_eq!(app.state.copy_mode_search.current_match, Some(0));
    }

    #[tokio::test]
    async fn copy_mode_search_extends_visual_selection() {
        let (mut app, _) = app_with_copy_screen(b"alpha\r\nbeta\r\ngamma\r\n");
        app.state.enter_copy_mode(&app.terminal_runtimes);
        if let Some(copy_mode) = app.state.copy_mode.as_mut() {
            copy_mode.cursor_row = 0;
            copy_mode.cursor_col = 0;
        }

        app.handle_copy_mode_key(TerminalKey::new(KeyCode::Char('v'), KeyModifiers::empty()));
        enter_copy_mode_search(&mut app, "gamma");

        let selection = app.state.selection.as_ref().expect("selection");
        let ((start_row, start_col), (end_row, end_col)) = selection.ordered_cells();
        assert_eq!((start_row, start_col), (0, 0));
        assert_eq!((end_row, end_col), (2, 0));
    }

    #[tokio::test]
    async fn copy_mode_esc_cancels_search_prompt_without_moving() {
        let (mut app, _) = app_with_copy_screen(b"alpha\r\nbeta\r\n");
        app.state.enter_copy_mode(&app.terminal_runtimes);
        if let Some(copy_mode) = app.state.copy_mode.as_mut() {
            copy_mode.cursor_row = 0;
            copy_mode.cursor_col = 0;
        }

        app.handle_copy_mode_key(TerminalKey::new(KeyCode::Char('/'), KeyModifiers::empty()));
        app.handle_copy_mode_key(TerminalKey::new(KeyCode::Char('b'), KeyModifiers::empty()));
        app.handle_copy_mode_key(TerminalKey::new(KeyCode::Esc, KeyModifiers::empty()));

        let copy_mode = app.state.copy_mode.expect("copy mode");
        assert_eq!(copy_mode.cursor_row, 0);
        assert_eq!(copy_mode.cursor_col, 0);
        assert!(!app.state.copy_mode_search.active);
        assert!(app.state.copy_mode_search.query.is_empty());
        assert!(app.state.copy_mode_search.last_query.is_empty());
    }

    #[tokio::test]
    async fn copy_mode_shift_v_y_copies_visible_line() {
        let (mut app, _) = app_with_copy_screen(b"alpha\r\nbeta\r\n");
        app.state.enter_copy_mode(&app.terminal_runtimes);
        if let Some(copy_mode) = app.state.copy_mode.as_mut() {
            copy_mode.cursor_row = 1;
            copy_mode.cursor_col = 2;
        }

        app.handle_copy_mode_key(TerminalKey::new(KeyCode::Char('v'), KeyModifiers::SHIFT));
        app.handle_copy_mode_key(TerminalKey::new(KeyCode::Char('y'), KeyModifiers::empty()));

        assert_eq!(copy_mode_clipboard_text(&mut app), "beta");
        assert_eq!(app.state.mode, Mode::Terminal);
    }

    #[tokio::test]
    async fn copy_mode_shift_v_extends_linewise_down() {
        let (mut app, _) = app_with_copy_screen(b"alpha\r\nbeta\r\ngamma\r\n");
        app.state.enter_copy_mode(&app.terminal_runtimes);
        if let Some(copy_mode) = app.state.copy_mode.as_mut() {
            copy_mode.cursor_row = 0;
            copy_mode.cursor_col = 2;
        }

        app.handle_copy_mode_key(TerminalKey::new(KeyCode::Char('v'), KeyModifiers::SHIFT));
        app.handle_copy_mode_key(TerminalKey::new(KeyCode::Char('j'), KeyModifiers::empty()));
        app.handle_copy_mode_key(TerminalKey::new(KeyCode::Char('y'), KeyModifiers::empty()));

        assert_eq!(copy_mode_clipboard_text(&mut app), "alpha\nbeta");
    }

    #[tokio::test]
    async fn copy_mode_shift_v_extends_linewise_up() {
        let (mut app, _) = app_with_copy_screen(b"alpha\r\nbeta\r\ngamma\r\n");
        app.state.enter_copy_mode(&app.terminal_runtimes);
        if let Some(copy_mode) = app.state.copy_mode.as_mut() {
            copy_mode.cursor_row = 1;
            copy_mode.cursor_col = 2;
        }

        app.handle_copy_mode_key(TerminalKey::new(KeyCode::Char('v'), KeyModifiers::SHIFT));
        app.handle_copy_mode_key(TerminalKey::new(KeyCode::Char('k'), KeyModifiers::empty()));
        app.handle_copy_mode_key(TerminalKey::new(KeyCode::Char('y'), KeyModifiers::empty()));

        assert_eq!(copy_mode_clipboard_text(&mut app), "alpha\nbeta");
    }

    #[tokio::test]
    async fn copy_mode_shift_v_reverses_without_character_tail() {
        let (mut app, _) = app_with_copy_screen(b"alpha\r\nbeta\r\ngamma\r\n");
        app.state.enter_copy_mode(&app.terminal_runtimes);
        if let Some(copy_mode) = app.state.copy_mode.as_mut() {
            copy_mode.cursor_row = 1;
            copy_mode.cursor_col = 2;
        }

        app.handle_copy_mode_key(TerminalKey::new(KeyCode::Char('v'), KeyModifiers::SHIFT));
        app.handle_copy_mode_key(TerminalKey::new(KeyCode::Char('j'), KeyModifiers::empty()));
        app.handle_copy_mode_key(TerminalKey::new(KeyCode::Char('k'), KeyModifiers::empty()));
        app.handle_copy_mode_key(TerminalKey::new(KeyCode::Char('k'), KeyModifiers::empty()));
        app.handle_copy_mode_key(TerminalKey::new(KeyCode::Char('y'), KeyModifiers::empty()));

        assert_eq!(copy_mode_clipboard_text(&mut app), "alpha\nbeta");
    }

    #[tokio::test]
    async fn copy_mode_shift_v_horizontal_motion_keeps_linewise_selection() {
        let (mut app, _) = app_with_copy_screen(b"alpha\r\nbeta\r\n");
        app.state.enter_copy_mode(&app.terminal_runtimes);
        if let Some(copy_mode) = app.state.copy_mode.as_mut() {
            copy_mode.cursor_row = 1;
            copy_mode.cursor_col = 2;
        }

        app.handle_copy_mode_key(TerminalKey::new(KeyCode::Char('v'), KeyModifiers::SHIFT));
        app.handle_copy_mode_key(TerminalKey::new(KeyCode::Char('h'), KeyModifiers::empty()));
        app.handle_copy_mode_key(TerminalKey::new(KeyCode::Char('l'), KeyModifiers::empty()));
        app.handle_copy_mode_key(TerminalKey::new(KeyCode::Char('y'), KeyModifiers::empty()));

        assert_eq!(copy_mode_clipboard_text(&mut app), "beta");
    }

    #[tokio::test]
    async fn copy_mode_shift_v_page_up_keeps_linewise_scrollback_selection() {
        let bytes = numbered_lines_bytes(64);
        let (mut app, pane_id) = app_with_copy_scrollback(&bytes);
        app.state.enter_copy_mode(&app.terminal_runtimes);
        if let Some(copy_mode) = app.state.copy_mode.as_mut() {
            copy_mode.cursor_row = 0;
            copy_mode.cursor_col = 2;
        }

        let anchor_row = copy_mode_viewport_top_row(&app, pane_id);
        app.handle_copy_mode_key(TerminalKey::new(KeyCode::Char('v'), KeyModifiers::SHIFT));
        app.handle_copy_mode_key(TerminalKey::new(KeyCode::PageUp, KeyModifiers::empty()));
        let cursor_row = copy_mode_viewport_top_row(&app, pane_id);
        app.handle_copy_mode_key(TerminalKey::new(KeyCode::Char('y'), KeyModifiers::empty()));

        assert!(cursor_row < anchor_row);
        let expected = (cursor_row..=anchor_row)
            .map(|row| format!("{row:06}"))
            .collect::<Vec<_>>()
            .join("\n");
        assert_eq!(copy_mode_clipboard_text(&mut app), expected);
        assert_eq!(copy_mode_offset_from_bottom(&app, pane_id), 0);
    }

    #[tokio::test]
    async fn copy_mode_selection_scroll_survives_view_recompute() {
        let bytes = numbered_lines_bytes(64);
        let (mut app, pane_id) = app_with_copy_scrollback(&bytes);
        app.state.enter_copy_mode(&app.terminal_runtimes);
        if let Some(copy_mode) = app.state.copy_mode.as_mut() {
            copy_mode.cursor_row = 0;
        }

        app.handle_copy_mode_key(TerminalKey::new(KeyCode::Char('v'), KeyModifiers::empty()));
        crate::ui::compute_view_with_runtime_registry(
            &mut app.state,
            &app.terminal_runtimes,
            Rect::new(0, 0, 20, 5),
        );
        assert!(app.state.selection_viewport_pin.is_none());

        app.handle_copy_mode_key(TerminalKey::new(KeyCode::Char('k'), KeyModifiers::empty()));
        let scrolled_offset = copy_mode_offset_from_bottom(&app, pane_id);
        assert!(scrolled_offset > 0);

        crate::ui::compute_view_with_runtime_registry(
            &mut app.state,
            &app.terminal_runtimes,
            Rect::new(0, 0, 20, 5),
        );

        assert_eq!(copy_mode_offset_from_bottom(&app, pane_id), scrolled_offset);
    }

    #[tokio::test]
    async fn copy_mode_page_up_uses_tmux_page_size() {
        let bytes = numbered_lines_bytes(64);
        let (mut app, pane_id) = app_with_copy_scrollback(&bytes);
        app.state.enter_copy_mode(&app.terminal_runtimes);
        let height = app.state.copy_mode.expect("copy mode").cursor_row + 1;
        let expected_lines = copy_mode_page_lines(height, false);

        app.handle_copy_mode_key(TerminalKey::new(KeyCode::PageUp, KeyModifiers::empty()));

        assert_eq!(copy_mode_offset_from_bottom(&app, pane_id), expected_lines);
    }

    #[tokio::test]
    async fn copy_mode_ctrl_u_moves_cursor_when_history_top_clamps() {
        let bytes = numbered_lines_bytes(64);
        let (mut app, pane_id) = app_with_copy_scrollback(&bytes);
        app.state.enter_copy_mode(&app.terminal_runtimes);
        let bottom = app.state.copy_mode.expect("copy mode").cursor_row;
        let lines = copy_mode_page_lines(bottom + 1, true);
        let metrics = copy_mode_scroll_metrics(&app, pane_id);
        assert!(metrics.max_offset_from_bottom >= lines);
        app.state.set_pane_scroll_offset(
            &app.terminal_runtimes,
            pane_id,
            metrics.max_offset_from_bottom - lines + 1,
        );
        if let Some(copy_mode) = app.state.copy_mode.as_mut() {
            copy_mode.cursor_row = bottom;
        }

        app.handle_copy_mode_key(TerminalKey::new(KeyCode::Char('u'), KeyModifiers::CONTROL));

        let copy_mode = app.state.copy_mode.expect("copy mode");
        let expected_cursor_delta = 1;
        assert_eq!(
            copy_mode_offset_from_bottom(&app, pane_id),
            metrics.max_offset_from_bottom
        );
        assert_eq!(
            copy_mode.cursor_row,
            bottom.saturating_sub(expected_cursor_delta as u16)
        );
    }

    #[tokio::test]
    async fn copy_mode_ctrl_d_moves_cursor_when_live_bottom_clamps() {
        let bytes = numbered_lines_bytes(64);
        let (mut app, pane_id) = app_with_copy_scrollback(&bytes);
        app.state.enter_copy_mode(&app.terminal_runtimes);
        let bottom = app.state.copy_mode.expect("copy mode").cursor_row;
        let lines = copy_mode_page_lines(bottom + 1, true);
        assert!(lines > 1);
        app.state
            .set_pane_scroll_offset(&app.terminal_runtimes, pane_id, lines - 1);
        if let Some(copy_mode) = app.state.copy_mode.as_mut() {
            copy_mode.cursor_row = 0;
        }

        app.handle_copy_mode_key(TerminalKey::new(KeyCode::Char('d'), KeyModifiers::CONTROL));

        let copy_mode = app.state.copy_mode.expect("copy mode");
        assert_eq!(copy_mode_offset_from_bottom(&app, pane_id), 0);
        assert_eq!(copy_mode.cursor_row, 1);
    }

    #[tokio::test]
    async fn copy_mode_q_exits_and_returns_to_bottom_after_scrollback() {
        let bytes = numbered_lines_bytes(64);
        let (mut app, pane_id) = app_with_copy_scrollback(&bytes);
        app.state.enter_copy_mode(&app.terminal_runtimes);

        app.handle_copy_mode_key(TerminalKey::new(KeyCode::PageUp, KeyModifiers::empty()));
        assert!(copy_mode_offset_from_bottom(&app, pane_id) > 0);

        app.handle_copy_mode_key(TerminalKey::new(KeyCode::Char('q'), KeyModifiers::empty()));

        assert_eq!(app.state.mode, Mode::Terminal);
        assert!(app.state.copy_mode.is_none());
        assert_eq!(copy_mode_offset_from_bottom(&app, pane_id), 0);
    }

    #[tokio::test]
    async fn copy_mode_q_restores_entry_scrollback_offset() {
        let bytes = numbered_lines_bytes(64);
        let (mut app, pane_id) = app_with_copy_scrollback(&bytes);
        let entry_offset = 3;
        app.state
            .set_pane_scroll_offset(&app.terminal_runtimes, pane_id, entry_offset);
        assert_eq!(copy_mode_offset_from_bottom(&app, pane_id), entry_offset);

        app.state.enter_copy_mode(&app.terminal_runtimes);
        app.handle_copy_mode_key(TerminalKey::new(KeyCode::PageUp, KeyModifiers::empty()));
        assert!(copy_mode_offset_from_bottom(&app, pane_id) > entry_offset);

        app.handle_copy_mode_key(TerminalKey::new(KeyCode::Char('q'), KeyModifiers::empty()));

        assert_eq!(app.state.mode, Mode::Terminal);
        assert!(app.state.copy_mode.is_none());
        assert_eq!(copy_mode_offset_from_bottom(&app, pane_id), entry_offset);
    }

    #[tokio::test]
    async fn shifted_punctuation_keys_work_with_enhanced_key_reporting() {
        let (mut app, _) = app_with_copy_screen(b"foo\r\n\r\nbar\r\n");
        app.state.enter_copy_mode(&app.terminal_runtimes);
        if let Some(copy_mode) = app.state.copy_mode.as_mut() {
            copy_mode.cursor_row = 2;
            copy_mode.cursor_col = 2;
        }

        app.handle_copy_mode_key(TerminalKey::new(KeyCode::Char('6'), KeyModifiers::SHIFT));
        assert_eq!(app.state.copy_mode.expect("copy mode").cursor_col, 0);

        app.handle_copy_mode_key(TerminalKey::new(KeyCode::Char(']'), KeyModifiers::SHIFT));
        assert_eq!(app.state.copy_mode.expect("copy mode").cursor_row, 3);

        app.handle_copy_mode_key(TerminalKey::new(KeyCode::Char('['), KeyModifiers::SHIFT));
        assert_eq!(app.state.copy_mode.expect("copy mode").cursor_row, 1);

        app.handle_copy_mode_key(
            TerminalKey::new(KeyCode::Char(']'), KeyModifiers::SHIFT)
                .with_shifted_codepoint('}' as u32),
        );
        assert_eq!(app.state.copy_mode.expect("copy mode").cursor_row, 3);
    }

    #[tokio::test]
    async fn copy_mode_v_y_copies_selection_and_exits() {
        let (mut app, _) = app_with_copy_screen(b"alpha\nbeta\n");
        app.state.enter_copy_mode(&app.terminal_runtimes);
        if let Some(copy_mode) = app.state.copy_mode.as_mut() {
            copy_mode.cursor_row = 0;
            copy_mode.cursor_col = 0;
        }
        app.handle_copy_mode_key(TerminalKey::new(KeyCode::Char('v'), KeyModifiers::empty()));
        app.handle_copy_mode_key(TerminalKey::new(KeyCode::Char('l'), KeyModifiers::empty()));
        app.handle_copy_mode_key(TerminalKey::new(KeyCode::Char('l'), KeyModifiers::empty()));
        app.handle_copy_mode_key(TerminalKey::new(KeyCode::Char('y'), KeyModifiers::empty()));

        match app.event_rx.try_recv().expect("clipboard event") {
            AppEvent::ClipboardWrite { content } => assert_eq!(content, b"alp"),
            other => panic!("unexpected event: {other:?}"),
        }
        assert_eq!(app.state.mode, Mode::Terminal);
        assert!(app.state.copy_mode.is_none());
    }

    #[tokio::test]
    async fn copy_mode_o_opens_scrollback_in_pager() {
        let (mut app, _) = app_with_copy_screen(b"alpha\r\nbeta\r\n");
        app.state.enter_copy_mode(&app.terminal_runtimes);
        let output_path = unique_temp_path("copy-mode-pager");
        app.state.terminal_pager = format!("sh -c 'cp \"$1\" {}' sh", output_path.display());

        app.handle_copy_mode_key(TerminalKey::new(KeyCode::Char('o'), KeyModifiers::empty()));

        let content = wait_for_file(&output_path);
        assert!(content.contains("alpha"));
        assert!(content.contains("beta"));
        assert_eq!(app.state.mode, Mode::Terminal);
        assert!(app.state.copy_mode.is_none());

        let _ = std::fs::remove_file(output_path);
    }

    #[tokio::test]
    async fn copy_mode_shift_o_opens_scrollback_in_editor() {
        let (mut app, _) = app_with_copy_screen(b"alpha\r\nbeta\r\n");
        app.state.enter_copy_mode(&app.terminal_runtimes);
        let output_path = unique_temp_path("copy-mode-editor");
        app.state.terminal_editor = format!("sh -c 'cp \"$1\" {}' sh", output_path.display());

        app.handle_copy_mode_key(TerminalKey::new(KeyCode::Char('O'), KeyModifiers::SHIFT));

        let content = wait_for_file(&output_path);
        assert!(content.contains("alpha"));
        assert!(content.contains("beta"));
        assert_eq!(app.state.mode, Mode::Terminal);
        assert!(app.state.copy_mode.is_none());

        let _ = std::fs::remove_file(output_path);
    }
}
