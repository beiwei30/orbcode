use orbcode_protocol::{
    ProviderToolDefinition, SessionGoal, SessionGoalStatus, ToolUseCompletionKind,
};
use orbcode_tools::{ToolCapability, ToolSpec, ToolStatus};
use serde::Deserialize;
use serde_json::{Value, json};

use super::{GoalError, GoalSetRequest, GoalUpdateAuthority, SessionManager};
use crate::tool_flow::{BufferedToolResult, BufferedToolUseCompletion, ToolUseOutcome};

const GET_GOAL_SPEC: ToolSpec = ToolSpec {
    name: "get_goal",
    status: ToolStatus::Available,
    summary: "Read the current persistent goal and its revision, status, budget, usage, elapsed time, and last supervised turn.",
    requires_tools_permission: false,
    requires_network_permission: false,
    capability: ToolCapability::Internal,
    provider_hidden: false,
};

const CREATE_GOAL_SPEC: ToolSpec = ToolSpec {
    name: "create_goal",
    status: ToolStatus::Available,
    summary: "Create one active persistent session goal. This rejects an unfinished existing goal and may replace a completed goal.",
    requires_tools_permission: false,
    requires_network_permission: false,
    capability: ToolCapability::Internal,
    provider_hidden: false,
};

const UPDATE_GOAL_SPEC: ToolSpec = ToolSpec {
    name: "update_goal",
    status: ToolStatus::Available,
    summary: "Mark the current persistent goal complete or blocked using its identity and revision. Use blocked only after the same blocking condition has recurred for at least three consecutive goal turns; include that condition as stop_reason.",
    requires_tools_permission: false,
    requires_network_permission: false,
    capability: ToolCapability::Internal,
    provider_hidden: false,
};

const GOAL_TOOL_SPECS: [ToolSpec; 3] = [GET_GOAL_SPEC, CREATE_GOAL_SPEC, UPDATE_GOAL_SPEC];

pub fn persistent_goal_tool_specs() -> &'static [ToolSpec] {
    &GOAL_TOOL_SPECS
}

pub fn persistent_goal_tool_definitions() -> Vec<ProviderToolDefinition> {
    vec![
        ProviderToolDefinition {
            name: "get_goal".to_string(),
            description: GET_GOAL_SPEC.summary.to_string(),
            input_schema: json!({
                "type": "object",
                "properties": {},
                "additionalProperties": false
            }),
        },
        ProviderToolDefinition {
            name: "create_goal".to_string(),
            description: CREATE_GOAL_SPEC.summary.to_string(),
            input_schema: json!({
                "type": "object",
                "properties": {
                    "objective": { "type": "string", "minLength": 1 },
                    "token_budget": { "type": "integer", "minimum": 1 }
                },
                "required": ["objective"],
                "additionalProperties": false
            }),
        },
        ProviderToolDefinition {
            name: "update_goal".to_string(),
            description: UPDATE_GOAL_SPEC.summary.to_string(),
            input_schema: json!({
                "type": "object",
                "properties": {
                    "goal_id": { "type": "string", "minLength": 1 },
                    "expected_revision": { "type": "integer", "minimum": 1 },
                    "status": { "type": "string", "enum": ["complete", "blocked"] },
                    "stop_reason": { "type": "string", "minLength": 1 }
                },
                "required": ["goal_id", "expected_revision", "status"],
                "additionalProperties": false
            }),
        },
    ]
}

pub(crate) fn persistent_goal_tool_spec(name: &str) -> Option<ToolSpec> {
    GOAL_TOOL_SPECS
        .iter()
        .find(|spec| spec.name == name)
        .cloned()
}

pub(crate) fn is_persistent_goal_tool(name: &str) -> bool {
    persistent_goal_tool_spec(name).is_some()
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct CreateGoalToolInput {
    objective: String,
    #[serde(default)]
    token_budget: Option<u64>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct UpdateGoalToolInput {
    goal_id: String,
    expected_revision: u64,
    status: SessionGoalStatus,
    #[serde(default)]
    stop_reason: Option<String>,
}

impl SessionManager {
    pub(super) async fn invoke_persistent_goal_tool_and_buffer_result(
        &self,
        session_id: &str,
        tool_use_id: &str,
        tool_name: &str,
        tool_input: &str,
    ) -> BufferedToolUseCompletion {
        let result = self
            .execute_persistent_goal_tool(session_id, tool_name, tool_input)
            .await;
        let (content, is_error, completion_kind) = match result {
            Ok(value) => (value.to_string(), false, ToolUseCompletionKind::Success),
            Err(error) => (
                json!({
                    "error": {
                        "code": goal_error_code(&error),
                        "message": error.to_string()
                    }
                })
                .to_string(),
                true,
                ToolUseCompletionKind::ExecutionFailed,
            ),
        };
        BufferedToolUseCompletion {
            outcome: ToolUseOutcome::Continue,
            result: BufferedToolResult {
                tool_use_id: tool_use_id.to_string(),
                tool_name: tool_name.to_string(),
                content,
                is_error,
                metadata: None,
                completion_kind,
            },
        }
    }

    async fn execute_persistent_goal_tool(
        &self,
        session_id: &str,
        tool_name: &str,
        tool_input: &str,
    ) -> Result<Value, GoalError> {
        match tool_name {
            "get_goal" => {
                parse_empty_input(tool_input)?;
                let goal = self.get_goal(session_id).await?;
                Ok(json!({ "goal": goal }))
            }
            "create_goal" => {
                let input: CreateGoalToolInput = parse_input(tool_input)?;
                let current = self.get_goal(session_id).await?;
                let (expected_revision, replace) = match current.as_ref() {
                    None => (None, false),
                    Some(goal) if goal.status == SessionGoalStatus::Complete => {
                        (Some(goal.revision), true)
                    }
                    Some(_) => return Err(GoalError::UnfinishedGoal),
                };
                let goal = self
                    .set_goal(GoalSetRequest {
                        session_id: session_id.to_string(),
                        expected_revision,
                        replace,
                        objective: Some(input.objective),
                        status: None,
                        token_budget: input.token_budget.map(Some),
                        stop_reason: None,
                        authority: GoalUpdateAuthority::Model,
                    })
                    .await?;
                Ok(goal_result(goal))
            }
            "update_goal" => {
                let input: UpdateGoalToolInput = parse_input(tool_input)?;
                if !matches!(
                    input.status,
                    SessionGoalStatus::Complete | SessionGoalStatus::Blocked
                ) {
                    return Err(GoalError::InvalidTransition {
                        from: self
                            .get_goal(session_id)
                            .await?
                            .map_or(SessionGoalStatus::Active, |goal| goal.status),
                        to: input.status,
                        authority: GoalUpdateAuthority::Model,
                    });
                }
                let current = self.get_goal(session_id).await?.ok_or(GoalError::Missing)?;
                if current.goal_id != input.goal_id {
                    return Err(GoalError::GoalIdentityMismatch {
                        expected: input.goal_id,
                        actual: current.goal_id,
                    });
                }
                let goal = self
                    .set_goal(GoalSetRequest {
                        session_id: session_id.to_string(),
                        expected_revision: Some(input.expected_revision),
                        replace: false,
                        objective: None,
                        status: Some(input.status),
                        token_budget: None,
                        stop_reason: input.stop_reason.map(Some),
                        authority: GoalUpdateAuthority::Model,
                    })
                    .await?;
                Ok(goal_result(goal))
            }
            _ => Err(GoalError::Core(crate::CoreError::Tool(format!(
                "unknown persistent goal tool: {tool_name}"
            )))),
        }
    }
}

fn parse_empty_input(input: &str) -> Result<(), GoalError> {
    let value: Value = parse_input(input)?;
    if value.as_object().is_some_and(serde_json::Map::is_empty) {
        Ok(())
    } else {
        Err(goal_input_error("get_goal expects an empty JSON object"))
    }
}

fn parse_input<T: serde::de::DeserializeOwned>(input: &str) -> Result<T, GoalError> {
    serde_json::from_str(if input.trim().is_empty() { "{}" } else { input })
        .map_err(|error| goal_input_error(&format!("invalid persistent goal tool input: {error}")))
}

fn goal_input_error(message: &str) -> GoalError {
    GoalError::Core(crate::CoreError::Tool(message.to_string()))
}

fn goal_result(goal: SessionGoal) -> Value {
    let final_usage = (goal.status == SessionGoalStatus::Complete && goal.token_budget.is_some())
        .then(|| {
            json!({
                "tokens_used": goal.tokens_used,
                "token_budget": goal.token_budget,
                "elapsed_seconds": goal.elapsed_seconds
            })
        });
    json!({ "goal": goal, "final_usage": final_usage })
}

fn goal_error_code(error: &GoalError) -> &'static str {
    match error {
        GoalError::SessionNotFound(_) => "session_not_found",
        GoalError::Missing => "missing",
        GoalError::StaleRevision { .. } => "stale_revision",
        GoalError::GoalIdentityMismatch { .. } => "goal_identity_mismatch",
        GoalError::EmptyObjective => "empty_objective",
        GoalError::InvalidTokenBudget => "invalid_token_budget",
        GoalError::UnfinishedGoal => "unfinished_goal",
        GoalError::ReplacementNotAllowed => "replacement_not_allowed",
        GoalError::InvalidTransition { .. } => "invalid_transition",
        GoalError::BudgetIncreaseRequired => "budget_increase_required",
        GoalError::InvalidStopReason => "invalid_stop_reason",
        GoalError::Core(_) => "internal_error",
    }
}
