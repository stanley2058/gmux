use crate::terminal::TerminalId;

/// Viewport state for a pane.
///
/// Terminal identity, cwd, and labels live in TerminalState.
pub struct PaneState {
    pub attached_terminal_id: TerminalId,
    /// Whether the user has seen this pane since its last activity marker.
    pub seen: bool,
}

impl PaneState {
    pub fn new(attached_terminal_id: TerminalId) -> Self {
        Self {
            attached_terminal_id,
            seen: true,
        }
    }
}
