use ratatui::layout::Rect;

use crate::app::state::{AppState, ViewLayout};

use super::ScrollbarClickTarget;

impl AppState {
    pub(super) fn pane_panel_rect(&self) -> Rect {
        let sidebar = self.view.sidebar_rect;
        if self.sidebar_collapsed || sidebar.width <= 1 || sidebar.height == 0 {
            return Rect::default();
        }
        crate::ui::expanded_pane_panel_rect(sidebar)
    }

    pub(super) fn pane_panel_scrollbar_target_at(
        &self,
        col: u16,
        row: u16,
    ) -> Option<ScrollbarClickTarget> {
        let area = self.pane_panel_rect();
        let metrics = crate::ui::pane_panel_scroll_metrics(self, area);
        let track = crate::ui::pane_panel_scrollbar_rect(self, area)?;
        if col < track.x
            || col >= track.x + track.width
            || row < track.y
            || row >= track.y + track.height
        {
            return None;
        }
        if let Some(grab_row_offset) = crate::ui::scrollbar_thumb_grab_offset(metrics, track, row) {
            Some(ScrollbarClickTarget::Thumb { grab_row_offset })
        } else {
            Some(ScrollbarClickTarget::Track {
                offset_from_bottom: crate::ui::scrollbar_offset_from_row(metrics, track, row),
            })
        }
    }

    pub(super) fn pane_panel_offset_for_drag_row(
        &self,
        row: u16,
        grab_row_offset: u16,
    ) -> Option<usize> {
        let area = self.pane_panel_rect();
        let metrics = crate::ui::pane_panel_scroll_metrics(self, area);
        let track = crate::ui::pane_panel_scrollbar_rect(self, area)?;
        Some(crate::ui::scrollbar_offset_from_drag_row(
            metrics,
            track,
            row,
            grab_row_offset,
        ))
    }

    pub(super) fn set_pane_panel_offset_from_bottom(&mut self, offset_from_bottom: usize) {
        let area = self.pane_panel_rect();
        let metrics = crate::ui::pane_panel_scroll_metrics(self, area);
        self.pane_panel_scroll = metrics
            .max_offset_from_bottom
            .saturating_sub(offset_from_bottom);
    }

    pub(super) fn scroll_pane_panel(&mut self, delta: i16) {
        let area = self.pane_panel_rect();
        let max_scroll = crate::ui::pane_panel_scroll_metrics(self, area).max_offset_from_bottom;
        if delta.is_negative() {
            self.pane_panel_scroll = self
                .pane_panel_scroll
                .saturating_sub(delta.unsigned_abs() as usize);
        } else {
            self.pane_panel_scroll = self
                .pane_panel_scroll
                .saturating_add(delta as usize)
                .min(max_scroll);
        }
    }

    pub(crate) fn sidebar_footer_rect(&self) -> Rect {
        let footer = crate::ui::expanded_sidebar_footer_rect(self.view.sidebar_rect);
        if self.sidebar_collapsed || footer == Rect::default() {
            return Rect::default();
        }
        footer
    }

    pub(crate) fn sidebar_new_button_rect(&self) -> Rect {
        let footer = self.sidebar_footer_rect();
        let width = 5u16.min(footer.width.max(1));
        Rect::new(footer.x, footer.y, width, footer.height)
    }

    pub(crate) fn global_launcher_rect(&self) -> Rect {
        if self.view.layout == ViewLayout::Mobile {
            return self.view.mobile_menu_hit_area;
        }

        let footer = self.sidebar_footer_rect();
        let width = if self.global_menu_attention_badge_visible() {
            8
        } else {
            6
        };
        let toggle = crate::ui::expanded_sidebar_toggle_rect(self.view.sidebar_rect);
        let max_x_exclusive = if toggle == Rect::default() {
            footer.x + footer.width
        } else {
            toggle.x
        };
        let available_width = max_x_exclusive.saturating_sub(footer.x).max(1);
        let width = width.min(available_width);
        let x = max_x_exclusive.saturating_sub(width).max(footer.x);
        Rect::new(x, footer.y, width, footer.height)
    }

    pub(crate) fn global_menu_labels(&self) -> Vec<&'static str> {
        let mut labels = vec!["settings", "keybinds", "reload config"];
        labels.push("detach");
        labels
    }

    pub(crate) fn global_menu_rect(&self) -> Rect {
        let screen = self.screen_rect();
        let launcher = self.global_launcher_rect();
        let labels = self.global_menu_labels();
        let content_width = labels
            .iter()
            .map(|label| {
                let badge_width = if self.global_menu_item_has_badge(label) {
                    2
                } else {
                    0
                };
                label.chars().count() as u16 + badge_width
            })
            .max()
            .unwrap_or(8)
            .saturating_add(2);
        let menu_w = content_width.saturating_add(2).min(screen.width.max(1));
        let menu_h = (labels.len() as u16 + 2).min(screen.height.max(1));
        let max_x = screen.x + screen.width.saturating_sub(menu_w);
        let desired_x = launcher.x + launcher.width.saturating_sub(menu_w);
        let x = desired_x.min(max_x);
        let y = launcher.y.saturating_sub(menu_h);
        Rect::new(x, y, menu_w, menu_h)
    }

    pub(super) fn on_sidebar_divider(&self, col: u16, row: u16) -> bool {
        if self.sidebar_collapsed {
            return false;
        }
        let sidebar = self.view.sidebar_rect;
        let toggle = crate::ui::expanded_sidebar_toggle_rect(sidebar);
        let on_toggle = toggle.width > 0
            && col >= toggle.x
            && col < toggle.x + toggle.width
            && row >= toggle.y
            && row < toggle.y + toggle.height;
        sidebar.width > 0
            && !on_toggle
            && col == sidebar.x + sidebar.width.saturating_sub(1)
            && row >= sidebar.y
            && row < sidebar.y + sidebar.height
    }

    pub(super) fn on_sidebar_toggle(&self, col: u16, row: u16) -> bool {
        let rect = if self.sidebar_collapsed {
            crate::ui::collapsed_sidebar_toggle_rect(self.view.sidebar_rect)
        } else {
            crate::ui::expanded_sidebar_toggle_rect(self.view.sidebar_rect)
        };
        rect.width > 0
            && col >= rect.x
            && col < rect.x + rect.width
            && row >= rect.y
            && row < rect.y + rect.height
    }

    pub(super) fn set_manual_sidebar_width(&mut self, divider_col: u16) {
        let sidebar = self.view.sidebar_rect;
        let width = divider_col.saturating_sub(sidebar.x).saturating_add(1);
        self.sidebar_width = width.clamp(self.sidebar_min_width, self.sidebar_max_width);
        self.sidebar_width_source = crate::app::state::SidebarWidthSource::Manual;
        self.mark_session_dirty();
    }

    fn collapsed_detail_session_idx(&self) -> Option<usize> {
        self.session_index()
    }

    pub(super) fn collapsed_pane_detail_target_at(
        &self,
        row: u16,
    ) -> Option<(usize, usize, crate::layout::PaneId)> {
        if !self.sidebar_collapsed {
            return None;
        }

        let (_, _, detail_area) = crate::ui::collapsed_sidebar_sections(self.view.sidebar_rect);
        let detail_content_area = Rect::new(
            detail_area.x,
            detail_area.y,
            detail_area.width,
            detail_area.height.saturating_sub(1),
        );
        if detail_content_area == Rect::default()
            || row < detail_content_area.y
            || row >= detail_content_area.y + detail_content_area.height
        {
            return None;
        }

        let ws_idx = self.collapsed_detail_session_idx()?;
        let ws = self.session()?;
        let detail_idx = (row - detail_content_area.y) as usize;
        let details = ws.pane_details(&self.terminals);
        let detail = details.get(detail_idx)?;
        Some((ws_idx, detail.tab_idx, detail.pane_id))
    }

    pub(super) fn on_pane_panel_scope_toggle(&self, col: u16, row: u16) -> bool {
        if self.sidebar_collapsed {
            return false;
        }

        let detail_area = self.pane_panel_rect();
        let rect = crate::ui::pane_panel_toggle_rect(detail_area, self.pane_panel_scope);
        rect.width > 0
            && col >= rect.x
            && col < rect.x + rect.width
            && row >= rect.y
            && row < rect.y + rect.height
    }

    pub(super) fn pane_detail_target_at(
        &self,
        row: u16,
    ) -> Option<(usize, usize, crate::layout::PaneId)> {
        if self.sidebar_collapsed {
            return None;
        }

        let detail_area = self.pane_panel_rect();
        let metrics = crate::ui::pane_panel_scroll_metrics(self, detail_area);
        let body =
            crate::ui::pane_panel_body_rect(detail_area, crate::ui::should_show_scrollbar(metrics));
        if body.height < 2 || row < body.y || row >= body.y + body.height {
            return None;
        }

        let mut row_y = body.y;
        for detail in crate::ui::pane_panel_entries(self)
            .into_iter()
            .skip(self.pane_panel_scroll)
        {
            if row_y.saturating_add(1) >= body.y + body.height {
                break;
            }
            if row == row_y || row == row_y + 1 {
                return Some((detail.ws_idx, detail.tab_idx, detail.pane_id));
            }
            row_y = row_y.saturating_add(2);
            if row_y < body.y + body.height {
                row_y = row_y.saturating_add(1);
            }
        }
        None
    }
}

#[cfg(test)]
mod tests {
    use crossterm::event::{MouseButton, MouseEventKind};
    use ratatui::layout::Rect;

    use super::super::{app_for_mouse_test, capture_snapshot, mouse};
    use crate::{
        app::state::{DragTarget, Mode, PanePanelScope},
        workspace::Workspace,
    };

    #[test]
    fn clicking_launcher_opens_global_menu() {
        let mut app = app_for_mouse_test();
        let rect = app.state.global_launcher_rect();

        app.handle_mouse(mouse(
            MouseEventKind::Down(MouseButton::Left),
            rect.x + rect.width.saturating_sub(1),
            rect.y,
        ));

        assert_eq!(app.state.mode, Mode::GlobalMenu);
    }

    #[test]
    fn hovering_global_menu_updates_highlight() {
        let mut app = app_for_mouse_test();
        let launcher = app.state.global_launcher_rect();
        app.handle_mouse(mouse(
            MouseEventKind::Down(MouseButton::Left),
            launcher.x,
            launcher.y,
        ));

        let menu = app.state.global_menu_rect();
        app.handle_mouse(mouse(MouseEventKind::Moved, menu.x + 2, menu.y + 2));

        assert_eq!(app.state.global_menu.highlighted, 1);
    }

    #[test]
    fn clicking_keybinds_menu_item_opens_help() {
        let mut app = app_for_mouse_test();
        let launcher = app.state.global_launcher_rect();
        app.handle_mouse(mouse(
            MouseEventKind::Down(MouseButton::Left),
            launcher.x,
            launcher.y,
        ));

        let menu = app.state.global_menu_rect();
        app.handle_mouse(mouse(
            MouseEventKind::Down(MouseButton::Left),
            menu.x + 2,
            menu.y + 2,
        ));

        assert_eq!(app.state.mode, Mode::KeybindHelp);
    }

    #[test]
    fn clicking_settings_menu_item_opens_settings() {
        let mut app = app_for_mouse_test();
        let launcher = app.state.global_launcher_rect();
        app.handle_mouse(mouse(
            MouseEventKind::Down(MouseButton::Left),
            launcher.x,
            launcher.y,
        ));

        let menu = app.state.global_menu_rect();
        app.handle_mouse(mouse(
            MouseEventKind::Down(MouseButton::Left),
            menu.x + 2,
            menu.y + 1,
        ));

        assert_eq!(app.state.mode, Mode::Settings);
    }

    #[test]
    fn clicking_reload_config_menu_item_requests_reload() {
        let mut app = app_for_mouse_test();
        let launcher = app.state.global_launcher_rect();
        app.handle_mouse(mouse(
            MouseEventKind::Down(MouseButton::Left),
            launcher.x,
            launcher.y,
        ));

        let menu = app.state.global_menu_rect();
        app.handle_mouse(mouse(
            MouseEventKind::Down(MouseButton::Left),
            menu.x + 2,
            menu.y + 3,
        ));

        assert!(app.state.request_reload_config);
        assert_eq!(app.state.mode, Mode::Navigate);
    }

    #[test]
    fn persistence_mode_menu_surfaces_detach_action() {
        let mut app = app_for_mouse_test();
        app.state.detach_exits = false;

        let launcher = app.state.global_launcher_rect();
        app.handle_mouse(mouse(
            MouseEventKind::Down(MouseButton::Left),
            launcher.x,
            launcher.y,
        ));

        assert_eq!(
            app.state.global_menu_labels(),
            vec!["settings", "keybinds", "reload config", "detach"]
        );

        let menu = app.state.global_menu_rect();
        app.handle_mouse(mouse(
            MouseEventKind::Down(MouseButton::Left),
            menu.x + 2,
            menu.y + 3,
        ));

        assert!(app.state.detach_requested);
        assert!(!app.state.should_quit);
        assert_ne!(app.state.mode, Mode::GlobalMenu);
    }

    #[test]
    fn clicking_pane_detail_row_switches_to_correct_tab_and_pane() {
        let mut app = app_for_mouse_test();
        let mut ws = Workspace::test_new("test");
        ws.tabs[0].set_custom_name("main".into());
        let first_tab = ws.test_add_tab(Some("logs"));
        let second_pane = ws.tabs[first_tab].root_pane;
        app.state.sessions = vec![ws];
        app.state.ensure_test_terminals();
        app.state.active_session = Some(0);
        app.state.selected_session = 0;
        app.state.mode = Mode::Terminal;

        let detail_area = app.state.pane_panel_rect();
        let body = crate::ui::pane_panel_body_rect(detail_area, false);
        app.handle_mouse(mouse(
            MouseEventKind::Down(MouseButton::Left),
            body.x + 1,
            body.y + 3,
        ));

        assert_eq!(app.state.sessions[0].active_tab, 1);
        assert_eq!(app.state.sessions[0].tabs[1].layout.focused(), second_pane);
        assert_eq!(app.state.mode, Mode::Terminal);
        let snapshot = capture_snapshot(&app.state);
        assert_eq!(snapshot.active_tab, first_tab);
        assert_eq!(snapshot.tabs[first_tab].focused, Some(second_pane.raw()));
    }

    #[test]
    fn clicking_pane_panel_toggle_switches_scope() {
        let mut app = app_for_mouse_test();
        app.state.sessions = vec![Workspace::test_new("test")];
        app.state.active_session = Some(0);
        app.state.selected_session = 0;
        app.state.mode = Mode::Terminal;
        app.state.pane_panel_scroll = 3;

        let detail_area = app.state.pane_panel_rect();
        let toggle = crate::ui::pane_panel_toggle_rect(detail_area, app.state.pane_panel_scope);
        app.handle_mouse(mouse(
            MouseEventKind::Down(MouseButton::Left),
            toggle.x,
            toggle.y,
        ));

        assert_eq!(app.state.pane_panel_scope, PanePanelScope::Current);
        assert_eq!(app.state.pane_panel_scroll, 0);
    }

    #[test]
    fn clicking_all_scope_pane_row_collapses_to_session_tab() {
        let mut app = app_for_mouse_test();
        let first = Workspace::test_new("one");

        let second = Workspace::test_new("two");
        let second_pane = second.tabs[0].root_pane;

        app.state.sessions = vec![first, second];
        app.state.ensure_test_terminals();
        app.state.active_session = Some(0);
        app.state.selected_session = 0;
        app.state.mode = Mode::Terminal;
        app.state.pane_panel_scope = PanePanelScope::All;

        let detail_area = app.state.pane_panel_rect();
        app.handle_mouse(mouse(
            MouseEventKind::Down(MouseButton::Left),
            detail_area.x + 2,
            detail_area.y + 6,
        ));

        assert_eq!(app.state.sessions.len(), 1);
        assert_eq!(app.state.active_session, Some(0));
        assert_eq!(app.state.selected_session, 0);
        assert_eq!(app.state.sessions[0].active_tab, 1);
        assert_eq!(app.state.sessions[0].tabs[1].layout.focused(), second_pane);
    }

    #[test]
    fn scrolling_pane_panel_with_wheel_updates_pane_panel_scroll() {
        let mut app = app_for_mouse_test();
        let mut ws = Workspace::test_new("test");

        let mut tabs = Vec::new();
        for tab_name in ["logs", "review", "ops", "db", "ci", "deploy"] {
            let tab_idx = ws.test_add_tab(Some(tab_name));
            let pane_id = ws.tabs[tab_idx].root_pane;
            tabs.push((tab_idx, pane_id));
        }

        app.state.sessions = vec![ws];
        app.state.ensure_test_terminals();
        for (tab_idx, pane_id) in tabs {
            let terminal_id = app.state.sessions[0].tabs[tab_idx].panes[&pane_id]
                .attached_terminal_id
                .clone();
            assert!(app.state.terminals.contains_key(&terminal_id));
        }
        app.state.active_session = Some(0);
        app.state.selected_session = 0;
        app.state.mode = Mode::Terminal;

        let detail_area = app.state.pane_panel_rect();
        assert!(crate::ui::should_show_scrollbar(
            crate::ui::pane_panel_scroll_metrics(&app.state, detail_area)
        ));

        app.handle_mouse(mouse(
            MouseEventKind::ScrollDown,
            detail_area.x + 1,
            detail_area.y + 4,
        ));

        assert_eq!(app.state.pane_panel_scroll, 1);
        assert_eq!(app.state.selected_session, 0);
    }

    #[test]
    fn clicking_scrolled_pane_detail_row_switches_to_correct_tab_and_pane() {
        let mut app = app_for_mouse_test();
        let mut ws = Workspace::test_new("test");
        let second_tab = ws.test_add_tab(Some("logs"));
        let second_pane = ws.tabs[second_tab].root_pane;
        let mut extra_tabs = Vec::new();
        for tab_name in ["review", "ops"] {
            let tab_idx = ws.test_add_tab(Some(tab_name));
            let pane_id = ws.tabs[tab_idx].root_pane;
            extra_tabs.push((tab_idx, pane_id));
        }

        app.state.sessions = vec![ws];
        app.state.ensure_test_terminals();
        for (tab_idx, pane_id) in extra_tabs {
            let terminal_id = app.state.sessions[0].tabs[tab_idx].panes[&pane_id]
                .attached_terminal_id
                .clone();
            assert!(app.state.terminals.contains_key(&terminal_id));
        }
        app.state.active_session = Some(0);
        app.state.selected_session = 0;
        app.state.mode = Mode::Terminal;
        app.state.pane_panel_scroll = 1;

        let detail_area = app.state.pane_panel_rect();
        let body = crate::ui::pane_panel_body_rect(detail_area, true);
        app.handle_mouse(mouse(
            MouseEventKind::Down(MouseButton::Left),
            body.x + 1,
            body.y,
        ));

        assert_eq!(app.state.sessions[0].active_tab, second_tab);
        assert_eq!(
            app.state.sessions[0].tabs[second_tab].layout.focused(),
            second_pane
        );
        assert_eq!(app.state.mode, Mode::Terminal);
    }

    #[test]
    fn clicking_collapsed_pane_row_switches_to_correct_tab_and_pane() {
        let mut app = app_for_mouse_test();
        let mut ws = Workspace::test_new("test");
        let second_tab = ws.test_add_tab(Some("logs"));
        let second_pane = ws.tabs[second_tab].root_pane;
        app.state.sessions = vec![ws];
        app.state.ensure_test_terminals();
        app.state.active_session = Some(0);
        app.state.selected_session = 0;
        app.state.mode = Mode::Terminal;
        app.state.sidebar_collapsed = true;
        app.state.view.sidebar_rect = Rect::new(0, 0, 4, 20);
        app.state.view.terminal_area = Rect::new(4, 0, 80, 20);

        let (_, _, detail_area) =
            crate::ui::collapsed_sidebar_sections(app.state.view.sidebar_rect);
        app.handle_mouse(mouse(
            MouseEventKind::Down(MouseButton::Left),
            detail_area.x,
            detail_area.y + 1,
        ));

        assert_eq!(app.state.sessions[0].active_tab, 1);
        assert_eq!(app.state.sessions[0].tabs[1].layout.focused(), second_pane);
        assert_eq!(app.state.mode, Mode::Terminal);
    }

    #[test]
    fn clicking_collapsed_pane_row_uses_visible_session_not_selected_legacy_workspace() {
        let mut app = app_for_mouse_test();
        let mut first = Workspace::test_new("one");
        let second_tab = first.test_add_tab(Some("logs"));
        let target_pane = first.tabs[second_tab].root_pane;
        let second = Workspace::test_new("two");
        app.state.sessions = vec![first, second];
        app.state.ensure_test_terminals();
        app.state.active_session = Some(0);
        app.state.selected_session = 1;
        app.state.mode = Mode::Navigate;
        app.state.sidebar_collapsed = true;
        app.state.view.sidebar_rect = Rect::new(0, 0, 4, 20);
        app.state.view.terminal_area = Rect::new(4, 0, 80, 20);

        let (_, _, detail_area) =
            crate::ui::collapsed_sidebar_sections(app.state.view.sidebar_rect);
        app.handle_mouse(mouse(
            MouseEventKind::Down(MouseButton::Left),
            detail_area.x,
            detail_area.y + 1,
        ));

        assert_eq!(app.state.sessions.len(), 1);
        assert_eq!(app.state.active_session, Some(0));
        assert_eq!(app.state.selected_session, 0);
        assert_eq!(app.state.sessions[0].active_tab, 1);
        assert_eq!(app.state.sessions[0].tabs[1].layout.focused(), target_pane);
        assert_eq!(app.state.mode, Mode::Terminal);
    }

    #[test]
    fn clicking_collapsed_sidebar_toggle_expands_sidebar() {
        let mut app = app_for_mouse_test();
        app.state.sidebar_collapsed = true;
        app.state.view.sidebar_rect = Rect::new(0, 0, 4, 20);
        app.state.view.terminal_area = Rect::new(4, 0, 80, 20);

        let toggle = crate::ui::collapsed_sidebar_toggle_rect(app.state.view.sidebar_rect);
        app.handle_mouse(mouse(
            MouseEventKind::Down(MouseButton::Left),
            toggle.x,
            toggle.y,
        ));

        assert!(!app.state.sidebar_collapsed);
    }

    #[test]
    fn clicking_expanded_sidebar_toggle_collapses_sidebar() {
        let mut app = app_for_mouse_test();
        app.state.sidebar_collapsed = false;
        app.state.view.sidebar_rect = Rect::new(0, 0, 26, 20);
        app.state.view.terminal_area = Rect::new(26, 0, 80, 20);

        let toggle = crate::ui::expanded_sidebar_toggle_rect(app.state.view.sidebar_rect);
        app.handle_mouse(mouse(
            MouseEventKind::Down(MouseButton::Left),
            toggle.x,
            toggle.y,
        ));

        assert!(app.state.sidebar_collapsed);
        assert!(app.state.drag.is_none());
    }

    #[test]
    fn expanded_sidebar_has_no_workspace_rows() {
        let mut app = app_for_mouse_test();
        app.state.sessions = vec![Workspace::test_new("a"), Workspace::test_new("b")];
        app.state.active_session = Some(0);
        app.state.selected_session = 0;
        crate::ui::compute_view(&mut app.state, Rect::new(0, 0, 106, 20));

        let sidebar = app.state.view.sidebar_rect;
        assert!(app.state.pane_panel_rect().height >= sidebar.height.saturating_sub(2));
    }

    #[test]
    fn wheel_over_sidebar_outside_pane_panel_does_not_select_sessions() {
        let mut app = app_for_mouse_test();
        app.state.sessions = vec![
            Workspace::test_new("main"),
            Workspace::test_new("normal"),
            Workspace::test_new("issue"),
        ];
        app.state.active_session = Some(0);
        app.state.selected_session = 0;
        app.state.mode = Mode::Navigate;
        crate::ui::compute_view(&mut app.state, Rect::new(0, 0, 106, 30));
        let footer = app.state.sidebar_footer_rect();

        app.handle_mouse(mouse(MouseEventKind::ScrollDown, footer.x + 1, footer.y));

        assert_eq!(app.state.selected_session, 0);
    }

    #[test]
    fn dragging_expanded_sidebar_body_does_not_reorder_sessions() {
        let mut app = app_for_mouse_test();
        app.state.sessions = vec![
            Workspace::test_new("a"),
            Workspace::test_new("b"),
            Workspace::test_new("c"),
        ];
        app.state.active_session = Some(1);
        app.state.selected_session = 2;
        crate::ui::compute_view(&mut app.state, Rect::new(0, 0, 106, 20));
        let detail_area = app.state.pane_panel_rect();
        let source_row = detail_area.y + 1;
        let target_row = source_row.saturating_sub(1);

        app.handle_mouse(mouse(
            MouseEventKind::Down(MouseButton::Left),
            2,
            source_row,
        ));
        app.handle_mouse(mouse(
            MouseEventKind::Drag(MouseButton::Left),
            2,
            target_row,
        ));
        assert!(app.state.drag.is_none());
        app.handle_mouse(mouse(MouseEventKind::Up(MouseButton::Left), 2, target_row));

        let names: Vec<_> = app
            .state
            .sessions
            .iter()
            .map(|ws| ws.display_name())
            .collect();
        assert_eq!(names, vec!["a", "b", "c"]);
        assert_eq!(app.state.active_session, Some(1));
        assert_eq!(app.state.selected_session, 2);
    }

    #[test]
    fn clicking_tab_scroll_button_reveals_hidden_tabs_without_renaming() {
        let mut app = app_for_mouse_test();
        let mut ws = Workspace::test_new("test");
        ws.test_add_tab(Some("logs"));
        ws.test_add_tab(Some("review"));
        ws.test_add_tab(Some("ops"));
        ws.test_add_tab(Some("notes"));
        app.state.sessions = vec![ws];
        app.state.active_session = Some(0);
        app.state.selected_session = 0;
        crate::ui::compute_view(&mut app.state, Rect::new(0, 0, 65, 20));

        let right = app.state.view.tab_scroll_right_hit_area;
        assert!(right.width > 0);

        app.handle_mouse(mouse(
            MouseEventKind::Down(MouseButton::Left),
            right.x + 1,
            right.y,
        ));

        assert_eq!(app.state.tab_scroll, 1);
        assert!(!app.state.tab_scroll_follow_active);
        assert_eq!(app.state.sessions[0].active_tab, 0);
        assert_eq!(app.state.view.tab_hit_areas[0].width, 0);
        assert!(app.state.sessions[0].tabs[0].custom_name.is_none());
        assert_eq!(
            app.state.sessions[0].tabs[1].custom_name.as_deref(),
            Some("logs")
        );
    }

    #[test]
    fn clicking_last_visible_tab_at_right_edge_does_not_overscroll() {
        let mut app = app_for_mouse_test();
        let mut ws = Workspace::test_new("test");
        for name in [
            "one", "two", "three", "four", "five", "six", "seven", "eight",
        ] {
            ws.test_add_tab(Some(name));
        }
        app.state.sessions = vec![ws];
        app.state.active_session = Some(0);
        app.state.selected_session = 0;
        app.state.tab_scroll = usize::MAX;
        app.state.tab_scroll_follow_active = false;
        crate::ui::compute_view(&mut app.state, Rect::new(0, 0, 65, 20));

        let last_idx = app.state.sessions[0].tabs.len() - 1;
        let target = app.state.view.tab_hit_areas[last_idx];
        let clamped_scroll = app.state.tab_scroll;
        assert!(target.width > 0, "last tab should already be visible");

        app.handle_mouse(mouse(
            MouseEventKind::Down(MouseButton::Left),
            target.x + 1,
            target.y,
        ));
        app.handle_mouse(mouse(
            MouseEventKind::Up(MouseButton::Left),
            target.x + 1,
            target.y,
        ));

        assert_eq!(app.state.sessions[0].active_tab, last_idx);
        assert_eq!(app.state.tab_scroll, clamped_scroll);
        assert!(app.state.view.tab_hit_areas[last_idx].width > 0);
    }

    #[test]
    fn dragging_tab_reorders_auto_and_custom_names_without_materializing_numbers() {
        let mut app = app_for_mouse_test();
        let mut ws = Workspace::test_new("test");
        ws.test_add_tab(Some("foo"));
        ws.test_add_tab(None);
        let moved_root = ws.tabs[0].root_pane;
        app.state.sessions = vec![ws];
        app.state.active_session = Some(0);
        app.state.selected_session = 0;
        crate::ui::compute_view(&mut app.state, Rect::new(0, 0, 106, 20));

        let source = app.state.view.tab_hit_areas[0];
        let last = app.state.view.tab_hit_areas[2];
        let drop_col = last.x + last.width;

        app.handle_mouse(mouse(
            MouseEventKind::Down(MouseButton::Left),
            source.x + 1,
            source.y,
        ));
        app.handle_mouse(mouse(
            MouseEventKind::Drag(MouseButton::Left),
            drop_col,
            source.y,
        ));
        assert!(matches!(
            app.state.drag.as_ref().map(|drag| &drag.target),
            Some(DragTarget::TabReorder {
                session_idx: 0,
                source_tab_idx: 0,
                insert_idx: Some(3),
            })
        ));
        app.handle_mouse(mouse(
            MouseEventKind::Up(MouseButton::Left),
            drop_col,
            source.y,
        ));

        let labels: Vec<_> = app.state.sessions[0]
            .tabs
            .iter()
            .map(|tab| tab.display_name())
            .collect();
        assert_eq!(labels, vec!["foo", "2", "3"]);
        assert_eq!(
            app.state.sessions[0].tabs[0].custom_name.as_deref(),
            Some("foo")
        );
        assert!(app.state.sessions[0].tabs[1].custom_name.is_none());
        assert!(app.state.sessions[0].tabs[2].custom_name.is_none());
        assert_eq!(app.state.sessions[0].tabs[2].root_pane, moved_root);
        assert_eq!(app.state.sessions[0].active_tab, 2);
    }

    #[test]
    fn dragging_sidebar_divider_sets_manual_width() {
        let mut app = app_for_mouse_test();

        app.handle_mouse(mouse(MouseEventKind::Down(MouseButton::Left), 25, 5));
        app.handle_mouse(mouse(MouseEventKind::Drag(MouseButton::Left), 30, 5));

        assert_eq!(app.state.sidebar_width, 31);
    }

    #[test]
    fn dragging_sidebar_bottom_divider_still_sets_manual_width() {
        let mut app = app_for_mouse_test();
        let divider_col = app.state.view.sidebar_rect.x + app.state.view.sidebar_rect.width - 1;
        let bottom_row = app.state.view.sidebar_rect.y + app.state.view.sidebar_rect.height - 1;

        app.handle_mouse(mouse(
            MouseEventKind::Down(MouseButton::Left),
            divider_col,
            bottom_row,
        ));
        app.handle_mouse(mouse(
            MouseEventKind::Drag(MouseButton::Left),
            divider_col + 5,
            bottom_row,
        ));

        assert_eq!(app.state.sidebar_width, 31);
    }

    #[test]
    fn dragging_past_max_clamps_to_configured_max() {
        let mut app = app_for_mouse_test();
        app.state.sidebar_max_width = 30;

        app.handle_mouse(mouse(MouseEventKind::Down(MouseButton::Left), 25, 5));
        app.handle_mouse(mouse(MouseEventKind::Drag(MouseButton::Left), 50, 5));

        assert_eq!(app.state.sidebar_width, 30);
    }

    #[test]
    fn dragging_below_min_clamps_to_configured_min() {
        let mut app = app_for_mouse_test();
        app.state.sidebar_min_width = 22;

        app.handle_mouse(mouse(MouseEventKind::Down(MouseButton::Left), 25, 5));
        app.handle_mouse(mouse(MouseEventKind::Drag(MouseButton::Left), 5, 5));

        assert_eq!(app.state.sidebar_width, 22);
    }

    #[test]
    fn dragging_inside_expanded_sidebar_does_not_change_section_split() {
        let mut app = app_for_mouse_test();
        let detail_area = app.state.pane_panel_rect();

        app.handle_mouse(mouse(
            MouseEventKind::Down(MouseButton::Left),
            detail_area.x + 1,
            detail_area.y + 1,
        ));
        app.handle_mouse(mouse(
            MouseEventKind::Drag(MouseButton::Left),
            detail_area.x + 1,
            detail_area.y + 4,
        ));

        assert!(app.state.drag.is_none());
    }

    #[test]
    fn double_clicking_sidebar_divider_resets_default_width() {
        let mut app = app_for_mouse_test();
        app.state.default_sidebar_width = 26;
        app.state.sidebar_width = 30;

        app.handle_mouse(mouse(MouseEventKind::Down(MouseButton::Left), 25, 5));
        app.handle_mouse(mouse(MouseEventKind::Up(MouseButton::Left), 25, 5));
        app.handle_mouse(mouse(MouseEventKind::Down(MouseButton::Left), 25, 5));

        assert_eq!(app.state.sidebar_width, 26);
        assert!(app.state.drag.is_none());
    }
}
