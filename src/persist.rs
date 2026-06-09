//! Session persistence — save/restore tabs, layouts, and working directories.
//!
//! Stored at `~/.config/gmux/session.json`.
//! Optional pane screen history is stored separately at `session-history.json`.

mod io;
mod restore;
mod snapshot;

pub use self::io::{clear, clear_history, load, load_history, save};
pub use self::restore::restore;
pub use self::restore::RestoreOptions;
#[cfg(unix)]
pub use self::restore::{handoff_pane_aliases, restore_handoff};
pub use self::snapshot::{
    capture, capture_history, DirectionSnapshot, LayoutSnapshot, SessionHistorySnapshot,
    SessionSnapshot, TabSnapshot,
};
