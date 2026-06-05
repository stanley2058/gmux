use std::collections::HashMap;

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Request {
    pub id: String,
    #[serde(flatten)]
    pub method: Method,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "method", content = "params")]
pub enum Method {
    #[serde(rename = "ping")]
    Ping(PingParams),
    #[serde(rename = "server.stop")]
    ServerStop(EmptyParams),
    #[serde(rename = "server.live_handoff")]
    ServerLiveHandoff(ServerLiveHandoffParams),
    #[serde(rename = "server.reload_config")]
    ServerReloadConfig(EmptyParams),
    #[serde(rename = "tab.create")]
    TabCreate(TabCreateParams),
    #[serde(rename = "tab.list")]
    TabList(TabListParams),
    #[serde(rename = "tab.get")]
    TabGet(TabTarget),
    #[serde(rename = "tab.focus")]
    TabFocus(TabTarget),
    #[serde(rename = "tab.rename")]
    TabRename(TabRenameParams),
    #[serde(rename = "tab.close")]
    TabClose(TabTarget),
    #[serde(rename = "pane.split")]
    PaneSplit(PaneSplitParams),
    #[serde(rename = "pane.list")]
    PaneList(PaneListParams),
    #[serde(rename = "pane.get")]
    PaneGet(PaneTarget),
    #[serde(rename = "pane.focus")]
    PaneFocus(PaneFocusParams),
    #[serde(rename = "pane.resize")]
    PaneResize(PaneResizeParams),
    #[serde(rename = "pane.rename")]
    PaneRename(PaneRenameParams),
    #[serde(rename = "pane.send_text")]
    PaneSendText(PaneSendTextParams),
    #[serde(rename = "pane.send_keys")]
    PaneSendKeys(PaneSendKeysParams),
    #[serde(rename = "pane.send_input")]
    PaneSendInput(PaneSendInputParams),
    #[serde(rename = "pane.read")]
    PaneRead(PaneReadParams),
    #[serde(rename = "pane.close")]
    PaneClose(PaneTarget),
    #[serde(rename = "events.subscribe")]
    EventsSubscribe(EventsSubscribeParams),
    #[serde(rename = "events.wait")]
    EventsWait(EventsWaitParams),
    #[serde(rename = "pane.wait_for_output")]
    PaneWaitForOutput(PaneWaitForOutputParams),
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
pub struct EmptyParams {}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
pub struct PingParams {}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PaneTarget {
    pub pane_id: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TabTarget {
    pub tab_id: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TabCreateParams {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cwd: Option<String>,
    #[serde(default)]
    pub focus: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub label: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(deny_unknown_fields)]
pub struct TabListParams {}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TabRenameParams {
    pub tab_id: String,
    pub label: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PaneSplitParams {
    pub target_pane_id: String,
    pub direction: SplitDirection,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cwd: Option<String>,
    #[serde(default)]
    pub focus: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SplitDirection {
    Right,
    Down,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PaneDirection {
    Left,
    Right,
    Up,
    Down,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(deny_unknown_fields)]
pub struct PaneListParams {}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(deny_unknown_fields)]
pub struct PaneFocusParams {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pane_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub direction: Option<PaneDirection>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PaneResizeParams {
    pub direction: PaneDirection,
    #[serde(default = "default_resize_amount")]
    pub amount: u16,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PaneRenameParams {
    pub pane_id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub label: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PaneSendTextParams {
    pub pane_id: String,
    pub text: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PaneSendKeysParams {
    pub pane_id: String,
    pub keys: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PaneSendInputParams {
    pub pane_id: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub text: String,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub keys: Vec<String>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ServerLiveHandoffParams {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub import_exe: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub expected_protocol: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub expected_version: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PaneReadParams {
    pub pane_id: String,
    pub source: ReadSource,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub lines: Option<u32>,
    #[serde(default)]
    pub format: ReadFormat,
    #[serde(default = "default_true")]
    pub strip_ansi: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ReadSource {
    Visible,
    Recent,
    RecentUnwrapped,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum ReadFormat {
    #[default]
    Text,
    Ansi,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct EventsSubscribeParams {
    pub subscriptions: Vec<Subscription>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type")]
pub enum Subscription {
    #[serde(rename = "tab.created")]
    TabCreated {},
    #[serde(rename = "tab.closed")]
    TabClosed {},
    #[serde(rename = "tab.focused")]
    TabFocused {},
    #[serde(rename = "tab.renamed")]
    TabRenamed {},
    #[serde(rename = "pane.created")]
    PaneCreated {},
    #[serde(rename = "pane.closed")]
    PaneClosed {},
    #[serde(rename = "pane.focused")]
    PaneFocused {},
    #[serde(rename = "pane.exited")]
    PaneExited {},
    #[serde(rename = "pane.agent_detected")]
    PaneAgentDetected {},
    #[serde(rename = "pane.output_matched")]
    PaneOutputMatched {
        pane_id: String,
        source: ReadSource,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        lines: Option<u32>,
        r#match: OutputMatch,
        #[serde(default = "default_true")]
        strip_ansi: bool,
    },
    #[serde(rename = "pane.agent_status_changed")]
    PaneAgentStatusChanged {
        pane_id: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        agent_status: Option<AgentStatus>,
    },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct EventsWaitParams {
    pub match_event: EventMatch,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub timeout_ms: Option<u64>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PaneWaitForOutputParams {
    pub pane_id: String,
    pub source: ReadSource,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub lines: Option<u32>,
    pub r#match: OutputMatch,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub timeout_ms: Option<u64>,
    #[serde(default = "default_true")]
    pub strip_ansi: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum OutputMatch {
    Substring { value: String },
    Regex { value: String },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "event", rename_all = "snake_case", deny_unknown_fields)]
pub enum EventMatch {
    TabCreated {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        tab_id: Option<String>,
    },
    TabClosed {
        tab_id: String,
    },
    TabRenamed {
        tab_id: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        label: Option<String>,
    },
    TabFocused {
        tab_id: String,
    },
    PaneCreated {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        pane_id: Option<String>,
    },
    PaneClosed {
        pane_id: String,
    },
    PaneFocused {
        pane_id: String,
    },
    PaneOutputChanged {
        pane_id: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        min_revision: Option<u64>,
    },
    PaneExited {
        pane_id: String,
    },
    PaneAgentDetected {
        pane_id: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        agent: Option<String>,
    },
    PaneAgentStatusChanged {
        pane_id: String,
        agent_status: AgentStatus,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum EventKind {
    TabCreated,
    TabClosed,
    TabRenamed,
    TabFocused,
    PaneCreated,
    PaneClosed,
    PaneFocused,
    PaneOutputChanged,
    PaneExited,
    PaneAgentDetected,
    PaneAgentStatusChanged,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SuccessResponse {
    pub id: String,
    pub result: ResponseResult,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ErrorResponse {
    pub id: String,
    pub error: ErrorBody,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ErrorBody {
    pub code: String,
    pub message: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ServerCapabilities {
    pub live_handoff: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ResponseResult {
    Pong {
        version: String,
        protocol: u32,
        #[serde(default)]
        capabilities: Option<ServerCapabilities>,
    },
    TabInfo {
        tab: TabInfo,
    },
    TabCreated {
        tab: TabInfo,
        root_pane: PaneInfo,
    },
    TabList {
        tabs: Vec<TabInfo>,
    },
    PaneInfo {
        pane: PaneInfo,
    },
    PaneList {
        panes: Vec<PaneInfo>,
    },
    PaneRead {
        read: PaneReadResult,
    },
    SubscriptionStarted {},
    WaitMatched {
        event: EventEnvelope,
    },
    OutputMatched {
        pane_id: String,
        revision: u64,
        matched_line: Option<String>,
        read: PaneReadResult,
    },
    ConfigReload {
        status: crate::config::ConfigReloadStatus,
        diagnostics: Vec<String>,
    },
    Ok {},
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TabInfo {
    pub tab_id: String,
    pub workspace_id: String,
    pub number: usize,
    pub label: String,
    pub focused: bool,
    pub pane_count: usize,
    pub agent_status: AgentStatus,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PaneInfo {
    pub pane_id: String,
    pub terminal_id: String,
    pub workspace_id: String,
    pub tab_id: String,
    pub focused: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cwd: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub foreground_cwd: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub label: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub agent: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub display_agent: Option<String>,
    pub agent_status: AgentStatus,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub custom_status: Option<String>,
    #[serde(default, skip_serializing_if = "HashMap::is_empty")]
    pub state_labels: HashMap<String, String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub agent_session: Option<AgentSessionInfo>,
    pub revision: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AgentSessionInfo {
    pub source: String,
    pub agent: String,
    pub kind: String,
    pub value: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PaneReadResult {
    pub pane_id: String,
    pub workspace_id: String,
    pub tab_id: String,
    pub source: ReadSource,
    pub format: ReadFormat,
    pub text: String,
    pub revision: u64,
    pub truncated: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct EventEnvelope {
    pub event: EventKind,
    pub data: EventData,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum SubscriptionEventKind {
    #[serde(rename = "pane.output_matched")]
    PaneOutputMatched,
    #[serde(rename = "pane.agent_status_changed")]
    PaneAgentStatusChanged,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SubscriptionEventEnvelope {
    pub event: SubscriptionEventKind,
    pub data: SubscriptionEventData,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum SubscriptionEventData {
    PaneOutputMatched(PaneOutputMatchedEvent),
    PaneAgentStatusChanged(PaneAgentStatusChangedEvent),
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PaneOutputMatchedEvent {
    pub pane_id: String,
    pub matched_line: String,
    pub read: PaneReadResult,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PaneAgentStatusChangedEvent {
    pub pane_id: String,
    pub workspace_id: String,
    pub agent_status: AgentStatus,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub agent: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub custom_status: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub display_agent: Option<String>,
    #[serde(default, skip_serializing_if = "HashMap::is_empty")]
    pub state_labels: HashMap<String, String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum EventData {
    TabCreated {
        tab: TabInfo,
    },
    TabClosed {
        tab_id: String,
        workspace_id: String,
    },
    TabRenamed {
        tab_id: String,
        workspace_id: String,
        label: String,
    },
    TabFocused {
        tab_id: String,
        workspace_id: String,
    },
    PaneCreated {
        pane: PaneInfo,
    },
    PaneClosed {
        pane_id: String,
        workspace_id: String,
    },
    PaneFocused {
        pane_id: String,
        workspace_id: String,
    },
    PaneOutputChanged {
        pane_id: String,
        workspace_id: String,
        revision: u64,
    },
    PaneExited {
        pane_id: String,
        workspace_id: String,
    },
    PaneAgentDetected {
        pane_id: String,
        workspace_id: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        agent: Option<String>,
    },
    PaneAgentStatusChanged {
        pane_id: String,
        workspace_id: String,
        agent_status: AgentStatus,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        agent: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        title: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        display_agent: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        custom_status: Option<String>,
        #[serde(default, skip_serializing_if = "HashMap::is_empty")]
        state_labels: HashMap<String, String>,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AgentStatus {
    Idle,
    Working,
    Blocked,
    Done,
    Unknown,
}

fn default_true() -> bool {
    true
}

fn default_resize_amount() -> u16 {
    1
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn request_round_trips_for_pane_read() {
        let request = Request {
            id: "req_1".into(),
            method: Method::PaneRead(PaneReadParams {
                pane_id: "p_1".into(),
                source: ReadSource::Recent,
                lines: Some(80),
                format: ReadFormat::Text,
                strip_ansi: true,
            }),
        };

        let json = serde_json::to_string(&request).unwrap();
        let restored: Request = serde_json::from_str(&json).unwrap();
        assert_eq!(restored, request);
    }

    #[test]
    fn request_uses_dot_method_names() {
        let request = Request {
            id: "req_1".into(),
            method: Method::TabCreate(TabCreateParams {
                cwd: Some("/tmp".into()),
                focus: true,
                label: Some("api".into()),
            }),
        };

        let json = serde_json::to_value(&request).unwrap();
        assert_eq!(json["method"], "tab.create");
    }

    #[test]
    fn request_round_trips_for_server_stop() {
        let request = Request {
            id: "req_stop".into(),
            method: Method::ServerStop(EmptyParams::default()),
        };

        let json = serde_json::to_value(&request).unwrap();
        assert_eq!(json["method"], "server.stop");
        let restored: Request = serde_json::from_value(json).unwrap();
        assert_eq!(restored, request);
    }

    #[test]
    fn request_round_trips_for_server_reload_config() {
        let request = Request {
            id: "req_reload".into(),
            method: Method::ServerReloadConfig(EmptyParams::default()),
        };

        let json = serde_json::to_value(&request).unwrap();
        assert_eq!(json["method"], "server.reload_config");
        let restored: Request = serde_json::from_value(json).unwrap();
        assert_eq!(restored, request);
    }

    #[test]
    fn unknown_method_is_rejected() {
        let json = r#"{"id":"req_1","method":"nope","params":{}}"#;
        let err = serde_json::from_str::<Request>(json)
            .unwrap_err()
            .to_string();
        assert!(err.contains("unknown variant"));
    }

    #[test]
    fn missing_required_params_are_rejected() {
        let json = r#"{"id":"req_1","method":"pane.send_text","params":{"pane_id":"p_1"}}"#;
        let err = serde_json::from_str::<Request>(json)
            .unwrap_err()
            .to_string();
        assert!(err.contains("text"));
    }

    #[test]
    fn pane_send_input_defaults_to_empty_text_and_keys() {
        let json = r#"
        {
            "id": "req_1",
            "method": "pane.send_input",
            "params": {
                "pane_id": "p_1"
            }
        }
        "#;

        let request: Request = serde_json::from_str(json).unwrap();
        let Method::PaneSendInput(params) = request.method else {
            panic!("wrong method parsed");
        };
        assert_eq!(params.pane_id, "p_1");
        assert!(params.text.is_empty());
        assert!(params.keys.is_empty());
    }

    #[test]
    fn pane_focus_parses_direction() {
        let json = r#"
        {
            "id": "req_1",
            "method": "pane.focus",
            "params": {
                "direction": "left"
            }
        }
        "#;

        let request: Request = serde_json::from_str(json).unwrap();
        let Method::PaneFocus(params) = request.method else {
            panic!("wrong method parsed");
        };
        assert_eq!(params.direction, Some(PaneDirection::Left));
        assert_eq!(params.pane_id, None);
    }

    #[test]
    fn pane_resize_defaults_amount() {
        let json = r#"
        {
            "id": "req_1",
            "method": "pane.resize",
            "params": {
                "direction": "right"
            }
        }
        "#;

        let request: Request = serde_json::from_str(json).unwrap();
        let Method::PaneResize(params) = request.method else {
            panic!("wrong method parsed");
        };
        assert_eq!(params.direction, PaneDirection::Right);
        assert_eq!(params.amount, 1);
    }

    #[test]
    fn pane_wait_for_output_defaults_strip_ansi_to_true() {
        let json = r#"
        {
            "id": "req_1",
            "method": "pane.wait_for_output",
            "params": {
                "pane_id": "p_1",
                "source": "recent",
                "match": { "type": "substring", "value": "ready" }
            }
        }
        "#;

        let request: Request = serde_json::from_str(json).unwrap();
        let Method::PaneWaitForOutput(params) = request.method else {
            panic!("wrong method parsed");
        };
        assert!(params.strip_ansi);
    }

    #[test]
    fn pane_read_defaults_to_text_format() {
        let json = r#"
        {
            "id": "req_1",
            "method": "pane.read",
            "params": {
                "pane_id": "p_1",
                "source": "visible"
            }
        }
        "#;

        let request: Request = serde_json::from_str(json).unwrap();
        let Method::PaneRead(params) = request.method else {
            panic!("wrong method parsed");
        };
        assert_eq!(params.format, ReadFormat::Text);
    }

    #[test]
    fn event_envelope_round_trips() {
        let event = EventEnvelope {
            event: EventKind::PaneOutputChanged,
            data: EventData::PaneOutputChanged {
                pane_id: "p_1".into(),
                workspace_id: "w_1".into(),
                revision: 42,
            },
        };

        let json = serde_json::to_string(&event).unwrap();
        let restored: EventEnvelope = serde_json::from_str(&json).unwrap();
        assert_eq!(restored, event);
    }

    #[test]
    fn subscribe_request_parses_parameterized_subscriptions() {
        let json = r#"
        {
            "id": "sub_1",
            "method": "events.subscribe",
            "params": {
                "subscriptions": [
                    {
                        "type": "pane.output_matched",
                        "pane_id": "p_1_1",
                        "source": "recent",
                        "lines": 200,
                        "match": { "type": "substring", "value": "auth: received" }
                    },
                    {
                        "type": "pane.agent_status_changed",
                        "pane_id": "p_1_1",
                        "agent_status": "done"
                    }
                ]
            }
        }
        "#;

        let request: Request = serde_json::from_str(json).unwrap();
        let Method::EventsSubscribe(params) = request.method else {
            panic!("wrong method parsed");
        };
        assert_eq!(params.subscriptions.len(), 2);
        assert!(matches!(
            &params.subscriptions[0],
            Subscription::PaneOutputMatched {
                pane_id,
                source: ReadSource::Recent,
                lines: Some(200),
                r#match: OutputMatch::Substring { value },
                strip_ansi: true,
            } if pane_id == "p_1_1" && value == "auth: received"
        ));
        assert!(matches!(
            &params.subscriptions[1],
            Subscription::PaneAgentStatusChanged {
                pane_id,
                agent_status: Some(AgentStatus::Done),
            } if pane_id == "p_1_1"
        ));
    }

    #[test]
    fn subscription_event_envelope_round_trips() {
        let event = SubscriptionEventEnvelope {
            event: SubscriptionEventKind::PaneOutputMatched,
            data: SubscriptionEventData::PaneOutputMatched(PaneOutputMatchedEvent {
                pane_id: "p_1_1".into(),
                matched_line: "auth: received".into(),
                read: PaneReadResult {
                    pane_id: "p_1_1".into(),
                    workspace_id: "w_1".into(),
                    tab_id: "t_1_1".into(),
                    source: ReadSource::Recent,
                    format: ReadFormat::Text,
                    text: "auth: received\n".into(),
                    revision: 0,
                    truncated: false,
                },
            }),
        };

        let json = serde_json::to_string(&event).unwrap();
        assert!(json.contains("\"event\":\"pane.output_matched\""));
        let restored: SubscriptionEventEnvelope = serde_json::from_str(&json).unwrap();
        assert_eq!(restored, event);
    }

    #[test]
    fn success_response_round_trips() {
        let response = SuccessResponse {
            id: "req_1".into(),
            result: ResponseResult::Pong {
                version: "0.1.2".into(),
                protocol: 6,
                capabilities: Some(ServerCapabilities { live_handoff: true }),
            },
        };

        let json = serde_json::to_string(&response).unwrap();
        let restored: SuccessResponse = serde_json::from_str(&json).unwrap();
        assert_eq!(restored, response);
    }

    #[test]
    fn create_response_round_trips_with_root_pane() {
        let response = SuccessResponse {
            id: "req_2".into(),
            result: ResponseResult::TabCreated {
                tab: TabInfo {
                    tab_id: "w_1:2".into(),
                    workspace_id: "w_1".into(),
                    number: 2,
                    label: "review".into(),
                    focused: false,
                    pane_count: 1,
                    agent_status: AgentStatus::Unknown,
                },
                root_pane: PaneInfo {
                    pane_id: "w_1-3".into(),
                    terminal_id: "term_example".into(),
                    workspace_id: "w_1".into(),
                    tab_id: "w_1:2".into(),
                    focused: false,
                    cwd: Some("/tmp/review".into()),
                    foreground_cwd: None,
                    label: None,
                    agent: None,
                    title: None,
                    display_agent: None,
                    agent_status: AgentStatus::Unknown,
                    custom_status: None,
                    state_labels: HashMap::new(),
                    agent_session: None,
                    revision: 0,
                },
            },
        };

        let json = serde_json::to_string(&response).unwrap();
        assert!(json.contains("\"type\":\"tab_created\""));
        assert!(json.contains("\"root_pane\""));
        let restored: SuccessResponse = serde_json::from_str(&json).unwrap();
        assert_eq!(restored, response);
    }

    #[test]
    fn error_response_round_trips() {
        let response = ErrorResponse {
            id: "req_1".into(),
            error: ErrorBody {
                code: "pane_not_found".into(),
                message: "pane p_1 not found".into(),
            },
        };

        let json = serde_json::to_string(&response).unwrap();
        let restored: ErrorResponse = serde_json::from_str(&json).unwrap();
        assert_eq!(restored, response);
    }

    #[test]
    fn event_wait_parses_typed_match() {
        let json = r#"
        {
            "id": "req_9",
            "method": "events.wait",
            "params": {
                "match_event": {
                    "event": "pane_agent_status_changed",
                    "pane_id": "p_1",
                    "agent_status": "done"
                },
                "timeout_ms": 30000
            }
        }
        "#;

        let request: Request = serde_json::from_str(json).unwrap();
        let Method::EventsWait(params) = request.method else {
            panic!("wrong method parsed");
        };
        assert_eq!(
            params.match_event,
            EventMatch::PaneAgentStatusChanged {
                pane_id: "p_1".into(),
                agent_status: AgentStatus::Done,
            }
        );
    }

    #[test]
    fn event_wait_rejects_workspace_filters() {
        let json = r#"
        {
            "id": "req_10",
            "method": "events.wait",
            "params": {
                "match_event": {
                    "event": "tab_created",
                    "workspace_id": "1"
                }
            }
        }
        "#;

        let err = serde_json::from_str::<Request>(json)
            .unwrap_err()
            .to_string();
        assert!(err.contains("workspace_id"), "error: {err}");
    }
}
