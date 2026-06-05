use std::collections::HashMap;
use std::time::Instant;

use crate::detect::AgentState;

use super::TerminalState;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EffectivePresentation {
    pub title: Option<String>,
    pub display_agent: Option<String>,
    pub custom_status: Option<String>,
    pub state_labels: HashMap<String, String>,
}

impl EffectivePresentation {
    pub(super) fn empty() -> Self {
        Self {
            title: None,
            display_agent: None,
            custom_status: None,
            state_labels: HashMap::new(),
        }
    }
}

impl TerminalState {
    pub fn effective_custom_status(&self) -> Option<String> {
        self.effective_presentation_for_state_at(self.state, Instant::now())
            .custom_status
    }

    pub fn effective_title(&self) -> Option<String> {
        None
    }

    pub fn effective_presentation(&self) -> EffectivePresentation {
        self.effective_presentation_for_state_at(self.state, Instant::now())
    }

    pub(super) fn effective_presentation_for_state_at(
        &self,
        state: AgentState,
        now: Instant,
    ) -> EffectivePresentation {
        let mut presentation = EffectivePresentation::empty();
        presentation.custom_status = self.effective_custom_status_for_state_at(state, now);
        presentation
    }

    fn effective_custom_status_for_state_at(
        &self,
        state: AgentState,
        now: Instant,
    ) -> Option<String> {
        if self.visible_blocker_overrides_hook()
            || self.visible_working_overrides_hook()
            || self.visible_idle_masks_hook_custom_status(state, now)
        {
            return None;
        }

        self.hook_authority
            .as_ref()
            .and_then(|authority| authority.custom_status.clone())
    }
}
