use ratatui::{
    layout::Rect,
    style::{Modifier, Style},
    text::{Line, Span},
    widgets::{Clear, Paragraph},
    Frame,
};

use super::{
    scrollbar::{render_scrollbar, should_show_scrollbar},
    widgets::{panel_contrast_fg, render_panel_shell},
};
use crate::app::state::{AppState, NavigatorRow, NavigatorTarget};
use crate::terminal::TerminalRuntimeRegistry;

pub(super) fn render_navigator_overlay(
    app: &AppState,
    terminal_runtimes: &TerminalRuntimeRegistry,
    frame: &mut Frame,
) {
    let popup = app.navigator_popup_rect();
    let Some(inner) = render_panel_shell(frame, popup, app.palette.accent, app.palette.panel_bg)
    else {
        return;
    };

    let search = app.navigator_search_rect();
    let body = app.navigator_body_rect();
    let detail = app.navigator_detail_rect();
    let footer = app.navigator_footer_rect();
    render_search(app, frame, search);

    if body.height > 0 {
        render_separator(frame, Rect::new(inner.x, search.y + 1, inner.width, 1), app);
        render_rows(app, terminal_runtimes, frame, body);
        render_navigator_scrollbar(app, terminal_runtimes, frame, body);
    }
    render_detail(app, terminal_runtimes, frame, detail);
    render_footer(app, frame, footer);
}

fn render_search(app: &AppState, frame: &mut Frame, area: Rect) {
    let p = &app.palette;
    let focus_style = if app.navigator.search_focused {
        Style::default().fg(p.accent).add_modifier(Modifier::BOLD)
    } else {
        Style::default().fg(p.overlay0)
    };
    let count = app
        .session_tab_entries()
        .map(|entry| entry.tab.panes.len())
        .sum::<usize>();
    let mut spans = vec![Span::styled(" / ", focus_style)];
    let query = app.navigator.query.trim();
    if query.is_empty() {
        spans.push(Span::styled(
            "search panes",
            Style::default().fg(p.overlay0),
        ));
    } else {
        spans.push(Span::styled(query.to_string(), Style::default().fg(p.text)));
    }
    spans.push(Span::styled(
        format!(
            "{count:>width$} panes",
            width = area.width.saturating_sub(16) as usize
        ),
        Style::default().fg(p.overlay0),
    ));
    frame.render_widget(Paragraph::new(Line::from(spans)), area);
}

fn render_separator(frame: &mut Frame, area: Rect, app: &AppState) {
    if area.height == 0 || area.width == 0 {
        return;
    }
    let line = "─".repeat(area.width as usize);
    frame.render_widget(
        Paragraph::new(line).style(Style::default().fg(app.palette.surface1)),
        area,
    );
}

fn render_rows(
    app: &AppState,
    terminal_runtimes: &TerminalRuntimeRegistry,
    frame: &mut Frame,
    body: Rect,
) {
    let rows = app.navigator_rows_from(terminal_runtimes);
    let start = app.navigator.scroll.min(rows.len());
    let end = rows.len().min(start.saturating_add(body.height as usize));
    for (visible_idx, row) in rows[start..end].iter().enumerate() {
        let idx = start + visible_idx;
        let y = body.y + visible_idx as u16;
        let rect = Rect::new(body.x, y, body.width, 1);
        let selected = idx == app.navigator.selected;
        render_row(app, frame, rect, row, selected);
    }
}

fn render_row(app: &AppState, frame: &mut Frame, rect: Rect, row: &NavigatorRow, selected: bool) {
    let p = &app.palette;
    frame.render_widget(Clear, rect);
    let base_style = if selected {
        Style::default().bg(p.accent).fg(panel_contrast_fg(p))
    } else {
        Style::default().bg(p.panel_bg).fg(p.text)
    };
    let dim_style = if selected {
        base_style
    } else {
        Style::default().fg(p.overlay0).bg(p.panel_bg)
    };
    let text_style = if selected {
        base_style.add_modifier(Modifier::BOLD)
    } else if row.is_current {
        Style::default()
            .fg(p.text)
            .bg(p.panel_bg)
            .add_modifier(Modifier::BOLD)
    } else {
        Style::default().fg(p.subtext0).bg(p.panel_bg)
    };
    let prefix = if row.depth > 0 { "├─" } else { "  " };
    let current = if row.is_current { "◆" } else { " " };
    let marker = if selected { "→" } else { " " };
    let indent = "  ".repeat(row.depth as usize);
    let left_fixed = format!(" {indent}{prefix} {marker} {current} ");
    let meta_width = metadata_width(rect.width);
    let left_budget = rect
        .width
        .saturating_sub(meta_width)
        .saturating_sub(left_fixed.chars().count() as u16)
        .saturating_sub(1) as usize;
    let title = truncate_text(&row.label, left_budget);

    let spans = vec![
        Span::styled(left_fixed, dim_style),
        Span::styled(title, text_style),
    ];
    frame.render_widget(Paragraph::new(Line::from(spans)).style(base_style), rect);

    if meta_width > 0 {
        let meta_rect = Rect::new(
            rect.x + rect.width.saturating_sub(meta_width),
            rect.y,
            meta_width,
            1,
        );
        let meta = truncate_text(&row.meta, meta_width.saturating_sub(2) as usize);
        let meta_style = if selected {
            base_style
        } else {
            Style::default().fg(p.overlay0).bg(p.panel_bg)
        };
        frame.render_widget(
            Paragraph::new(format!(" {meta}")).style(meta_style),
            meta_rect,
        );
    }
}

fn render_navigator_scrollbar(
    app: &AppState,
    terminal_runtimes: &TerminalRuntimeRegistry,
    frame: &mut Frame,
    body: Rect,
) {
    if body.width <= 1 || body.height == 0 {
        return;
    }
    let rows = app.navigator_rows_from(terminal_runtimes).len();
    let viewport = body.height as usize;
    if rows <= viewport {
        return;
    }
    let metrics = crate::pane::ScrollMetrics {
        viewport_rows: viewport,
        offset_from_bottom: rows
            .saturating_sub(viewport)
            .saturating_sub(app.navigator.scroll),
        max_offset_from_bottom: rows.saturating_sub(viewport),
    };
    if !should_show_scrollbar(metrics) {
        return;
    }
    let track = Rect::new(body.x + body.width - 1, body.y, 1, body.height);
    render_scrollbar(
        frame,
        metrics,
        track,
        app.palette.surface_dim,
        app.palette.overlay0,
        "▕",
    );
}

fn metadata_width(width: u16) -> u16 {
    if width >= 90 {
        28
    } else if width >= 68 {
        20
    } else if width >= 52 {
        14
    } else {
        0
    }
}

fn render_detail(
    app: &AppState,
    terminal_runtimes: &TerminalRuntimeRegistry,
    frame: &mut Frame,
    area: Rect,
) {
    if area.height == 0 || area.width == 0 {
        return;
    }
    render_separator(frame, area, app);
    let detail = selected_detail(app, terminal_runtimes);
    if detail.is_empty() {
        return;
    }
    let text = middle_elide(&detail, area.width.saturating_sub(2) as usize);
    frame.render_widget(
        Paragraph::new(format!(" {text}")).style(Style::default().fg(app.palette.overlay0)),
        area,
    );
}

fn selected_detail(app: &AppState, terminal_runtimes: &TerminalRuntimeRegistry) -> String {
    let rows = app.navigator_rows_from(terminal_runtimes);
    let Some(row) = rows.get(app.navigator.selected) else {
        return String::new();
    };
    match row.target {
        NavigatorTarget::Tab { ws_idx, tab_idx } => {
            tab_detail(app, terminal_runtimes, ws_idx, tab_idx)
        }
        NavigatorTarget::Pane {
            ws_idx,
            tab_idx,
            pane_id,
        } => pane_detail(app, terminal_runtimes, ws_idx, tab_idx, pane_id),
    }
}

fn tab_detail(
    app: &AppState,
    terminal_runtimes: &TerminalRuntimeRegistry,
    ws_idx: usize,
    tab_idx: usize,
) -> String {
    let Some(entry) = app
        .session_tab_entries()
        .find(|entry| entry.session_idx == ws_idx && entry.tab_idx == tab_idx)
    else {
        return String::new();
    };
    let ws = entry.session;
    let tab = entry.tab;
    let session_label = app
        .session()
        .map(|ws| ws.display_name_from(&app.terminals, terminal_runtimes))
        .unwrap_or_else(|| "session".to_string());
    let tab_label = crate::workspace::session_tab_display_name(ws_idx, ws, tab_idx, tab);
    let parts = vec![
        session_label,
        format!("tab: {tab_label}"),
        format!("{} panes", tab.panes.len()),
    ];
    parts.join(" · ")
}

fn pane_detail(
    app: &AppState,
    terminal_runtimes: &TerminalRuntimeRegistry,
    ws_idx: usize,
    tab_idx: usize,
    pane_id: crate::layout::PaneId,
) -> String {
    let Some(entry) = app
        .session_tab_entries()
        .find(|entry| entry.session_idx == ws_idx && entry.tab_idx == tab_idx)
    else {
        return String::new();
    };
    let ws = entry.session;
    let tab = entry.tab;
    let session_label = app
        .session()
        .map(|ws| ws.display_name_from(&app.terminals, terminal_runtimes))
        .unwrap_or_else(|| "session".to_string());
    let mut parts = vec![session_label];
    let multi_tab = app.session_tab_count() > 1;
    if multi_tab {
        let tab_label = crate::workspace::session_tab_display_name(ws_idx, ws, tab_idx, tab);
        parts.push(format!("tab: {tab_label}"));
    }
    if let Some(pane_number) = ws.public_pane_number(pane_id) {
        parts.push(format!("pane {pane_number}"));
    }
    if let Some(terminal_id) = tab.terminal_id(pane_id) {
        if let Some(terminal) = app.terminals.get(terminal_id) {
            if let Some(title) = terminal.effective_title() {
                parts.push(title);
            } else {
                parts.push("shell".to_string());
            }
        }
    }
    parts.join(" · ")
}

fn middle_elide(text: &str, max_width: usize) -> String {
    let len = text.chars().count();
    if len <= max_width {
        return text.to_string();
    }
    if max_width <= 1 {
        return "…".to_string();
    }
    let left = max_width.saturating_sub(1) / 2;
    let right = max_width.saturating_sub(1).saturating_sub(left);
    let prefix: String = text.chars().take(left).collect();
    let suffix: String = text
        .chars()
        .rev()
        .take(right)
        .collect::<Vec<_>>()
        .into_iter()
        .rev()
        .collect();
    format!("{prefix}…{suffix}")
}

fn render_footer(app: &AppState, frame: &mut Frame, area: Rect) {
    if area.height == 0 {
        return;
    }
    let p = &app.palette;
    let key = Style::default().fg(p.accent).add_modifier(Modifier::BOLD);
    let dim = Style::default().fg(p.overlay0);
    let line = Line::from(vec![
        Span::styled(" enter", key),
        Span::styled(" switch  ", dim),
        Span::styled("/", key),
        Span::styled(" search  ", dim),
        Span::styled("j/k/↑↓", key),
        Span::styled(" move  ", dim),
        Span::styled("esc", key),
        Span::styled(" close", dim),
    ]);
    frame.render_widget(Paragraph::new(line), area);
}

fn truncate_text(text: &str, max_width: usize) -> String {
    let len = text.chars().count();
    if len <= max_width {
        return text.to_string();
    }
    if max_width == 0 {
        return String::new();
    }
    if max_width == 1 {
        return "…".to_string();
    }
    let prefix: String = text.chars().take(max_width.saturating_sub(1)).collect();
    format!("{prefix}…")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn details_describe_legacy_session_as_session_tab() {
        let first = crate::workspace::Workspace::test_new("one");
        let second = crate::workspace::Workspace::test_new("two");
        let pane_id = second.tabs[0].root_pane;

        let mut app = AppState::test_new();
        app.sessions = vec![first, second];
        app.active_session = Some(0);
        app.selected_session = 0;

        let terminal_runtimes = TerminalRuntimeRegistry::new();

        assert_eq!(
            tab_detail(&app, &terminal_runtimes, 1, 0),
            "one · tab: two · 1 panes"
        );
        assert_eq!(
            pane_detail(&app, &terminal_runtimes, 1, 0, pane_id),
            "one · tab: two · pane 1"
        );
    }
}
