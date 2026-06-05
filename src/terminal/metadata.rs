use super::TerminalState;
use std::collections::HashMap;

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
        None
    }

    pub fn effective_title(&self) -> Option<String> {
        None
    }

    #[cfg(test)]
    pub fn effective_presentation(&self) -> EffectivePresentation {
        EffectivePresentation::empty()
    }

    pub(super) fn effective_presentation_for_state_at(
        &self,
        _state: crate::detect::AgentState,
        _now: std::time::Instant,
    ) -> EffectivePresentation {
        EffectivePresentation::empty()
    }
}
