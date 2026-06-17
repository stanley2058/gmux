use crate::app::state::{AppState, PanePanelScope, SettingsEditField, SettingsPage, THEME_NAMES};
use crate::config::{NewTerminalCwdConfig, ShellModeConfig, ToastDelivery};

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum SettingsRowAction {
    Open(SettingsPage),
    Edit(SettingsEditField),
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
    Readonly,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct SettingsRow {
    pub label: String,
    pub value: Option<String>,
    pub hint: &'static str,
    pub selected: bool,
    pub readonly: bool,
    pub action: SettingsRowAction,
}

impl SettingsRow {
    fn category(label: &'static str, page: SettingsPage) -> Self {
        Self {
            label: label.to_string(),
            value: None,
            hint: "",
            selected: false,
            readonly: false,
            action: SettingsRowAction::Open(page),
        }
    }

    fn open(
        label: &'static str,
        value: impl Into<String>,
        hint: &'static str,
        page: SettingsPage,
    ) -> Self {
        Self {
            label: label.to_string(),
            value: Some(value.into()),
            hint,
            selected: false,
            readonly: false,
            action: SettingsRowAction::Open(page),
        }
    }

    fn edit(
        label: &'static str,
        value: impl Into<String>,
        hint: &'static str,
        field: SettingsEditField,
    ) -> Self {
        Self {
            label: label.to_string(),
            value: Some(value.into()),
            hint,
            selected: false,
            readonly: false,
            action: SettingsRowAction::Edit(field),
        }
    }

    fn readonly(label: &'static str, value: impl Into<String>) -> Self {
        Self {
            label: label.to_string(),
            value: Some(value.into()),
            hint: "edit config.toml",
            selected: false,
            readonly: true,
            action: SettingsRowAction::Readonly,
        }
    }

    fn top_bool(
        label: &'static str,
        enabled: bool,
        key: &'static str,
        context: &'static str,
    ) -> Self {
        Self {
            label: label.to_string(),
            value: Some(on_off(enabled)),
            hint: "next launch",
            selected: false,
            readonly: false,
            action: SettingsRowAction::SaveTopLevelBool {
                key,
                value: !enabled,
                context,
            },
        }
    }

    fn section_bool(
        label: &'static str,
        enabled: bool,
        hint: &'static str,
        section: &'static str,
        key: &'static str,
        context: &'static str,
    ) -> Self {
        Self {
            label: label.to_string(),
            value: Some(on_off(enabled)),
            hint,
            selected: false,
            readonly: false,
            action: SettingsRowAction::SaveSectionBool {
                section,
                key,
                value: !enabled,
                context,
            },
        }
    }

    fn choice(
        label: &'static str,
        selected: bool,
        hint: &'static str,
        action: SettingsRowAction,
    ) -> Self {
        Self {
            label: label.to_string(),
            value: None,
            hint,
            selected,
            readonly: false,
            action,
        }
    }
}

pub(crate) fn settings_rows(state: &AppState) -> Vec<SettingsRow> {
    match state.settings.page {
        SettingsPage::Main => main_rows(),
        SettingsPage::Theme => theme_rows(state),
        SettingsPage::ThemePicker => theme_picker_rows(state),
        SettingsPage::ThemeCustom => theme_custom_rows(),
        SettingsPage::Notifications => notifications_rows(state),
        SettingsPage::ToastDelivery => toast_delivery_rows(state),
        SettingsPage::Terminal => terminal_rows(state),
        SettingsPage::ShellMode => shell_mode_rows(state),
        SettingsPage::NewTerminalCwd => new_terminal_cwd_rows(state),
        SettingsPage::Interface => interface_rows(state),
        SettingsPage::PanePanelScope => pane_panel_scope_rows(state),
        SettingsPage::Mouse => mouse_rows(state),
        SettingsPage::RightClickPassthroughModifier => right_click_modifier_rows(state),
        SettingsPage::Remote => remote_rows(state),
        SettingsPage::Advanced => advanced_rows(state),
        SettingsPage::Experiments => experiments_rows(state),
        SettingsPage::CjkImeCursorShape => cjk_ime_cursor_shape_rows(state),
    }
}

pub(crate) fn edit_field_label(field: SettingsEditField) -> &'static str {
    match field {
        SettingsEditField::PaneTerm => "pane TERM",
        SettingsEditField::TerminalEditor => "editor",
        SettingsEditField::TerminalPager => "pager",
        SettingsEditField::DefaultShell => "default shell",
        SettingsEditField::NewTerminalCwdPath => "new terminal cwd path",
        SettingsEditField::SidebarWidth => "sidebar width",
        SettingsEditField::SidebarMinWidth => "sidebar min width",
        SettingsEditField::SidebarMaxWidth => "sidebar max width",
        SettingsEditField::MobileWidthThreshold => "mobile width threshold",
        SettingsEditField::MouseScrollLines => "mouse scroll lines",
        SettingsEditField::ScrollbackLimitBytes => "scrollback limit bytes",
    }
}

pub(crate) fn edit_field_initial_value(state: &AppState, field: SettingsEditField) -> String {
    match field {
        SettingsEditField::PaneTerm => state.pane_term.clone(),
        SettingsEditField::TerminalEditor => state.terminal_editor.clone(),
        SettingsEditField::TerminalPager => state.terminal_pager.clone(),
        SettingsEditField::DefaultShell => state.default_shell.clone(),
        SettingsEditField::NewTerminalCwdPath => match &state.new_terminal_cwd {
            NewTerminalCwdConfig::Path(path) => path.clone(),
            _ => String::new(),
        },
        SettingsEditField::SidebarWidth => state.default_sidebar_width.to_string(),
        SettingsEditField::SidebarMinWidth => state.sidebar_min_width.to_string(),
        SettingsEditField::SidebarMaxWidth => state.sidebar_max_width.to_string(),
        SettingsEditField::MobileWidthThreshold => state.mobile_width_threshold.to_string(),
        SettingsEditField::MouseScrollLines => state.mouse_scroll_lines.to_string(),
        SettingsEditField::ScrollbackLimitBytes => state.pane_scrollback_limit_bytes.to_string(),
    }
}

fn main_rows() -> Vec<SettingsRow> {
    vec![
        SettingsRow::category("theme", SettingsPage::Theme),
        SettingsRow::category("notifications", SettingsPage::Notifications),
        SettingsRow::category("terminal", SettingsPage::Terminal),
        SettingsRow::category("interface", SettingsPage::Interface),
        SettingsRow::category("mouse", SettingsPage::Mouse),
        SettingsRow::category("remote", SettingsPage::Remote),
        SettingsRow::category("advanced", SettingsPage::Advanced),
        SettingsRow::category("experiments", SettingsPage::Experiments),
    ]
}

fn theme_rows(state: &AppState) -> Vec<SettingsRow> {
    vec![
        SettingsRow::open(
            "theme",
            &state.theme_name,
            "live",
            SettingsPage::ThemePicker,
        ),
        SettingsRow::open(
            "custom colors",
            "readonly",
            "edit config.toml",
            SettingsPage::ThemeCustom,
        ),
    ]
}

fn theme_picker_rows(state: &AppState) -> Vec<SettingsRow> {
    THEME_NAMES
        .iter()
        .map(|name| {
            let selected = normalize_name(name) == normalize_name(&state.theme_name);
            SettingsRow::choice(
                name,
                selected,
                "live",
                SettingsRowAction::SaveTheme((*name).to_string()),
            )
        })
        .collect()
}

fn theme_custom_rows() -> Vec<SettingsRow> {
    [
        "accent",
        "panel_bg",
        "surface0",
        "surface1",
        "surface_dim",
        "overlay0",
        "overlay1",
        "text",
        "subtext0",
        "mauve",
        "green",
        "yellow",
        "red",
        "blue",
        "teal",
        "peach",
    ]
    .into_iter()
    .map(|label| SettingsRow::readonly(label, "readonly"))
    .collect()
}

fn notifications_rows(state: &AppState) -> Vec<SettingsRow> {
    vec![SettingsRow::open(
        "notification popups",
        toast_delivery_label(state.toast_delivery()),
        "live",
        SettingsPage::ToastDelivery,
    )]
}

fn toast_delivery_rows(state: &AppState) -> Vec<SettingsRow> {
    [
        ("off", ToastDelivery::Off),
        ("inside gmux", ToastDelivery::Gmux),
        ("via terminal", ToastDelivery::Terminal),
        ("via system", ToastDelivery::System),
    ]
    .into_iter()
    .map(|(label, delivery)| {
        SettingsRow::choice(
            label,
            state.toast_delivery() == delivery,
            "live",
            SettingsRowAction::SaveToastDelivery(delivery),
        )
    })
    .collect()
}

fn terminal_rows(state: &AppState) -> Vec<SettingsRow> {
    vec![
        SettingsRow::edit(
            "pane TERM",
            &state.pane_term,
            "new panes",
            SettingsEditField::PaneTerm,
        ),
        SettingsRow::edit(
            "editor",
            opener_setting_label(&state.terminal_editor, "EDITOR", "vi"),
            "scrollback",
            SettingsEditField::TerminalEditor,
        ),
        SettingsRow::edit(
            "pager",
            opener_setting_label(&state.terminal_pager, "PAGER", "less -R"),
            "scrollback",
            SettingsEditField::TerminalPager,
        ),
        SettingsRow::edit(
            "default shell",
            if state.default_shell.is_empty() {
                "SHELL"
            } else {
                &state.default_shell
            },
            "new panes",
            SettingsEditField::DefaultShell,
        ),
        SettingsRow::open(
            "shell mode",
            shell_mode_label(state.shell_mode),
            "new panes",
            SettingsPage::ShellMode,
        ),
        SettingsRow::open(
            "new terminal cwd",
            new_terminal_cwd_label(&state.new_terminal_cwd),
            "new panes",
            SettingsPage::NewTerminalCwd,
        ),
        SettingsRow::section_bool(
            "restore running processes",
            state.restore_processes,
            "next restore",
            "terminal",
            "restore_processes",
            "process restore setting",
        ),
    ]
}

fn shell_mode_rows(state: &AppState) -> Vec<SettingsRow> {
    [
        ("auto", ShellModeConfig::Auto),
        ("login", ShellModeConfig::Login),
        ("non-login", ShellModeConfig::NonLogin),
    ]
    .into_iter()
    .map(|(label, mode)| {
        SettingsRow::choice(
            label,
            state.shell_mode == mode,
            "new panes",
            SettingsRowAction::SaveSectionValue {
                section: "terminal",
                key: "shell_mode",
                value: quoted(shell_mode_config_value(mode)),
                context: "shell mode",
            },
        )
    })
    .collect()
}

fn new_terminal_cwd_rows(state: &AppState) -> Vec<SettingsRow> {
    let cwd = &state.new_terminal_cwd;
    vec![
        SettingsRow::choice(
            "follow focused pane",
            matches!(cwd, NewTerminalCwdConfig::Follow),
            "new panes",
            save_terminal_cwd("follow"),
        ),
        SettingsRow::choice(
            "home directory",
            matches!(cwd, NewTerminalCwdConfig::Home),
            "new panes",
            save_terminal_cwd("home"),
        ),
        SettingsRow::choice(
            "current gmux cwd",
            matches!(cwd, NewTerminalCwdConfig::Current),
            "new panes",
            save_terminal_cwd("current"),
        ),
        SettingsRow {
            label: "custom path".to_string(),
            value: Some(match cwd {
                NewTerminalCwdConfig::Path(path) => path.clone(),
                _ => "unset".to_string(),
            }),
            hint: "new panes",
            selected: matches!(cwd, NewTerminalCwdConfig::Path(_)),
            readonly: false,
            action: SettingsRowAction::Edit(SettingsEditField::NewTerminalCwdPath),
        },
    ]
}

fn opener_setting_label(configured: &str, env_var: &str, fallback: &str) -> String {
    if !configured.is_empty() {
        return configured.to_string();
    }
    match std::env::var(env_var) {
        Ok(value) if !value.is_empty() => format!("${env_var}: {value}"),
        _ => fallback.to_string(),
    }
}

fn interface_rows(state: &AppState) -> Vec<SettingsRow> {
    vec![
        SettingsRow::edit(
            "sidebar width",
            state.default_sidebar_width.to_string(),
            "live",
            SettingsEditField::SidebarWidth,
        ),
        SettingsRow::edit(
            "sidebar min width",
            state.sidebar_min_width.to_string(),
            "live",
            SettingsEditField::SidebarMinWidth,
        ),
        SettingsRow::edit(
            "sidebar max width",
            state.sidebar_max_width.to_string(),
            "live",
            SettingsEditField::SidebarMaxWidth,
        ),
        SettingsRow::edit(
            "mobile width threshold",
            state.mobile_width_threshold.to_string(),
            "live",
            SettingsEditField::MobileWidthThreshold,
        ),
        SettingsRow::open(
            "pane panel scope",
            pane_panel_scope_label(state.pane_panel_scope),
            "live",
            SettingsPage::PanePanelScope,
        ),
        SettingsRow::section_bool(
            "confirm close",
            state.confirm_close,
            "live",
            "ui",
            "confirm_close",
            "confirm close setting",
        ),
        SettingsRow::section_bool(
            "prompt new tab name",
            state.prompt_new_tab_name,
            "live",
            "ui",
            "prompt_new_tab_name",
            "new tab prompt setting",
        ),
        SettingsRow::section_bool(
            "redraw on focus gained",
            state.redraw_on_focus_gained,
            "live",
            "ui",
            "redraw_on_focus_gained",
            "focus redraw setting",
        ),
        SettingsRow::top_bool(
            "show onboarding on next launch",
            state.show_onboarding_on_next_launch,
            "onboarding",
            "onboarding setting",
        ),
        SettingsRow::readonly("legacy accent", "readonly"),
    ]
}

fn pane_panel_scope_rows(state: &AppState) -> Vec<SettingsRow> {
    [
        ("current session", PanePanelScope::Current, "current"),
        ("all sessions", PanePanelScope::All, "all"),
    ]
    .into_iter()
    .map(|(label, scope, value)| {
        SettingsRow::choice(
            label,
            state.pane_panel_scope == scope,
            "live",
            SettingsRowAction::SaveSectionValue {
                section: "ui",
                key: "pane_panel_scope",
                value: quoted(value),
                context: "pane panel scope",
            },
        )
    })
    .collect()
}

fn mouse_rows(state: &AppState) -> Vec<SettingsRow> {
    vec![
        SettingsRow::section_bool(
            "mouse capture",
            state.mouse_capture,
            "live",
            "ui",
            "mouse_capture",
            "mouse capture setting",
        ),
        SettingsRow::open(
            "right-click passthrough modifier",
            right_click_modifier_label(state),
            "live",
            SettingsPage::RightClickPassthroughModifier,
        ),
        SettingsRow::edit(
            "mouse scroll lines",
            state.mouse_scroll_lines.to_string(),
            "live",
            SettingsEditField::MouseScrollLines,
        ),
    ]
}

fn right_click_modifier_rows(state: &AppState) -> Vec<SettingsRow> {
    [
        ("off", ""),
        ("ctrl", "ctrl"),
        ("alt", "alt"),
        ("super", "super"),
        ("meta", "meta"),
        ("hyper", "hyper"),
        ("ctrl+alt", "ctrl+alt"),
        ("alt+super", "alt+super"),
    ]
    .into_iter()
    .map(|(label, value)| {
        SettingsRow::choice(
            label,
            right_click_modifier_label(state) == label,
            "live",
            SettingsRowAction::SaveSectionValue {
                section: "ui",
                key: "right_click_passthrough_modifier",
                value: quoted(value),
                context: "right-click passthrough modifier",
            },
        )
    })
    .collect()
}

fn remote_rows(state: &AppState) -> Vec<SettingsRow> {
    vec![SettingsRow::section_bool(
        "manage ssh config",
        state.remote_manage_ssh_config,
        "remote only",
        "remote",
        "manage_ssh_config",
        "remote ssh config setting",
    )]
}

fn advanced_rows(state: &AppState) -> Vec<SettingsRow> {
    vec![SettingsRow::edit(
        "scrollback limit bytes",
        state.pane_scrollback_limit_bytes.to_string(),
        "new panes",
        SettingsEditField::ScrollbackLimitBytes,
    )]
}

fn experiments_rows(state: &AppState) -> Vec<SettingsRow> {
    vec![
        SettingsRow::section_bool(
            "allow nested gmux",
            state.allow_nested_gmux,
            "next launch",
            "experimental",
            "allow_nested",
            "nested gmux setting",
        ),
        SettingsRow::section_bool(
            "kitty graphics",
            state.kitty_graphics_enabled,
            "live",
            "experimental",
            "kitty_graphics",
            "kitty graphics setting",
        ),
        SettingsRow::section_bool(
            "pane screen history",
            state.pane_history_persistence,
            "live",
            "experimental",
            "pane_history",
            "pane screen history",
        ),
        SettingsRow::section_bool(
            "reveal hidden cursor for CJK IME",
            state.reveal_hidden_cursor_for_cjk_ime,
            "live",
            "experimental",
            "reveal_hidden_cursor_for_cjk_ime",
            "CJK IME cursor setting",
        ),
        SettingsRow::open(
            "CJK IME cursor shape",
            cjk_ime_cursor_shape_label(state.cjk_ime_cursor_shape),
            "live",
            SettingsPage::CjkImeCursorShape,
        ),
        SettingsRow::section_bool(
            "switch to ascii input source in prefix (macOS)",
            state.switch_ascii_input_source_in_prefix,
            "live",
            "experimental",
            "switch_ascii_input_source_in_prefix",
            "prefix ascii input source",
        ),
    ]
}

fn cjk_ime_cursor_shape_rows(state: &AppState) -> Vec<SettingsRow> {
    [
        ("block", 1),
        ("steady_block", 2),
        ("underline", 3),
        ("steady_underline", 4),
        ("bar", 5),
        ("steady_bar", 6),
    ]
    .into_iter()
    .map(|(label, shape)| {
        SettingsRow::choice(
            label,
            state.cjk_ime_cursor_shape == shape,
            "live",
            SettingsRowAction::SaveSectionValue {
                section: "experimental",
                key: "cjk_ime_cursor_shape",
                value: quoted(label),
                context: "CJK IME cursor shape",
            },
        )
    })
    .collect()
}

fn save_terminal_cwd(value: &'static str) -> SettingsRowAction {
    SettingsRowAction::SaveSectionValue {
        section: "terminal",
        key: "new_cwd",
        value: quoted(value),
        context: "new terminal cwd",
    }
}

fn normalize_name(name: &str) -> String {
    name.to_lowercase().replace([' ', '_'], "-")
}

fn on_off(enabled: bool) -> String {
    if enabled { "on" } else { "off" }.to_string()
}

fn quoted(value: &str) -> String {
    format!("\"{}\"", value.replace('\\', "\\\\").replace('"', "\\\""))
}

fn toast_delivery_label(delivery: ToastDelivery) -> &'static str {
    match delivery {
        ToastDelivery::Off => "off",
        ToastDelivery::Gmux => "inside gmux",
        ToastDelivery::Terminal => "via terminal",
        ToastDelivery::System => "via system",
    }
}

fn shell_mode_label(mode: ShellModeConfig) -> &'static str {
    match mode {
        ShellModeConfig::Auto => "auto",
        ShellModeConfig::Login => "login",
        ShellModeConfig::NonLogin => "non-login",
    }
}

fn shell_mode_config_value(mode: ShellModeConfig) -> &'static str {
    match mode {
        ShellModeConfig::Auto => "auto",
        ShellModeConfig::Login => "login",
        ShellModeConfig::NonLogin => "non_login",
    }
}

fn new_terminal_cwd_label(cwd: &NewTerminalCwdConfig) -> String {
    match cwd {
        NewTerminalCwdConfig::Follow => "follow".to_string(),
        NewTerminalCwdConfig::Home => "home".to_string(),
        NewTerminalCwdConfig::Current => "current".to_string(),
        NewTerminalCwdConfig::Path(path) => path.clone(),
    }
}

fn pane_panel_scope_label(scope: PanePanelScope) -> &'static str {
    match scope {
        PanePanelScope::Current => "current",
        PanePanelScope::All => "all",
    }
}

fn right_click_modifier_label(state: &AppState) -> String {
    let Some(modifiers) = state.right_click_passthrough_modifiers else {
        return "off".to_string();
    };
    let mut parts = Vec::new();
    if modifiers.contains(crossterm::event::KeyModifiers::CONTROL) {
        parts.push("ctrl");
    }
    if modifiers.contains(crossterm::event::KeyModifiers::ALT) {
        parts.push("alt");
    }
    if modifiers.contains(crossterm::event::KeyModifiers::SUPER) {
        parts.push("super");
    }
    if modifiers.contains(crossterm::event::KeyModifiers::META) {
        parts.push("meta");
    }
    if modifiers.contains(crossterm::event::KeyModifiers::HYPER) {
        parts.push("hyper");
    }
    if parts.is_empty() {
        "off".to_string()
    } else {
        parts.join("+")
    }
}

fn cjk_ime_cursor_shape_label(shape: u8) -> &'static str {
    match shape {
        1 => "block",
        2 => "steady_block",
        3 => "underline",
        4 => "steady_underline",
        5 => "bar",
        6 => "steady_bar",
        _ => "steady_block",
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::app::state::SettingsPage;

    #[test]
    fn settings_main_skips_keybindings() {
        let mut state = AppState::test_new();
        state.settings.page = SettingsPage::Main;

        let labels: Vec<String> = settings_rows(&state)
            .into_iter()
            .map(|row| row.label)
            .collect();

        assert!(!labels.iter().any(|label| label.contains("key")));
        assert!(labels.contains(&"theme".to_string()));
        assert!(labels.contains(&"experiments".to_string()));
    }

    #[test]
    fn theme_custom_colors_are_readonly() {
        let mut state = AppState::test_new();
        state.settings.page = SettingsPage::ThemeCustom;

        let rows = settings_rows(&state);

        assert!(rows.iter().any(|row| row.label == "accent"));
        assert!(rows.iter().all(|row| row.readonly));
    }

    #[test]
    fn scalar_settings_catalog_covers_non_keybinding_config_scope() {
        let mut state = AppState::test_new();
        let expected = [
            (SettingsPage::Theme, &["theme", "custom colors"] as &[&str]),
            (SettingsPage::Notifications, &["notification popups"]),
            (
                SettingsPage::Terminal,
                &[
                    "pane TERM",
                    "default shell",
                    "shell mode",
                    "new terminal cwd",
                    "restore running processes",
                ],
            ),
            (
                SettingsPage::Interface,
                &[
                    "sidebar width",
                    "sidebar min width",
                    "sidebar max width",
                    "mobile width threshold",
                    "pane panel scope",
                    "confirm close",
                    "prompt new tab name",
                    "redraw on focus gained",
                    "show onboarding on next launch",
                    "legacy accent",
                ],
            ),
            (
                SettingsPage::Mouse,
                &[
                    "mouse capture",
                    "right-click passthrough modifier",
                    "mouse scroll lines",
                ],
            ),
            (SettingsPage::Remote, &["manage ssh config"]),
            (SettingsPage::Advanced, &["scrollback limit bytes"]),
            (
                SettingsPage::Experiments,
                &[
                    "allow nested gmux",
                    "kitty graphics",
                    "pane screen history",
                    "reveal hidden cursor for CJK IME",
                    "CJK IME cursor shape",
                    "switch to ascii input source in prefix (macOS)",
                ],
            ),
        ];

        for (page, labels) in expected {
            state.settings.page = page;
            let rows = settings_rows(&state);
            for label in labels {
                assert!(
                    rows.iter().any(|row| row.label == *label),
                    "missing {label:?} on {page:?}"
                );
            }
        }
    }
}
