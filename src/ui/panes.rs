use ratatui::{
    layout::{Direction, Rect},
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Borders, Paragraph},
    Frame,
};
use std::collections::HashMap;

use super::scrollbar::{render_pane_scrollbar, should_show_scrollbar};
use super::widgets::panel_contrast_fg;
use crate::app::state::Palette;
use crate::app::{AppState, Mode};
use crate::layout::{PaneInfo, SplitBorder};
use crate::terminal::{TerminalRuntime, TerminalRuntimeRegistry};

pub(crate) fn pane_is_scrolled_back(rt: &TerminalRuntime) -> bool {
    rt.scroll_metrics()
        .is_some_and(|metrics| metrics.offset_from_bottom > 0)
}

fn terminal_inner_rect_with_scrollbar(pane_inner: Rect) -> (Rect, Rect) {
    if pane_inner.width <= 4 {
        return (pane_inner, Rect::default());
    }

    let inner_rect = Rect::new(
        pane_inner.x,
        pane_inner.y,
        pane_inner.width.saturating_sub(1),
        pane_inner.height,
    );
    let scrollbar_rect = Rect::new(
        pane_inner.x + pane_inner.width.saturating_sub(1),
        pane_inner.y,
        1,
        pane_inner.height,
    );
    (inner_rect, scrollbar_rect)
}

fn terminal_inner_rect(rt: &TerminalRuntime, pane_inner: Rect) -> (Rect, Option<Rect>) {
    if !rt.scroll_metrics().is_some_and(should_show_scrollbar) {
        return (pane_inner, None);
    }

    let (inner_rect, scrollbar_rect) = terminal_inner_rect_with_scrollbar(pane_inner);
    (
        inner_rect,
        (scrollbar_rect.width > 0).then_some(scrollbar_rect),
    )
}

fn pane_inner_rect(area: Rect, framed: bool) -> Rect {
    if framed {
        Block::default().borders(Borders::ALL).inner(area)
    } else {
        area
    }
}

fn ranges_overlap(a_start: u16, a_len: u16, b_start: u16, b_len: u16) -> bool {
    a_start < b_start.saturating_add(b_len) && b_start < a_start.saturating_add(a_len)
}

fn merged_pane_inner_rect(rect: Rect, split_borders: &[SplitBorder]) -> Rect {
    let mut inner = rect;
    for border in split_borders {
        match border.direction {
            Direction::Horizontal
                if border.pos == rect.x
                    && ranges_overlap(rect.y, rect.height, border.area.y, border.area.height) =>
            {
                inner.x = inner.x.saturating_add(1);
                inner.width = inner.width.saturating_sub(1);
            }
            Direction::Vertical
                if border.pos == rect.y
                    && ranges_overlap(rect.x, rect.width, border.area.x, border.area.width) =>
            {
                inner.y = inner.y.saturating_add(1);
                inner.height = inner.height.saturating_sub(1);
            }
            _ => {}
        }
    }
    inner
}

fn runtime_for_tab_pane<'a>(
    terminal_runtimes: &'a TerminalRuntimeRegistry,
    tab: &'a crate::workspace::Tab,
    pane_id: crate::layout::PaneId,
) -> Option<(&'a crate::terminal::TerminalId, &'a TerminalRuntime)> {
    let terminal_id = tab.terminal_id(pane_id)?;
    #[cfg(test)]
    if let Some(runtime) = tab.runtimes.get(&pane_id) {
        return Some((terminal_id, runtime));
    }
    terminal_runtimes
        .get(terminal_id)
        .map(|runtime| (terminal_id, runtime))
}

/// Resize every visible runtime in a tab to the geometry it would receive if the tab were selected.
pub(super) fn resize_tab_panes(
    app: &AppState,
    terminal_runtimes: &TerminalRuntimeRegistry,
    tab: &crate::workspace::Tab,
    area: Rect,
    cell_size: crate::kitty_graphics::HostCellSize,
) {
    let multi_pane = tab.layout.pane_count() > 1;

    if tab.zoomed {
        let focused_id = tab.layout.focused();
        if let Some((terminal_id, rt)) = runtime_for_tab_pane(terminal_runtimes, tab, focused_id) {
            let pane_inner = if multi_pane {
                area
            } else {
                pane_inner_rect(area, false)
            };
            let (inner_rect, _) = terminal_inner_rect(rt, pane_inner);
            if !app.direct_attach_resize_locks.contains(terminal_id) {
                rt.resize(
                    inner_rect.height,
                    inner_rect.width,
                    cell_size.width_px,
                    cell_size.height_px,
                );
            }
        }
        return;
    }

    let split_borders = tab.layout.splits(area);
    for info in tab.layout.panes(area) {
        let pane_inner = if multi_pane {
            merged_pane_inner_rect(info.rect, &split_borders)
        } else {
            area
        };

        if let Some((terminal_id, rt)) = runtime_for_tab_pane(terminal_runtimes, tab, info.id) {
            let (inner_rect, _) = terminal_inner_rect(rt, pane_inner);
            if !app.direct_attach_resize_locks.contains(terminal_id) {
                rt.resize(
                    inner_rect.height,
                    inner_rect.width,
                    cell_size.width_px,
                    cell_size.height_px,
                );
            }
        }
    }
}

/// Compute pane layout info and optionally resize pane runtimes to match.
pub(super) fn compute_pane_infos(
    app: &AppState,
    terminal_runtimes: &TerminalRuntimeRegistry,
    area: Rect,
    resize_panes: bool,
    cell_size: crate::kitty_graphics::HostCellSize,
) -> Vec<PaneInfo> {
    let Some(ws_idx) = app.session_index() else {
        return Vec::new();
    };
    let Some(ws) = app.session() else {
        return Vec::new();
    };

    let multi_pane = ws.layout.pane_count() > 1;

    if ws.zoomed {
        let focused_id = ws.layout.focused();
        let pane_inner = if multi_pane {
            area
        } else {
            pane_inner_rect(area, false)
        };
        let mut inner_rect = pane_inner;
        let mut scrollbar_rect = None;
        if let Some(rt) = app.runtime_for_pane_in_session_at(terminal_runtimes, ws_idx, focused_id)
        {
            (inner_rect, scrollbar_rect) = terminal_inner_rect(rt, pane_inner);
            if resize_panes
                && ws.terminal_id(focused_id).is_some_and(|terminal_id| {
                    !app.direct_attach_resize_locks.contains(terminal_id)
                })
            {
                rt.resize(
                    inner_rect.height,
                    inner_rect.width,
                    cell_size.width_px,
                    cell_size.height_px,
                );
            }
        }
        return vec![PaneInfo {
            id: focused_id,
            rect: area,
            inner_rect,
            scrollbar_rect,
            is_focused: true,
        }];
    }

    let mut pane_infos = ws.layout.panes(area);
    let split_borders = ws.layout.splits(area);

    for info in &mut pane_infos {
        let pane_inner = if multi_pane {
            merged_pane_inner_rect(info.rect, &split_borders)
        } else {
            area
        };

        let mut inner_rect = pane_inner;
        let mut scrollbar_rect = None;
        if let Some(rt) = app.runtime_for_pane_in_session_at(terminal_runtimes, ws_idx, info.id) {
            (inner_rect, scrollbar_rect) = terminal_inner_rect(rt, pane_inner);
            if resize_panes
                && ws.terminal_id(info.id).is_some_and(|terminal_id| {
                    !app.direct_attach_resize_locks.contains(terminal_id)
                })
            {
                rt.resize(
                    inner_rect.height,
                    inner_rect.width,
                    cell_size.width_px,
                    cell_size.height_px,
                );
            }
        }

        info.inner_rect = inner_rect;
        info.scrollbar_rect = scrollbar_rect;
    }

    pane_infos
}

pub(super) fn render_panes(
    app: &AppState,
    terminal_runtimes: &TerminalRuntimeRegistry,
    frame: &mut Frame,
    area: Rect,
) {
    let Some(ws_idx) = app.session_index() else {
        render_empty(app, frame, area);
        return;
    };
    let Some(ws) = app.session() else {
        render_empty(app, frame, area);
        return;
    };

    let multi_pane = ws.layout.pane_count() > 1;
    let terminal_active = app.mode == Mode::Terminal;

    for info in &app.view.pane_infos {
        if let Some(rt) = app.runtime_for_pane_in_session_at(terminal_runtimes, ws_idx, info.id) {
            let show_cursor = info.is_focused && terminal_active && !pane_is_scrolled_back(rt);
            rt.render(frame, info.inner_rect, show_cursor);
            render_pane_scrollbar(app, frame, info, rt);

            let should_dim = !info.is_focused && multi_pane && !terminal_active;
            if should_dim {
                let inner = info.inner_rect;
                let buf = frame.buffer_mut();
                for y in inner.y..inner.y + inner.height {
                    for x in inner.x..inner.x + inner.width {
                        let cell = &mut buf[(x, y)];
                        cell.set_style(cell.style().add_modifier(Modifier::DIM));
                    }
                }
            }

            render_selection_highlight(
                &app.selection,
                frame,
                info.id,
                info.inner_rect,
                rt.scroll_metrics(),
                &app.palette,
                app.host_terminal_theme,
            );
            render_copy_mode_cursor(app, frame, info);
        }
    }

    render_pane_borders(app, frame, terminal_active);
}

fn top_separator_rect(app: &AppState) -> Option<Rect> {
    if app.view.layout != crate::app::state::ViewLayout::Desktop {
        return None;
    }

    let terminal = app.view.terminal_area;
    if terminal.width == 0 || terminal.y == 0 {
        return None;
    }

    Some(Rect::new(terminal.x, terminal.y - 1, terminal.width, 1))
}

#[derive(Clone, Copy, Default)]
struct BorderCell {
    up: bool,
    down: bool,
    left: bool,
    right: bool,
    focused: bool,
}

impl BorderCell {
    fn horizontal(focused: bool, left: bool, right: bool) -> Self {
        Self {
            left,
            right,
            focused,
            ..Self::default()
        }
    }

    fn vertical(focused: bool, up: bool, down: bool) -> Self {
        Self {
            up,
            down,
            focused,
            ..Self::default()
        }
    }
}

fn merge_border_cell(
    cells: &mut HashMap<(u16, u16), BorderCell>,
    x: u16,
    y: u16,
    cell: BorderCell,
) {
    let entry = cells.entry((x, y)).or_default();
    entry.up |= cell.up;
    entry.down |= cell.down;
    entry.left |= cell.left;
    entry.right |= cell.right;
    entry.focused |= cell.focused;
}

fn border_symbol(cell: BorderCell) -> &'static str {
    match (cell.up, cell.down, cell.left, cell.right) {
        (true, true, true, true) => "┼",
        (false, true, true, true) => "┬",
        (true, false, true, true) => "┴",
        (true, true, false, true) => "├",
        (true, true, true, false) => "┤",
        (false, true, false, true) => "┌",
        (false, true, true, false) => "┐",
        (true, false, false, true) => "└",
        (true, false, true, false) => "┘",
        (_, _, true, true) | (_, _, true, false) | (_, _, false, true) => "─",
        (true, true, _, _) | (true, false, _, _) | (false, true, _, _) => "│",
        _ => " ",
    }
}

fn collect_top_separator_cells(
    app: &AppState,
    terminal_active: bool,
    cells: &mut HashMap<(u16, u16), BorderCell>,
) {
    let Some(separator) = top_separator_rect(app) else {
        return;
    };
    let focused = app
        .view
        .pane_infos
        .iter()
        .find(|info| info.is_focused)
        .map(|info| info.rect);

    for x in separator.x..separator.x + separator.width {
        let focused_segment = terminal_active
            && focused.is_some_and(|rect| {
                rect.y == app.view.terminal_area.y
                    && x >= rect.x
                    && x < rect.x.saturating_add(rect.width)
            });
        merge_border_cell(
            cells,
            x,
            separator.y,
            BorderCell::horizontal(
                focused_segment,
                x > separator.x,
                x + 1 < separator.x + separator.width,
            ),
        );
    }
}

fn split_border_style(app: &AppState, focused_segment: bool) -> Style {
    if focused_segment {
        Style::default().fg(app.palette.accent)
    } else {
        Style::default().fg(app.palette.overlay0)
    }
}

fn overlapping_range(a_start: u16, a_len: u16, b_start: u16, b_len: u16) -> Option<(u16, u16)> {
    let start = a_start.max(b_start);
    let end = a_start
        .saturating_add(a_len)
        .min(b_start.saturating_add(b_len));
    (start < end).then_some((start, end))
}

fn focused_split_border_segment(
    border: &SplitBorder,
    focused: Rect,
    panes: &[PaneInfo],
) -> Option<(u16, u16)> {
    match border.direction {
        Direction::Horizontal => {
            let focused_left_of_border = border.pos == focused.x.saturating_add(focused.width);
            let focused_right_of_border = border.pos == focused.x;
            if !focused_left_of_border && !focused_right_of_border {
                return None;
            }

            let overlap =
                overlapping_range(focused.y, focused.height, border.area.y, border.area.height)?;
            if overlap
                == (
                    border.area.y,
                    border.area.y.saturating_add(border.area.height),
                )
                && panes.len() == 2
            {
                let midpoint = border.area.y + border.area.height.saturating_add(1) / 2;
                if focused_left_of_border {
                    Some((border.area.y, midpoint))
                } else {
                    Some((midpoint, border.area.y.saturating_add(border.area.height)))
                }
            } else {
                Some(overlap)
            }
        }
        Direction::Vertical => {
            if border.pos != focused.y {
                return None;
            }
            overlapping_range(focused.x, focused.width, border.area.x, border.area.width)
        }
    }
}

fn collect_split_border_cells(
    app: &AppState,
    terminal_active: bool,
    cells: &mut HashMap<(u16, u16), BorderCell>,
) {
    let focused = app
        .view
        .pane_infos
        .iter()
        .find(|info| info.is_focused)
        .map(|info| info.rect);
    let terminal = app.view.terminal_area;
    let terminal_right = terminal.x.saturating_add(terminal.width);
    let terminal_bottom = terminal.y.saturating_add(terminal.height);
    let top_separator = top_separator_rect(app);

    for border in &app.view.split_borders {
        let focused_segment = if terminal_active {
            focused
                .and_then(|rect| focused_split_border_segment(border, rect, &app.view.pane_infos))
        } else {
            None
        };
        match border.direction {
            Direction::Horizontal => {
                let x = border.pos;
                if x < terminal.x || x >= terminal_right {
                    continue;
                }
                let y_start = border.area.y.max(terminal.y);
                let y_end = border
                    .area
                    .y
                    .saturating_add(border.area.height)
                    .min(terminal_bottom);
                if y_start >= y_end {
                    continue;
                }
                if y_start == terminal.y {
                    if let Some(separator) = top_separator {
                        let focused_cell = focused_segment
                            .is_some_and(|(start, end)| y_start >= start && y_start < end);
                        merge_border_cell(
                            cells,
                            x,
                            separator.y,
                            BorderCell::vertical(focused_cell, false, true),
                        );
                    }
                }
                for y in y_start..y_end {
                    merge_border_cell(
                        cells,
                        x,
                        y,
                        BorderCell::vertical(
                            focused_segment.is_some_and(|(start, end)| y >= start && y < end),
                            y > y_start || top_separator.is_some_and(|_| y_start == terminal.y),
                            y + 1 < y_end,
                        ),
                    );
                }
            }
            Direction::Vertical => {
                let y = border.pos;
                if y < terminal.y || y >= terminal_bottom {
                    continue;
                }
                let x_start = border.area.x.max(terminal.x);
                let x_end = border
                    .area
                    .x
                    .saturating_add(border.area.width)
                    .min(terminal_right);
                if x_start >= x_end {
                    continue;
                }
                for x in x_start..x_end {
                    merge_border_cell(
                        cells,
                        x,
                        y,
                        BorderCell::horizontal(
                            focused_segment.is_some_and(|(start, end)| x >= start && x < end),
                            x > x_start,
                            x + 1 < x_end,
                        ),
                    );
                }
            }
        }
    }
}

fn render_pane_borders(app: &AppState, frame: &mut Frame, terminal_active: bool) {
    let mut cells = HashMap::new();
    collect_top_separator_cells(app, terminal_active, &mut cells);
    collect_split_border_cells(app, terminal_active, &mut cells);

    for ((x, y), cell) in cells {
        frame.buffer_mut()[(x, y)]
            .set_symbol(border_symbol(cell))
            .set_style(split_border_style(app, cell.focused));
    }
}

fn render_copy_mode_cursor(app: &AppState, frame: &mut Frame, info: &PaneInfo) {
    if app.mode != Mode::Copy {
        return;
    }
    let Some(copy_mode) = app.copy_mode else {
        return;
    };
    if copy_mode.pane_id != info.id
        || copy_mode.cursor_row >= info.inner_rect.height
        || copy_mode.cursor_col >= info.inner_rect.width
    {
        return;
    }

    let x = info.inner_rect.x + copy_mode.cursor_col;
    let y = info.inner_rect.y + copy_mode.cursor_row;
    let cell = &mut frame.buffer_mut()[(x, y)];
    cell.set_style(
        Style::default()
            .fg(panel_contrast_fg(&app.palette))
            .bg(app.palette.accent)
            .add_modifier(Modifier::BOLD),
    );
}

fn render_selection_highlight(
    selection: &Option<crate::selection::Selection>,
    frame: &mut Frame,
    pane_id: crate::layout::PaneId,
    inner: Rect,
    scroll_metrics: Option<crate::pane::ScrollMetrics>,
    p: &Palette,
    host_theme: crate::terminal_theme::TerminalTheme,
) {
    if let Some(sel) = selection {
        if sel.is_visible() && sel.pane_id == pane_id {
            let buf = frame.buffer_mut();
            let style = automatic_selection_style(p, host_theme);
            for y in 0..inner.height {
                for x in 0..inner.width {
                    if sel.contains(y, x, scroll_metrics) {
                        let cell = &mut buf[(inner.x + x, inner.y + y)];
                        cell.set_style(style);
                    }
                }
            }
        }
    }
}

type Rgb = (u8, u8, u8);

fn automatic_selection_style(
    p: &Palette,
    host_theme: crate::terminal_theme::TerminalTheme,
) -> Style {
    let bg = automatic_selection_bg(p, host_theme);
    Style::reset().fg(selection_fg_for_bg(bg, p)).bg(bg)
}

fn automatic_selection_bg(p: &Palette, host_theme: crate::terminal_theme::TerminalTheme) -> Color {
    let Some(background) = host_theme.background.map(terminal_theme_to_rgb) else {
        return selection_palette_background(p);
    };

    let target = if relative_luminance(background) < 0.5 {
        (255, 255, 255)
    } else {
        (0, 0, 0)
    };
    let selected = mix_rgb(background, target, 0.28);
    Color::Rgb(selected.0, selected.1, selected.2)
}

fn selection_palette_background(p: &Palette) -> Color {
    if p.panel_bg == Color::Reset {
        p.surface_dim
    } else {
        p.panel_bg
    }
}

fn terminal_theme_to_rgb(color: crate::terminal_theme::RgbColor) -> Rgb {
    (color.r, color.g, color.b)
}

fn selection_fg_for_bg(bg: Color, p: &Palette) -> Color {
    color_to_rgb(bg)
        .map(|bg| {
            if relative_luminance(bg) < 0.5 {
                Color::White
            } else {
                Color::Black
            }
        })
        .unwrap_or_else(|| panel_contrast_fg(p))
}

fn mix_rgb(base: Rgb, target: Rgb, amount: f32) -> Rgb {
    fn channel(base: u8, target: u8, amount: f32) -> u8 {
        (f32::from(base) + (f32::from(target) - f32::from(base)) * amount).round() as u8
    }
    (
        channel(base.0, target.0, amount),
        channel(base.1, target.1, amount),
        channel(base.2, target.2, amount),
    )
}

fn relative_luminance(color: Rgb) -> f32 {
    fn channel(value: u8) -> f32 {
        let value = f32::from(value) / 255.0;
        if value <= 0.03928 {
            value / 12.92
        } else {
            ((value + 0.055) / 1.055).powf(2.4)
        }
    }
    0.2126 * channel(color.0) + 0.7152 * channel(color.1) + 0.0722 * channel(color.2)
}

fn color_to_rgb(color: Color) -> Option<Rgb> {
    match color {
        Color::Reset => None,
        Color::Black => Some((0, 0, 0)),
        Color::Red => Some((128, 0, 0)),
        Color::Green => Some((0, 128, 0)),
        Color::Yellow => Some((128, 128, 0)),
        Color::Blue => Some((0, 0, 128)),
        Color::Magenta => Some((128, 0, 128)),
        Color::Cyan => Some((0, 128, 128)),
        Color::Gray => Some((192, 192, 192)),
        Color::DarkGray => Some((128, 128, 128)),
        Color::LightRed => Some((255, 0, 0)),
        Color::LightGreen => Some((0, 255, 0)),
        Color::LightYellow => Some((255, 255, 0)),
        Color::LightBlue => Some((0, 0, 255)),
        Color::LightMagenta => Some((255, 0, 255)),
        Color::LightCyan => Some((0, 255, 255)),
        Color::White => Some((255, 255, 255)),
        Color::Rgb(r, g, b) => Some((r, g, b)),
        Color::Indexed(_) => None,
    }
}

fn render_empty(app: &AppState, frame: &mut Frame, area: Rect) {
    let p = &app.palette;
    let lines = vec![
        Line::from(""),
        Line::from(""),
        Line::from(Span::styled(
            "  No session panes yet",
            Style::default().fg(p.overlay0),
        )),
        Line::from(""),
        Line::from(Span::styled(
            "  Create a tab from the CLI, then add panes as needed.",
            Style::default().fg(p.overlay1),
        )),
        Line::from(Span::styled(
            "  For example: gmux new-tab --focus",
            Style::default().fg(p.overlay1),
        )),
    ];
    frame.render_widget(
        Paragraph::new(lines).block(
            Block::default()
                .borders(Borders::ALL)
                .border_style(Style::default().fg(p.surface_dim)),
        ),
        area,
    );
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::layout::PaneId;
    use crate::selection::Selection;
    use crate::terminal::TerminalRuntime;
    use crate::workspace::Workspace;

    fn test_app_with_workspace(workspace: Workspace) -> AppState {
        let mut app = AppState::test_new();
        app.sessions = vec![workspace];
        app.active_session = Some(0);
        app.selected_session = 0;
        app.mode = Mode::Terminal;
        app
    }

    fn draw_panes(app: &mut AppState, width: u16, height: u16) -> ratatui::buffer::Buffer {
        crate::ui::compute_view(app, Rect::new(0, 0, width, height));
        let backend = ratatui::backend::TestBackend::new(width, height);
        let mut terminal = ratatui::Terminal::new(backend).expect("test terminal");
        let terminal_runtimes = TerminalRuntimeRegistry::new();
        terminal
            .draw(|frame| render_panes(app, &terminal_runtimes, frame, app.view.terminal_area))
            .expect("draw panes");
        terminal.backend().buffer().clone()
    }

    fn cell_fg(buffer: &ratatui::buffer::Buffer, x: u16, y: u16) -> Option<Color> {
        buffer[(x, y)].style().fg
    }

    fn cell_symbol(buffer: &ratatui::buffer::Buffer, x: u16, y: u16) -> &str {
        buffer[(x, y)].symbol()
    }

    #[tokio::test]
    async fn pane_uses_full_width_before_scrollback_exists() {
        let mut app = AppState::test_new();
        let mut workspace = Workspace::test_new("test");
        let root_pane = workspace.tabs[0].root_pane;
        workspace.tabs[0].runtimes.insert(
            root_pane,
            TerminalRuntime::test_with_scrollback_bytes(40, 8, 1024, b"ready\n"),
        );
        app.sessions = vec![workspace];
        app.active_session = Some(0);

        let area = Rect::new(10, 3, 40, 8);
        let terminal_runtimes = TerminalRuntimeRegistry::new();
        let infos = compute_pane_infos(
            &app,
            &terminal_runtimes,
            area,
            false,
            crate::kitty_graphics::HostCellSize::default(),
        );
        let info = &infos[0];

        assert_eq!(info.rect, area);
        assert_eq!(info.scrollbar_rect, None);
        assert_eq!(info.inner_rect, Rect::new(10, 3, 40, 8));
    }

    #[tokio::test]
    async fn zoomed_pane_uses_full_width_before_scrollback_exists() {
        let mut app = AppState::test_new();
        let mut workspace = Workspace::test_new("test");
        workspace.zoomed = true;
        let root_pane = workspace.tabs[0].root_pane;
        workspace.tabs[0].runtimes.insert(
            root_pane,
            TerminalRuntime::test_with_scrollback_bytes(40, 8, 1024, b"ready\n"),
        );
        app.sessions = vec![workspace];
        app.active_session = Some(0);

        let area = Rect::new(10, 3, 40, 8);
        let terminal_runtimes = TerminalRuntimeRegistry::new();
        let infos = compute_pane_infos(
            &app,
            &terminal_runtimes,
            area,
            false,
            crate::kitty_graphics::HostCellSize::default(),
        );
        let info = &infos[0];

        assert_eq!(info.rect, area);
        assert_eq!(info.scrollbar_rect, None);
        assert_eq!(info.inner_rect, Rect::new(10, 3, 40, 8));
    }

    #[tokio::test]
    async fn zoomed_multi_pane_uses_full_terminal_area() {
        let mut app = AppState::test_new();
        let mut workspace = Workspace::test_new("test");
        let focused_pane = workspace.test_split(ratatui::layout::Direction::Horizontal);
        workspace.zoomed = true;
        workspace.tabs[0].runtimes.insert(
            focused_pane,
            TerminalRuntime::test_with_scrollback_bytes(40, 8, 1024, b"ready\n"),
        );
        app.sessions = vec![workspace];
        app.active_session = Some(0);

        let area = Rect::new(10, 3, 40, 8);
        let terminal_runtimes = TerminalRuntimeRegistry::new();
        let infos = compute_pane_infos(
            &app,
            &terminal_runtimes,
            area,
            false,
            crate::kitty_graphics::HostCellSize::default(),
        );
        let info = &infos[0];

        assert_eq!(info.id, focused_pane);
        assert_eq!(info.rect, area);
        assert_eq!(info.scrollbar_rect, None);
        assert_eq!(info.inner_rect, Rect::new(10, 3, 40, 8));
    }

    #[tokio::test]
    async fn split_panes_share_one_border_column() {
        let mut app = AppState::test_new();
        let mut workspace = Workspace::test_new("test");
        let left = workspace.tabs[0].root_pane;
        let right = workspace.test_split(ratatui::layout::Direction::Horizontal);
        workspace.tabs[0].runtimes.insert(
            left,
            TerminalRuntime::test_with_screen_bytes(20, 8, b"left"),
        );
        workspace.tabs[0].runtimes.insert(
            right,
            TerminalRuntime::test_with_screen_bytes(20, 8, b"right"),
        );
        app.sessions = vec![workspace];
        app.active_session = Some(0);

        let area = Rect::new(0, 0, 40, 8);
        let terminal_runtimes = TerminalRuntimeRegistry::new();
        let infos = compute_pane_infos(
            &app,
            &terminal_runtimes,
            area,
            false,
            crate::kitty_graphics::HostCellSize::default(),
        );
        let left_info = infos.iter().find(|info| info.id == left).unwrap();
        let right_info = infos.iter().find(|info| info.id == right).unwrap();

        assert_eq!(left_info.inner_rect, Rect::new(0, 0, 20, 8));
        assert_eq!(right_info.inner_rect, Rect::new(21, 0, 19, 8));
    }

    #[test]
    fn zoomed_pane_does_not_render_hidden_split_border() {
        let mut workspace = Workspace::test_new("test");
        let focused_pane = workspace.test_split(ratatui::layout::Direction::Horizontal);
        workspace.tabs[0].layout.focus_pane(focused_pane);
        workspace.zoomed = true;
        let mut app = test_app_with_workspace(workspace);

        let buffer = draw_panes(&mut app, 100, 20);

        assert!(app.view.split_borders.is_empty());
        assert_eq!(app.view.pane_infos.len(), 1);
        assert_eq!(app.view.pane_infos[0].id, focused_pane);
        assert_eq!(cell_symbol(&buffer, 50, app.view.terminal_area.y), " ");
    }

    #[test]
    fn left_right_split_focus_owns_top_border_half_and_vertical_half() {
        let mut workspace = Workspace::test_new("test");
        let left = workspace.tabs[0].root_pane;
        let right = workspace.test_split(ratatui::layout::Direction::Horizontal);
        workspace.tabs[0].layout.focus_pane(left);
        let mut app = test_app_with_workspace(workspace);

        let buffer = draw_panes(&mut app, 100, 20);
        let accent = app.palette.accent;
        let neutral = app.palette.overlay0;

        assert_eq!(app.view.terminal_area, Rect::new(0, 2, 100, 18));
        assert_eq!(cell_fg(&buffer, 25, 1), Some(accent));
        assert_eq!(cell_fg(&buffer, 75, 1), Some(neutral));
        assert_eq!(cell_symbol(&buffer, 50, 1), "┬");
        assert_eq!(cell_fg(&buffer, 50, 2), Some(accent));
        assert_eq!(cell_fg(&buffer, 50, 10), Some(accent));
        assert_eq!(cell_fg(&buffer, 50, 11), Some(neutral));

        app.sessions[0].tabs[0].layout.focus_pane(right);
        let buffer = draw_panes(&mut app, 100, 20);

        assert_eq!(cell_fg(&buffer, 25, 1), Some(neutral));
        assert_eq!(cell_fg(&buffer, 75, 1), Some(accent));
        assert_eq!(cell_symbol(&buffer, 50, 1), "┬");
        assert_eq!(cell_fg(&buffer, 50, 2), Some(neutral));
        assert_eq!(cell_fg(&buffer, 50, 11), Some(accent));
        assert_eq!(cell_fg(&buffer, 50, 19), Some(accent));
    }

    #[test]
    fn top_bottom_split_focus_uses_top_or_middle_border() {
        let mut workspace = Workspace::test_new("test");
        let top = workspace.tabs[0].root_pane;
        let bottom = workspace.test_split(ratatui::layout::Direction::Vertical);
        workspace.tabs[0].layout.focus_pane(top);
        let mut app = test_app_with_workspace(workspace);

        let buffer = draw_panes(&mut app, 100, 20);
        let accent = app.palette.accent;
        let neutral = app.palette.overlay0;

        assert_eq!(cell_fg(&buffer, 25, 1), Some(accent));
        assert_eq!(cell_fg(&buffer, 75, 1), Some(accent));
        assert_eq!(cell_fg(&buffer, 50, 11), Some(neutral));

        app.sessions[0].tabs[0].layout.focus_pane(bottom);
        let buffer = draw_panes(&mut app, 100, 20);

        assert_eq!(cell_fg(&buffer, 25, 1), Some(neutral));
        assert_eq!(cell_fg(&buffer, 75, 1), Some(neutral));
        assert_eq!(cell_fg(&buffer, 50, 11), Some(accent));
    }

    #[test]
    fn three_pane_focus_highlights_adjacent_owned_segments() {
        let mut workspace = Workspace::test_new("test");
        let left = workspace.tabs[0].root_pane;
        let right_top = workspace.test_split(ratatui::layout::Direction::Horizontal);
        let right_bottom = workspace.test_split(ratatui::layout::Direction::Vertical);
        workspace.tabs[0].layout.focus_pane(left);
        let mut app = test_app_with_workspace(workspace);

        let buffer = draw_panes(&mut app, 100, 20);
        let accent = app.palette.accent;
        let neutral = app.palette.overlay0;

        assert_eq!(cell_fg(&buffer, 25, 1), Some(accent));
        assert_eq!(cell_fg(&buffer, 75, 1), Some(neutral));
        assert_eq!(cell_fg(&buffer, 50, 2), Some(accent));
        assert_eq!(cell_fg(&buffer, 50, 11), Some(accent));
        assert_eq!(cell_fg(&buffer, 50, 19), Some(accent));

        app.sessions[0].tabs[0].layout.focus_pane(right_top);
        let buffer = draw_panes(&mut app, 100, 20);

        assert_eq!(cell_fg(&buffer, 25, 1), Some(neutral));
        assert_eq!(cell_fg(&buffer, 75, 1), Some(accent));
        assert_eq!(cell_fg(&buffer, 50, 2), Some(accent));
        assert_eq!(cell_fg(&buffer, 50, 10), Some(accent));
        assert_eq!(cell_fg(&buffer, 50, 11), Some(neutral));
        assert_eq!(cell_symbol(&buffer, 50, 11), "├");
        assert_eq!(cell_fg(&buffer, 75, 11), Some(neutral));

        app.sessions[0].tabs[0].layout.focus_pane(right_bottom);
        let buffer = draw_panes(&mut app, 100, 20);

        assert_eq!(cell_fg(&buffer, 75, 1), Some(neutral));
        assert_eq!(cell_fg(&buffer, 50, 10), Some(neutral));
        assert_eq!(cell_fg(&buffer, 50, 11), Some(accent));
        assert_eq!(cell_fg(&buffer, 75, 11), Some(accent));
        assert_eq!(cell_fg(&buffer, 50, 19), Some(accent));
    }

    #[test]
    fn three_column_middle_focus_highlights_both_full_borders() {
        let mut workspace = Workspace::test_new("test");
        let _left = workspace.tabs[0].root_pane;
        let middle = workspace.test_split(ratatui::layout::Direction::Horizontal);
        let _right = workspace.test_split(ratatui::layout::Direction::Horizontal);
        workspace.tabs[0].layout.focus_pane(middle);
        let mut app = test_app_with_workspace(workspace);

        let buffer = draw_panes(&mut app, 100, 20);
        let accent = app.palette.accent;

        for y in app.view.terminal_area.y..app.view.terminal_area.y + app.view.terminal_area.height
        {
            assert_eq!(cell_fg(&buffer, 50, y), Some(accent));
            assert_eq!(cell_fg(&buffer, 75, y), Some(accent));
        }
    }

    #[tokio::test]
    async fn tiny_pane_does_not_reserve_scrollbar_gutter() {
        let mut app = AppState::test_new();
        let mut workspace = Workspace::test_new("test");
        let root_pane = workspace.tabs[0].root_pane;
        workspace.tabs[0].runtimes.insert(
            root_pane,
            TerminalRuntime::test_with_scrollback_bytes(4, 8, 1024, b"ready\n"),
        );
        app.sessions = vec![workspace];
        app.active_session = Some(0);

        let area = Rect::new(10, 3, 4, 8);
        let terminal_runtimes = TerminalRuntimeRegistry::new();
        let infos = compute_pane_infos(
            &app,
            &terminal_runtimes,
            area,
            false,
            crate::kitty_graphics::HostCellSize::default(),
        );
        let info = &infos[0];

        assert_eq!(info.rect, area);
        assert_eq!(info.scrollbar_rect, None);
        assert_eq!(info.inner_rect, area);
    }

    #[tokio::test]
    async fn pane_scrollbar_reserves_last_column_from_terminal_area() {
        let mut app = AppState::test_new();
        let mut workspace = Workspace::test_new("test");
        let root_pane = workspace.tabs[0].root_pane;
        workspace.tabs[0].runtimes.insert(
            root_pane,
            TerminalRuntime::test_with_scrollback_bytes(
                40,
                8,
                1024,
                b"one\ntwo\nthree\nfour\nfive\nsix\nseven\neight\nnine\nten\n",
            ),
        );
        app.sessions = vec![workspace];
        app.active_session = Some(0);

        let area = Rect::new(10, 3, 40, 8);
        let terminal_runtimes = TerminalRuntimeRegistry::new();
        let infos = compute_pane_infos(
            &app,
            &terminal_runtimes,
            area,
            false,
            crate::kitty_graphics::HostCellSize::default(),
        );
        let info = &infos[0];

        assert_eq!(info.rect, area);
        assert_eq!(info.scrollbar_rect, Some(Rect::new(49, 3, 1, 8)));
        assert_eq!(info.inner_rect, Rect::new(10, 3, 39, 8));
    }

    #[test]
    fn selection_highlight_uses_one_uniform_style() {
        let palette = Palette::catppuccin();
        let host_theme = crate::terminal_theme::TerminalTheme {
            foreground: None,
            background: Some(crate::terminal_theme::RgbColor {
                r: 12,
                g: 14,
                b: 16,
            }),
        };
        let expected_style = automatic_selection_style(&palette, host_theme);
        let selection = Some(Selection::range(PaneId::from_raw(1), 0, 0, 2, None));
        let backend = ratatui::backend::TestBackend::new(4, 1);
        let mut terminal = ratatui::Terminal::new(backend).unwrap();

        terminal
            .draw(|frame| {
                let buf = frame.buffer_mut();
                buf[(0, 0)].set_style(
                    Style::default()
                        .fg(Color::Rgb(10, 220, 120))
                        .bg(Color::Black),
                );
                buf[(1, 0)].set_style(
                    Style::default()
                        .fg(Color::Rgb(220, 180, 40))
                        .bg(Color::DarkGray)
                        .add_modifier(Modifier::BOLD),
                );
                buf[(2, 0)].set_style(Style::default().fg(Color::Blue).bg(Color::Reset));
                render_selection_highlight(
                    &selection,
                    frame,
                    PaneId::from_raw(1),
                    Rect::new(0, 0, 4, 1),
                    None,
                    &palette,
                    host_theme,
                );
            })
            .unwrap();

        let buffer = terminal.backend().buffer();
        let first = buffer[(0, 0)].style();
        let second = buffer[(1, 0)].style();
        let third = buffer[(2, 0)].style();

        assert_eq!(first.fg, expected_style.fg);
        assert_eq!(second.fg, expected_style.fg);
        assert_eq!(third.fg, expected_style.fg);
        assert_eq!(first.bg, expected_style.bg);
        assert_eq!(second.bg, expected_style.bg);
        assert_eq!(third.bg, expected_style.bg);
        assert_eq!(first.add_modifier, expected_style.add_modifier);
        assert_eq!(second.add_modifier, expected_style.add_modifier);
        assert_eq!(third.add_modifier, expected_style.add_modifier);
        assert!(!second.add_modifier.contains(Modifier::BOLD));
    }

    #[test]
    fn automatic_selection_background_uses_host_background() {
        let bg = automatic_selection_bg(
            &Palette::terminal(),
            crate::terminal_theme::TerminalTheme {
                foreground: Some(crate::terminal_theme::RgbColor {
                    r: 230,
                    g: 230,
                    b: 230,
                }),
                background: Some(crate::terminal_theme::RgbColor {
                    r: 12,
                    g: 14,
                    b: 16,
                }),
            },
        );

        let Color::Rgb(r, g, b) = bg else {
            panic!("selection background should resolve to rgb");
        };
        assert!(relative_luminance((r, g, b)) > relative_luminance((12, 14, 16)));
    }

    #[test]
    fn empty_pane_view_does_not_advertise_workspace_creation() {
        let app = AppState::test_new();
        let mut terminal = ratatui::Terminal::new(ratatui::backend::TestBackend::new(80, 12))
            .expect("test terminal");
        let terminal_runtimes = TerminalRuntimeRegistry::new();

        terminal
            .draw(|frame| render_panes(&app, &terminal_runtimes, frame, Rect::new(0, 0, 80, 12)))
            .expect("draw empty panes");

        let rendered = terminal
            .backend()
            .buffer()
            .content()
            .iter()
            .map(|cell| cell.symbol())
            .collect::<String>();
        assert!(rendered.contains("No session panes yet"));
        assert!(rendered.contains("gmux new-tab --focus"));
        assert!(!rendered.contains("workspace"));
        assert!(!rendered.contains("unset"));
        assert!(!rendered.contains("create one"));
    }
}
