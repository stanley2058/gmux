use std::path::Path;

use serde::{Deserialize, Serialize};

const MAX_SESSION_ID_LEN: usize = 512;
const MAX_SESSION_PATH_LEN: usize = 4096;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AgentSessionRef {
    pub kind: AgentSessionRefKind,
    pub value: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AgentSessionRefKind {
    Id,
    Path,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PersistedAgentSession {
    pub source: String,
    pub agent: String,
    pub session_ref: AgentSessionRef,
}

impl AgentSessionRef {
    pub fn id(value: impl Into<String>) -> Option<Self> {
        let value = value.into();
        valid_session_id(&value).then_some(Self {
            kind: AgentSessionRefKind::Id,
            value,
        })
    }

    pub fn path(value: impl Into<String>) -> Option<Self> {
        let value = value.into();
        valid_session_path(&value).then_some(Self {
            kind: AgentSessionRefKind::Path,
            value,
        })
    }
}

pub fn session_ref_from_report(
    source: &str,
    agent: &str,
    agent_session_id: Option<String>,
    _agent_session_path: Option<String>,
) -> Option<AgentSessionRef> {
    if !is_official_agent_source(source, agent) {
        return None;
    }

    if agent == "pi" {
        return _agent_session_path
            .and_then(AgentSessionRef::path)
            .or_else(|| agent_session_id.and_then(AgentSessionRef::id));
    }

    agent_session_id.and_then(AgentSessionRef::id)
}

pub fn is_reserved_native_state_source(source: &str, agent: &str) -> bool {
    matches!(
        (source, agent),
        ("gmux:claude", "claude")
            | ("gmux:codex", "codex")
            | ("gmux:droid", "droid")
            | ("gmux:opencode", "opencode")
    )
}

fn is_official_agent_source(source: &str, agent: &str) -> bool {
    matches!(
        (source, agent),
        ("gmux:claude", "claude")
            | ("gmux:codex", "codex")
            | ("gmux:copilot", "copilot")
            | ("gmux:droid", "droid")
            | ("gmux:pi", "pi")
            | ("gmux:hermes", "hermes")
            | ("gmux:opencode", "opencode")
    )
}

fn valid_session_id(value: &str) -> bool {
    !value.is_empty() && value.len() <= MAX_SESSION_ID_LEN && !value.chars().any(char::is_control)
}

fn valid_session_path(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= MAX_SESSION_PATH_LEN
        && !value.chars().any(char::is_control)
        && Path::new(value).is_absolute()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn report_ref_prefers_pi_path_and_validates_values() {
        let session_ref = session_ref_from_report(
            "gmux:pi",
            "pi",
            Some("pi-id".into()),
            Some("/tmp/pi-session.jsonl".into()),
        )
        .unwrap();
        assert_eq!(session_ref.kind, AgentSessionRefKind::Path);
        assert_eq!(session_ref.value, "/tmp/pi-session.jsonl");

        assert!(session_ref_from_report("gmux:pi", "pi", Some("bad\nid".into()), None).is_none());
        assert!(
            session_ref_from_report("gmux:pi", "pi", None, Some("relative.jsonl".into())).is_none()
        );
        assert!(session_ref_from_report("custom:pi", "pi", Some("pi-id".into()), None).is_none());
        assert!(session_ref_from_report(
            "gmux:claude",
            "claude",
            None,
            Some("/tmp/claude-session".into())
        )
        .is_none());

        let session_ref =
            session_ref_from_report("gmux:copilot", "copilot", Some("copilot-id".into()), None)
                .unwrap();
        assert_eq!(session_ref.kind, AgentSessionRefKind::Id);
        assert_eq!(session_ref.value, "copilot-id");
        assert!(session_ref_from_report(
            "gmux:copilot",
            "copilot",
            None,
            Some("/tmp/copilot-session".into())
        )
        .is_none());

        let session_ref =
            session_ref_from_report("gmux:droid", "droid", Some("droid-id".into()), None).unwrap();
        assert_eq!(session_ref.kind, AgentSessionRefKind::Id);
        assert_eq!(session_ref.value, "droid-id");
        assert!(session_ref_from_report(
            "gmux:droid",
            "droid",
            None,
            Some("/tmp/droid-session".into())
        )
        .is_none());
    }
}
