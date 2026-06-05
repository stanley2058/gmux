//! Internal app events delivered via channel.
//!
//! Background tasks (PTY child watchers, future hook listeners, etc.) send
//! events to the main loop through this channel. No polling needed.

use std::time::Instant;

use crate::detect::{Agent, AgentState};
use crate::layout::PaneId;
use crate::workspace::{GitStatusCacheEntry, WorkspaceGitStatus};

#[derive(Debug)]
pub struct WorktreeAddResult {
    pub path: std::path::PathBuf,
    pub result: Result<(), String>,
}

#[derive(Debug)]
pub struct WorktreeRemoveResult {
    pub workspace_id: String,
    pub path: std::path::PathBuf,
    pub result: Result<(), String>,
}

/// An event from a background task to the main loop.
#[derive(Debug)]
pub enum AppEvent {
    /// A pane's child process exited.
    PaneDied { pane_id: PaneId },
    /// Fallback detector state changed in a pane.
    StateChanged {
        pane_id: PaneId,
        agent: Option<Agent>,
        state: AgentState,
        visible_blocker: bool,
        visible_idle: bool,
        visible_working: bool,
        process_exited: bool,
        observed_at: Instant,
    },
    /// A new version is available through the active installation manager.
    UpdateReady {
        version: String,
        install_command: String,
    },
    /// A pane child emitted a valid OSC 52 clipboard write. The main loop
    /// re-emits it through gmux's own clipboard writer.
    ClipboardWrite { content: Vec<u8> },
    /// Background git status refresh completed for workspaces.
    GitStatusRefreshed {
        results: Vec<WorkspaceGitStatus>,
        cache_updates: Vec<(std::path::PathBuf, GitStatusCacheEntry)>,
    },
    /// Background `git worktree add` completed.
    WorktreeAddFinished(WorktreeAddResult),
    /// Background `git worktree remove` completed.
    WorktreeRemoveFinished(WorktreeRemoveResult),
}
