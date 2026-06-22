use ratatui::{
    layout::{Constraint, Layout, Rect},
    style::{Modifier, Style},
    text::{Line, Span},
    widgets::{List, ListItem, ListState, Paragraph},
    Frame,
};

use super::widgets::{
    action_button_row_rects, centered_popup_rect, modal_stack_areas, panel_contrast_fg,
    render_action_button, render_panel_shell, ActionButtonSpec,
};
use crate::app::{settings_catalog, state::SettingsPage, AppState};

pub(super) fn render_settings_overlay(app: &AppState, frame: &mut Frame, area: Rect) {
    let p = &app.palette;
    let Some(popup) = centered_popup_rect(area, 82, 24) else {
        return;
    };

    super::dim_background(frame, area);

    let Some(inner) = render_panel_shell(frame, popup, p.accent, p.panel_bg) else {
        return;
    };
    if inner.height < 4 || inner.width < 10 {
        return;
    }

    let stack = modal_stack_areas(inner, 3, 2, 0, 1);
    let header_rows = Layout::vertical([
        Constraint::Length(1),
        Constraint::Length(1),
        Constraint::Length(1),
    ])
    .areas::<3>(stack.header);

    frame.render_widget(
        Paragraph::new(Line::from(vec![Span::styled(
            settings_title(app),
            Style::default().fg(p.text).add_modifier(Modifier::BOLD),
        )])),
        header_rows[0],
    );
    frame.render_widget(
        Paragraph::new(settings_description(app)).style(Style::default().fg(p.overlay1)),
        header_rows[1],
    );

    let sep = "─".repeat(inner.width as usize);
    frame.render_widget(
        Paragraph::new(Span::styled(&sep, Style::default().fg(p.surface0))),
        header_rows[2],
    );

    if app.settings.edit.is_some() {
        render_settings_editor(app, frame, stack.content);
    } else {
        render_settings_page(app, frame, stack.content);
    }

    if let Some(footer_area) = stack.footer {
        let footer_rows = Layout::vertical([Constraint::Length(1), Constraint::Length(1)])
            .areas::<2>(footer_area);
        let primary_label = settings_primary_button_label(app.settings.page);
        let show_primary = settings_show_primary_action(app);
        let (apply_rect, close_rect) =
            settings_button_rects(inner, app.settings.page, show_primary);
        if let Some(apply_rect) = apply_rect {
            render_action_button(
                frame,
                apply_rect,
                Some("↵"),
                primary_label,
                Style::default()
                    .fg(panel_contrast_fg(p))
                    .bg(p.accent)
                    .add_modifier(Modifier::BOLD),
            );
        }
        render_action_button(
            frame,
            close_rect,
            Some("q"),
            "close",
            Style::default()
                .fg(p.text)
                .bg(p.surface0)
                .add_modifier(Modifier::BOLD),
        );

        frame.render_widget(
            Paragraph::new(Line::from(vec![
                Span::styled(" ↑↓", Style::default().fg(p.overlay0)),
                Span::styled(" select  ", Style::default().fg(p.overlay1)),
                Span::styled("enter", Style::default().fg(p.overlay0)),
                Span::styled(" open/toggle  ", Style::default().fg(p.overlay1)),
                Span::styled("esc/backspace", Style::default().fg(p.overlay0)),
                Span::styled(" back", Style::default().fg(p.overlay1)),
                Span::styled("  q", Style::default().fg(p.overlay0)),
                Span::styled(" close", Style::default().fg(p.overlay1)),
            ])),
            footer_rows[0],
        );
    }
}

pub(crate) fn settings_primary_button_label(_page: SettingsPage) -> &'static str {
    "select"
}

pub(crate) fn settings_show_primary_action(_app: &AppState) -> bool {
    true
}

pub(crate) fn settings_button_rects(
    inner: Rect,
    page: SettingsPage,
    show_primary: bool,
) -> (Option<Rect>, Rect) {
    if !show_primary {
        let rects = action_button_row_rects(
            inner,
            &[ActionButtonSpec {
                hint: Some("q"),
                label: "close",
            }],
            2,
            inner.height.saturating_sub(1),
        );
        return (None, rects[0]);
    }

    let rects = action_button_row_rects(
        inner,
        &[
            ActionButtonSpec {
                hint: Some("↵"),
                label: settings_primary_button_label(page),
            },
            ActionButtonSpec {
                hint: Some("q"),
                label: "close",
            },
        ],
        2,
        inner.height.saturating_sub(1),
    );
    (Some(rects[0]), rects[1])
}

fn render_settings_page(app: &AppState, frame: &mut Frame, area: Rect) {
    let p = &app.palette;
    let rows = settings_catalog::settings_rows(app);
    let items: Vec<ListItem> = rows
        .iter()
        .map(|row| {
            let mut spans = Vec::new();
            let marker = if row.selected { "✓ " } else { "  " };
            spans.push(Span::styled(marker, Style::default().fg(p.green)));
            spans.push(Span::styled(
                row.label.as_str(),
                Style::default().fg(p.subtext0),
            ));
            if let Some(value) = &row.value {
                spans.push(Span::styled(": ", Style::default().fg(p.overlay0)));
                spans.push(Span::styled(value.as_str(), Style::default().fg(p.text)));
            }
            if row.readonly {
                spans.push(Span::styled("  readonly", Style::default().fg(p.overlay0)));
            } else if matches!(row.action, settings_catalog::SettingsRowAction::Open(_)) {
                spans.push(Span::styled("  >", Style::default().fg(p.overlay0)));
            }
            if !row.hint.is_empty() {
                spans.push(Span::styled("  ", Style::default().fg(p.overlay0)));
                spans.push(Span::styled(row.hint, Style::default().fg(p.overlay0)));
            }
            ListItem::new(Line::from(spans))
        })
        .collect();

    let list = List::new(items)
        .highlight_style(
            Style::default()
                .bg(p.surface0)
                .fg(p.text)
                .add_modifier(Modifier::BOLD),
        )
        .highlight_symbol(" ▸ ")
        .style(Style::default().fg(p.subtext0));

    let mut state = ListState::default().with_selected(Some(app.settings.list.selected));
    frame.render_stateful_widget(list, area, &mut state);
}

fn render_settings_editor(app: &AppState, frame: &mut Frame, area: Rect) {
    let p = &app.palette;
    let Some(edit) = &app.settings.edit else {
        return;
    };
    let [label_area, input_area, error_area] = Layout::vertical([
        Constraint::Length(2),
        Constraint::Length(1),
        Constraint::Length(2),
    ])
    .areas::<3>(area);

    super::widgets::render_modal_description(
        frame,
        label_area,
        settings_catalog::edit_field_label(edit.field),
        Style::default().fg(p.overlay1),
    );

    frame.render_widget(
        Paragraph::new(format!(" {}█", edit.input)).style(
            Style::default()
                .fg(p.text)
                .bg(p.surface0)
                .add_modifier(Modifier::BOLD),
        ),
        input_area,
    );

    if let Some(error) = &edit.error {
        frame.render_widget(
            Paragraph::new(error.as_str()).style(Style::default().fg(p.red)),
            error_area,
        );
    }
}

fn settings_title(app: &AppState) -> String {
    let mut pages = Vec::new();
    let mut page = Some(app.settings.page);
    while let Some(current) = page {
        pages.push(current.label());
        page = current.parent();
    }
    pages.reverse();
    format!(" {}", pages.join(" / "))
}

fn settings_description(app: &AppState) -> &'static str {
    if app.settings.edit.is_some() {
        return "enter saves, esc cancels the edit";
    }
    match app.settings.page {
        SettingsPage::Main => "choose a settings group",
        SettingsPage::ThemeCustom => "structured theme color overrides are readonly here",
        _ => "scalar config fields only; keybindings stay in the readonly keybindings menu",
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::app::Mode;
    use ratatui::{backend::TestBackend, Terminal};

    fn rendered_settings(app: &AppState) -> String {
        let mut terminal =
            Terminal::new(TestBackend::new(90, 26)).expect("test terminal should initialize");
        terminal
            .draw(|frame| render_settings_overlay(app, frame, Rect::new(0, 0, 90, 26)))
            .expect("settings overlay should render");

        terminal
            .backend()
            .buffer()
            .content()
            .iter()
            .map(|cell| cell.symbol())
            .collect::<String>()
    }

    #[test]
    fn main_settings_renders_hierarchical_categories() {
        let mut app = AppState::test_new();
        app.settings.page = SettingsPage::Main;
        app.mode = Mode::Settings;

        let rendered = rendered_settings(&app);

        assert!(rendered.contains("theme"));
        assert!(rendered.contains("notifications"));
        assert!(rendered.contains("experiments"));
        assert!(!rendered.contains("keybindings"));
    }

    #[test]
    fn experiments_render_switch_ascii_input_source_row() {
        let mut app = AppState::test_new();
        app.switch_ascii_input_source_in_prefix = true;
        app.settings.page = SettingsPage::Experiments;
        app.settings.list.selected = 5;
        app.mode = Mode::Settings;

        let rendered = rendered_settings(&app);

        assert!(rendered.contains("switch to ascii input source in prefix (macOS): on"));
    }

    #[test]
    fn theme_custom_rows_render_readonly_hint() {
        let mut app = AppState::test_new();
        app.settings.page = SettingsPage::ThemeCustom;
        app.mode = Mode::Settings;

        let rendered = rendered_settings(&app);

        assert!(rendered.contains("accent: readonly"));
        assert!(rendered.contains("edit config.toml"));
    }
}
