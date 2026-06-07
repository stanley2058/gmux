//! Internal app events delivered via channel.
//!
//! Background tasks (PTY child watchers, future hook listeners, etc.) send
//! events to the main loop through this channel. No polling needed.

use crate::layout::PaneId;
/// An event from a background task to the main loop.
#[derive(Debug)]
pub enum AppEvent {
    /// A pane's child process exited.
    PaneDied { pane_id: PaneId },
    /// A pane child emitted a valid OSC 52 clipboard write. The main loop
    /// re-emits it through gmux's own clipboard writer.
    ClipboardWrite { content: Vec<u8> },
}
