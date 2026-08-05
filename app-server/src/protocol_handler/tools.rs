use orbcode_app_server_protocol::ResponseResult;
use serde::Deserialize;
use serde_json::Value;

use super::{core_error, success, try_parse};
use crate::AppServer;

impl AppServer {
    pub(super) fn handle_tools_list(&self, _params: Option<Value>) -> ResponseResult {
        let tools: Vec<Value> = self
            .list_tools()
            .into_iter()
            .map(|t| {
                let unavailable_reason = matches!(
                    t.name,
                    "ask-user-question" | "AskUserQuestion"
                )
                .then_some(
                    "available to the provider only for turns owned by a client that declares the full interactive_questions capability",
                );
                serde_json::json!({
                    "name": t.name,
                    "summary": t.summary,
                    "requires_tools_permission": t.requires_tools_permission,
                    "requires_network_permission": t.requires_network_permission,
                    "provider_hidden": t.provider_hidden,
                    "unavailable_reason": unavailable_reason,
                })
            })
            .collect();
        success(tools)
    }

    pub(super) async fn handle_tools_invoke(&self, params: Option<Value>) -> ResponseResult {
        #[derive(Deserialize)]
        struct Params {
            name: String,
            #[serde(default = "default_empty_object")]
            input: String,
        }
        let p: Params = try_parse!(params);
        match self.invoke_tool(&p.name, p.input).await {
            Ok(outcome) => success(serde_json::json!({
                "name": outcome.name,
                "summary": outcome.summary,
                "output": outcome.output,
                "metadata": outcome.metadata,
                "changed_paths": outcome.changed_paths,
            })),
            Err(e) => core_error(e),
        }
    }

    pub(super) async fn handle_tools_skills(&self, params: Option<Value>) -> ResponseResult {
        #[derive(Deserialize)]
        struct Params {
            #[serde(default)]
            session_id: Option<String>,
        }
        let p: Params = try_parse!(params);
        match p.session_id.as_deref() {
            Some(session_id) => match self.skill_definitions_for_session(session_id).await {
                Ok(skills) => success(skills),
                Err(e) => core_error(e),
            },
            None => success(self.skill_definitions().await),
        }
    }

    pub(super) fn handle_tools_agents(&self, _params: Option<Value>) -> ResponseResult {
        let agents: Vec<Value> = self
            .agent_definitions()
            .into_iter()
            .map(|a| {
                serde_json::json!({
                    "agent_type": a.agent_type,
                    "description": a.description,
                })
            })
            .collect();
        success(agents)
    }

    pub(super) async fn handle_tools_agents_with_warnings(
        &self,
        _params: Option<Value>,
    ) -> ResponseResult {
        let (definitions, warnings) = self.agent_definitions_with_warnings().await;
        success(serde_json::json!({
            "definitions": definitions,
            "warnings": warnings,
        }))
    }

    pub(super) async fn handle_tools_plan(&self, _params: Option<Value>) -> ResponseResult {
        match self.plan_overview().await {
            Ok(plan) => success(serde_json::json!({
                "plan_file": plan.plan_file,
                "state_file": plan.state_file,
                "in_plan_mode": plan.in_plan_mode,
                "plan": plan.plan,
            })),
            Err(e) => core_error(e),
        }
    }

    pub(super) async fn handle_tools_task_list(&self, params: Option<Value>) -> ResponseResult {
        #[derive(Deserialize)]
        struct Params {
            task_list_id: String,
        }
        let p: Params = try_parse!(params);
        match self.load_task_list_snapshot(&p.task_list_id).await {
            Ok(snapshot) => success(serde_json::json!({
                "task_list_id": snapshot.task_list_id,
                "directory": snapshot.directory,
                "tasks": snapshot.tasks.iter().map(|t| serde_json::json!({
                    "id": t.id,
                    "subject": t.subject,
                    "description": t.description,
                    "status": format!("{:?}", t.status),
                })).collect::<Vec<_>>(),
            })),
            Err(e) => core_error(e),
        }
    }

    pub(super) async fn handle_tools_enter_plan(&self, _params: Option<Value>) -> ResponseResult {
        match self.enter_plan_mode().await {
            Ok(outcome) => success(serde_json::json!({
                "name": outcome.name,
                "summary": outcome.summary,
                "output": outcome.output,
                "metadata": outcome.metadata,
            })),
            Err(e) => core_error(e),
        }
    }
}

fn default_empty_object() -> String {
    "{}".to_string()
}
