use std::sync::Arc;
use std::time::Instant;

use crate::protocol::{CellData, CursorState, FrameData};

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) struct ServerRenderDebug {
    pub(crate) render_us: Option<u64>,
    pub(crate) frame_build_us: Option<u64>,
}

#[derive(Clone, Debug)]
pub(crate) struct AppFrameSnapshot {
    pub(crate) generation: u64,
    pub(crate) active_client_id: u64,
    pub(crate) active_size: (u16, u16),
    pub(crate) frame: Arc<FrameData>,
    pub(crate) created_at: Instant,
    pub(crate) debug: ServerRenderDebug,
}

impl AppFrameSnapshot {
    pub(crate) fn new(
        generation: u64,
        active_client_id: u64,
        frame: FrameData,
        debug: ServerRenderDebug,
    ) -> Self {
        let active_size = (frame.width, frame.height);
        Self {
            generation,
            active_client_id,
            active_size,
            frame: Arc::new(frame),
            created_at: Instant::now(),
            debug,
        }
    }
}

pub(crate) fn fit_frame_to_client_size(frame: &FrameData, cols: u16, rows: u16) -> FrameData {
    if frame.width == cols && frame.height == rows {
        return frame.clone();
    }

    let blank = blank_cell();
    let source_width = usize::from(frame.width);
    let fitted_width = usize::from(cols);
    let fitted_height = usize::from(rows);
    let copy_width = usize::from(frame.width.min(cols));
    let copy_height = usize::from(frame.height.min(rows));

    let mut hyperlinks = Vec::<String>::new();
    let mut hyperlink_remap = Vec::<Option<u32>>::new();
    hyperlink_remap.resize(frame.hyperlinks.len(), None);

    let mut cells = vec![blank; fitted_width.saturating_mul(fitted_height)];
    for y in 0..copy_height {
        for x in 0..copy_width {
            let source_index = y * source_width + x;
            let target_index = y * fitted_width + x;
            let mut cell = frame.cells[source_index].clone();
            if let Some(index) = cell.hyperlink {
                cell.hyperlink = usize::try_from(index)
                    .ok()
                    .and_then(|index| frame.hyperlinks.get(index).map(|uri| (index, uri)))
                    .map(|(index, uri)| {
                        *hyperlink_remap[index].get_or_insert_with(|| {
                            let next = hyperlinks.len() as u32;
                            hyperlinks.push(uri.clone());
                            next
                        })
                    });
            }
            cells[target_index] = cell;
        }
    }

    let mut fitted = frame.clone();
    fitted.width = cols;
    fitted.height = rows;
    fitted.cells = cells;
    fitted.cursor = fit_cursor(frame.cursor.as_ref(), cols, rows);
    fitted.hyperlinks = hyperlinks;
    fitted.graphics.clear();
    fitted
}

fn blank_cell() -> CellData {
    CellData {
        symbol: " ".to_owned(),
        fg: 0,
        bg: 0,
        modifier: 0,
        underline_color: 0,
        underline_style: crate::protocol::UNDERLINE_NONE,
        overline: false,
        skip: false,
        hyperlink: None,
    }
}

fn fit_cursor(cursor: Option<&CursorState>, cols: u16, rows: u16) -> Option<CursorState> {
    cursor
        .filter(|cursor| cursor.x < cols && cursor.y < rows)
        .cloned()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::protocol::{CellData, CursorState, FrameData};

    fn cell(symbol: &str) -> CellData {
        CellData {
            symbol: symbol.to_owned(),
            fg: 0,
            bg: 0,
            modifier: 0,
            underline_color: 0,
            underline_style: crate::protocol::UNDERLINE_NONE,
            overline: false,
            skip: false,
            hyperlink: None,
        }
    }

    fn frame(width: u16, height: u16, symbols: &[&str]) -> FrameData {
        FrameData {
            cells: symbols.iter().map(|symbol| cell(symbol)).collect(),
            width,
            height,
            cursor: None,
            hyperlinks: Vec::new(),
            graphics: Vec::new(),
            debug_timing: None,
        }
    }

    #[test]
    fn fit_frame_clips_smaller_clients() {
        let mut frame = frame(3, 2, &["a", "b", "c", "d", "e", "f"]);
        frame.cursor = Some(CursorState {
            x: 2,
            y: 1,
            visible: true,
            shape: 0,
        });

        let fitted = fit_frame_to_client_size(&frame, 2, 1);

        assert_eq!(fitted.width, 2);
        assert_eq!(fitted.height, 1);
        assert_eq!(
            fitted
                .cells
                .iter()
                .map(|cell| cell.symbol.as_str())
                .collect::<Vec<_>>(),
            ["a", "b"]
        );
        assert_eq!(fitted.cursor, None);
    }

    #[test]
    fn fit_frame_pads_larger_clients() {
        let frame = frame(2, 1, &["a", "b"]);

        let fitted = fit_frame_to_client_size(&frame, 3, 2);

        assert_eq!(fitted.width, 3);
        assert_eq!(fitted.height, 2);
        assert_eq!(
            fitted
                .cells
                .iter()
                .map(|cell| cell.symbol.as_str())
                .collect::<Vec<_>>(),
            ["a", "b", " ", " ", " ", " "]
        );
    }

    #[test]
    fn fit_frame_remaps_visible_hyperlinks() {
        let mut frame = frame(3, 1, &["a", "b", "c"]);
        frame.hyperlinks = vec!["hidden".to_owned(), "visible".to_owned()];
        frame.cells[0].hyperlink = Some(0);
        frame.cells[1].hyperlink = Some(1);

        let fitted = fit_frame_to_client_size(&frame, 2, 1);

        assert_eq!(fitted.hyperlinks, vec!["hidden", "visible"]);
        assert_eq!(fitted.cells[0].hyperlink, Some(0));
        assert_eq!(fitted.cells[1].hyperlink, Some(1));

        let fitted = fit_frame_to_client_size(&frame, 1, 1);

        assert_eq!(fitted.hyperlinks, vec!["hidden"]);
        assert_eq!(fitted.cells[0].hyperlink, Some(0));
    }

    #[test]
    fn fit_frame_strips_graphics_for_fitted_mirrors() {
        let mut frame = frame(1, 1, &["a"]);
        frame.graphics = b"graphics".to_vec();

        let fitted = fit_frame_to_client_size(&frame, 2, 1);

        assert!(fitted.graphics.is_empty());
    }
}
