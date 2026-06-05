use std::collections::HashMap;

use crate::layout::PaneId;
use crate::terminal::{TerminalId, TerminalState};

use super::{Tab, Workspace};

/// Detail info for a single pane, used by the sidebar pane list.
pub struct PaneDetail {
    pub pane_id: PaneId,
    pub tab_idx: usize,
    pub tab_label: String,
    pub label: String,
    pub custom_status: Option<String>,
}

impl Tab {
    pub fn has_working_pane(&self, _terminals: &HashMap<TerminalId, TerminalState>) -> bool {
        false
    }

    pub fn pane_details(&self, terminals: &HashMap<TerminalId, TerminalState>) -> Vec<PaneDetail> {
        self.layout
            .pane_ids()
            .iter()
            .filter_map(|id| {
                let pane = self.panes.get(id)?;
                let terminal = terminals.get(&pane.attached_terminal_id)?;
                let fallback_pane_number = self
                    .layout
                    .pane_ids()
                    .iter()
                    .position(|pane_id| pane_id == id)
                    .map(|idx| idx + 1)
                    .unwrap_or(1);
                let label = terminal
                    .effective_title()
                    .or_else(|| terminal.manual_label.as_deref().map(str::to_string))
                    .or_else(|| launch_label(terminal.launch_argv.as_ref()))
                    .unwrap_or_else(|| format!("pane {fallback_pane_number}"));
                Some(PaneDetail {
                    pane_id: *id,
                    tab_idx: self.number.saturating_sub(1),
                    tab_label: self.display_name(),
                    label,
                    custom_status: terminal.effective_custom_status(),
                })
            })
            .collect()
    }
}

fn launch_label(argv: Option<&Vec<String>>) -> Option<String> {
    let argv = argv?;
    let command = argv.first()?;
    std::path::Path::new(command)
        .file_name()
        .and_then(|name| name.to_str())
        .map(str::to_string)
        .or_else(|| Some(command.clone()))
}

impl Workspace {
    pub fn has_working_pane(&self, terminals: &HashMap<TerminalId, TerminalState>) -> bool {
        self.tabs.iter().any(|tab| tab.has_working_pane(terminals))
    }

    pub fn pane_details(&self, terminals: &HashMap<TerminalId, TerminalState>) -> Vec<PaneDetail> {
        let multi_tab = self.tabs.len() > 1;
        self.tabs
            .iter()
            .flat_map(|tab| tab.pane_details(terminals))
            .map(|mut detail| {
                if multi_tab {
                    detail.label = format!("{}·{}", detail.tab_label, detail.label);
                }
                detail
            })
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn terminal_for_pane(ws: &Workspace, pane_id: PaneId) -> TerminalState {
        TerminalState::new(ws.terminal_id(pane_id).unwrap().clone(), "/tmp".into())
    }

    #[test]
    fn pane_details_prefers_manual_label_over_fallback_label() {
        let ws = Workspace::test_new("test");
        let root_pane = ws.tabs[0].root_pane;
        let mut terminals = HashMap::new();
        let mut terminal = terminal_for_pane(&ws, root_pane);
        terminal.set_manual_label("planner".into());
        terminals.insert(terminal.id.clone(), terminal);

        let labels: Vec<_> = ws
            .pane_details(&terminals)
            .into_iter()
            .map(|detail| detail.label)
            .collect();

        assert_eq!(labels, vec!["planner".to_string()]);
    }

    #[test]
    fn pane_details_includes_tab_context_for_multi_tab_workspace() {
        let mut ws = Workspace::test_new("test");
        ws.tabs[0].custom_name = Some("main".into());
        let root_pane = ws.tabs[0].root_pane;
        let second_tab = ws.test_add_tab(Some("review"));
        let review_pane = ws.tabs[second_tab].root_pane;
        let mut terminals = HashMap::new();
        let mut root_terminal = terminal_for_pane(&ws, root_pane);
        root_terminal.set_manual_label("root".into());
        terminals.insert(root_terminal.id.clone(), root_terminal);
        let mut review_terminal = terminal_for_pane(&ws, review_pane);
        review_terminal.set_manual_label("review-pane".into());
        terminals.insert(review_terminal.id.clone(), review_terminal);

        let labels: Vec<_> = ws
            .pane_details(&terminals)
            .into_iter()
            .map(|detail| (detail.tab_label, detail.label))
            .collect();

        assert_eq!(
            labels,
            vec![
                ("main".into(), "main·root".into()),
                ("review".into(), "review·review-pane".into()),
            ]
        );
    }
}
