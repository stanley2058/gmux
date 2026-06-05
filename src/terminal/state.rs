use std::path::PathBuf;

use crate::terminal::TerminalId;

#[path = "metadata.rs"]
mod metadata;

/// Pure state for a server-owned terminal.
///
/// During the migration this is still one-to-one with a pane-backed PTY, but
/// pane/view state no longer owns terminal identity, cwd, or labels.
pub struct TerminalState {
    pub id: TerminalId,
    pub cwd: PathBuf,
    pub manual_label: Option<String>,
    pub revision: u64,
    pub launch_argv: Option<Vec<String>>,
    pub respawn_shell_on_exit: bool,
}

impl TerminalState {
    pub fn new(id: TerminalId, cwd: PathBuf) -> Self {
        Self {
            id,
            cwd,
            manual_label: None,
            revision: 0,
            launch_argv: None,
            respawn_shell_on_exit: false,
        }
    }

    pub fn with_launch_argv(mut self, argv: Vec<String>) -> Self {
        self.launch_argv = Some(argv);
        self
    }

    pub fn with_respawn_shell_on_exit(mut self) -> Self {
        self.respawn_shell_on_exit = true;
        self
    }

    pub fn set_manual_label(&mut self, label: String) {
        let label = label.trim().to_string();
        self.manual_label = (!label.is_empty()).then_some(label);
    }

    pub fn clear_manual_label(&mut self) {
        self.manual_label = None;
    }

    pub fn clear_launch_metadata_after_respawn(&mut self) {
        self.launch_argv = None;
        self.respawn_shell_on_exit = false;
    }

    pub fn border_label(&self) -> Option<String> {
        self.effective_title().or_else(|| self.manual_label.clone())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn test_terminal() -> TerminalState {
        TerminalState::new(TerminalId::alloc(), "/tmp".into())
    }

    #[test]
    fn border_label_uses_title_and_manual_label_only() {
        let mut terminal = test_terminal();

        assert_eq!(terminal.border_label(), None);

        terminal.set_manual_label(" reviewer ".into());
        assert_eq!(terminal.border_label().as_deref(), Some("reviewer"));

        terminal.set_manual_label("   ".into());
        assert_eq!(terminal.border_label(), None);

        terminal.set_manual_label("reviewer".into());
        terminal.clear_manual_label();
        assert_eq!(terminal.border_label(), None);
    }

    #[test]
    fn respawn_cleanup_resets_launch_metadata() {
        let mut terminal = test_terminal();
        terminal.respawn_shell_on_exit = true;
        terminal.launch_argv = Some(vec!["echo".into(), "done".into()]);

        terminal.clear_launch_metadata_after_respawn();

        assert!(terminal.launch_argv.is_none());
        assert!(!terminal.respawn_shell_on_exit);
    }
}
