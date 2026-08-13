use std::sync::Arc;
use std::sync::atomic::AtomicBool;
use std::time::Duration;

use orbcode_config::{AgentSource, AppConfigOverrides};
use orbcode_mcp::{McpAuth, McpServerConfig, McpServerStatus, McpServerTrust, McpTransport};
use orbcode_protocol::StreamEvent;
use orbcode_session_store::ChildSessionStatus;
use orbcode_tools::{
    BackgroundTaskKind, BackgroundTaskStatus, ToolError, read_background_task_record,
};
use tokio::sync::mpsc;
use tokio::time::sleep;

use super::support::{test_manager, test_manager_with_overrides};
use crate::agent_loop::tool_round::{ToolRoundScheduler, ToolRoundToolUse};

const MCP_SKILL_BODY_MARKER: &str = "MCP_GUIDE_BODY_FROM_SERVER";

fn docs_mcp_skill_server_config() -> McpServerConfig {
    let script = r#"while IFS= read -r line; do
  id=$(printf '%s\n' "$line" | sed -n 's/.*"id":\([0-9][0-9]*\).*/\1/p')
  if [ -z "$id" ]; then id=1; fi
  case "$line" in
    *'"method":"initialize"'*)
      result='{"protocolVersion":"2024-11-05","capabilities":{"prompts":{}},"serverInfo":{"name":"docs","version":"0.1.0"}}'
      ;;
    *'"method":"prompts/list"'*)
      result='{"prompts":[{"name":"guide","description":"Use docs guide.","_meta":{"skill":true}},{"name":"plain","description":"Plain prompt."}]}'
      ;;
    *'"method":"prompts/get"'*)
      result='{"description":"Use docs guide.","messages":[{"role":"user","content":{"type":"text","text":"MCP_GUIDE_BODY_FROM_SERVER"}}]}'
      ;;
    *)
      result='{}'
      ;;
  esac
  printf '{"jsonrpc":"2.0","id":%s,"result":%s}\n' "$id" "$result"
done"#;

    McpServerConfig {
        id: "docs".to_string(),
        transport: McpTransport::Stdio,
        endpoint: "sh".to_string(),
        args: vec!["-c".to_string(), script.to_string()],
        env: std::collections::BTreeMap::new(),
        cwd: None,
        headers: std::collections::BTreeMap::new(),
        enabled: true,
        status: McpServerStatus::Ready,
        error: None,
        summary: "Docs MCP".to_string(),
        auth: McpAuth::None,
        trust: McpServerTrust::Trusted,
        transport_type_hint: None,
        source: None,
    }
}

async fn register_docs_mcp_skill_server(manager: &super::super::SessionManager) {
    manager
        .mcp
        .upsert_server(docs_mcp_skill_server_config())
        .await
        .expect("register docs MCP skill server");
}

#[tokio::test]
async fn refresh_agent_definitions_loads_project_fixture_and_overrides_built_in() {
    let mut manager = test_manager().await;

    // SessionManager initializes with built-in agents.
    let built_in = manager
        .lookup_agent_definition("general-purpose")
        .expect("built-in general-purpose agent present");
    assert_eq!(built_in.source, AgentSource::BuiltIn);

    // Write a project agent fixture that overrides the built-in by agent_type
    // and also adds a brand-new agent definition.
    let cwd = manager.config().cwd.clone();
    let project_agents = cwd.join(".claude").join("agents");
    tokio::fs::create_dir_all(&project_agents)
        .await
        .expect("create project agents dir");
    tokio::fs::write(
        project_agents.join("general-purpose.md"),
        concat!(
            "---\n",
            "name: general-purpose\n",
            "description: Project override of the general-purpose agent.\n",
            "model: project-model\n",
            "tools: Read, Grep\n",
            "---\n",
            "Project-defined general-purpose instructions.\n",
        ),
    )
    .await
    .expect("write project agent");
    tokio::fs::write(
        project_agents.join("rust-reviewer.md"),
        concat!(
            "---\n",
            "name: rust-reviewer\n",
            "description: Reviews Rust code for ownership issues.\n",
            "tools: Read, Grep, Bash\n",
            "---\n",
            "Be pedantic about ownership.\n",
        ),
    )
    .await
    .expect("write rust reviewer agent");

    manager.refresh_agent_definitions().await;

    let overridden = manager
        .lookup_agent_definition("general-purpose")
        .expect("project override present");
    assert_eq!(overridden.source, AgentSource::ProjectSettings);
    assert_eq!(overridden.model.as_deref(), Some("project-model"));
    assert_eq!(
        overridden
            .tools
            .as_deref()
            .map(<[std::string::String]>::to_vec),
        Some(vec!["Read".to_string(), "Grep".to_string()])
    );
    assert!(overridden.prompt.contains("Project-defined"));

    let new_agent = manager
        .lookup_agent_definition("rust-reviewer")
        .expect("new project agent present");
    assert_eq!(new_agent.source, AgentSource::ProjectSettings);
    assert_eq!(
        new_agent
            .tools
            .as_deref()
            .map(<[std::string::String]>::to_vec),
        Some(vec![
            "Read".to_string(),
            "Grep".to_string(),
            "Bash".to_string()
        ])
    );
}

#[tokio::test]
async fn lookup_agent_definition_returns_none_for_unknown_type() {
    let manager = test_manager().await;
    assert!(manager.lookup_agent_definition("does-not-exist").is_none());
}

#[tokio::test]
async fn agent_tool_persists_child_session_metadata_and_marks_completed() {
    let manager = test_manager_with_overrides(AppConfigOverrides {
        allow_network: Some(true),
        allow_tools: Some(true),
        ..AppConfigOverrides::default()
    })
    .await;
    let (session, _) = manager.start_or_resume(None).await.expect("create session");
    let session_id = session.session_id.clone();

    // No child metadata before the agent runs.
    assert!(
        manager
            .child_sessions_for(&session_id)
            .await
            .expect("list children")
            .is_empty()
    );

    let mut rx = manager
        .submit_turn(
            &session_id,
            r#"#tool:Agent {"description":"Summarize repo","prompt":"summarize the workspace","subagent_type":"general-purpose"}"#,
        )
        .await
        .expect("submit turn");

    while let Some(event) = rx.recv().await {
        if matches!(event, StreamEvent::TurnFinished { .. }) {
            break;
        }
    }

    let children = manager
        .child_sessions_for(&session_id)
        .await
        .expect("list children");
    assert_eq!(children.len(), 1, "exactly one child metadata expected");
    let child = &children[0];
    assert_eq!(child.parent_session_id, session_id);
    assert_eq!(child.agent_type, "general-purpose");
    assert_eq!(child.status, ChildSessionStatus::Completed);
    assert!(child.ended_at.is_some());
    assert!(child.error_message.is_none());
    assert!(child.prompt_preview.contains("summarize the workspace"));
    assert_eq!(
        child.source_tool_use_id,
        format!("toolu-{session_id}"),
        "metadata must point back at the parent's Agent tool_use",
    );
    assert!(
        child
            .child_session_id
            .starts_with(&format!("{session_id}:agent-")),
        "child id should be derived from parent + agent suffix: {}",
        child.child_session_id,
    );

    // Parent's list_sessions must not surface the child as a top-level session
    // — child transcripts are deliberately kept off disk so resume cannot mix
    // them into the parent timeline.
    let summaries = manager.list_sessions().await.expect("list sessions");
    assert!(
        summaries
            .iter()
            .any(|summary| summary.session_id == session_id),
        "parent session present in list_sessions",
    );
    assert!(
        !summaries
            .iter()
            .any(|summary| summary.session_id == child.child_session_id),
        "child session must not appear as a separate top-level session",
    );
}

#[tokio::test]
async fn agent_preloaded_skills_appear_in_child_provider_request_system_prompt() {
    let mut manager = test_manager_with_overrides(AppConfigOverrides {
        allow_network: Some(true),
        allow_tools: Some(true),
        ..AppConfigOverrides::default()
    })
    .await;
    let home = manager.config().home_dir.clone();
    let cwd = manager.config().cwd.clone();

    // Write a project skill that the agent will preload.
    let project_skills = cwd.join(".claude").join("skills").join("rust-patterns");
    tokio::fs::create_dir_all(&project_skills).await.unwrap();
    tokio::fs::write(
        project_skills.join("SKILL.md"),
        "---\nname: rust-patterns\ndescription: Idiomatic Rust\n---\nPREFER_BORROW_INSTEAD_OF_CLONE_MARKER\n",
    )
    .await
    .unwrap();
    // Also write a user-level skill with the same name that the project must
    // override, plus an unrelated user skill that should NOT be injected.
    let user_skill_override = home.join("skills").join("rust-patterns");
    tokio::fs::create_dir_all(&user_skill_override)
        .await
        .unwrap();
    tokio::fs::write(
        user_skill_override.join("SKILL.md"),
        "---\nname: rust-patterns\n---\nUSER_VERSION_THAT_PROJECT_OVERRIDES\n",
    )
    .await
    .unwrap();
    let unrelated = home.join("skills").join("other-skill");
    tokio::fs::create_dir_all(&unrelated).await.unwrap();
    tokio::fs::write(
        unrelated.join("SKILL.md"),
        "---\nname: other-skill\n---\nUNRELATED_SKILL_NOT_REQUESTED\n",
    )
    .await
    .unwrap();

    // Write a project agent that declares the rust-patterns skill plus an
    // unknown one to confirm unknown names are silently dropped.
    let project_agents = cwd.join(".claude").join("agents");
    tokio::fs::create_dir_all(&project_agents).await.unwrap();
    tokio::fs::write(
        project_agents.join("rust-reviewer.md"),
        concat!(
            "---\n",
            "name: rust-reviewer\n",
            "description: Reviews Rust code for ownership.\n",
            "skills: rust-patterns, does-not-exist\n",
            "---\n",
            "Review Rust diffs.\n",
        ),
    )
    .await
    .unwrap();
    manager.refresh_agent_definitions().await;

    let (session, _) = manager.start_or_resume(None).await.expect("create session");
    let session_id = session.session_id.clone();

    let mut rx = manager
        .submit_turn(
            &session_id,
            r#"#tool:Agent {"description":"Review repo","prompt":"review the workspace","subagent_type":"rust-reviewer"}"#,
        )
        .await
        .expect("submit turn");

    while let Some(event) = rx.recv().await {
        if matches!(event, StreamEvent::TurnFinished { .. }) {
            break;
        }
    }

    let agent_snapshot = manager
        .last_provider_request_snapshot_for_source("agent")
        .await
        .expect("child agent request snapshot recorded");
    assert!(
        agent_snapshot.body_json.contains("## Preloaded skills"),
        "child request system prompt must contain skills section. body: {}",
        agent_snapshot.body_json,
    );
    assert!(
        agent_snapshot
            .body_json
            .contains("PREFER_BORROW_INSTEAD_OF_CLONE_MARKER"),
        "project-version skill body must be injected (overriding user). body: {}",
        agent_snapshot.body_json,
    );
    assert!(
        !agent_snapshot
            .body_json
            .contains("USER_VERSION_THAT_PROJECT_OVERRIDES"),
        "user-version body must NOT appear when project overrides it. body: {}",
        agent_snapshot.body_json,
    );
    assert!(
        !agent_snapshot
            .body_json
            .contains("UNRELATED_SKILL_NOT_REQUESTED"),
        "skills not requested by the agent must not leak in. body: {}",
        agent_snapshot.body_json,
    );

    // Parent isolation: the parent turn requests (source=="turn") must not
    // pick up the skill content even though the agent ran inside this turn.
    let parent_snapshot = manager
        .last_provider_request_snapshot_for_source("turn")
        .await
        .expect("parent turn snapshot recorded");
    assert!(
        !parent_snapshot.body_json.contains("## Preloaded skills"),
        "parent turn request must not contain agent skill section",
    );
    assert!(
        !parent_snapshot
            .body_json
            .contains("PREFER_BORROW_INSTEAD_OF_CLONE_MARKER"),
        "skill body must not leak into the parent turn request",
    );
}

#[tokio::test]
async fn agent_without_declared_skills_does_not_add_preload_section() {
    let manager = test_manager_with_overrides(AppConfigOverrides {
        allow_network: Some(true),
        allow_tools: Some(true),
        ..AppConfigOverrides::default()
    })
    .await;

    // A bare project skill exists, but no agent declares it.
    let cwd = manager.config().cwd.clone();
    let project_skills = cwd.join(".claude").join("skills").join("dormant-skill");
    tokio::fs::create_dir_all(&project_skills).await.unwrap();
    tokio::fs::write(
        project_skills.join("SKILL.md"),
        "---\nname: dormant-skill\n---\nDORMANT_SKILL_BODY_MARKER\n",
    )
    .await
    .unwrap();

    let (session, _) = manager.start_or_resume(None).await.expect("create session");
    let session_id = session.session_id.clone();

    let mut rx = manager
        .submit_turn(
            &session_id,
            r#"#tool:Agent {"description":"Summarize","prompt":"summarize the workspace","subagent_type":"general-purpose"}"#,
        )
        .await
        .expect("submit turn");
    while let Some(event) = rx.recv().await {
        if matches!(event, StreamEvent::TurnFinished { .. }) {
            break;
        }
    }

    let agent_snapshot = manager
        .last_provider_request_snapshot_for_source("agent")
        .await
        .expect("child agent request snapshot recorded");
    assert!(
        !agent_snapshot.body_json.contains("## Preloaded skills"),
        "agents without `skills:` should not gain a preload section",
    );
    assert!(
        !agent_snapshot
            .body_json
            .contains("DORMANT_SKILL_BODY_MARKER"),
        "skill bodies must not leak into agents that did not request them",
    );
}

#[tokio::test]
async fn agent_run_in_background_persists_durable_record_and_returns_immediately() {
    let manager = test_manager_with_overrides(AppConfigOverrides {
        allow_network: Some(true),
        allow_tools: Some(true),
        ..AppConfigOverrides::default()
    })
    .await;
    let (session, _) = manager.start_or_resume(None).await.expect("create session");
    let session_id = session.session_id.clone();
    let home_dir = manager.config().home_dir.clone();

    let mut rx = manager
        .submit_turn(
            &session_id,
            r#"#tool:Agent {"description":"Background explore","prompt":"summarize the workspace","subagent_type":"general-purpose","run_in_background":true}"#,
        )
        .await
        .expect("submit turn");

    let mut tool_result_payload = None;
    while let Some(event) = rx.recv().await {
        if let StreamEvent::UserMessage { message } = &event {
            for block in &message.blocks {
                if let orbcode_protocol::TranscriptBlock::ToolResult {
                    content, metadata, ..
                } = block
                    && content.contains("Background subagent started")
                {
                    tool_result_payload = Some((content.clone(), metadata.clone()));
                }
            }
        }
        if matches!(event, StreamEvent::TurnFinished { .. }) {
            break;
        }
    }
    let (content, metadata) =
        tool_result_payload.expect("background started tool result emitted to parent");
    assert!(content.contains("Task ID:"));
    assert!(
        content.contains("you MUST quote this task_id verbatim"),
        "started message must instruct model to quote task_id verbatim. content was:\n{content}",
    );
    let metadata_value: serde_json::Value =
        serde_json::from_str(metadata.as_deref().expect("metadata json")).expect("parse metadata");
    let task_id = metadata_value["task_id"]
        .as_str()
        .expect("task_id present")
        .to_string();
    assert_eq!(metadata_value["task_type"], "local_agent");
    assert!(task_id.starts_with("agent-"), "task_id format: {task_id}");
    assert!(
        content.contains(&task_id),
        "started message must include the literal task_id ({task_id}). content was:\n{content}",
    );

    // Parent's tool_use_id is reachable from the durable record so TaskOutput
    // can join the background task back to the parent transcript later.
    let deadline = std::time::Instant::now() + Duration::from_secs(5);
    let final_record = loop {
        let record = read_background_task_record(&home_dir, &task_id)
            .await
            .expect("read durable record")
            .expect("record present");
        if !record.status.is_active() {
            break record;
        }
        if std::time::Instant::now() >= deadline {
            panic!(
                "background agent did not finish: status={:?}",
                record.status
            );
        }
        sleep(Duration::from_millis(25)).await;
    };
    assert_eq!(final_record.task_kind, BackgroundTaskKind::LocalAgent);
    assert_eq!(final_record.status, BackgroundTaskStatus::Completed);
    assert_eq!(final_record.session_id, session_id);
    assert_eq!(final_record.agent_type.as_deref(), Some("general-purpose"));
    assert!(final_record.tool_use_id.is_some());
    assert!(final_record.child_session_id.is_some());
    assert!(
        final_record.result.is_some(),
        "completed background agent must persist a final result"
    );
    assert!(final_record.finished_at.is_some());

    let children = manager
        .child_sessions_for(&session_id)
        .await
        .expect("list children");
    let child = children
        .iter()
        .find(|child| {
            final_record.child_session_id.as_deref() == Some(child.child_session_id.as_str())
        })
        .expect("child metadata for background agent present");
    assert_eq!(child.status, ChildSessionStatus::Completed);
}

#[tokio::test]
async fn nested_agent_tool_invocation_is_rejected_without_state_leak() {
    let manager = test_manager().await;
    let (tx, mut rx) = mpsc::unbounded_channel::<StreamEvent>();
    let cancel = Arc::new(AtomicBool::new(false));

    let result = manager
        .invoke_nested_agent_tool(
            "session-1",
            "parent-tool-use",
            "child-agent-1",
            "Agent",
            "{\"description\":\"x\",\"prompt\":\"y\"}",
            None,
            true,
            true,
            &tx,
            cancel,
        )
        .await;

    let Err(ToolError::ExecutionFailed(msg)) = result else {
        panic!("expected ExecutionFailed, got {result:?}");
    };
    assert!(
        msg.contains("nested Agent"),
        "error message should reference nested Agent: {msg}"
    );
    drop(tx);
    let leaked: Vec<_> = std::iter::from_fn(|| rx.try_recv().ok()).collect();
    assert!(
        leaked.is_empty(),
        "nested Agent rejection must not emit stream events: {leaked:?}"
    );

    let children = manager
        .child_sessions_for("session-1")
        .await
        .expect("list children");
    assert!(
        children.is_empty(),
        "nested Agent rejection must not register a child session: {children:?}"
    );
}

#[tokio::test]
async fn streamed_tool_runtime_skill_loads_trusted_mcp_prompt_skill() {
    let manager = test_manager_with_overrides(AppConfigOverrides {
        allow_network: Some(true),
        allow_tools: Some(true),
        ..AppConfigOverrides::default()
    })
    .await;
    register_docs_mcp_skill_server(&manager).await;
    let (session, _) = manager.start_or_resume(None).await.expect("create session");
    let (tx, _rx) = mpsc::unbounded_channel::<StreamEvent>();
    let cancel = Arc::new(AtomicBool::new(false));
    let mut scheduler = ToolRoundScheduler::new();
    let ready_item = scheduler.accept_tool_use(ToolRoundToolUse::new(
        "toolu-streamed-skill",
        "Skill",
        r#"{"skill":"docs:guide","args":"topic"}"#,
    ));

    let execution = manager
        .start_streamed_tool_execution(&session.session_id, ready_item, &tx, cancel)
        .await
        .expect("streamed Skill should start");
    let completion = execution.finish().await.expect("finish streamed Skill");

    assert_eq!(
        completion.result.completion_kind,
        orbcode_protocol::ToolUseCompletionKind::Success
    );
    assert!(!completion.result.is_error);
    assert!(
        completion.result.content.contains(MCP_SKILL_BODY_MARKER),
        "streamed Skill must load MCP-backed skill definitions, got: {}",
        completion.result.content
    );
}

#[tokio::test]
async fn streamed_skill_loads_session_scoped_mcp_prompt_skill() {
    let manager = test_manager_with_overrides(AppConfigOverrides {
        allow_network: Some(true),
        allow_tools: Some(true),
        ..AppConfigOverrides::default()
    })
    .await;
    let (session, _) = manager
        .start_or_resume_with_setup(None, None, Vec::new(), vec![docs_mcp_skill_server_config()])
        .await
        .expect("create session with scoped MCP server");
    assert!(
        !manager
            .mcp
            .list_servers()
            .await
            .iter()
            .any(|server| server.id == "docs"),
        "session-scoped MCP server must not be globally visible"
    );
    let server_id = manager
        .mcp
        .list_servers_for_session(&session.session_id)
        .await
        .into_iter()
        .find(|server| server.summary == "Docs MCP")
        .expect("session MCP server visible")
        .id;
    manager
        .mcp
        .set_server_trust_for_session(&session.session_id, &server_id, McpServerTrust::Trusted)
        .await
        .expect("trust session MCP server");

    let (tx, _rx) = mpsc::unbounded_channel::<StreamEvent>();
    let cancel = Arc::new(AtomicBool::new(false));
    let mut scheduler = ToolRoundScheduler::new();
    let ready_item = scheduler.accept_tool_use(ToolRoundToolUse::new(
        "toolu-session-skill",
        "Skill",
        format!(r#"{{"skill":"{server_id}:guide","args":"topic"}}"#),
    ));

    let execution = manager
        .start_streamed_tool_execution(&session.session_id, ready_item, &tx, cancel)
        .await
        .expect("streamed session Skill should start");
    let completion = execution.finish().await.expect("finish session Skill");

    assert_eq!(
        completion.result.completion_kind,
        orbcode_protocol::ToolUseCompletionKind::Success
    );
    assert!(!completion.result.is_error);
    assert!(
        completion.result.content.contains(MCP_SKILL_BODY_MARKER),
        "session-scoped Skill must load MCP-backed skill definitions, got: {}",
        completion.result.content
    );
}

#[tokio::test]
async fn nested_agent_skill_tool_loads_trusted_mcp_prompt_skill() {
    let manager = test_manager_with_overrides(AppConfigOverrides {
        allow_network: Some(true),
        allow_tools: Some(true),
        ..AppConfigOverrides::default()
    })
    .await;
    register_docs_mcp_skill_server(&manager).await;
    let (tx, _rx) = mpsc::unbounded_channel::<StreamEvent>();
    let cancel = Arc::new(AtomicBool::new(false));

    let outcome = manager
        .invoke_nested_agent_tool(
            "session-1",
            "parent-tool-use",
            "child-agent-1",
            "Skill",
            r#"{"skill":"docs:guide","args":"topic"}"#,
            None,
            true,
            true,
            &tx,
            cancel,
        )
        .await
        .expect("nested Skill should load MCP skill");

    assert!(
        outcome.output.contains(MCP_SKILL_BODY_MARKER),
        "nested Skill must load MCP-backed skill definitions, got: {}",
        outcome.output
    );
}

#[tokio::test]
async fn nested_agent_network_tool_requires_inherited_parent_grant() {
    let manager = test_manager().await;
    let (tx, _rx) = mpsc::unbounded_channel::<StreamEvent>();

    let result = manager
        .invoke_nested_agent_tool(
            "session-1",
            "parent-tool-use",
            "child-agent-1",
            "WebFetch",
            r#"{"url":"https://example.com"}"#,
            None,
            true,
            false,
            &tx,
            Arc::new(AtomicBool::new(false)),
        )
        .await;

    let Err(ToolError::ExecutionFailed(message)) = result else {
        panic!("expected the nested network tool to be blocked, got {result:?}");
    };
    assert!(message.contains("permission this agent was not granted"));
}

#[tokio::test]
async fn agent_with_mcp_server_names_completes_with_filtered_registry() {
    let mut manager = test_manager_with_overrides(AppConfigOverrides {
        allow_network: Some(true),
        allow_tools: Some(true),
        ..AppConfigOverrides::default()
    })
    .await;
    let cwd = manager.config().cwd.clone();

    let project_agents = cwd.join(".claude").join("agents");
    tokio::fs::create_dir_all(&project_agents).await.unwrap();
    tokio::fs::write(
        project_agents.join("mcp-scoped.md"),
        concat!(
            "---\n",
            "name: mcp-scoped\n",
            "description: Agent with scoped MCP servers.\n",
            "mcpServers: context7\n",
            "---\n",
            "You only have access to the context7 MCP server.\n",
        ),
    )
    .await
    .unwrap();
    manager.refresh_agent_definitions().await;

    let definition = manager
        .lookup_agent_definition("mcp-scoped")
        .expect("mcp-scoped agent present");
    assert_eq!(
        definition.mcp_server_names,
        Some(vec!["context7".to_string()]),
        "agent definition must parse mcpServers field"
    );

    let (session, _) = manager.start_or_resume(None).await.expect("create session");
    let session_id = session.session_id.clone();

    let mut rx = manager
        .submit_turn(
            &session_id,
            r#"#tool:Agent {"description":"Search docs","prompt":"find the API reference","subagent_type":"mcp-scoped"}"#,
        )
        .await
        .expect("submit turn");

    while let Some(event) = rx.recv().await {
        if matches!(event, StreamEvent::TurnFinished { .. }) {
            break;
        }
    }

    let children = manager
        .child_sessions_for(&session_id)
        .await
        .expect("list children");
    assert_eq!(children.len(), 1, "exactly one child session");
    let child = &children[0];
    assert_eq!(child.agent_type, "mcp-scoped");
    assert_eq!(child.status, ChildSessionStatus::Completed);

    let agent_snapshot = manager
        .last_provider_request_snapshot_for_source("agent")
        .await
        .expect("child agent request snapshot");
    assert!(
        agent_snapshot.body_json.contains("context7"),
        "child system prompt should reference scoped MCP context: {}",
        &agent_snapshot.body_json[..agent_snapshot.body_json.len().min(500)],
    );

    let parent_servers = manager.mcp.list_servers().await;
    assert!(
        parent_servers.is_empty() || parent_servers.iter().all(|s| s.id != "context7"),
        "parent MCP registry must not have been mutated by child agent lifecycle"
    );
}
