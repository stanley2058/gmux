use ratatui::{
    layout::{Alignment, Rect},
    style::{Modifier, Style},
    text::{Line, Span},
    widgets::Paragraph,
    Frame,
};

use super::scrollbar::{render_scrollbar, should_show_scrollbar};
use crate::app::state::{Palette, PanePanelScope};
use crate::app::{AppState, Mode};
use crate::terminal::TerminalRuntimeRegistry;

const PANE_PANEL_HEADER_ROWS: u16 = 3;

pub(crate) struct PanePanelEntry {
    pub ws_idx: usize,
    pub tab_idx: usize,
    pub pane_id: crate::layout::PaneId,
    pub primary_label: String,
    pub primary_tab_label: Option<String>,
}

pub(crate) fn expanded_sidebar_content_rect(area: Rect) -> Rect {
    let content = Rect::new(area.x, area.y, area.width.saturating_sub(1), area.height);
    if content.width == 0 || content.height == 0 {
        return Rect::default();
    }

    content
}

pub(crate) fn expanded_sidebar_footer_rect(area: Rect) -> Rect {
    let content = expanded_sidebar_content_rect(area);
    if content == Rect::default() {
        return Rect::default();
    }

    Rect::new(
        content.x,
        content.y + content.height.saturating_sub(1),
        content.width,
        1,
    )
}

pub(crate) fn expanded_pane_panel_rect(area: Rect) -> Rect {
    let content = expanded_sidebar_content_rect(area);
    if content == Rect::default() {
        return Rect::default();
    }

    Rect::new(
        content.x,
        content.y,
        content.width,
        content.height.saturating_sub(1),
    )
}

fn pane_panel_current_context_idx(app: &AppState) -> Option<usize> {
    if matches!(
        app.mode,
        Mode::Navigate
            | Mode::RenamePane
            | Mode::Resize
            | Mode::ConfirmClose
            | Mode::ContextMenu
            | Mode::Settings
            | Mode::GlobalMenu
            | Mode::KeybindHelp
            | Mode::ProductAnnouncement
    ) {
        Some(app.selected)
    } else {
        app.active
    }
}

fn pane_panel_toggle_label(scope: PanePanelScope) -> &'static str {
    match scope {
        PanePanelScope::Current => "current",
        PanePanelScope::All => "all",
    }
}

pub(crate) fn pane_panel_toggle_rect(area: Rect, scope: PanePanelScope) -> Rect {
    if area.width == 0 || area.height < 2 {
        return Rect::default();
    }

    let label = pane_panel_toggle_label(scope);
    let width = label.chars().count() as u16;
    Rect::new(
        area.x + area.width.saturating_sub(width),
        area.y + 1,
        width,
        1,
    )
}

pub(crate) fn pane_panel_entries(app: &AppState) -> Vec<PanePanelEntry> {
    pane_panel_entries_with_runtimes(app, None)
}

pub(crate) fn pane_panel_entries_from(
    app: &AppState,
    terminal_runtimes: &TerminalRuntimeRegistry,
) -> Vec<PanePanelEntry> {
    pane_panel_entries_with_runtimes(app, Some(terminal_runtimes))
}

fn pane_panel_entries_with_runtimes(
    app: &AppState,
    terminal_runtimes: Option<&TerminalRuntimeRegistry>,
) -> Vec<PanePanelEntry> {
    let empty_runtimes;
    let terminal_runtimes = match terminal_runtimes {
        Some(terminal_runtimes) => terminal_runtimes,
        None => {
            empty_runtimes = TerminalRuntimeRegistry::new();
            &empty_runtimes
        }
    };

    match app.pane_panel_scope {
        PanePanelScope::Current => {
            let Some(ws_idx) = pane_panel_current_context_idx(app) else {
                return Vec::new();
            };
            let Some(ws) = app.workspaces.get(ws_idx) else {
                return Vec::new();
            };
            ws.pane_details(&app.terminals)
                .into_iter()
                .map(|detail| PanePanelEntry {
                    ws_idx,
                    tab_idx: detail.tab_idx,
                    pane_id: detail.pane_id,
                    primary_label: detail.label,
                    primary_tab_label: None,
                })
                .collect()
        }
        PanePanelScope::All => app
            .workspaces
            .iter()
            .enumerate()
            .flat_map(|(ws_idx, ws)| {
                let multi_tab = ws.tabs.len() > 1;
                let session_label = ws.display_name_from(&app.terminals, terminal_runtimes);
                ws.pane_details(&app.terminals)
                    .into_iter()
                    .map(move |detail| PanePanelEntry {
                        ws_idx,
                        tab_idx: detail.tab_idx,
                        pane_id: detail.pane_id,
                        primary_label: session_label.clone(),
                        primary_tab_label: multi_tab.then_some(detail.tab_label),
                    })
            })
            .collect(),
    }
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

fn format_pane_panel_primary_label(entry: &PanePanelEntry, max_width: usize) -> String {
    let Some(tab_label) = entry.primary_tab_label.as_deref() else {
        return truncate_text(&entry.primary_label, max_width);
    };

    let separator = " · ";
    let separator_width = separator.chars().count();
    if max_width <= separator_width + 2 {
        return truncate_text(
            &format!("{}{}{}", entry.primary_label, separator, tab_label),
            max_width,
        );
    }

    let available = max_width.saturating_sub(separator_width);
    let min_tab = 4.min(available.saturating_sub(1)).max(1);
    let preferred_primary = ((available * 2) / 3).max(1);
    let mut primary_budget = preferred_primary
        .min(available.saturating_sub(min_tab))
        .max(1);
    let mut tab_budget = available.saturating_sub(primary_budget);

    let primary_len = entry.primary_label.chars().count();
    let tab_len = tab_label.chars().count();

    if primary_len < primary_budget {
        let spare = primary_budget - primary_len;
        primary_budget = primary_len;
        tab_budget = (tab_budget + spare).min(available.saturating_sub(primary_budget));
    }
    if tab_len < tab_budget {
        let spare = tab_budget - tab_len;
        tab_budget = tab_len;
        primary_budget = (primary_budget + spare).min(available.saturating_sub(tab_budget));
    }

    format!(
        "{}{}{}",
        truncate_text(&entry.primary_label, primary_budget),
        separator,
        truncate_text(tab_label, tab_budget)
    )
}

pub(crate) fn pane_panel_body_rect(area: Rect, has_scrollbar: bool) -> Rect {
    if area.width == 0 || area.height <= PANE_PANEL_HEADER_ROWS {
        return Rect::default();
    }

    let body_y = area.y.saturating_add(PANE_PANEL_HEADER_ROWS);
    let body_height = (area.y + area.height).saturating_sub(body_y);
    let body_width = area.width.saturating_sub(u16::from(has_scrollbar));
    Rect::new(area.x, body_y, body_width, body_height)
}

fn pane_panel_visible_count(area: Rect) -> usize {
    let body = pane_panel_body_rect(area, false);
    if body.width == 0 || body.height < 2 {
        return 0;
    }

    let mut used_rows = 0u16;
    let mut visible = 0usize;
    while used_rows.saturating_add(2) <= body.height {
        used_rows = used_rows.saturating_add(2);
        visible += 1;
        if used_rows < body.height {
            used_rows = used_rows.saturating_add(1);
        }
    }
    visible
}

pub(crate) fn pane_panel_scroll_metrics(app: &AppState, area: Rect) -> crate::pane::ScrollMetrics {
    let viewport_rows = pane_panel_visible_count(area);
    let total_rows = pane_panel_entries(app).len();
    let max_offset_from_bottom = total_rows.saturating_sub(viewport_rows);
    let offset_from_bottom = total_rows
        .saturating_sub(app.pane_panel_scroll)
        .saturating_sub(viewport_rows);

    crate::pane::ScrollMetrics {
        offset_from_bottom,
        max_offset_from_bottom,
        viewport_rows,
    }
}

pub(crate) fn pane_panel_scrollbar_rect(app: &AppState, area: Rect) -> Option<Rect> {
    let metrics = pane_panel_scroll_metrics(app, area);
    let body = pane_panel_body_rect(area, true);
    (should_show_scrollbar(metrics) && body.width > 0 && body.height > 0).then_some(Rect::new(
        area.x + area.width.saturating_sub(1),
        body.y,
        1,
        body.height,
    ))
}

/// Auto-scale sidebar width based on session identity and pane detail.
pub(crate) fn collapsed_sidebar_sections(area: Rect) -> (Rect, Option<u16>, Rect) {
    let content = Rect::new(area.x, area.y, area.width.saturating_sub(1), area.height);
    if content.width == 0 || content.height == 0 {
        return (Rect::default(), None, Rect::default());
    }

    if content.height < 7 {
        return (content, None, Rect::default());
    }

    let total_h = content.height as usize;
    let ws_h = total_h.div_ceil(2);
    let detail_h = total_h.saturating_sub(ws_h + 1);
    if ws_h == 0 || detail_h == 0 {
        return (content, None, Rect::default());
    }

    let divider_y = content.y + ws_h as u16;
    let ws_area = Rect::new(content.x, content.y, content.width, ws_h as u16);
    let detail_area = Rect::new(content.x, divider_y + 1, content.width, detail_h as u16);
    (ws_area, Some(divider_y), detail_area)
}

/// Collapsed sidebar: session glance on top, compact pane list below.
pub(super) fn render_sidebar_collapsed(app: &AppState, frame: &mut Frame, area: Rect) {
    let is_navigating = matches!(app.mode, Mode::Navigate);

    let p = &app.palette;
    let sep_style = if is_navigating {
        Style::default().fg(p.accent)
    } else {
        Style::default().fg(p.surface_dim)
    };
    let sep_x = area.x + area.width.saturating_sub(1);
    let buf = frame.buffer_mut();
    for y in area.y..area.y + area.height {
        buf[(sep_x, y)].set_symbol("│");
        buf[(sep_x, y)].set_style(sep_style);
    }

    let (ws_area, divider_y, detail_area) = collapsed_sidebar_sections(area);
    if ws_area == Rect::default() {
        render_sidebar_toggle(app, frame, area, true, p);
        return;
    }

    for visible_idx in 0..app.workspaces.len() {
        let y = ws_area.y + visible_idx as u16;
        if y >= ws_area.y + ws_area.height {
            break;
        }
        let is_selected = visible_idx == app.selected && is_navigating;
        let is_active = Some(visible_idx) == app.active;
        let (icon, icon_style) = session_dot(is_active, is_selected, p);
        let row_style = if is_selected {
            Style::default().bg(p.surface0)
        } else if is_active {
            Style::default().bg(p.surface_dim)
        } else {
            Style::default()
        };
        let num_style = if is_selected {
            Style::default().fg(p.overlay1).bg(p.surface0)
        } else if is_active {
            Style::default().fg(p.text).bg(p.surface_dim)
        } else {
            Style::default().fg(p.overlay0)
        };

        if is_selected || is_active {
            let buf = frame.buffer_mut();
            for x in ws_area.x..ws_area.x + ws_area.width {
                buf[(x, y)].set_style(row_style);
            }
        }

        frame.render_widget(
            Paragraph::new(Line::from(vec![
                Span::styled(format!("{}", visible_idx + 1), num_style),
                Span::styled(" ", row_style),
                Span::styled(icon, icon_style),
            ])),
            Rect::new(ws_area.x, y, ws_area.width, 1),
        );
    }

    if let Some(divider_y) = divider_y {
        let buf = frame.buffer_mut();
        for x in ws_area.x..ws_area.x + ws_area.width {
            buf[(x, divider_y)].set_symbol("─");
            buf[(x, divider_y)].set_style(Style::default().fg(p.surface_dim));
        }
    }

    let detail_ws_idx = if is_navigating {
        Some(app.selected)
    } else {
        app.active
    };
    let detail_content_area = Rect::new(
        detail_area.x,
        detail_area.y,
        detail_area.width,
        detail_area.height.saturating_sub(1),
    );
    if detail_content_area != Rect::default() {
        if let Some(ws_idx) = detail_ws_idx {
            if let Some(ws) = app.workspaces.get(ws_idx) {
                for (detail_idx, detail) in ws.pane_details(&app.terminals).iter().enumerate() {
                    let y = detail_content_area.y + detail_idx as u16;
                    if y >= detail_content_area.y + detail_content_area.height {
                        break;
                    }
                    let pane_num = ws
                        .public_pane_number(detail.pane_id)
                        .unwrap_or(detail_idx + 1);
                    let pane_style = if app.is_active_pane(ws_idx, detail.tab_idx, detail.pane_id) {
                        Style::default().fg(p.text).add_modifier(Modifier::BOLD)
                    } else {
                        Style::default().fg(p.overlay0)
                    };
                    frame.render_widget(
                        Paragraph::new(Line::from(vec![Span::styled(
                            format!("{pane_num}"),
                            pane_style,
                        )])),
                        Rect::new(detail_content_area.x, y, detail_content_area.width, 1),
                    );
                }
            }
        }
    }

    render_sidebar_toggle(app, frame, area, true, p);
}

fn session_dot(active: bool, selected: bool, p: &Palette) -> (&'static str, Style) {
    if active {
        ("●", Style::default().fg(p.accent))
    } else if selected {
        ("●", Style::default().fg(p.overlay1))
    } else {
        ("○", Style::default().fg(p.overlay0))
    }
}

pub(super) fn render_sidebar(
    app: &AppState,
    terminal_runtimes: &TerminalRuntimeRegistry,
    frame: &mut Frame,
    area: Rect,
) {
    let p = &app.palette;
    let is_navigating = matches!(app.mode, Mode::Navigate);
    let sep_style = if is_navigating {
        Style::default().fg(p.accent)
    } else {
        Style::default().fg(p.surface_dim)
    };

    let sep_x = area.x + area.width.saturating_sub(1);
    let buf = frame.buffer_mut();
    for y in area.y..area.y + area.height {
        buf[(sep_x, y)].set_symbol("│");
        buf[(sep_x, y)].set_style(sep_style);
    }

    let detail_area = expanded_pane_panel_rect(area);
    render_pane_detail(app, terminal_runtimes, frame, detail_area);
    render_sidebar_footer(app, frame, area);
    render_sidebar_toggle(app, frame, area, false, p);
}

fn render_sidebar_footer(app: &AppState, frame: &mut Frame, area: Rect) {
    let p = &app.palette;
    let footer = expanded_sidebar_footer_rect(area);
    if !app.mouse_capture || footer == Rect::default() {
        return;
    }

    let new_rect = app.sidebar_new_button_rect();
    frame.render_widget(
        Paragraph::new(Span::styled(" tab", Style::default().fg(p.overlay0))),
        new_rect,
    );

    let menu_rect = app.global_launcher_rect();
    let menu_line = if app.global_menu_attention_badge_visible() {
        Line::from(vec![
            Span::styled(
                "● ",
                Style::default().fg(p.accent).add_modifier(Modifier::BOLD),
            ),
            Span::styled("menu", Style::default().fg(p.overlay0)),
        ])
    } else {
        Line::from(vec![Span::styled("menu", Style::default().fg(p.overlay0))])
    };
    frame.render_widget(
        Paragraph::new(menu_line).alignment(Alignment::Right),
        menu_rect,
    );
}

fn render_pane_detail(
    app: &AppState,
    terminal_runtimes: &TerminalRuntimeRegistry,
    frame: &mut Frame,
    area: Rect,
) {
    let p = &app.palette;

    if area.height < 3 {
        return;
    }

    let sep_line = "─".repeat(area.width as usize);
    frame.render_widget(
        Paragraph::new(Span::styled(&sep_line, Style::default().fg(p.surface_dim))),
        Rect::new(area.x, area.y, area.width, 1),
    );

    frame.render_widget(
        Paragraph::new(Line::from(vec![Span::styled(
            " panes",
            Style::default().fg(p.overlay0).add_modifier(Modifier::BOLD),
        )])),
        Rect::new(area.x, area.y + 1, area.width, 1),
    );
    let toggle_rect = pane_panel_toggle_rect(area, app.pane_panel_scope);
    if toggle_rect != Rect::default() {
        frame.render_widget(
            Paragraph::new(Span::styled(
                pane_panel_toggle_label(app.pane_panel_scope),
                Style::default().fg(p.overlay0).add_modifier(Modifier::BOLD),
            ))
            .alignment(Alignment::Right),
            toggle_rect,
        );
    }

    let details = pane_panel_entries_from(app, terminal_runtimes);
    let metrics = pane_panel_scroll_metrics(app, area);
    let scrollbar_rect = pane_panel_scrollbar_rect(app, area);
    let body = pane_panel_body_rect(area, should_show_scrollbar(metrics));
    if body == Rect::default() {
        return;
    }

    let mut row_y = body.y;
    let body_bottom = body.y + body.height;
    for detail in details.iter().skip(app.pane_panel_scroll) {
        if row_y.saturating_add(1) >= body_bottom {
            break;
        }

        let is_active = app.is_active_pane(detail.ws_idx, detail.tab_idx, detail.pane_id);

        let row_style = if is_active {
            Style::default().bg(p.surface_dim)
        } else {
            Style::default()
        };

        let name_style = if is_active {
            Style::default().fg(p.text).add_modifier(Modifier::BOLD)
        } else {
            Style::default().fg(p.subtext0).add_modifier(Modifier::BOLD)
        };
        let detail_style = Style::default().fg(p.overlay0).add_modifier(Modifier::DIM);

        let primary_label =
            format_pane_panel_primary_label(detail, body.width.saturating_sub(1) as usize);
        let name_line = Line::from(vec![
            Span::styled(" ", Style::default()),
            Span::styled(primary_label, name_style),
        ]);
        frame.render_widget(
            Paragraph::new(name_line).style(row_style),
            Rect::new(body.x, row_y, body.width, 1),
        );
        row_y += 1;

        let status_spans = vec![
            Span::styled(" ", Style::default()),
            Span::styled("pane", detail_style),
        ];
        frame.render_widget(
            Paragraph::new(Line::from(status_spans)).style(row_style),
            Rect::new(body.x, row_y, body.width, 1),
        );
        row_y += 1;

        if row_y < body_bottom {
            row_y += 1;
        }
    }

    if let Some(track) = scrollbar_rect {
        render_scrollbar(frame, metrics, track, p.surface_dim, p.overlay0, "▕");
    }
}

pub(crate) fn collapsed_sidebar_toggle_rect(area: Rect) -> Rect {
    let bottom_y = area.y + area.height.saturating_sub(1);
    let content_w = area.width.saturating_sub(1);
    if content_w == 0 || area.height == 0 {
        return Rect::default();
    }
    let x = area.x + content_w / 2;
    Rect::new(x, bottom_y, 1, 1)
}

pub(crate) fn expanded_sidebar_toggle_rect(area: Rect) -> Rect {
    if area.width <= 1 || area.height == 0 {
        return Rect::default();
    }
    Rect::new(
        area.x + area.width.saturating_sub(2),
        area.y + area.height.saturating_sub(1),
        1,
        1,
    )
}

fn render_sidebar_toggle(
    app: &AppState,
    frame: &mut Frame,
    area: Rect,
    collapsed: bool,
    p: &Palette,
) {
    let toggle_area = if collapsed {
        collapsed_sidebar_toggle_rect(area)
    } else {
        expanded_sidebar_toggle_rect(area)
    };
    if toggle_area == Rect::default() {
        return;
    }
    let icon = if collapsed { "»" } else { "«" };
    let icon_style = if collapsed && app.global_menu_attention_badge_visible() {
        Style::default().fg(p.accent).add_modifier(Modifier::BOLD)
    } else {
        Style::default().fg(p.overlay0)
    };
    frame.render_widget(Paragraph::new(Span::styled(icon, icon_style)), toggle_area);
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::workspace::Workspace;
    use ratatui::{backend::TestBackend, Terminal};

    #[test]
    fn render_sidebar_toggle_draws_expanded_collapse_icon() {
        let app = crate::app::state::AppState::test_new();
        let area = Rect::new(0, 0, 26, 20);
        let mut terminal =
            Terminal::new(TestBackend::new(26, 20)).expect("test terminal should initialize");

        terminal
            .draw(|frame| render_sidebar_toggle(&app, frame, area, false, &app.palette))
            .expect("sidebar toggle should render");

        let toggle = expanded_sidebar_toggle_rect(area);
        assert_eq!(
            terminal.backend().buffer()[(toggle.x, toggle.y)].symbol(),
            "«"
        );
    }

    #[test]
    fn expanded_sidebar_toggle_sits_inside_sidebar_content() {
        let area = Rect::new(0, 0, 26, 20);
        let toggle = expanded_sidebar_toggle_rect(area);

        assert_eq!(toggle.x, area.x + area.width - 2);
        assert_eq!(toggle.y, area.y + area.height - 1);
    }

    #[test]
    fn all_pane_panel_entries_use_session_and_optional_tab_labels() {
        let mut app = crate::app::state::AppState::test_new();
        let first = Workspace::test_new("one");
        let mut second = Workspace::test_new("two");
        second.test_add_tab(Some("logs"));

        app.workspaces = vec![first, second];
        app.ensure_test_terminals();
        app.active = Some(0);
        app.selected = 0;
        app.pane_panel_scope = PanePanelScope::All;

        let entries = pane_panel_entries(&app);
        assert_eq!(entries[0].primary_label, "one");
        assert!(entries[0].primary_tab_label.is_none());
        assert!(entries.iter().any(|entry| {
            entry.primary_label == "two" && entry.primary_tab_label.as_deref() == Some("logs")
        }));
    }

    #[tokio::test]
    async fn all_pane_panel_entries_use_live_root_runtime_cwd_for_session_label() {
        let unique = format!(
            "gmux-pane-panel-runtime-cwd-{}-{}",
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

        let mut app = crate::app::state::AppState::test_new();
        let mut workspace = Workspace::test_new("stale-name");
        workspace.custom_name = None;
        workspace.identity_cwd = stale_cwd.clone();
        let pane = workspace.tabs[0].root_pane;

        app.workspaces = vec![workspace];
        app.ensure_test_terminals();
        let terminal_id = app.workspaces[0].tabs[0].panes[&pane]
            .attached_terminal_id
            .clone();
        let terminal = app.terminals.get_mut(&terminal_id).unwrap();
        terminal.cwd = stale_cwd;
        app.active = Some(0);
        app.selected = 0;
        app.pane_panel_scope = PanePanelScope::All;

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

        let mut runtime_registry = TerminalRuntimeRegistry::new();
        runtime_registry.insert(terminal_id, runtime);
        let entries = pane_panel_entries_from(&app, &runtime_registry);
        let primary_label = entries[0].primary_label.clone();

        for (_, runtime) in runtime_registry.drain() {
            runtime.shutdown();
        }
        let _ = std::fs::remove_dir_all(root);

        assert_eq!(primary_label, "gmux");
    }

    #[test]
    fn all_primary_label_truncates_session_and_tab() {
        let entry = PanePanelEntry {
            ws_idx: 0,
            tab_idx: 0,
            pane_id: crate::layout::PaneId::from_raw(1),
            primary_label: "agent-browser".into(),
            primary_tab_label: Some("test-escalation".into()),
        };

        let label = format_pane_panel_primary_label(&entry, 18);

        assert_eq!(label, "agent-bro… · test…");
    }

    #[test]
    fn expanded_sidebar_uses_single_pane_panel_with_footer() {
        let area = Rect::new(0, 0, 20, 5);

        assert_eq!(expanded_sidebar_content_rect(area), Rect::new(0, 0, 19, 5));
        assert_eq!(expanded_pane_panel_rect(area), Rect::new(0, 0, 19, 4));
        assert_eq!(expanded_sidebar_footer_rect(area), Rect::new(0, 4, 19, 1));
    }
}
