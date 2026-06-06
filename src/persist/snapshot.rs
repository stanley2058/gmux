use std::collections::HashMap;
use std::path::PathBuf;

use ratatui::layout::Direction;
use serde::{Deserialize, Serialize};

use crate::layout::Node;
use crate::terminal::TerminalRuntimeRegistry;
use crate::workspace::SessionUiState;

/// Current snapshot format version.
pub(super) const SNAPSHOT_VERSION: u32 = 4;

/// Serializable snapshot of the entire gmux session.
#[derive(Serialize, Deserialize)]
pub struct SessionSnapshot {
    /// Format version — used to detect incompatible changes.
    #[serde(default)]
    pub version: u32,
    #[serde(default)]
    pub tabs: Vec<TabSnapshot>,
    #[serde(default)]
    pub active_tab: usize,
}

#[derive(Clone, Serialize, Deserialize)]
pub struct SessionHistorySnapshot {
    /// Format version follows the matching session snapshot version.
    #[serde(default)]
    pub version: u32,
    #[serde(default)]
    pub tabs: Vec<TabHistorySnapshot>,
}

#[derive(Clone, Serialize, Deserialize)]
pub struct LegacyWorkspaceHistorySnapshot {
    pub tabs: Vec<TabHistorySnapshot>,
}

#[derive(Clone, Serialize, Deserialize)]
pub struct TabHistorySnapshot {
    pub panes: HashMap<u32, PaneHistorySnapshot>,
}

#[derive(Clone, Serialize, Deserialize)]
pub struct SessionStateSnapshot {
    #[serde(default)]
    pub id: Option<String>,
    #[serde(default)]
    pub custom_name: Option<String>,
    pub identity_cwd: PathBuf,
    pub tabs: Vec<TabSnapshot>,
    #[serde(default)]
    pub active_tab: usize,
}

#[derive(Deserialize)]
struct LegacyWorkspaceSnapshot {
    #[serde(default)]
    custom_name: Option<String>,
    layout: LayoutSnapshot,
    panes: HashMap<u32, PaneSnapshot>,
    zoomed: bool,
    #[serde(default)]
    focused: Option<u32>,
    #[serde(default)]
    root_pane: Option<u32>,
}

#[derive(Clone, Serialize, Deserialize)]
pub struct TabSnapshot {
    #[serde(default)]
    pub custom_name: Option<String>,
    pub layout: LayoutSnapshot,
    pub panes: HashMap<u32, PaneSnapshot>,
    pub zoomed: bool,
    #[serde(default)]
    pub focused: Option<u32>,
    #[serde(default)]
    pub root_pane: Option<u32>,
}

#[derive(Clone, Serialize, Deserialize)]
pub struct PaneSnapshot {
    pub cwd: PathBuf,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub label: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub launch_argv: Option<Vec<String>>,
}

#[derive(Clone, Serialize, Deserialize)]
pub struct PaneHistorySnapshot {
    pub ansi: String,
    pub lines: usize,
}

/// Serializable BSP tree.
#[derive(Clone, Serialize, Deserialize)]
pub enum LayoutSnapshot {
    Pane(u32),
    Split {
        direction: DirectionSnapshot,
        ratio: f32,
        first: Box<LayoutSnapshot>,
        second: Box<LayoutSnapshot>,
    },
}

#[derive(Clone, Serialize, Deserialize)]
pub enum DirectionSnapshot {
    Horizontal,
    Vertical,
}

impl From<LegacyWorkspaceSnapshot> for SessionStateSnapshot {
    fn from(snap: LegacyWorkspaceSnapshot) -> Self {
        let identity_cwd = legacy_identity_cwd(&snap);
        let tab = TabSnapshot {
            custom_name: None,
            layout: snap.layout,
            panes: snap.panes,
            zoomed: snap.zoomed,
            focused: snap.focused,
            root_pane: snap.root_pane,
        };

        Self {
            id: None,
            custom_name: snap.custom_name,
            identity_cwd,
            tabs: vec![tab],
            active_tab: 0,
        }
    }
}

#[derive(Deserialize)]
struct RawSessionSnapshot {
    #[serde(default)]
    version: u32,
    #[serde(default)]
    tabs: Option<Vec<TabSnapshot>>,
    #[serde(default)]
    active_tab: usize,
    #[serde(default)]
    workspaces: Vec<serde_json::Value>,
    #[serde(default)]
    active: Option<usize>,
}

fn migrate_snapshot(raw: RawSessionSnapshot) -> Result<SessionSnapshot, String> {
    let (tabs, active_tab) = if let Some(tabs) = raw.tabs {
        let active_tab = raw.active_tab.min(tabs.len().saturating_sub(1));
        (tabs, active_tab)
    } else {
        let legacy_sessions = raw
            .workspaces
            .into_iter()
            .map(migrate_legacy_workspace)
            .collect::<Result<Vec<_>, _>>()?;
        let active_tab = active_tab_from_sessions(&legacy_sessions, raw.active);
        let tabs = flatten_session_state_tabs(&legacy_sessions);
        (tabs, active_tab)
    };
    Ok(SessionSnapshot {
        version: raw.version,
        tabs,
        active_tab,
    })
}

fn flatten_session_state_tabs(session_states: &[SessionStateSnapshot]) -> Vec<TabSnapshot> {
    let mut tabs = Vec::new();
    for session_state in session_states {
        let mut session_tabs = session_state.tabs.clone();
        if let (Some(name), Some(first_tab)) =
            (session_state.custom_name.as_ref(), session_tabs.first_mut())
        {
            if first_tab.custom_name.is_none() {
                first_tab.custom_name = Some(name.clone());
            }
        }
        tabs.extend(session_tabs);
    }
    tabs
}

fn active_tab_from_sessions(
    session_states: &[SessionStateSnapshot],
    active_session: Option<usize>,
) -> usize {
    let Some(active_session) = active_session else {
        return 0;
    };
    let mut offset = 0;
    for (idx, session_state) in session_states.iter().enumerate() {
        if idx == active_session {
            return offset
                + session_state
                    .active_tab
                    .min(session_state.tabs.len().saturating_sub(1));
        }
        offset += session_state.tabs.len();
    }
    0
}

fn migrate_legacy_workspace(raw: serde_json::Value) -> Result<SessionStateSnapshot, String> {
    if raw.get("identity_cwd").is_some() {
        return serde_json::from_value(raw).map_err(|e| e.to_string());
    }

    if raw.get("layout").is_some() {
        let legacy =
            serde_json::from_value::<LegacyWorkspaceSnapshot>(raw).map_err(|e| e.to_string())?;
        return Ok(legacy.into());
    }

    Err("workspace snapshot is neither current nor legacy format".to_string())
}

fn legacy_identity_cwd(snap: &LegacyWorkspaceSnapshot) -> PathBuf {
    let root_pane = snap
        .root_pane
        .or_else(|| first_pane_id_in_layout(&snap.layout));

    root_pane
        .and_then(|pane_id| snap.panes.get(&pane_id))
        .map(|pane| pane.cwd.clone())
        .or_else(|| {
            first_pane_id_in_layout(&snap.layout)
                .and_then(|pane_id| snap.panes.get(&pane_id))
                .map(|pane| pane.cwd.clone())
        })
        .or_else(|| {
            snap.panes
                .keys()
                .min()
                .and_then(|pane_id| snap.panes.get(pane_id))
                .map(|pane| pane.cwd.clone())
        })
        .unwrap_or_else(|| std::env::current_dir().unwrap_or_else(|_| "/".into()))
}

fn first_pane_id_in_layout(layout: &LayoutSnapshot) -> Option<u32> {
    match layout {
        LayoutSnapshot::Pane(id) => Some(*id),
        LayoutSnapshot::Split { first, second, .. } => {
            first_pane_id_in_layout(first).or_else(|| first_pane_id_in_layout(second))
        }
    }
}

/// Capture the current app state into a serializable snapshot.
pub fn capture(
    session: Option<&SessionUiState>,
    terminals: &std::collections::HashMap<
        crate::terminal::TerminalId,
        crate::terminal::TerminalState,
    >,
    terminal_runtimes: &TerminalRuntimeRegistry,
) -> SessionSnapshot {
    let Some(session_state) = session
        .map(|session_state| capture_session_state(session_state, terminals, terminal_runtimes))
    else {
        return SessionSnapshot {
            version: SNAPSHOT_VERSION,
            tabs: Vec::new(),
            active_tab: 0,
        };
    };
    let active_tab = session_state
        .active_tab
        .min(session_state.tabs.len().saturating_sub(1));
    let tabs = flatten_session_state_tabs(std::slice::from_ref(&session_state));
    SessionSnapshot {
        version: SNAPSHOT_VERSION,
        tabs,
        active_tab,
    }
}

fn capture_session_state(
    ws: &SessionUiState,
    terminals: &std::collections::HashMap<
        crate::terminal::TerminalId,
        crate::terminal::TerminalState,
    >,
    terminal_runtimes: &TerminalRuntimeRegistry,
) -> SessionStateSnapshot {
    SessionStateSnapshot {
        id: Some(ws.id.clone()),
        custom_name: ws.custom_name.clone(),
        identity_cwd: ws
            .resolved_identity_cwd_from(terminals, terminal_runtimes)
            .unwrap_or_else(|| ws.identity_cwd.clone()),
        tabs: ws
            .tabs
            .iter()
            .map(|tab| capture_tab(tab, terminals, terminal_runtimes))
            .collect(),
        active_tab: ws.active_tab,
    }
}

fn capture_tab(
    tab: &crate::workspace::Tab,
    terminals: &std::collections::HashMap<
        crate::terminal::TerminalId,
        crate::terminal::TerminalState,
    >,
    terminal_runtimes: &TerminalRuntimeRegistry,
) -> TabSnapshot {
    let mut panes = HashMap::new();
    for id in tab.panes.keys() {
        let cwd = tab
            .cwd_for_pane(*id, terminals, terminal_runtimes)
            .unwrap_or_else(|| std::env::current_dir().unwrap_or_else(|_| "/".into()));
        let label = tab
            .panes
            .get(id)
            .and_then(|pane| terminals.get(&pane.attached_terminal_id))
            .and_then(|terminal| terminal.manual_label.clone());
        let launch_argv = tab
            .panes
            .get(id)
            .and_then(|pane| terminals.get(&pane.attached_terminal_id))
            .and_then(|terminal| terminal.launch_argv.clone());
        panes.insert(
            id.raw(),
            PaneSnapshot {
                cwd,
                label,
                launch_argv,
            },
        );
    }
    TabSnapshot {
        custom_name: tab.custom_name.clone(),
        layout: capture_node(tab.layout.root()),
        panes,
        zoomed: tab.zoomed,
        focused: Some(tab.layout.focused().raw()),
        root_pane: Some(tab.root_pane.raw()),
    }
}

/// Capture pane screen history separately from the structural session snapshot.
pub fn capture_history(
    session: Option<&SessionUiState>,
    terminal_runtimes: &TerminalRuntimeRegistry,
) -> SessionHistorySnapshot {
    let tabs = session
        .into_iter()
        .flat_map(|session_state| session_state.tabs.iter())
        .map(|tab| TabHistorySnapshot {
            panes: capture_tab_history(tab, terminal_runtimes),
        })
        .collect();
    SessionHistorySnapshot {
        version: SNAPSHOT_VERSION,
        tabs,
    }
}

fn capture_tab_history(
    tab: &crate::workspace::Tab,
    terminal_runtimes: &TerminalRuntimeRegistry,
) -> HashMap<u32, PaneHistorySnapshot> {
    let mut panes = HashMap::new();
    for (id, pane) in &tab.panes {
        if let Some(history) = capture_pane_history(Some(pane), terminal_runtimes) {
            panes.insert(id.raw(), history);
        }
    }
    panes
}

fn capture_pane_history(
    pane: Option<&crate::pane::PaneState>,
    terminal_runtimes: &TerminalRuntimeRegistry,
) -> Option<PaneHistorySnapshot> {
    let ansi = terminal_runtimes
        .get(&pane?.attached_terminal_id)?
        .snapshot_history()?;
    let lines = ansi.lines().count();
    Some(PaneHistorySnapshot { ansi, lines })
}

pub(super) fn capture_node(node: &Node) -> LayoutSnapshot {
    match node {
        Node::Pane(id) => LayoutSnapshot::Pane(id.raw()),
        Node::Split {
            direction,
            ratio,
            first,
            second,
        } => LayoutSnapshot::Split {
            direction: match direction {
                Direction::Horizontal => DirectionSnapshot::Horizontal,
                Direction::Vertical => DirectionSnapshot::Vertical,
            },
            ratio: *ratio,
            first: Box::new(capture_node(first)),
            second: Box::new(capture_node(second)),
        },
    }
}

pub(super) fn parse_snapshot(content: &str) -> Result<SessionSnapshot, String> {
    let raw = serde_json::from_str::<RawSessionSnapshot>(content).map_err(|e| e.to_string())?;
    if raw.version > SNAPSHOT_VERSION {
        return Err(format!(
            "snapshot version {} is newer than supported {}",
            raw.version, SNAPSHOT_VERSION
        ));
    }
    migrate_snapshot(raw)
}

pub(super) fn parse_history_snapshot(content: &str) -> Result<SessionHistorySnapshot, String> {
    #[derive(Deserialize)]
    struct RawSessionHistorySnapshot {
        #[serde(default)]
        version: u32,
        #[serde(default)]
        tabs: Option<Vec<TabHistorySnapshot>>,
        #[serde(default)]
        workspaces: Vec<LegacyWorkspaceHistorySnapshot>,
    }

    let raw =
        serde_json::from_str::<RawSessionHistorySnapshot>(content).map_err(|e| e.to_string())?;
    if raw.version > SNAPSHOT_VERSION {
        return Err(format!(
            "history snapshot version {} is newer than supported {}",
            raw.version, SNAPSHOT_VERSION
        ));
    }
    let tabs = if let Some(tabs) = raw.tabs {
        tabs
    } else {
        raw.workspaces
            .iter()
            .flat_map(|workspace| workspace.tabs.clone())
            .collect()
    };
    Ok(SessionHistorySnapshot {
        version: raw.version,
        tabs,
    })
}

pub(super) fn snapshot_file_version(content: &str) -> Option<u32> {
    serde_json::from_str::<RawSessionSnapshot>(content)
        .ok()
        .map(|raw| raw.version)
}

#[cfg(test)]
mod tests {
    use std::collections::HashMap;
    use std::path::PathBuf;

    use ratatui::layout::{Direction, Rect};

    use super::*;
    use crate::app::{AppState, Mode};
    use crate::layout::NavDirection;
    use crate::workspace::Workspace;

    fn session_fixture(name: &str) -> &'static str {
        match name {
            "current-gmux" => {
                include_str!("../../tests/fixtures/session/current-gmux-session.json")
            }
            "current-gmux-dev" => {
                include_str!("../../tests/fixtures/session/current-gmux-dev-session.json")
            }
            "legacy-pre-tabs-v2" => {
                include_str!("../../tests/fixtures/session/legacy-pre-tabs-v2.json")
            }
            other => panic!("unknown session fixture: {other}"),
        }
    }

    fn state_with_workspaces(names: &[&str]) -> AppState {
        let mut state = AppState::test_new();
        state.sessions = names.iter().map(|name| Workspace::test_new(name)).collect();
        state.ensure_test_terminals();
        if !state.sessions.is_empty() {
            state.active_session = Some(0);
            state.selected_session = 0;
            state.mode = Mode::Terminal;
        }
        state
    }

    fn capture_from_state(state: &AppState) -> SessionSnapshot {
        let terminal_runtimes = TerminalRuntimeRegistry::new();
        capture_from_state_with_runtimes(state, &terminal_runtimes)
    }

    fn capture_from_state_with_runtimes(
        state: &AppState,
        terminal_runtimes: &TerminalRuntimeRegistry,
    ) -> SessionSnapshot {
        capture(state.session(), &state.terminals, terminal_runtimes)
    }

    fn capture_history_from_state_with_runtimes(
        state: &AppState,
        terminal_runtimes: &TerminalRuntimeRegistry,
    ) -> SessionHistorySnapshot {
        capture_history(state.session(), terminal_runtimes)
    }

    fn root_split_ratio(tab: &TabSnapshot) -> Option<f32> {
        match &tab.layout {
            LayoutSnapshot::Split { ratio, .. } => Some(*ratio),
            LayoutSnapshot::Pane(_) => None,
        }
    }

    #[test]
    fn round_trip_empty_session() {
        let snap = SessionSnapshot {
            version: SNAPSHOT_VERSION,
            tabs: vec![],
            active_tab: 0,
        };
        let json = serde_json::to_string(&snap).unwrap();
        let restored = parse_snapshot(&json).unwrap();
        assert!(restored.tabs.is_empty());
        assert_eq!(restored.active_tab, 0);
    }

    #[test]
    fn round_trip_layout_snapshot() {
        let layout = LayoutSnapshot::Split {
            direction: DirectionSnapshot::Horizontal,
            ratio: 0.6,
            first: Box::new(LayoutSnapshot::Pane(0)),
            second: Box::new(LayoutSnapshot::Split {
                direction: DirectionSnapshot::Vertical,
                ratio: 0.5,
                first: Box::new(LayoutSnapshot::Pane(1)),
                second: Box::new(LayoutSnapshot::Pane(2)),
            }),
        };
        let json = serde_json::to_string(&layout).unwrap();
        let restored: LayoutSnapshot = serde_json::from_str(&json).unwrap();

        match restored {
            LayoutSnapshot::Split { ratio, .. } => assert!((ratio - 0.6).abs() < 0.01),
            _ => panic!("expected split"),
        }
    }

    #[test]
    fn round_trip_full_session_snapshot() {
        let mut panes = HashMap::new();
        panes.insert(
            0,
            PaneSnapshot {
                cwd: PathBuf::from("/home/can/Projects/gmux"),
                label: None,
                launch_argv: None,
            },
        );
        panes.insert(
            1,
            PaneSnapshot {
                cwd: PathBuf::from("/home/can/Projects/website"),
                label: Some("website".into()),
                launch_argv: None,
            },
        );

        let tabs = vec![TabSnapshot {
            custom_name: Some("api".to_string()),
            layout: LayoutSnapshot::Split {
                direction: DirectionSnapshot::Horizontal,
                ratio: 0.5,
                first: Box::new(LayoutSnapshot::Pane(0)),
                second: Box::new(LayoutSnapshot::Pane(1)),
            },
            panes,
            zoomed: false,
            focused: Some(0),
            root_pane: Some(0),
        }];
        let snap = SessionSnapshot {
            tabs: tabs.clone(),
            active_tab: 0,
            version: SNAPSHOT_VERSION,
        };

        let json = serde_json::to_string_pretty(&snap).unwrap();
        assert!(!json.contains("\"workspaces\""));
        assert!(json.contains("\"tabs\""));
        let restored = parse_snapshot(&json).unwrap();

        assert_eq!(restored.tabs.len(), 1);
        assert_eq!(restored.tabs[0].panes.len(), 2);
        assert_eq!(
            restored.tabs[0].panes[&0].cwd,
            PathBuf::from("/home/can/Projects/gmux")
        );
        assert_eq!(restored.tabs[0].panes[&1].label.as_deref(), Some("website"));
    }

    #[test]
    fn current_session_fixture_parses() {
        let snap = parse_snapshot(session_fixture("current-gmux")).unwrap();

        assert_eq!(snap.version, 3);
        assert_eq!(snap.tabs.len(), 3);
        assert_eq!(snap.active_tab, 0);
        assert_eq!(snap.tabs[0].custom_name.as_deref(), Some("separate-pane"));
        assert_eq!(snap.tabs[1].custom_name.as_deref(), Some("p"));
        assert_eq!(
            snap.tabs[2].panes[&3].cwd,
            PathBuf::from("/home/test/projects/project-b")
        );
    }

    #[test]
    fn current_dev_session_fixture_parses_additive_fields() {
        let snap = parse_snapshot(session_fixture("current-gmux-dev")).unwrap();

        assert_eq!(snap.version, 3);
        assert_eq!(snap.tabs.len(), 3);
        assert_eq!(snap.active_tab, 1);
        assert_eq!(snap.tabs[2].panes.len(), 2);
    }

    #[test]
    fn old_snapshot_ui_fields_are_ignored() {
        let json = serde_json::json!({
            "version": SNAPSHOT_VERSION,
            "workspaces": [],
            "active": null,
            "selected": 0,
            "pane_panel_scope": "CurrentWorkspace",
            "sidebar_width": 31,
            "sidebar_section_split": 0.4,
            "collapsed_pane_keys": ["repo-key"]
        })
        .to_string();

        let restored = parse_snapshot(&json).unwrap();

        assert!(restored.tabs.is_empty());
    }

    #[test]
    fn old_pane_snapshot_with_embedded_history_is_ignored() {
        let json = serde_json::json!({
            "version": SNAPSHOT_VERSION,
            "workspaces": [{
                "id": "wtest",
                "identity_cwd": "/tmp",
                "tabs": [{
                    "layout": { "Pane": 0 },
                    "panes": {
                        "0": {
                            "cwd": "/tmp",
                            "history": {
                                "ansi": "legacy-secret",
                                "lines": 1
                            }
                        }
                    },
                    "zoomed": false,
                    "focused": 0,
                    "root_pane": 0
                }],
                "active_tab": 0
            }],
            "active": 0,
            "selected": 0
        })
        .to_string();

        let restored = parse_snapshot(&json).unwrap();

        let encoded = serde_json::to_string(&restored).unwrap();
        assert!(!encoded.contains("legacy-secret"));
        assert!(!encoded.contains("\"history\""));
    }

    #[test]
    fn legacy_workspace_snapshot_migrates_to_single_tab() {
        let snap = parse_snapshot(session_fixture("legacy-pre-tabs-v2")).unwrap();
        let tab = &snap.tabs[0];

        assert_eq!(snap.version, 2);
        assert_eq!(snap.tabs.len(), 1);
        assert_eq!(tab.custom_name.as_deref(), Some("legacy"));
        assert_eq!(snap.active_tab, 0);
        assert_eq!(tab.focused, Some(1));
        assert_eq!(tab.root_pane, Some(0));
        assert_eq!(tab.panes[&0].cwd, PathBuf::from("/tmp/pion"));
        assert_eq!(tab.panes[&1].cwd, PathBuf::from("/tmp/gmux"));
    }

    #[test]
    fn capture_contract_tracks_collapsed_session_tabs() {
        let mut state = state_with_workspaces(&["a", "b", "c"]);
        state.sessions.swap(0, 1);
        state.active_session = Some(0);
        state.selected_session = 2;
        state.collapse_to_single_session();

        let snapshot = capture_from_state(&state);
        let names: Vec<_> = snapshot
            .tabs
            .iter()
            .map(|tab| tab.custom_name.as_deref())
            .collect();
        assert_eq!(names, vec![Some("b"), Some("a"), Some("c")]);
        assert_eq!(snapshot.active_tab, 0);
    }

    #[test]
    fn capture_contract_tracks_session_tab_names_and_active_tab() {
        let mut state = state_with_workspaces(&["one"]);
        state.sessions[0].custom_name = Some("renamed-workspace".into());
        let second_tab = state.sessions[0].test_add_tab(Some("logs"));
        state.sessions[0].switch_tab(second_tab);
        state.sessions[0].tabs[0].set_custom_name("main".into());

        let snapshot = capture_from_state(&state);
        assert_eq!(snapshot.active_tab, second_tab);
        assert_eq!(snapshot.tabs[0].custom_name.as_deref(), Some("main"));
        assert_eq!(snapshot.tabs[1].custom_name.as_deref(), Some("logs"));
    }

    #[test]
    fn capture_contract_tracks_session_closure() {
        let mut state = state_with_workspaces(&["one", "two"]);
        state.selected_session = 1;
        state.active_session = Some(1);

        state.close_session();

        let snapshot = capture_from_state(&state);
        assert!(snapshot.tabs.is_empty());
        assert_eq!(snapshot.active_tab, 0);
    }

    #[test]
    fn capture_contract_tracks_layout_focus_zoom_and_root_pane() {
        let mut state = state_with_workspaces(&["one"]);
        let root = state.sessions[0].tabs[0].root_pane;
        let second = state.sessions[0].test_split(Direction::Horizontal);
        state.sessions[0].tabs[0].layout.focus_pane(second);
        state.toggle_zoom();

        let snapshot = capture_from_state(&state);
        let tab = &snapshot.tabs[0];
        assert!(matches!(tab.layout, LayoutSnapshot::Split { .. }));
        assert_eq!(tab.focused, Some(second.raw()));
        assert_eq!(tab.root_pane, Some(root.raw()));
        assert!(tab.zoomed);
        assert_eq!(tab.panes.len(), 2);
    }

    #[test]
    fn capture_contract_tracks_focus_navigation() {
        let mut state = state_with_workspaces(&["one"]);
        let root = state.sessions[0].tabs[0].root_pane;
        let second = state.sessions[0].test_split(Direction::Horizontal);
        crate::ui::compute_view(&mut state, Rect::new(0, 0, 106, 20));

        state.navigate_pane(NavDirection::Right);

        let snapshot = capture_from_state(&state);
        assert_eq!(snapshot.tabs[0].focused, Some(second.raw()));
        assert_ne!(snapshot.tabs[0].focused, Some(root.raw()));
    }

    #[test]
    fn capture_contract_tracks_resize_ratio_changes() {
        let mut state = state_with_workspaces(&["one"]);
        state.sessions[0].test_split(Direction::Horizontal);
        crate::ui::compute_view(&mut state, Rect::new(0, 0, 106, 20));
        let before = capture_from_state(&state);

        state.resize_pane(NavDirection::Right);

        let after = capture_from_state(&state);
        let before_ratio = root_split_ratio(&before.tabs[0]).unwrap();
        let after_ratio = root_split_ratio(&after.tabs[0]).unwrap();
        assert_ne!(before_ratio, after_ratio);
    }

    #[test]
    fn capture_contract_tracks_tab_closure() {
        let mut state = state_with_workspaces(&["one"]);
        let second_tab = state.sessions[0].test_add_tab(Some("logs"));
        state.switch_tab(second_tab);

        state.close_tab();

        let snapshot = capture_from_state(&state);
        assert_eq!(snapshot.tabs.len(), 1);
        assert_eq!(snapshot.active_tab, 0);
        assert_eq!(snapshot.tabs[0].custom_name.as_deref(), Some("one"));
    }

    #[test]
    fn capture_contract_tracks_pane_closure() {
        let mut state = state_with_workspaces(&["one"]);
        state.sessions[0].test_split(Direction::Horizontal);

        state.close_pane();

        let snapshot = capture_from_state(&state);
        let tab = &snapshot.tabs[0];
        assert_eq!(tab.panes.len(), 1);
        assert!(matches!(tab.layout, LayoutSnapshot::Pane(_)));
        assert!(!tab.zoomed);
    }

    #[test]
    fn capture_contract_tracks_session_state_cwds() {
        let mut state = state_with_workspaces(&["one"]);
        let root = state.sessions[0].tabs[0].root_pane;
        state.sessions[0].identity_cwd = PathBuf::from("/tmp/pion");
        let second = state.sessions[0].test_split(Direction::Horizontal);
        state.ensure_test_terminals();
        let root_terminal_id = state.sessions[0].tabs[0].panes[&root]
            .attached_terminal_id
            .clone();
        state.terminals.get_mut(&root_terminal_id).unwrap().cwd = PathBuf::from("/tmp/pion");
        let second_terminal_id = state.sessions[0].tabs[0].panes[&second]
            .attached_terminal_id
            .clone();
        state.terminals.get_mut(&second_terminal_id).unwrap().cwd = PathBuf::from("/tmp/gmux");

        let snapshot = capture_from_state(&state);
        let tab = &snapshot.tabs[0];
        assert_eq!(tab.panes[&root.raw()].cwd, PathBuf::from("/tmp/pion"));
        assert_eq!(tab.panes[&second.raw()].cwd, PathBuf::from("/tmp/gmux"));
    }

    #[tokio::test]
    async fn capture_contract_tracks_pane_history_from_runtime() {
        let state = state_with_workspaces(&["one"]);
        let root = state.sessions[0].tabs[0].root_pane;
        let terminal_id = state.sessions[0].tabs[0].panes[&root]
            .attached_terminal_id
            .clone();
        let mut terminal_runtimes = TerminalRuntimeRegistry::new();
        terminal_runtimes.insert(
            terminal_id,
            crate::terminal::TerminalRuntime::test_with_scrollback_bytes(
                20,
                3,
                4096,
                b"alpha\r\nbeta\r\ngamma\r\n",
            ),
        );

        let snapshot = capture_from_state_with_runtimes(&state, &terminal_runtimes);
        let encoded = serde_json::to_string(&snapshot).unwrap();
        assert!(!encoded.contains("alpha"));
        assert!(!encoded.contains("\"history\""));

        let history_snapshot = capture_history_from_state_with_runtimes(&state, &terminal_runtimes);
        let history = &history_snapshot.tabs[0].panes[&root.raw()];

        assert!(history.ansi.contains("alpha"));
        assert!(history.ansi.contains("gamma"));
        assert!(history.lines >= 3);
    }

    #[tokio::test]
    async fn capture_contract_tracks_history_for_each_pane() {
        let mut state = state_with_workspaces(&["one"]);
        let first = state.sessions[0].tabs[0].root_pane;
        let second = state.sessions[0].test_split(Direction::Horizontal);
        let first_terminal_id = state.sessions[0].tabs[0].panes[&first]
            .attached_terminal_id
            .clone();
        let second_terminal_id = state.sessions[0].tabs[0].panes[&second]
            .attached_terminal_id
            .clone();
        let mut terminal_runtimes = TerminalRuntimeRegistry::new();
        terminal_runtimes.insert(
            first_terminal_id,
            crate::terminal::TerminalRuntime::test_with_scrollback_bytes(
                20,
                3,
                4096,
                b"first-pane-history\r\n",
            ),
        );
        terminal_runtimes.insert(
            second_terminal_id,
            crate::terminal::TerminalRuntime::test_with_scrollback_bytes(
                20,
                3,
                4096,
                b"second-pane-history\r\n",
            ),
        );

        let snapshot = capture_from_state_with_runtimes(&state, &terminal_runtimes);
        let encoded = serde_json::to_string(&snapshot).unwrap();
        assert!(!encoded.contains("first-pane-history"));
        assert!(!encoded.contains("second-pane-history"));

        let history_snapshot = capture_history_from_state_with_runtimes(&state, &terminal_runtimes);
        let tab = &history_snapshot.tabs[0];
        let first_history = &tab.panes[&first.raw()];
        let second_history = &tab.panes[&second.raw()];

        assert!(first_history.ansi.contains("first-pane-history"));
        assert!(second_history.ansi.contains("second-pane-history"));
    }

    #[test]
    fn old_unversioned_snapshot_loads_as_version_0() {
        let json = r#"{"workspaces":[],"active":null,"selected":0}"#;
        let snap = parse_snapshot(json).unwrap();
        assert_eq!(snap.version, 0);
    }

    #[test]
    fn future_version_is_rejected() {
        let json = r#"{"version":999,"workspaces":[],"active":null,"selected":0}"#;
        assert!(parse_snapshot(json).is_err());
    }

    #[test]
    fn active_tab_default_is_zero() {
        let json = r#"{"custom_name":"test","identity_cwd":"/tmp","tabs":[]}"#;
        let session_state: SessionStateSnapshot = serde_json::from_str(json).unwrap();
        assert_eq!(session_state.active_tab, 0);
    }

    #[test]
    fn restore_falls_back_to_home_when_cwd_missing() {
        let mut panes = HashMap::new();
        panes.insert(
            0,
            PaneSnapshot {
                cwd: PathBuf::from("/tmp/this-directory-does-not-exist-for-gmux-test"),
                label: None,
                launch_argv: None,
            },
        );
        panes.insert(
            1,
            PaneSnapshot {
                cwd: std::env::var("HOME")
                    .map(PathBuf::from)
                    .unwrap_or_else(|_| PathBuf::from("/tmp")),
                label: None,
                launch_argv: None,
            },
        );

        let snap = SessionSnapshot {
            version: SNAPSHOT_VERSION,
            tabs: vec![TabSnapshot {
                custom_name: None,
                layout: LayoutSnapshot::Split {
                    direction: DirectionSnapshot::Horizontal,
                    ratio: 0.5,
                    first: Box::new(LayoutSnapshot::Pane(0)),
                    second: Box::new(LayoutSnapshot::Pane(1)),
                },
                panes: panes.clone(),
                zoomed: false,
                focused: Some(0),
                root_pane: Some(0),
            }],
            active_tab: 0,
        };

        let json = serde_json::to_string(&snap).unwrap();
        let restored = parse_snapshot(&json).unwrap();
        assert_eq!(restored.tabs.len(), 1);
        assert_eq!(
            restored.tabs[0].panes[&0].cwd,
            PathBuf::from("/tmp/this-directory-does-not-exist-for-gmux-test")
        );
    }
}
