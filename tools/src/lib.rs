pub use orbcode_protocol::{ProviderToolDefinition, SandboxMode};

mod bash;
mod bash_cwd;
mod catalog;
mod encoding;
mod file_state;
mod file_tools;
mod fs_text;
mod glob_tool;
mod grep_tool;
mod interaction;
mod local_shell_task;
mod lsp;
mod mcp_tools;
mod metadata;
mod notebook;
mod output;
mod payload;
mod permissions;
mod plan_tools;
mod process;
mod progress;
mod registry;
mod skills;
mod task_tools;
mod web_cache;
mod web_fetch;
mod web_search;
mod web_search_adapters;

pub use background_cancellation::{
    cancel_background_task, has_background_task_cancel_flag, register_background_task_cancel_flag,
    unregister_background_task_cancel_flag,
};
pub use bash::bash_input_requests_sandbox_escalation;
pub use catalog::{
    mcp_provider_tool_name, parse_mcp_provider_tool_name, parse_plugin_provider_tool_name,
    plugin_provider_tool_name, provider_facing_tool_name,
};
pub use file_state::FileReadState;
pub use local_shell_task::{
    CreateLocalShellTask, LocalShellAttempt, LocalShellTaskRecord, LocalShellTaskRegistry,
    LocalShellTaskStatus,
};
pub use metadata::{post_tool_response, tool_error_result_metadata, tool_result_metadata};
pub use plan_tools::workspace_plan_snapshot;
pub use skills::{
    McpSkillPrompt, SkillDefinition, SkillSource, load_skill_definitions,
    load_skill_definitions_with_bounded_mcp, load_skill_definitions_with_bounded_mcp_for_session,
    load_skill_definitions_with_bundled, load_skill_definitions_with_bundled_and_mcp,
    load_skill_definitions_with_mcp, resolve_requested_skills,
};
pub use task_tools::{
    BackgroundTaskKind, BackgroundTaskRecord, BackgroundTaskStatus, TaskListSnapshot,
    TaskListSummary, TaskStatusKind, TaskView, background_job_path, background_jobs_dir,
    background_log_path, background_logs_dir, list_local_agent_records_for_session,
    load_task_list_snapshot, read_background_task_record, session_task_list_id,
    task_record_to_view, workspace_task_list_dir, workspace_task_list_id,
    write_background_task_record,
};

mod background_cancellation;
mod background_progress;
pub use background_progress::{
    register_progress_stream, subscribe_progress_stream, unregister_progress_stream,
};
mod types;
pub use types::{
    AgentToolInput, AskUserRequest, InteractionToolVisibility, PluginDispatchError,
    ToolCancellationToken, ToolContext, ToolError, ToolOutcome, ToolProgressReporter, ToolRegistry,
    ToolSpec, ToolStatus, WorkspacePlanSnapshot,
};

#[cfg(test)]
mod tests;
