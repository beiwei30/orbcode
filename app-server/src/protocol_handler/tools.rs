use orbcode_app_server_protocol::{
    AgentDefinitionsResult, AgentDefinitionsWithWarningsResult, AgentSummary, EnterPlanModeResult,
    ResponseResult, SkillDefinitionsParams, SkillDefinitionsResult, TaskListParams, TaskListResult,
    TaskOverview, ToolInvokeParams, ToolInvokeResult, ToolOverview, ToolsListResult,
};
use serde_json::Value;

use super::{core_error, success, try_parse};
use crate::AppServer;
use crate::protocol_conversion::{
    agent_definition_to_wire, agent_load_warning_to_wire, skill_definition_to_wire,
};

impl AppServer {
    pub(super) fn handle_tools_list(&self, _params: Option<Value>) -> ResponseResult {
        let tools = self
            .list_tools()
            .into_iter()
            .map(|tool| ToolOverview {
                name: tool.name.to_string(),
                summary: tool.summary.to_string(),
                requires_tools_permission: tool.requires_tools_permission,
                requires_network_permission: tool.requires_network_permission,
                provider_hidden: tool.provider_hidden,
            })
            .collect();
        success(ToolsListResult(tools))
    }

    pub(super) async fn handle_tools_invoke(&self, params: Option<Value>) -> ResponseResult {
        let p: ToolInvokeParams = try_parse!(params);
        match self.invoke_tool(&p.name, p.input).await {
            Ok(outcome) => success(ToolInvokeResult {
                name: outcome.name,
                summary: outcome.summary,
                output: outcome.output,
                metadata: outcome.metadata,
                changed_paths: outcome.changed_paths,
            }),
            Err(e) => core_error(e),
        }
    }

    pub(super) async fn handle_tools_skills(&self, params: Option<Value>) -> ResponseResult {
        let p: SkillDefinitionsParams = try_parse!(params);
        match p.session_id.as_deref() {
            Some(session_id) => match self.skill_definitions_for_session(session_id).await {
                Ok(skills) => success(SkillDefinitionsResult(
                    skills.into_iter().map(skill_definition_to_wire).collect(),
                )),
                Err(e) => core_error(e),
            },
            None => success(SkillDefinitionsResult(
                self.skill_definitions()
                    .await
                    .into_iter()
                    .map(skill_definition_to_wire)
                    .collect(),
            )),
        }
    }

    pub(super) fn handle_tools_agents(&self, _params: Option<Value>) -> ResponseResult {
        let agents = self
            .agent_definitions()
            .into_iter()
            .map(|agent| AgentSummary {
                agent_type: agent.agent_type,
                description: agent.description,
            })
            .collect();
        success(AgentDefinitionsResult(agents))
    }

    pub(super) async fn handle_tools_agents_with_warnings(
        &self,
        _params: Option<Value>,
    ) -> ResponseResult {
        let (definitions, warnings) = self.agent_definitions_with_warnings().await;
        let definitions = definitions
            .into_iter()
            .map(agent_definition_to_wire)
            .collect::<Vec<_>>();
        let warnings = warnings
            .into_iter()
            .map(agent_load_warning_to_wire)
            .collect::<Vec<_>>();
        success(AgentDefinitionsWithWarningsResult {
            definitions,
            warnings,
        })
    }

    pub(super) async fn handle_tools_plan(&self, _params: Option<Value>) -> ResponseResult {
        match self.plan_overview().await {
            Ok(plan) => success(plan),
            Err(e) => core_error(e),
        }
    }

    pub(super) async fn handle_tools_task_list(&self, params: Option<Value>) -> ResponseResult {
        let p: TaskListParams = try_parse!(params);
        match self.load_task_list_snapshot(&p.task_list_id).await {
            Ok(snapshot) => success(TaskListResult {
                task_list_id: snapshot.task_list_id,
                directory: snapshot.directory,
                tasks: snapshot
                    .tasks
                    .into_iter()
                    .map(|task| TaskOverview {
                        id: task.id,
                        subject: task.subject,
                        description: task.description,
                        status: format!("{:?}", task.status),
                    })
                    .collect(),
            }),
            Err(e) => core_error(e),
        }
    }

    pub(super) async fn handle_tools_enter_plan(&self, _params: Option<Value>) -> ResponseResult {
        match self.enter_plan_mode().await {
            Ok(outcome) => success(EnterPlanModeResult {
                name: outcome.name,
                summary: outcome.summary,
                output: outcome.output,
                metadata: outcome.metadata,
            }),
            Err(e) => core_error(e),
        }
    }

    pub(super) async fn handle_tools_seed_read_state(
        &self,
        params: Option<Value>,
    ) -> ResponseResult {
        let p: orbcode_app_server_protocol::SeedReadStateParams = try_parse!(params);
        match self.seed_read_state(&p.session_id, &p.path, p.mtime).await {
            Ok(result) => success(result),
            Err(e) => core_error(e),
        }
    }
}
