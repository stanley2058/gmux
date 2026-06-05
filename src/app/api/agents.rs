use crate::api::schema::{
    AgentRenameParams, AgentSendParams, AgentStartParams, AgentTarget, ResponseResult,
};
use crate::app::App;

use super::responses::{encode_error, encode_success};

impl App {
    pub(super) fn handle_agent_list(&mut self, id: String) -> String {
        encode_success(id, ResponseResult::AgentList { agents: Vec::new() })
    }

    pub(super) fn handle_agent_get(&mut self, id: String, target: AgentTarget) -> String {
        agent_api_removed(id, &target.target)
    }

    pub(super) fn handle_agent_focus(&mut self, id: String, target: AgentTarget) -> String {
        agent_api_removed(id, &target.target)
    }

    pub(super) fn handle_agent_rename(&mut self, id: String, params: AgentRenameParams) -> String {
        let _ = params.name;
        agent_api_removed(id, &params.target)
    }

    pub(super) fn handle_agent_start(&mut self, id: String, params: AgentStartParams) -> String {
        let target = params.name;
        agent_api_removed(id, &target)
    }

    pub(super) fn handle_agent_read(
        &mut self,
        id: String,
        params: crate::api::schema::AgentReadParams,
    ) -> String {
        let _ = (params.source, params.format, params.lines);
        agent_api_removed(id, &params.target)
    }

    pub(super) fn handle_agent_send(&mut self, id: String, params: AgentSendParams) -> String {
        let _ = params.text;
        agent_api_removed(id, &params.target)
    }
}

fn agent_api_removed(id: String, target: &str) -> String {
    encode_error(
        id,
        "agent_api_removed",
        format!("agent target {target} is no longer available; use pane commands instead"),
    )
}
