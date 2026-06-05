use std::path::PathBuf;

use crate::detect::{Agent, AgentState};
use crate::terminal::TerminalId;

#[path = "metadata.rs"]
mod metadata;

/// Pure state for a server-owned terminal.
///
/// During the migration this is still one-to-one with a pane-backed PTY, but
/// pane/view state no longer owns terminal identity, cwd, labels, or agent
/// metadata.
pub struct TerminalState {
    pub id: TerminalId,
    pub cwd: PathBuf,
    pub detected_agent: Option<Agent>,
    pub fallback_state: AgentState,
    pub manual_label: Option<String>,
    pub state: AgentState,
    pub revision: u64,
    pub launch_argv: Option<Vec<String>>,
    pub respawn_shell_on_exit: bool,
}

impl TerminalState {
    pub fn new(id: TerminalId, cwd: PathBuf) -> Self {
        Self {
            id,
            cwd,
            detected_agent: None,
            fallback_state: AgentState::Unknown,
            manual_label: None,
            state: AgentState::Unknown,
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

    #[cfg(test)]
    pub fn set_detected_state(&mut self, agent: Option<Agent>, fallback_state: AgentState) {
        self.set_detected_state_with_visible_blocker(agent, fallback_state, false, false, false)
    }

    #[cfg(test)]
    pub fn set_detected_state_with_visible_blocker(
        &mut self,
        agent: Option<Agent>,
        fallback_state: AgentState,
        visible_blocker: bool,
        visible_idle: bool,
        process_exited: bool,
    ) {
        let _ = (visible_blocker, visible_idle, process_exited);
        self.detected_agent = agent;
        self.fallback_state = fallback_state;
        self.state = fallback_state;
    }

    pub fn set_manual_label(&mut self, label: String) {
        let label = label.trim().to_string();
        self.manual_label = (!label.is_empty()).then_some(label);
    }

    pub fn clear_manual_label(&mut self) {
        self.manual_label = None;
    }

    pub fn clear_agent_runtime_identity_after_respawn(&mut self) {
        self.detected_agent = None;
        self.fallback_state = AgentState::Unknown;
        self.state = AgentState::Unknown;
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
        terminal.set_detected_state(Some(Agent::Claude), AgentState::Idle);

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
    fn respawn_cleanup_resets_restored_agent_status() {
        let mut terminal = test_terminal();
        terminal.respawn_shell_on_exit = true;
        terminal.set_detected_state(Some(Agent::Codex), AgentState::Idle);

        terminal.clear_agent_runtime_identity_after_respawn();

        assert_eq!(terminal.state, AgentState::Unknown);
        assert!(terminal.detected_agent.is_none());
        assert!(!terminal.respawn_shell_on_exit);
    }
}
