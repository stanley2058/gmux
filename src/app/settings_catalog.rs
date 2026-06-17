use crate::app::state::{AppState, PanePanelScope, SettingsEditField, SettingsPage, THEME_NAMES};
use crate::config::{
    ConfigFieldEditor, ConfigFieldPath, ConfigFieldSpec, ConfigFieldUi, ConfigUiPage,
    NewTerminalCwdConfig, ShellModeConfig, ToastDelivery, CONFIG_FIELD_SPECS,
};

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

    fn readonly_with_hint(
        label: &'static str,
        value: impl Into<String>,
        hint: &'static str,
    ) -> Self {
        Self {
            label: label.to_string(),
            value: Some(value.into()),
            hint,
            selected: false,
            readonly: true,
            action: SettingsRowAction::Readonly,
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
        SettingsPage::Theme => scalar_setting_rows(state, SettingsPage::Theme),
        SettingsPage::ThemePicker => theme_picker_rows(state),
        SettingsPage::ThemeCustom => scalar_setting_rows(state, SettingsPage::ThemeCustom),
        SettingsPage::Notifications => scalar_setting_rows(state, SettingsPage::Notifications),
        SettingsPage::ToastDelivery => toast_delivery_rows(state),
        SettingsPage::Terminal => scalar_setting_rows(state, SettingsPage::Terminal),
        SettingsPage::ShellMode => shell_mode_rows(state),
        SettingsPage::NewTerminalCwd => new_terminal_cwd_rows(state),
        SettingsPage::Interface => scalar_setting_rows(state, SettingsPage::Interface),
        SettingsPage::PanePanelScope => pane_panel_scope_rows(state),
        SettingsPage::Mouse => scalar_setting_rows(state, SettingsPage::Mouse),
        SettingsPage::RightClickPassthroughModifier => right_click_modifier_rows(state),
        SettingsPage::Remote => scalar_setting_rows(state, SettingsPage::Remote),
        SettingsPage::Advanced => scalar_setting_rows(state, SettingsPage::Advanced),
        SettingsPage::Experiments => scalar_setting_rows(state, SettingsPage::Experiments),
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

fn bool_setting_value(state: &AppState, path: ConfigFieldPath) -> Option<bool> {
    match path {
        ConfigFieldPath::TopLevel("onboarding") => Some(state.show_onboarding_on_next_launch),
        ConfigFieldPath::Section {
            section: "terminal",
            key: "restore_processes",
        } => Some(state.restore_processes),
        ConfigFieldPath::Section {
            section: "ui",
            key: "confirm_close",
        } => Some(state.confirm_close),
        ConfigFieldPath::Section {
            section: "ui",
            key: "prompt_new_tab_name",
        } => Some(state.prompt_new_tab_name),
        ConfigFieldPath::Section {
            section: "ui",
            key: "redraw_on_focus_gained",
        } => Some(state.redraw_on_focus_gained),
        ConfigFieldPath::Section {
            section: "ui",
            key: "mouse_capture",
        } => Some(state.mouse_capture),
        ConfigFieldPath::Section {
            section: "remote",
            key: "manage_ssh_config",
        } => Some(state.remote_manage_ssh_config),
        ConfigFieldPath::Section {
            section: "experimental",
            key: "allow_nested",
        } => Some(state.allow_nested_gmux),
        ConfigFieldPath::Section {
            section: "experimental",
            key: "kitty_graphics",
        } => Some(state.kitty_graphics_enabled),
        ConfigFieldPath::Section {
            section: "experimental",
            key: "pane_history",
        } => Some(state.pane_history_persistence),
        ConfigFieldPath::Section {
            section: "experimental",
            key: "reveal_hidden_cursor_for_cjk_ime",
        } => Some(state.reveal_hidden_cursor_for_cjk_ime),
        ConfigFieldPath::Section {
            section: "experimental",
            key: "switch_ascii_input_source_in_prefix",
        } => Some(state.switch_ascii_input_source_in_prefix),
        _ => None,
    }
}

fn text_setting_value(
    state: &AppState,
    path: ConfigFieldPath,
) -> Option<(SettingsEditField, String)> {
    match path {
        ConfigFieldPath::Section {
            section: "terminal",
            key: "term",
        } => Some((SettingsEditField::PaneTerm, state.pane_term.clone())),
        ConfigFieldPath::Section {
            section: "terminal",
            key: "editor",
        } => Some((
            SettingsEditField::TerminalEditor,
            opener_setting_label(&state.terminal_editor, "EDITOR", "vi"),
        )),
        ConfigFieldPath::Section {
            section: "terminal",
            key: "pager",
        } => Some((
            SettingsEditField::TerminalPager,
            opener_setting_label(&state.terminal_pager, "PAGER", "less -R"),
        )),
        ConfigFieldPath::Section {
            section: "terminal",
            key: "default_shell",
        } => Some((
            SettingsEditField::DefaultShell,
            if state.default_shell.is_empty() {
                "SHELL".to_string()
            } else {
                state.default_shell.clone()
            },
        )),
        ConfigFieldPath::Section {
            section: "ui",
            key: "sidebar_width",
        } => Some((
            SettingsEditField::SidebarWidth,
            state.default_sidebar_width.to_string(),
        )),
        ConfigFieldPath::Section {
            section: "ui",
            key: "sidebar_min_width",
        } => Some((
            SettingsEditField::SidebarMinWidth,
            state.sidebar_min_width.to_string(),
        )),
        ConfigFieldPath::Section {
            section: "ui",
            key: "sidebar_max_width",
        } => Some((
            SettingsEditField::SidebarMaxWidth,
            state.sidebar_max_width.to_string(),
        )),
        ConfigFieldPath::Section {
            section: "ui",
            key: "mobile_width_threshold",
        } => Some((
            SettingsEditField::MobileWidthThreshold,
            state.mobile_width_threshold.to_string(),
        )),
        ConfigFieldPath::Section {
            section: "ui",
            key: "mouse_scroll_lines",
        } => Some((
            SettingsEditField::MouseScrollLines,
            state.mouse_scroll_lines.to_string(),
        )),
        ConfigFieldPath::Section {
            section: "advanced",
            key: "scrollback_limit_bytes",
        } => Some((
            SettingsEditField::ScrollbackLimitBytes,
            state.pane_scrollback_limit_bytes.to_string(),
        )),
        _ => None,
    }
}

fn subpage_setting_value(state: &AppState, path: ConfigFieldPath) -> Option<String> {
    match path {
        ConfigFieldPath::Section {
            section: "theme",
            key: "name",
        } => Some(state.theme_name.clone()),
        ConfigFieldPath::Section {
            section: "theme",
            key: "custom",
        } => Some("readonly".to_string()),
        ConfigFieldPath::Section {
            section: "ui.toast",
            key: "delivery",
        } => Some(toast_delivery_label(state.toast_delivery()).to_string()),
        ConfigFieldPath::Section {
            section: "terminal",
            key: "shell_mode",
        } => Some(shell_mode_label(state.shell_mode).to_string()),
        ConfigFieldPath::Section {
            section: "terminal",
            key: "new_cwd",
        } => Some(new_terminal_cwd_label(&state.new_terminal_cwd)),
        ConfigFieldPath::Section {
            section: "ui",
            key: "pane_panel_scope",
        } => Some(pane_panel_scope_label(state.pane_panel_scope).to_string()),
        ConfigFieldPath::Section {
            section: "ui",
            key: "right_click_passthrough_modifier",
        } => Some(right_click_modifier_label(state)),
        ConfigFieldPath::Section {
            section: "experimental",
            key: "cjk_ime_cursor_shape",
        } => Some(cjk_ime_cursor_shape_label(state.cjk_ime_cursor_shape).to_string()),
        _ => None,
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

fn scalar_setting_rows(state: &AppState, page: SettingsPage) -> Vec<SettingsRow> {
    CONFIG_FIELD_SPECS
        .iter()
        .filter(|spec| settings_page(spec.page) == page)
        .filter_map(|spec| setting_row(state, spec))
        .collect()
}

fn setting_row(state: &AppState, spec: &ConfigFieldSpec) -> Option<SettingsRow> {
    match spec.ui {
        ConfigFieldUi::Hidden { reason } => {
            let _ = reason;
            None
        }
        ConfigFieldUi::Readonly { value, reason } => {
            let _ = reason;
            Some(SettingsRow::readonly_with_hint(
                spec.label, value, spec.hint,
            ))
        }
        ConfigFieldUi::Editable(ConfigFieldEditor::Bool) => {
            let enabled = bool_setting_value(state, spec.path)?;
            let action = match spec.path {
                ConfigFieldPath::TopLevel(key) => SettingsRowAction::SaveTopLevelBool {
                    key,
                    value: !enabled,
                    context: spec.context,
                },
                ConfigFieldPath::Section { section, key } => SettingsRowAction::SaveSectionBool {
                    section,
                    key,
                    value: !enabled,
                    context: spec.context,
                },
            };
            Some(SettingsRow {
                label: spec.label.to_string(),
                value: Some(on_off(enabled)),
                hint: spec.hint,
                selected: false,
                readonly: false,
                action,
            })
        }
        ConfigFieldUi::Editable(ConfigFieldEditor::Text) => {
            let (field, value) = text_setting_value(state, spec.path)?;
            Some(SettingsRow::edit(spec.label, value, spec.hint, field))
        }
        ConfigFieldUi::Editable(ConfigFieldEditor::Subpage(subpage)) => {
            let value = subpage_setting_value(state, spec.path)?;
            Some(SettingsRow::open(
                spec.label,
                value,
                spec.hint,
                settings_page(subpage),
            ))
        }
    }
}

fn settings_page(page: ConfigUiPage) -> SettingsPage {
    match page {
        ConfigUiPage::Main => SettingsPage::Main,
        ConfigUiPage::Theme => SettingsPage::Theme,
        ConfigUiPage::ThemePicker => SettingsPage::ThemePicker,
        ConfigUiPage::ThemeCustom => SettingsPage::ThemeCustom,
        ConfigUiPage::Notifications => SettingsPage::Notifications,
        ConfigUiPage::ToastDelivery => SettingsPage::ToastDelivery,
        ConfigUiPage::Terminal => SettingsPage::Terminal,
        ConfigUiPage::ShellMode => SettingsPage::ShellMode,
        ConfigUiPage::NewTerminalCwd => SettingsPage::NewTerminalCwd,
        ConfigUiPage::Interface => SettingsPage::Interface,
        ConfigUiPage::PanePanelScope => SettingsPage::PanePanelScope,
        ConfigUiPage::Mouse => SettingsPage::Mouse,
        ConfigUiPage::RightClickPassthroughModifier => SettingsPage::RightClickPassthroughModifier,
        ConfigUiPage::Remote => SettingsPage::Remote,
        ConfigUiPage::Advanced => SettingsPage::Advanced,
        ConfigUiPage::Experiments => SettingsPage::Experiments,
        ConfigUiPage::CjkImeCursorShape => SettingsPage::CjkImeCursorShape,
    }
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

    #[test]
    fn setting_registry_has_no_duplicate_paths() {
        let mut seen = std::collections::HashSet::new();

        for spec in CONFIG_FIELD_SPECS {
            assert!(
                seen.insert(spec.path),
                "duplicate setting path: {:?}",
                spec.path
            );
        }
    }

    #[test]
    fn setting_registry_classifies_readonly_and_hidden_fields() {
        assert!(CONFIG_FIELD_SPECS.iter().any(|spec| {
            spec.path
                == ConfigFieldPath::Section {
                    section: "theme.custom",
                    key: "accent",
                }
                && matches!(spec.ui, ConfigFieldUi::Readonly { .. })
        }));
        assert!(CONFIG_FIELD_SPECS.iter().any(|spec| {
            spec.path
                == ConfigFieldPath::Section {
                    section: "ui",
                    key: "accent",
                }
                && matches!(spec.ui, ConfigFieldUi::Readonly { .. })
        }));
        assert!(CONFIG_FIELD_SPECS.iter().any(|spec| {
            spec.path == ConfigFieldPath::TopLevel("keys")
                && matches!(spec.ui, ConfigFieldUi::Hidden { .. })
        }));
        assert!(CONFIG_FIELD_SPECS.iter().any(|spec| {
            spec.path
                == ConfigFieldPath::Section {
                    section: "keys",
                    key: "command",
                }
                && matches!(spec.ui, ConfigFieldUi::Hidden { .. })
        }));
        assert!(CONFIG_FIELD_SPECS.iter().any(|spec| {
            spec.path
                == ConfigFieldPath::Section {
                    section: "ui.toast",
                    key: "enabled",
                }
                && matches!(spec.ui, ConfigFieldUi::Hidden { .. })
        }));
    }
}
