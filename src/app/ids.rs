use super::App;

impl App {
    pub(crate) fn find_pane(
        &self,
        pane_id: crate::layout::PaneId,
    ) -> Option<(usize, &crate::pane::PaneState)> {
        self.state
            .workspaces
            .iter()
            .enumerate()
            .find_map(|(ws_idx, ws)| ws.pane_state(pane_id).map(|pane| (ws_idx, pane)))
    }

    pub(super) fn public_tab_id(&self, ws_idx: usize, tab_idx: usize) -> Option<String> {
        Some(format!("t_{}", self.public_tab_number(ws_idx, tab_idx)?))
    }

    pub(super) fn public_pane_id(
        &self,
        ws_idx: usize,
        pane_id: crate::layout::PaneId,
    ) -> Option<String> {
        Some(format!("p_{}", self.public_pane_number(ws_idx, pane_id)?))
    }

    fn public_tab_number(&self, ws_idx: usize, tab_idx: usize) -> Option<usize> {
        self.state
            .session_tab_entries()
            .position(|(entry_ws_idx, entry_tab_idx, _, _)| {
                entry_ws_idx == ws_idx && entry_tab_idx == tab_idx
            })
            .map(|idx| idx + 1)
    }

    fn tab_by_public_number(&self, number: usize) -> Option<(usize, usize)> {
        let idx = number.checked_sub(1)?;
        self.state
            .session_tab_entries()
            .nth(idx)
            .map(|(ws_idx, tab_idx, _, _)| (ws_idx, tab_idx))
    }

    fn public_pane_number(&self, ws_idx: usize, pane_id: crate::layout::PaneId) -> Option<usize> {
        let ws = self.state.workspaces.get(ws_idx)?;
        let preceding = self
            .state
            .workspaces
            .iter()
            .take(ws_idx)
            .map(|ws| ws.public_pane_numbers.len())
            .sum::<usize>();
        Some(preceding + ws.public_pane_number(pane_id)?)
    }

    fn pane_by_public_number(&self, number: usize) -> Option<(usize, crate::layout::PaneId)> {
        let mut remaining = number.checked_sub(1)?;
        for (ws_idx, ws) in self.state.workspaces.iter().enumerate() {
            if remaining < ws.public_pane_numbers.len() {
                let local_number = remaining + 1;
                let pane_id = ws
                    .public_pane_numbers
                    .iter()
                    .find_map(|(pane_id, number)| (*number == local_number).then_some(*pane_id))?;
                return Some((ws_idx, pane_id));
            }
            remaining = remaining.checked_sub(ws.public_pane_numbers.len())?;
        }
        None
    }

    pub(super) fn parse_workspace_id(&self, id: &str) -> Option<usize> {
        self.state
            .workspaces
            .iter()
            .position(|workspace| workspace.id == id)
            .or_else(|| id.strip_prefix("w_")?.parse::<usize>().ok()?.checked_sub(1))
            .or_else(|| id.parse::<usize>().ok()?.checked_sub(1))
    }

    pub(super) fn parse_tab_id(&self, id: &str) -> Option<(usize, usize)> {
        if let Some(rest) = id.strip_prefix("t_") {
            if let Ok(number) = rest.parse::<usize>() {
                return self.tab_by_public_number(number);
            }

            let (ws_raw, tab_raw) = rest.rsplit_once('_')?;
            let ws_idx = self.parse_workspace_id(ws_raw)?;
            let tab_idx = tab_raw.parse::<usize>().ok()?.checked_sub(1)?;
            self.state.workspaces.get(ws_idx)?.tabs.get(tab_idx)?;
            return Some((ws_idx, tab_idx));
        }

        let (ws_raw, tab_raw) = id.rsplit_once(':')?;
        let ws_idx = self.parse_workspace_id(ws_raw)?;
        let tab_idx = tab_raw.parse::<usize>().ok()?.checked_sub(1)?;
        self.state.workspaces.get(ws_idx)?.tabs.get(tab_idx)?;
        Some((ws_idx, tab_idx))
    }

    fn resolve_raw_pane_id(&self, raw: u32) -> Option<crate::layout::PaneId> {
        if let Some(alias) = self.state.pane_id_aliases.get(&raw).copied() {
            return self.find_pane(alias).map(|_| alias);
        }
        let pane_id = crate::layout::PaneId::from_raw(raw);
        if self.find_pane(pane_id).is_some() {
            return Some(pane_id);
        }
        None
    }

    pub(super) fn parse_pane_id(&self, id: &str) -> Option<(usize, crate::layout::PaneId)> {
        if let Some(rest) = id.strip_prefix("p_") {
            if let Some((ws_raw, pane_raw)) = rest.rsplit_once('_') {
                let ws_idx = self.parse_workspace_id(ws_raw)?;
                let pane_id = self.resolve_raw_pane_id(pane_raw.parse::<u32>().ok()?)?;
                self.state.workspaces.get(ws_idx)?.pane_state(pane_id)?;
                return Some((ws_idx, pane_id));
            }

            let number = rest.parse::<usize>().ok()?;
            if let Some(target) = self.pane_by_public_number(number) {
                return Some(target);
            }
            let pane_id = self.resolve_raw_pane_id(number as u32)?;
            return self.find_pane(pane_id).map(|(ws_idx, _)| (ws_idx, pane_id));
        }

        let (ws_raw, pane_number_raw) = id.rsplit_once('-')?;
        let ws_idx = self.parse_workspace_id(ws_raw)?;
        let pane_number = pane_number_raw.parse::<usize>().ok()?;
        let ws = self.state.workspaces.get(ws_idx)?;
        let pane_id = ws
            .public_pane_numbers
            .iter()
            .find_map(|(pane_id, number)| (*number == pane_number).then_some(*pane_id))?;
        Some((ws_idx, pane_id))
    }
}
