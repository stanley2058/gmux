use super::TerminalState;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EffectivePresentation;

impl EffectivePresentation {
    pub(super) fn empty() -> Self {
        Self
    }
}

impl TerminalState {
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
