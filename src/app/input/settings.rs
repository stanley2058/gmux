use crossterm::event::{KeyCode, KeyEvent, KeyModifiers, MouseButton, MouseEvent, MouseEventKind};
use ratatui::layout::Rect;

use crate::{
    app::{
        settings_catalog::{self, SettingsRowAction},
        state::{AppState, SettingsEditField, SettingsEditState, SettingsPage},
        App, Mode,
    },
    config::ToastDelivery,
};

#[derive(Debug, Clone, PartialEq, Eq)]
// The shared `Save` verb is semantic: these actions persist settings.
#[allow(clippy::enum_variant_names)]
pub(super) enum SettingsAction {
    SaveTheme(String),
    SaveToastDelivery(ToastDelivery),
    SaveTopLevelBool {
        key: &'static str,
        value: bool,
        context: &'static str,
    },
    SaveSectionBool {
        section: &'static str,
        key: &'static str,
        value: bool,
        context: &'static str,
    },
    SaveSectionValue {
        section: &'static str,
        key: &'static str,
        value: String,
        context: &'static str,
    },
}

impl App {
    pub(crate) fn handle_settings_key(&mut self, key: KeyEvent) {
        if let Some(action) = update_settings_state(&mut self.state, key) {
            self.apply_settings_action(action);
        }
    }

    pub(super) fn apply_settings_action(&mut self, action: SettingsAction) {
        match action {
            SettingsAction::SaveTheme(name) => self.save_theme(&name),
            SettingsAction::SaveToastDelivery(delivery) => self.save_toast_delivery(delivery),
            SettingsAction::SaveTopLevelBool {
                key,
                value,
                context,
            } => self.save_top_level_bool(context, key, value),
            SettingsAction::SaveSectionBool {
                section,
                key,
                value,
                context,
            } => self.save_section_bool(context, section, key, value),
            SettingsAction::SaveSectionValue {
                section,
                key,
                value,
                context,
            } => self.save_section_value(context, section, key, &value),
        }
    }
}

fn cancel_settings(state: &mut AppState) {
    state.settings.edit = None;
    if let Some(palette) = state.settings.original_palette.take() {
        state.palette = palette;
    }
    if let Some(theme_name) = state.settings.original_theme.take() {
        state.theme_name = theme_name;
    }
    super::modal::leave_modal(state);
}

fn close_settings(state: &mut AppState) {
    state.settings.edit = None;
    state.settings.original_palette = None;
    state.settings.original_theme = None;
    super::modal::leave_modal(state);
}

fn go_back_or_close(state: &mut AppState) {
    if state.settings.edit.take().is_some() {
        return;
    }
    if let Some(parent) = state.settings.page.parent() {
        set_settings_page(state, parent);
    } else {
        cancel_settings(state);
    }
}

fn set_settings_page(state: &mut AppState, page: SettingsPage) {
    state.settings.page = page;
    let rows = settings_catalog::settings_rows(state);
    state.settings.list.selected = rows.iter().position(|row| row.selected).unwrap_or(0);
}

fn selected_row_action(state: &AppState) -> Option<SettingsRowAction> {
    settings_catalog::settings_rows(state)
        .get(state.settings.list.selected)
        .map(|row| row.action.clone())
}

fn apply_row_action(state: &mut AppState, action: SettingsRowAction) -> Option<SettingsAction> {
    match action {
        SettingsRowAction::Open(page) => {
            set_settings_page(state, page);
            None
        }
        SettingsRowAction::Edit(field) => {
            state.settings.edit = Some(SettingsEditState {
                field,
                input: settings_catalog::edit_field_initial_value(state, field),
                error: None,
            });
            None
        }
        SettingsRowAction::SaveTheme(name) => {
            state.settings.original_palette = None;
            state.settings.original_theme = None;
            set_settings_page(state, SettingsPage::Theme);
            Some(SettingsAction::SaveTheme(name))
        }
        SettingsRowAction::SaveToastDelivery(delivery) => {
            set_settings_page(state, SettingsPage::Notifications);
            Some(SettingsAction::SaveToastDelivery(delivery))
        }
        SettingsRowAction::SaveTopLevelBool {
            key,
            value,
            context,
        } => Some(SettingsAction::SaveTopLevelBool {
            key,
            value,
            context,
        }),
        SettingsRowAction::SaveSectionBool {
            section,
            key,
            value,
            context,
        } => Some(SettingsAction::SaveSectionBool {
            section,
            key,
            value,
            context,
        }),
        SettingsRowAction::SaveSectionValue {
            section,
            key,
            value,
            context,
        } => {
            if let Some(parent) = state.settings.page.parent() {
                set_settings_page(state, parent);
            }
            Some(SettingsAction::SaveSectionValue {
                section,
                key,
                value,
                context,
            })
        }
        SettingsRowAction::Readonly => None,
    }
}

pub(super) fn update_settings_state(state: &mut AppState, key: KeyEvent) -> Option<SettingsAction> {
    if state.settings.edit.is_some() {
        return update_settings_edit_state(state, key);
    }

    let row_count = settings_catalog::settings_rows(state).len();
    match key.code {
        KeyCode::Up | KeyCode::Char('k') => state.settings.list.move_prev(),
        KeyCode::Down | KeyCode::Char('j') => state.settings.list.move_next(row_count),
        KeyCode::Esc | KeyCode::Left | KeyCode::Char('h') => go_back_or_close(state),
        KeyCode::Enter | KeyCode::Char(' ') | KeyCode::Right | KeyCode::Char('l') => {
            if let Some(action) = selected_row_action(state) {
                return apply_row_action(state, action);
            }
        }
        _ => {
            if let Some(super::modal::ModalAction::Close) =
                super::modal::modal_action_from_key(&key, super::modal::SETTINGS_ACTIONS)
            {
                close_settings(state);
            }
        }
    }

    None
}

fn update_settings_edit_state(state: &mut AppState, key: KeyEvent) -> Option<SettingsAction> {
    match key.code {
        KeyCode::Esc => {
            state.settings.edit = None;
        }
        KeyCode::Enter => return save_edit_state(state),
        KeyCode::Backspace => {
            if let Some(edit) = &mut state.settings.edit {
                edit.input.pop();
                edit.error = None;
            }
        }
        KeyCode::Char(c) if key.modifiers.difference(KeyModifiers::SHIFT).is_empty() => {
            if let Some(edit) = &mut state.settings.edit {
                edit.input.push(c);
                edit.error = None;
            }
        }
        _ => {}
    }
    None
}

fn save_edit_state(state: &mut AppState) -> Option<SettingsAction> {
    let edit = state.settings.edit.clone()?;
    match edit_action(edit.field, &edit.input) {
        Ok(action) => {
            state.settings.edit = None;
            Some(action)
        }
        Err(error) => {
            if let Some(edit) = &mut state.settings.edit {
                edit.error = Some(error.to_string());
            }
            None
        }
    }
}

fn edit_action(field: SettingsEditField, input: &str) -> Result<SettingsAction, &'static str> {
    match field {
        SettingsEditField::PaneTerm => {
            let value = input.trim();
            if value.is_empty() {
                return Err("enter a TERM value");
            }
            Ok(save_value(
                "terminal",
                "term",
                toml_string(value),
                "pane TERM",
            ))
        }
        SettingsEditField::DefaultShell => Ok(save_value(
            "terminal",
            "default_shell",
            toml_string(input),
            "default shell",
        )),
        SettingsEditField::NewTerminalCwdPath => {
            let value = input.trim();
            if value.is_empty() {
                return Err("enter a path");
            }
            Ok(save_value(
                "terminal",
                "new_cwd",
                toml_string(value),
                "new terminal cwd",
            ))
        }
        SettingsEditField::SidebarWidth => {
            save_positive_u16(input, "ui", "sidebar_width", "sidebar width")
        }
        SettingsEditField::SidebarMinWidth => {
            save_positive_u16(input, "ui", "sidebar_min_width", "sidebar min width")
        }
        SettingsEditField::SidebarMaxWidth => {
            save_positive_u16(input, "ui", "sidebar_max_width", "sidebar max width")
        }
        SettingsEditField::MobileWidthThreshold => save_positive_u16(
            input,
            "ui",
            "mobile_width_threshold",
            "mobile width threshold",
        ),
        SettingsEditField::MouseScrollLines => {
            save_positive_usize(input, "ui", "mouse_scroll_lines", "mouse scroll lines")
        }
        SettingsEditField::ScrollbackLimitBytes => save_positive_usize(
            input,
            "advanced",
            "scrollback_limit_bytes",
            "scrollback limit bytes",
        ),
    }
}

fn save_positive_u16(
    input: &str,
    section: &'static str,
    key: &'static str,
    context: &'static str,
) -> Result<SettingsAction, &'static str> {
    let value = input
        .trim()
        .parse::<u16>()
        .map_err(|_| "enter a whole number")?;
    if value == 0 {
        return Err("enter a number greater than zero");
    }
    Ok(save_value(section, key, value.to_string(), context))
}

fn save_positive_usize(
    input: &str,
    section: &'static str,
    key: &'static str,
    context: &'static str,
) -> Result<SettingsAction, &'static str> {
    let value = input
        .trim()
        .parse::<usize>()
        .map_err(|_| "enter a whole number")?;
    if value == 0 {
        return Err("enter a number greater than zero");
    }
    Ok(save_value(section, key, value.to_string(), context))
}

fn save_value(
    section: &'static str,
    key: &'static str,
    value: String,
    context: &'static str,
) -> SettingsAction {
    SettingsAction::SaveSectionValue {
        section,
        key,
        value,
        context,
    }
}

fn toml_string(value: &str) -> String {
    format!("\"{}\"", value.replace('\\', "\\\\").replace('"', "\\\""))
}

pub(crate) fn open_settings(state: &mut AppState) {
    open_settings_at_page(state, SettingsPage::Main);
}

pub(crate) fn open_settings_at_page(state: &mut AppState, page: SettingsPage) {
    state.settings.original_palette = Some(state.palette.clone());
    state.settings.original_theme = Some(state.theme_name.clone());
    state.settings.edit = None;
    set_settings_page(state, page);
    state.mode = Mode::Settings;
}

impl AppState {
    fn settings_popup_rect(&self) -> Rect {
        crate::ui::centered_popup_rect(self.screen_rect(), 82, 24).unwrap_or_default()
    }

    fn settings_inner_rect(&self) -> Rect {
        let popup = self.settings_popup_rect();
        Rect::new(
            popup.x + 1,
            popup.y + 1,
            popup.width.saturating_sub(2),
            popup.height.saturating_sub(2),
        )
    }

    pub(crate) fn settings_content_rect(&self) -> Rect {
        let inner = self.settings_inner_rect();
        crate::ui::modal_stack_areas(inner, 3, 2, 0, 1).content
    }

    fn settings_list_index_at(&self, col: u16, row: u16) -> Option<usize> {
        if self.settings.edit.is_some() {
            return None;
        }
        let area = self.settings_content_rect();
        if row < area.y || row >= area.y + area.height || col < area.x || col >= area.x + area.width
        {
            return None;
        }
        let idx = (row - area.y) as usize;
        (idx < settings_catalog::settings_rows(self).len()).then_some(idx)
    }

    pub(super) fn handle_settings_mouse(&mut self, mouse: MouseEvent) -> Option<SettingsAction> {
        match mouse.kind {
            MouseEventKind::Down(MouseButton::Left) => {
                if let Some(idx) = self.settings_list_index_at(mouse.column, mouse.row) {
                    self.settings.list.select(idx);
                    return selected_row_action(self)
                        .and_then(|action| apply_row_action(self, action));
                }

                let inner = self.settings_inner_rect();
                let show_primary = crate::ui::settings_show_primary_action(self);
                let (apply, close) =
                    crate::ui::settings_button_rects(inner, self.settings.page, show_primary);
                let mut buttons = vec![(close, super::modal::ModalAction::Close)];
                if let Some(apply) = apply {
                    buttons.insert(0, (apply, super::modal::ModalAction::Apply));
                }
                match super::modal::modal_action_from_buttons(mouse.column, mouse.row, &buttons) {
                    Some(super::modal::ModalAction::Apply) => {
                        if self.settings.edit.is_some() {
                            save_edit_state(self)
                        } else {
                            selected_row_action(self)
                                .and_then(|action| apply_row_action(self, action))
                        }
                    }
                    Some(super::modal::ModalAction::Close) => {
                        close_settings(self);
                        None
                    }
                    _ => {
                        close_settings(self);
                        None
                    }
                }
            }
            _ => None,
        }
    }
}

#[cfg(test)]
mod tests {
    use crossterm::event::{KeyCode, KeyEvent, KeyModifiers, MouseEventKind};

    use super::super::{app_for_mouse_test, mouse, state_with_workspaces};
    use super::*;

    #[test]
    fn settings_escape_backs_out_of_submenu() {
        let mut state = state_with_workspaces(&["test"]);
        open_settings_at_page(&mut state, SettingsPage::Experiments);

        update_settings_state(
            &mut state,
            KeyEvent::new(KeyCode::Esc, KeyModifiers::empty()),
        );

        assert_eq!(state.mode, Mode::Settings);
        assert_eq!(state.settings.page, SettingsPage::Main);
    }

    #[test]
    fn settings_experiments_toggles_pane_history() {
        let mut state = state_with_workspaces(&["test"]);
        state.pane_history_persistence = false;
        open_settings_at_page(&mut state, SettingsPage::Experiments);
        state.settings.list.selected = 2;

        let action = update_settings_state(
            &mut state,
            KeyEvent::new(KeyCode::Enter, KeyModifiers::empty()),
        );

        assert_eq!(
            action,
            Some(SettingsAction::SaveSectionBool {
                section: "experimental",
                key: "pane_history",
                value: true,
                context: "pane screen history",
            })
        );
        assert_eq!(state.mode, Mode::Settings);
    }

    #[test]
    fn settings_text_editor_saves_default_shell() {
        let mut state = state_with_workspaces(&["test"]);
        open_settings_at_page(&mut state, SettingsPage::Terminal);
        state.settings.list.selected = 1;

        let action = update_settings_state(
            &mut state,
            KeyEvent::new(KeyCode::Enter, KeyModifiers::empty()),
        );
        assert_eq!(action, None);
        assert!(state.settings.edit.is_some());

        if let Some(edit) = &mut state.settings.edit {
            edit.input = "/bin/zsh".to_string();
        }
        let action = update_settings_state(
            &mut state,
            KeyEvent::new(KeyCode::Enter, KeyModifiers::empty()),
        );

        assert_eq!(
            action,
            Some(SettingsAction::SaveSectionValue {
                section: "terminal",
                key: "default_shell",
                value: "\"/bin/zsh\"".to_string(),
                context: "default shell",
            })
        );
        assert!(state.settings.edit.is_none());
    }

    #[test]
    fn settings_number_editor_rejects_zero() {
        let mut state = state_with_workspaces(&["test"]);
        open_settings_at_page(&mut state, SettingsPage::Mouse);
        state.settings.list.selected = 2;

        update_settings_state(
            &mut state,
            KeyEvent::new(KeyCode::Enter, KeyModifiers::empty()),
        );
        if let Some(edit) = &mut state.settings.edit {
            edit.input = "0".to_string();
        }

        let action = update_settings_state(
            &mut state,
            KeyEvent::new(KeyCode::Enter, KeyModifiers::empty()),
        );

        assert_eq!(action, None);
        assert_eq!(
            state
                .settings
                .edit
                .as_ref()
                .and_then(|edit| edit.error.as_deref()),
            Some("enter a number greater than zero")
        );
    }

    #[test]
    fn settings_mouse_click_toggles_switch_ascii_input_source_row() {
        let mut app = app_for_mouse_test();
        app.state.switch_ascii_input_source_in_prefix = false;
        open_settings_at_page(&mut app.state, SettingsPage::Experiments);

        let area = app.state.settings_content_rect();
        let action = app.state.handle_settings_mouse(mouse(
            MouseEventKind::Down(crossterm::event::MouseButton::Left),
            area.x + 2,
            area.y + 5,
        ));

        assert_eq!(
            action,
            Some(SettingsAction::SaveSectionBool {
                section: "experimental",
                key: "switch_ascii_input_source_in_prefix",
                value: true,
                context: "prefix ascii input source",
            })
        );
        assert_eq!(app.state.settings.list.selected, 5);
    }
}
