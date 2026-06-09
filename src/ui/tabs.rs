use ratatui::{
    layout::Rect,
    style::{Modifier, Style},
    text::{Line, Span},
    widgets::Paragraph,
    Frame,
};

use super::widgets::panel_contrast_fg;
use crate::app::state::Palette;
use crate::app::AppState;
use crate::session;
use crate::terminal::TerminalRuntimeRegistry;
use crate::workspace::{SessionUiState, Tab};

const NEW_TAB_WIDTH: u16 = 3;
const TAB_SCROLL_BUTTON_WIDTH: u16 = 3;
const TOP_BAR_EDGE_PADDING: u16 = 1;
const SESSION_MAX_WIDTH: u16 = 24;
const PROGRAM_MAX_WIDTH: u16 = 18;

#[derive(Clone, Copy)]
enum TopBarMenuSegmentKind {
    AttentionBadge,
    Label,
}

#[derive(Clone)]
struct TopBarMenuSegment {
    text: String,
    kind: TopBarMenuSegmentKind,
}

#[derive(Debug, Clone, Default)]
pub(crate) struct TabBarView {
    pub scroll: usize,
    pub tab_hit_areas: Vec<Rect>,
    pub scroll_left_hit_area: Rect,
    pub scroll_right_hit_area: Rect,
    pub new_tab_hit_area: Rect,
}

fn truncate_label(text: &str, max_width: u16) -> String {
    let max_width = max_width as usize;
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
    let prefix: String = text.chars().take(max_width - 1).collect();
    format!("{prefix}…")
}

fn tab_bar_label(tab: &Tab) -> String {
    match &tab.custom_name {
        Some(name) => format!("{}: {}", tab.number, name),
        None => tab.number.to_string(),
    }
}

fn tab_width(tab: &Tab) -> u16 {
    tab_bar_label(tab).chars().count() as u16 + 2
}

fn launch_label(argv: Option<&Vec<String>>) -> Option<String> {
    let command = argv?.first()?;
    std::path::Path::new(command)
        .file_name()
        .and_then(|name| name.to_str())
        .map(str::to_string)
        .or_else(|| Some(command.clone()))
}

fn focused_program_name(app: &AppState, terminal_runtimes: &TerminalRuntimeRegistry) -> String {
    let ws_idx = match app.session_index() {
        Some(ws_idx) => ws_idx,
        None => return "shell".to_string(),
    };
    let Some(ws) = app.session() else {
        return "shell".to_string();
    };
    let Some(pane_id) = ws.focused_pane_id() else {
        return "shell".to_string();
    };

    app.runtime_for_pane_in_session_at(terminal_runtimes, ws_idx, pane_id)
        .and_then(|runtime| runtime.foreground_process_name())
        .or_else(|| {
            ws.pane_state(pane_id)
                .and_then(|pane| app.terminals.get(&pane.attached_terminal_id))
                .and_then(|terminal| launch_label(terminal.launch_argv.as_ref()))
        })
        .unwrap_or_else(|| "shell".to_string())
}

fn session_name(_app: &AppState, _terminal_runtimes: &TerminalRuntimeRegistry) -> String {
    session::active_display_name()
}

fn top_bar_menu_segments(app: &AppState) -> Vec<TopBarMenuSegment> {
    let mut segments = Vec::new();
    if app.global_menu_attention_badge_visible() {
        segments.push(TopBarMenuSegment {
            text: "● ".to_string(),
            kind: TopBarMenuSegmentKind::AttentionBadge,
        });
    }
    segments.push(TopBarMenuSegment {
        text: if app.update.available.is_some() {
            "↑ menu".to_string()
        } else {
            "menu".to_string()
        },
        kind: TopBarMenuSegmentKind::Label,
    });
    segments
}

fn top_bar_menu_segments_width(segments: &[TopBarMenuSegment]) -> u16 {
    segments
        .iter()
        .map(|segment| segment.text.chars().count() as u16)
        .sum()
}

pub(crate) fn top_bar_menu_width(app: &AppState) -> u16 {
    top_bar_menu_segments_width(&top_bar_menu_segments(app))
        .saturating_add(TOP_BAR_EDGE_PADDING.saturating_mul(2))
}

fn top_bar_menu_bg(app: &AppState, p: &Palette) -> ratatui::style::Color {
    if app.update.available.is_some() {
        p.green
    } else {
        p.panel_bg
    }
}

fn top_bar_menu_line(app: &AppState, p: &Palette) -> Line<'static> {
    let bg = top_bar_menu_bg(app, p);
    let padding_style = Style::default().bg(bg);
    let mut spans = vec![Span::styled(" ", padding_style)];
    spans.extend(top_bar_menu_segments(app).into_iter().map(|segment| {
        let style = match segment.kind {
            TopBarMenuSegmentKind::AttentionBadge => Style::default()
                .fg(p.accent)
                .bg(bg)
                .add_modifier(Modifier::BOLD),
            TopBarMenuSegmentKind::Label if app.update.available.is_some() => Style::default()
                .fg(p.panel_bg)
                .bg(bg)
                .add_modifier(Modifier::BOLD),
            TopBarMenuSegmentKind::Label => Style::default().fg(p.overlay1).bg(bg),
        };
        Span::styled(segment.text, style)
    }));
    spans.push(Span::styled(" ", padding_style));
    Line::from(spans)
}

pub(crate) fn top_bar_tab_area(
    app: &AppState,
    terminal_runtimes: &TerminalRuntimeRegistry,
    area: Rect,
) -> Rect {
    if area.width == 0 || area.height == 0 {
        return Rect::default();
    }

    let session_w = session_name(app, terminal_runtimes)
        .chars()
        .count()
        .min(SESSION_MAX_WIDTH as usize) as u16;
    let program_w = focused_program_name(app, terminal_runtimes)
        .chars()
        .count()
        .min(PROGRAM_MAX_WIDTH as usize) as u16;
    let left_w = session_w
        .saturating_add(TOP_BAR_EDGE_PADDING)
        .saturating_add(3)
        .saturating_add(program_w)
        .saturating_add(3);
    let menu_w = top_bar_menu_width(app).min(area.width);
    let tabs_x = area.x.saturating_add(left_w.min(area.width));
    let menu_x = area.x + area.width.saturating_sub(menu_w);
    if tabs_x >= menu_x {
        return Rect::new(tabs_x.min(menu_x), area.y, 0, 1);
    }
    Rect::new(tabs_x, area.y, menu_x - tabs_x, 1)
}

fn layout_tab_hit_areas(session: &SessionUiState, area: Rect, scroll: usize) -> Vec<Rect> {
    let mut rects = vec![Rect::default(); session.tabs.len()];
    if area.width == 0 || area.height == 0 {
        return rects;
    }

    let mut x = area.x;
    let right = area.x + area.width;
    for (idx, rect) in rects.iter_mut().enumerate().skip(scroll) {
        if x >= right {
            break;
        }
        let desired = tab_width(&session.tabs[idx]);
        let remaining = right.saturating_sub(x);
        let width = desired.min(remaining).max(1);
        *rect = Rect::new(x, area.y, width, 1);
        x = x.saturating_add(width + 1);
    }
    rects
}

fn centered_tab_scroll(session: &SessionUiState, area: Rect) -> usize {
    let mut best_scroll = session.active_tab;
    let mut best_distance = u16::MAX;
    let viewport_center = area.x.saturating_mul(2).saturating_add(area.width);

    for scroll in 0..=session.active_tab {
        let rects = layout_tab_hit_areas(session, area, scroll);
        let Some(active_rect) = rects.get(session.active_tab).copied() else {
            continue;
        };
        if active_rect.width == 0 {
            continue;
        }

        let active_center = active_rect
            .x
            .saturating_mul(2)
            .saturating_add(active_rect.width);
        let distance = active_center.abs_diff(viewport_center);
        if distance <= best_distance {
            best_distance = distance;
            best_scroll = scroll;
        }
    }

    best_scroll
}

fn trailing_tab_controls_x(tab_hit_areas: &[Rect], fallback_x: u16) -> u16 {
    tab_hit_areas
        .iter()
        .rev()
        .find(|rect| rect.width > 0)
        .map(|rect| rect.x + rect.width)
        .unwrap_or(fallback_x)
}

fn max_tab_scroll(session: &SessionUiState, area: Rect) -> usize {
    (0..session.tabs.len())
        .find(|&scroll| {
            layout_tab_hit_areas(session, area, scroll)
                .last()
                .is_some_and(|rect| rect.width > 0)
        })
        .unwrap_or(0)
}

pub(crate) fn compute_tab_bar_view(
    session: &SessionUiState,
    area: Rect,
    current_scroll: usize,
    follow_active: bool,
    mouse_chrome: bool,
) -> TabBarView {
    if area.width == 0 || area.height == 0 {
        return TabBarView::default();
    }

    if !mouse_chrome {
        let max_scroll = max_tab_scroll(session, area);
        let scroll = if follow_active {
            centered_tab_scroll(session, area).min(max_scroll)
        } else {
            current_scroll.min(max_scroll)
        };
        return TabBarView {
            scroll,
            tab_hit_areas: layout_tab_hit_areas(session, area, scroll),
            scroll_left_hit_area: Rect::default(),
            scroll_right_hit_area: Rect::default(),
            new_tab_hit_area: Rect::default(),
        };
    }

    let area_right = area.x + area.width;
    let all_tabs_area = Rect::new(
        area.x,
        area.y,
        area.width.saturating_sub(NEW_TAB_WIDTH),
        area.height,
    );
    let all_tabs = layout_tab_hit_areas(session, all_tabs_area, 0);
    let overflow = all_tabs.iter().any(|rect| rect.width == 0);
    if !overflow {
        let new_tab_x = trailing_tab_controls_x(&all_tabs, area.x);
        let new_tab_hit_area = Rect::new(
            new_tab_x,
            area.y,
            area_right.saturating_sub(new_tab_x).min(NEW_TAB_WIDTH),
            1,
        );
        return TabBarView {
            scroll: 0,
            tab_hit_areas: all_tabs,
            scroll_left_hit_area: Rect::default(),
            scroll_right_hit_area: Rect::default(),
            new_tab_hit_area,
        };
    }

    let left_hit_area = Rect::new(area.x, area.y, TAB_SCROLL_BUTTON_WIDTH.min(area.width), 1);
    let tab_area_x = left_hit_area.x + left_hit_area.width;
    let reserved_trailing_width = NEW_TAB_WIDTH.saturating_add(TAB_SCROLL_BUTTON_WIDTH);
    let tab_area_right = area_right.saturating_sub(reserved_trailing_width);
    let tab_area = Rect::new(
        tab_area_x,
        area.y,
        tab_area_right.saturating_sub(tab_area_x),
        area.height,
    );

    let max_scroll = max_tab_scroll(session, tab_area);
    let scroll = if follow_active {
        centered_tab_scroll(session, tab_area).min(max_scroll)
    } else {
        current_scroll.min(max_scroll)
    };
    let tab_hit_areas = layout_tab_hit_areas(session, tab_area, scroll);
    let trailing_x = trailing_tab_controls_x(&tab_hit_areas, tab_area_x).min(tab_area_right);
    let right_hit_area = Rect::new(
        trailing_x,
        area.y,
        area_right
            .saturating_sub(trailing_x)
            .min(TAB_SCROLL_BUTTON_WIDTH),
        1,
    );
    let new_tab_x = right_hit_area.x + right_hit_area.width;
    let new_tab_hit_area = Rect::new(
        new_tab_x,
        area.y,
        area_right.saturating_sub(new_tab_x).min(NEW_TAB_WIDTH),
        1,
    );

    TabBarView {
        scroll,
        tab_hit_areas,
        scroll_left_hit_area: left_hit_area,
        scroll_right_hit_area: right_hit_area,
        new_tab_hit_area,
    }
}

fn tab_drop_indicator_x(
    app: &AppState,
    session: &SessionUiState,
    insert_idx: usize,
) -> Option<u16> {
    let mut visible_tabs = app
        .view
        .tab_hit_areas
        .iter()
        .enumerate()
        .filter(|(_, rect)| rect.width > 0);
    let first_visible = visible_tabs.clone().next()?;
    let last_visible = visible_tabs.next_back().unwrap_or(first_visible);

    if insert_idx == 0 {
        return Some(if first_visible.0 == 0 {
            first_visible.1.x
        } else {
            app.view.tab_scroll_left_hit_area.x + app.view.tab_scroll_left_hit_area.width
        });
    }

    if let Some((_, rect)) = app
        .view
        .tab_hit_areas
        .iter()
        .enumerate()
        .find(|(idx, rect)| *idx == insert_idx && rect.width > 0)
    {
        return Some(rect.x.saturating_sub(1));
    }

    if insert_idx >= session.tabs.len() {
        return Some(if last_visible.0 + 1 >= session.tabs.len() {
            last_visible.1.x + last_visible.1.width
        } else {
            app.view.tab_scroll_right_hit_area.x.saturating_sub(1)
        });
    }

    None
}

pub(super) fn render_tab_bar(
    app: &AppState,
    terminal_runtimes: &TerminalRuntimeRegistry,
    frame: &mut Frame,
    area: Rect,
) {
    if area.width == 0 || area.height == 0 {
        return;
    }
    let Some(session_idx) = app.session_index() else {
        return;
    };
    let Some(ws) = app.session() else {
        return;
    };

    let p = &app.palette;

    frame.render_widget(
        Paragraph::new(" ".repeat(area.width as usize)).style(Style::default().bg(p.panel_bg)),
        area,
    );

    let session = truncate_label(&session_name(app, terminal_runtimes), SESSION_MAX_WIDTH);
    let program = truncate_label(
        &focused_program_name(app, terminal_runtimes),
        PROGRAM_MAX_WIDTH,
    );
    let tabs_area = top_bar_tab_area(app, terminal_runtimes, area);
    let left_width = tabs_area.x.saturating_sub(area.x).min(area.width);
    if left_width > 0 {
        let line = Line::from(vec![
            Span::styled(" ", Style::default().bg(p.panel_bg)),
            Span::styled(
                session,
                Style::default()
                    .fg(p.overlay1)
                    .bg(p.panel_bg)
                    .add_modifier(Modifier::BOLD),
            ),
            Span::styled(" | ", Style::default().fg(p.overlay0).bg(p.panel_bg)),
            Span::styled(program, Style::default().fg(p.teal).bg(p.panel_bg)),
            Span::styled(" | ", Style::default().fg(p.overlay0).bg(p.panel_bg)),
        ]);
        frame.render_widget(
            Paragraph::new(line),
            Rect::new(area.x, area.y, left_width, 1),
        );
    }

    let first_visible_idx = app
        .view
        .tab_hit_areas
        .iter()
        .enumerate()
        .find(|(_, rect)| rect.width > 0)
        .map(|(idx, _)| idx);
    let last_visible_idx = app
        .view
        .tab_hit_areas
        .iter()
        .enumerate()
        .rev()
        .find(|(_, rect)| rect.width > 0)
        .map(|(idx, _)| idx);
    let can_scroll_left = app.view.tab_scroll_left_hit_area.width > 0 && app.tab_scroll > 0;
    let can_scroll_right = app.view.tab_scroll_right_hit_area.width > 0
        && last_visible_idx.is_some_and(|idx| idx + 1 < ws.tabs.len());

    if app.mouse_capture && app.view.tab_scroll_left_hit_area.width > 0 {
        let style = if can_scroll_left {
            Style::default().fg(p.overlay1).bg(p.surface0)
        } else {
            Style::default()
                .fg(p.overlay0)
                .bg(p.surface0)
                .add_modifier(Modifier::DIM)
        };
        frame.render_widget(
            Paragraph::new(" < ").style(style),
            app.view.tab_scroll_left_hit_area,
        );
    }

    if app.mouse_capture && app.view.tab_scroll_right_hit_area.width > 0 {
        let style = if can_scroll_right {
            Style::default().fg(p.overlay1).bg(p.surface0)
        } else {
            Style::default()
                .fg(p.overlay0)
                .bg(p.surface0)
                .add_modifier(Modifier::DIM)
        };
        frame.render_widget(
            Paragraph::new(" > ").style(style),
            app.view.tab_scroll_right_hit_area,
        );
    }

    for (idx, tab) in ws.tabs.iter().enumerate() {
        let Some(rect) = app.view.tab_hit_areas.get(idx).copied() else {
            break;
        };
        if rect.width == 0 {
            continue;
        }
        let active = idx == ws.active_tab;
        let style = if active {
            let base = Style::default().fg(panel_contrast_fg(p)).bg(p.accent);
            if tab.is_auto_named() {
                base.add_modifier(Modifier::DIM)
            } else {
                base.add_modifier(Modifier::BOLD)
            }
        } else if tab.is_auto_named() {
            Style::default()
                .fg(p.overlay0)
                .bg(p.surface0)
                .add_modifier(Modifier::DIM)
        } else {
            Style::default().fg(p.overlay1).bg(p.surface0)
        };
        let width = rect.width as usize;
        let name = tab_bar_label(tab);
        let text = format!(" {name:width$}", width = width.saturating_sub(1));
        frame.render_widget(Paragraph::new(text).style(style), rect);
    }

    if let Some(crate::app::state::DragState {
        target:
            crate::app::state::DragTarget::TabReorder {
                session_idx: drag_session_idx,
                insert_idx: Some(insert_idx),
                ..
            },
    }) = &app.drag
    {
        if *drag_session_idx == session_idx {
            if let Some(x) = tab_drop_indicator_x(app, ws, *insert_idx) {
                frame.buffer_mut()[(x.min(area.x + area.width.saturating_sub(1)), area.y)]
                    .set_symbol("│")
                    .set_style(Style::default().fg(p.accent));
            }
        }
    }

    if app.mouse_capture && app.view.new_tab_hit_area.width > 0 {
        frame.render_widget(
            Paragraph::new(" + ").style(Style::default().fg(p.overlay1)),
            app.view.new_tab_hit_area,
        );
    }

    let menu_rect = app.global_launcher_rect();
    if menu_rect.width > 0 {
        frame.render_widget(Paragraph::new(top_bar_menu_line(app, p)), menu_rect);
    }

    if first_visible_idx.is_some_and(|idx| idx > 0) {
        let x = if app.mouse_capture && app.view.tab_scroll_left_hit_area.width > 0 {
            app.view.tab_scroll_left_hit_area.x + app.view.tab_scroll_left_hit_area.width
        } else {
            area.x
        };
        if x < area.x + area.width {
            frame.buffer_mut()[(x, area.y)]
                .set_symbol("…")
                .set_style(Style::default().fg(p.overlay0));
        }
    }
    if last_visible_idx.is_some_and(|idx| idx + 1 < ws.tabs.len()) {
        let x = if app.mouse_capture && app.view.tab_scroll_right_hit_area.width > 0 {
            app.view.tab_scroll_right_hit_area.x.saturating_sub(1)
        } else {
            area.x + area.width.saturating_sub(1)
        };
        if x >= area.x && x < area.x + area.width {
            frame.buffer_mut()[(x, area.y)]
                .set_symbol("…")
                .set_style(Style::default().fg(p.overlay0));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn launch_label_uses_command_basename() {
        assert_eq!(
            launch_label(Some(&vec!["/usr/bin/fish".to_string()])),
            Some("fish".to_string())
        );
    }

    #[test]
    fn tab_width_uses_label_with_side_padding() {
        let ws = crate::workspace::Workspace::test_new("test");

        assert_eq!(tab_width(&ws.tabs[0]), 3);
    }

    #[test]
    fn top_bar_menu_width_tracks_rendered_segments() {
        let mut app = AppState::test_new();
        let normal_segments = top_bar_menu_segments(&app);
        let attention_segments = vec![
            TopBarMenuSegment {
                text: "● ".to_string(),
                kind: TopBarMenuSegmentKind::AttentionBadge,
            },
            TopBarMenuSegment {
                text: "menu".to_string(),
                kind: TopBarMenuSegmentKind::Label,
            },
        ];

        assert_eq!(top_bar_menu_segments_width(&normal_segments), 4);
        assert_eq!(top_bar_menu_width(&app), 6);
        assert_eq!(top_bar_menu_segments_width(&attention_segments), 6);

        app.update.available = Some(crate::update::UpdateRelease {
            version: "0.2.0".to_string(),
            tag: "v0.2.0".to_string(),
            asset_name: "gmux-linux-x86_64.tar.gz".to_string(),
        });
        assert_eq!(top_bar_menu_width(&app), 8);
    }
}
