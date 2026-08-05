use super::*;
use std::fmt::Write as _;

pub fn transcript_research_fixture_state() -> TuiState {
    let mut state = normal_state("", 0);
    state.cwd = PathBuf::from("/Users/user/github/sample-workspace-main/crates/render-fixtures");
    state.cwd_display = "~/github/sample-workspace-main/crates/render-fixtures".to_string();
    state.ui_version = "2.1.119".to_string();
    state.model_display_name = "glm-4.7".to_string();
    state.editor_mode = EditorMode::Insert;
    state.messages = vec![
            TranscriptMessage::from_blocks(
                MessageRole::User,
                vec![TranscriptBlock::Text {
                    text: "梳理一下这个仓库的目录结构，并总结主要模块的职责"
                        .to_string(),
                }],
            ),
            TranscriptMessage::from_blocks(
                MessageRole::Assistant,
                vec![TranscriptBlock::Text {
                    text: "我来先看一下仓库的目录结构，整理主要模块的职责。".to_string(),
                }],
            ),
            TranscriptMessage::from_blocks(
                MessageRole::Assistant,
                vec![
                    TranscriptBlock::ToolUse {
                        id: "glob-1".to_string(),
                        name: "Glob".to_string(),
                        input: "{\"pattern\":\"**/Cargo.toml\"}".to_string(),
                    },
                    TranscriptBlock::ToolUse {
                        id: "list-1".to_string(),
                        name: "Bash".to_string(),
                        input: "{\"command\":\"ls -la\"}".to_string(),
                    },
                ],
            ),
            TranscriptMessage::from_blocks(
                MessageRole::User,
                vec![
                    TranscriptBlock::ToolResult {
                        tool_use_id: "glob-1".to_string(),
                        content: "Cargo.toml".to_string().into(),
                        is_error: false,
                        metadata: Some(
                            "{\"status\":\"completed\",\"summary\":\"Searched for workspace manifests.\"}"
                                .to_string(),
                        ),
                    },
                    TranscriptBlock::ToolResult {
                        tool_use_id: "list-1".to_string(),
                        content: "total 60".to_string().into(),
                        is_error: false,
                        metadata: Some(
                            "{\"status\":\"completed\",\"summary\":\"Listed workspace files.\"}"
                                .to_string(),
                        ),
                    },
                ],
            ),
            TranscriptMessage::from_blocks(
                MessageRole::Assistant,
                vec![TranscriptBlock::Text {
                    text: "我先确认当前目录和文件列表。".to_string(),
                }],
            ),
            TranscriptMessage::from_blocks(
                MessageRole::Assistant,
                vec![TranscriptBlock::ToolUse {
                    id: "bash-1".to_string(),
                    name: "Bash".to_string(),
                    input: "{\"description\":\"pwd && ls -la\",\"command\":\"pwd && ls -la\"}"
                        .to_string(),
                }],
            ),
            TranscriptMessage::from_blocks(
                MessageRole::User,
                vec![TranscriptBlock::ToolResult {
                    tool_use_id: "bash-1".to_string(),
                    content: "/Users/user/github/sample-workspace-main/crates/render-fixtures\n\
total 60\n\
drwxr-xr-x 16 user staff 512 Apr 22 14:40 ."
                        .to_string().into(),
                    is_error: false,
                    metadata: Some(
                        "{\"status\":\"completed\",\"summary\":\"Listed workspace files.\"}"
                            .to_string(),
                    ),
                }],
            ),
            TranscriptMessage::from_blocks(
                MessageRole::Assistant,
                vec![TranscriptBlock::Text {
                    text: "目录结构小结\nRust 主路径已经具备稳定的 transcript 汇总输出。\n现在这条场景可以被 fixture 测试固定下来。"
                        .to_string(),
                }],
            ),
        ];
    state
}

pub fn transcript_workflow_cards_fixture_state() -> TuiState {
    let mut state = normal_state("", 0);
    state.cwd = PathBuf::from("/Users/user/github/sample-workspace-main/crates/render-fixtures");
    state.cwd_display = "~/github/sample-workspace-main/crates/render-fixtures".to_string();
    state.model_display_name = "glm-4.7".to_string();
    state.expanded_tool_details = true;
    state.messages = vec![
            TranscriptMessage::from_blocks(
                MessageRole::Assistant,
                vec![TranscriptBlock::ToolUse {
                    id: "task-update".to_string(),
                    name: "TaskUpdate".to_string(),
                    input: "{\"task_id\":\"transcript-rendering\",\"status\":\"in_progress\",\"title\":\"Polish transcript rendering\"}".to_string(),
                }],
            ),
            TranscriptMessage::from_blocks(
                MessageRole::User,
                vec![TranscriptBlock::ToolResult {
                    tool_use_id: "task-update".to_string(),
                    content: String::new().into(),
                    is_error: false,
                    metadata: Some("{\"status\":\"completed\",\"summary\":\"Updated task `transcript-rendering`.\",\"changedPaths\":[\"/tmp/tasks/transcript-rendering.json\"]}".to_string()),
                }],
            ),
            TranscriptMessage::from_blocks(
                MessageRole::Assistant,
                vec![TranscriptBlock::ToolUse {
                    id: "skill-1".to_string(),
                    name: "Skill".to_string(),
                    input: "{\"name\":\"rust-review\",\"arguments\":\"focus on render polish\"}"
                        .to_string(),
                }],
            ),
            TranscriptMessage::from_blocks(
                MessageRole::User,
                vec![TranscriptBlock::ToolResult {
                    tool_use_id: "skill-1".to_string(),
                    content: String::new().into(),
                    is_error: false,
                    metadata: Some(
                        "{\"status\":\"completed\",\"summary\":\"Loaded skill `rust-review`.\"}"
                            .to_string(),
                    ),
                }],
            ),
            TranscriptMessage::from_blocks(
                MessageRole::Assistant,
                vec![TranscriptBlock::ToolUse {
                    id: "lsp-1".to_string(),
                    name: "LSP".to_string(),
                    input: "{\"operation\":\"goToDefinition\",\"path\":\"/Users/user/github/sample-workspace-main/crates/render-fixtures/src/main.rs\",\"symbol\":\"main\"}".to_string(),
                }],
            ),
            TranscriptMessage::from_blocks(
                MessageRole::User,
                vec![TranscriptBlock::ToolResult {
                    tool_use_id: "lsp-1".to_string(),
                    content: String::new().into(),
                    is_error: false,
                    metadata: Some(
                        "{\"status\":\"completed\",\"summary\":\"Found 1 definition for `main`.\"}"
                            .to_string(),
                    ),
                }],
            ),
            TranscriptMessage::from_blocks(
                MessageRole::Assistant,
                vec![TranscriptBlock::ToolUse {
                    id: "mcp-1".to_string(),
                    name: "CallMcpTool".to_string(),
                    input: "{\"server_id\":\"docs\",\"tool_name\":\"inspect\"}".to_string(),
                }],
            ),
            TranscriptMessage::from_blocks(
                MessageRole::User,
                vec![TranscriptBlock::ToolResult {
                    tool_use_id: "mcp-1".to_string(),
                    content: String::new().into(),
                    is_error: false,
                    metadata: Some(
                        "{\"status\":\"completed\",\"summary\":\"Invoked MCP tool `inspect` on `docs`.\"}"
                            .to_string(),
                    ),
                }],
            ),
        ];
    state
}

pub fn transcript_typescript_terminal_excerpt_fixture_state() -> TuiState {
    let mut state = normal_state("", 0);
    state.cwd = PathBuf::from("/Users/user/github/sample-workspace-main/crates/render-fixtures");
    state.cwd_display = "~/github/sample-workspace-main/crates/render-fixtures".to_string();
    state.ui_version = "2.1.119".to_string();
    state.model_display_name = "glm-4.7".to_string();
    state.editor_mode = EditorMode::Insert;

    let sample_root = "/Users/user/github/sample-workspace-main";
    let cwd_reset =
        "Shell cwd was reset to /Users/user/github/sample-workspace-main/crates/render-fixtures";
    let head_lines = std::iter::once("src/app.ts".to_string())
        .chain(std::iter::once("src/index.tsx".to_string()))
        .chain(std::iter::once("src/Widget.ts".to_string()))
        .chain((0..61).map(|index| format!("src/generated/file-{index}.ts")))
        .collect::<Vec<_>>()
        .join("\n");

    state.messages = vec![
            TranscriptMessage::from_blocks(
                MessageRole::User,
                vec![TranscriptBlock::Text {
                    text: "梳理一下这个仓库的目录结构，并统计一下源文件数量"
                        .to_string(),
                }],
            ),
            TranscriptMessage::from_blocks(
                MessageRole::Assistant,
                vec![TranscriptBlock::Text {
                    text: "我来先看一下仓库的目录结构，然后统计源文件数量。".to_string(),
                }],
            ),
            TranscriptMessage::from_blocks(
                MessageRole::Assistant,
                vec![
                    TranscriptBlock::ToolUse {
                        id: "glob-1".to_string(),
                        name: "Glob".to_string(),
                        input: "{\"pattern\":\"**/Cargo.toml\"}".to_string(),
                    },
                    TranscriptBlock::ToolUse {
                        id: "list-1".to_string(),
                        name: "Bash".to_string(),
                        input: "{\"command\":\"ls -la\"}".to_string(),
                    },
                ],
            ),
            TranscriptMessage::from_blocks(
                MessageRole::User,
                vec![
                    TranscriptBlock::ToolResult {
                        tool_use_id: "glob-1".to_string(),
                        content: "crates/render-fixtures/Cargo.toml".to_string().into(),
                        is_error: false,
                        metadata: None,
                    },
                    TranscriptBlock::ToolResult {
                        tool_use_id: "list-1".to_string(),
                        content: "total 4".to_string().into(),
                        is_error: false,
                        metadata: None,
                    },
                ],
            ),
            TranscriptMessage::from_blocks(
                MessageRole::Assistant,
                vec![TranscriptBlock::ToolUse {
                    id: "bash-pwd".to_string(),
                    name: "Bash".to_string(),
                    input: "{\"command\":\"pwd\"}".to_string(),
                }],
            ),
            TranscriptMessage::from_blocks(
                MessageRole::User,
                vec![TranscriptBlock::ToolResult {
                    tool_use_id: "bash-pwd".to_string(),
                    content: format!("{sample_root}/crates/render-fixtures").into(),
                    is_error: false,
                    metadata: None,
                }],
            ),
            TranscriptMessage::from_blocks(
                MessageRole::Assistant,
                vec![
                    TranscriptBlock::ToolUse {
                        id: "search-1".to_string(),
                        name: "Glob".to_string(),
                        input: "{\"pattern\":\"src/**/*\"}".to_string(),
                    },
                    TranscriptBlock::ToolUse {
                        id: "search-2".to_string(),
                        name: "Grep".to_string(),
                        input: format!("{{\"pattern\":\"use .*;\",\"path\":\"{sample_root}/src\"}}"),
                    },
                    TranscriptBlock::ToolUse {
                        id: "read-1".to_string(),
                        name: "Read".to_string(),
                        input: format!("{{\"file_path\":\"{sample_root}/src/app.ts\"}}"),
                    },
                    TranscriptBlock::ToolUse {
                        id: "read-2".to_string(),
                        name: "Read".to_string(),
                        input: format!("{{\"file_path\":\"{sample_root}/src/index.tsx\"}}"),
                    },
                    TranscriptBlock::ToolUse {
                        id: "read-3".to_string(),
                        name: "Read".to_string(),
                        input: format!("{{\"file_path\":\"{sample_root}/src/Widget.ts\"}}"),
                    },
                    TranscriptBlock::ToolUse {
                        id: "read-4".to_string(),
                        name: "Read".to_string(),
                        input: format!("{{\"file_path\":\"{sample_root}/src/router.ts\"}}"),
                    },
                    TranscriptBlock::ToolUse {
                        id: "read-5".to_string(),
                        name: "Read".to_string(),
                        input: format!("{{\"file_path\":\"{sample_root}/src/store.ts\"}}"),
                    },
                    TranscriptBlock::ToolUse {
                        id: "read-6".to_string(),
                        name: "Read".to_string(),
                        input: format!("{{\"file_path\":\"{sample_root}/src/App.tsx\"}}"),
                    },
                    TranscriptBlock::ToolUse {
                        id: "read-7".to_string(),
                        name: "Read".to_string(),
                        input: format!("{{\"file_path\":\"{sample_root}/src/theme.ts\"}}"),
                    },
                    TranscriptBlock::ToolUse {
                        id: "read-8".to_string(),
                        name: "Read".to_string(),
                        input: format!("{{\"file_path\":\"{sample_root}/src/server.ts\"}}"),
                    },
                    TranscriptBlock::ToolUse {
                        id: "list-2".to_string(),
                        name: "Bash".to_string(),
                        input: "{\"command\":\"ls -la src/components/ | head -70\"}"
                            .to_string(),
                    },
                ],
            ),
            TranscriptMessage::from_blocks(
                MessageRole::User,
                vec![
                    TranscriptBlock::ToolResult {
                        tool_use_id: "search-1".to_string(),
                        content: "src".to_string().into(),
                        is_error: false,
                        metadata: None,
                    },
                    TranscriptBlock::ToolResult {
                        tool_use_id: "search-2".to_string(),
                        content: "src/index.tsx:1:import { renderApp } from \"./app\"".to_string().into(),
                        is_error: false,
                        metadata: None,
                    },
                    TranscriptBlock::ToolResult {
                        tool_use_id: "read-1".to_string(),
                        content: "120 lines".to_string().into(),
                        is_error: false,
                        metadata: None,
                    },
                    TranscriptBlock::ToolResult {
                        tool_use_id: "read-2".to_string(),
                        content: "90 lines".to_string().into(),
                        is_error: false,
                        metadata: None,
                    },
                    TranscriptBlock::ToolResult {
                        tool_use_id: "read-3".to_string(),
                        content: "80 lines".to_string().into(),
                        is_error: false,
                        metadata: None,
                    },
                    TranscriptBlock::ToolResult {
                        tool_use_id: "read-4".to_string(),
                        content: "140 lines".to_string().into(),
                        is_error: false,
                        metadata: None,
                    },
                    TranscriptBlock::ToolResult {
                        tool_use_id: "read-5".to_string(),
                        content: "75 lines".to_string().into(),
                        is_error: false,
                        metadata: None,
                    },
                    TranscriptBlock::ToolResult {
                        tool_use_id: "read-6".to_string(),
                        content: "30 lines".to_string().into(),
                        is_error: false,
                        metadata: None,
                    },
                    TranscriptBlock::ToolResult {
                        tool_use_id: "read-7".to_string(),
                        content: "55 lines".to_string().into(),
                        is_error: false,
                        metadata: None,
                    },
                    TranscriptBlock::ToolResult {
                        tool_use_id: "read-8".to_string(),
                        content: "65 lines".to_string().into(),
                        is_error: false,
                        metadata: None,
                    },
                    TranscriptBlock::ToolResult {
                        tool_use_id: "list-2".to_string(),
                        content: "src/components".to_string().into(),
                        is_error: false,
                        metadata: None,
                    },
                ],
            ),
            TranscriptMessage::from_blocks(
                MessageRole::Assistant,
                vec![TranscriptBlock::ToolUse {
                    id: "bash-wc".to_string(),
                    name: "Bash".to_string(),
                    input:
                        "{\"command\":\"cd .. && find src -name \\\"*.ts\\\" -o -name \\\"*.tsx\\\" | wc -l\"}"
                            .to_string(),
                }],
            ),
            TranscriptMessage::from_blocks(
                MessageRole::User,
                vec![TranscriptBlock::ToolResult {
                    tool_use_id: "bash-wc".to_string(),
                    content: format!("2799\n{cwd_reset}").into(),
                    is_error: false,
                    metadata: None,
                }],
            ),
            TranscriptMessage::from_blocks(
                MessageRole::Assistant,
                vec![TranscriptBlock::ToolUse {
                    id: "bash-head".to_string(),
                    name: "Bash".to_string(),
                    input:
                        "{\"command\":\"cd .. && find src -name \\\"*.ts\\\" -o -name \\\"*.tsx\\\" | head -100\"}"
                            .to_string(),
                }],
            ),
            TranscriptMessage::from_blocks(
                MessageRole::User,
                vec![TranscriptBlock::ToolResult {
                    tool_use_id: "bash-head".to_string(),
                    content: format!("{head_lines}\n{cwd_reset}").into(),
                    is_error: false,
                    metadata: None,
                }],
            ),
        ];
    state
}

pub fn live_progress_metric_state(progress_messages: Vec<Value>) -> TuiState {
    let mut state = normal_state("", 0);
    state.request_in_flight = true;
    state.upsert_live_tool_activity(LiveToolActivity {
        request_id: Some("request-1".to_string()),
        tool_use_id: "agent-1".to_string(),
        tool_name: "Agent".to_string(),
        tool_input: serde_json::json!({
            "description": "Explore renderer",
            "prompt": "Inspect render performance paths",
            "subagent_type": "explorer"
        })
        .to_string(),
        status_line: "Running Agent".to_string(),
        progress_messages,
        is_error: false,
    });
    state.in_progress_tool_use_ids.insert("agent-1".to_string());
    state
}

pub fn permission_render_metrics_state() -> TuiState {
    let mut state = normal_state("", 0);
    fill_long_transcript(&mut state, 48);
    state.overlay = Some(OverlayState::PermissionRequest(
        PermissionOverlayState::new(long_agent_permission_request()),
    ));
    state
}

pub fn large_workspace_diff() -> WorkspaceDiff {
    let mut status = Vec::new();
    let mut unstaged_diff = String::new();
    for file_index in 0..8 {
        let path = format!("src/render_fixture_{file_index}.rs");
        status.push(format!(" M {path}"));
        write!(
            unstaged_diff,
            "diff --git a/{path} b/{path}\n\
                 index 1111111..2222222 100644\n\
                 --- a/{path}\n\
                 +++ b/{path}\n\
                 @@ -1,60 +1,60 @@\n"
        )
        .expect("writing to String cannot fail");
        for line_index in 0..60 {
            write!(
                unstaged_diff,
                "-let old_value_{line_index:02} = {line_index};\n\
                     +let new_value_{line_index:02} = {line_index};\n"
            )
            .expect("writing to String cannot fail");
        }
    }

    WorkspaceDiff {
        cwd: PathBuf::from("/tmp/render-metrics"),
        status: status.join("\n"),
        staged_diff: String::new(),
        unstaged_diff,
        untracked_files: Vec::new(),
    }
}

pub fn state_with_status_overlay(overlay: OverlayState) -> TuiState {
    let mut state = normal_state("", 0);
    fill_long_transcript(&mut state, 80);
    state.overlay = Some(overlay);
    state
}

pub fn synthetic_model_options(count: usize, current: usize) -> Vec<ModelOption> {
    (0..count)
        .map(|index| ModelOption {
            value: Some(format!("model-{index}")),
            label: format!("Model {index:03}"),
            description: format!("Synthetic model option {index} for render metrics fixture"),
            current: index == current,
        })
        .collect()
}

pub fn synthetic_output_style_options(count: usize, current: usize) -> Vec<OutputStyleOption> {
    (0..count)
        .map(|index| OutputStyleOption {
            value: format!("style-{index}"),
            label: format!("Style {index:03}"),
            description: format!("Synthetic output style {index} for render metrics fixture"),
            current: index == current,
        })
        .collect()
}

pub fn synthetic_config_options(count: usize) -> Vec<ConfigOption> {
    (0..count)
        .map(|index| ConfigOption {
            label: format!("Synthetic setting {index:03}"),
            value: format!("value-{index}"),
            description: format!("Synthetic config option {index} for render metrics fixture"),
            current: index % 19 == 0,
            action: ConfigAction::Readonly,
        })
        .collect()
}

pub fn large_sandbox_settings() -> SandboxLocalSettings {
    SandboxLocalSettings {
        enabled: true,
        excluded_commands: (0..80)
            .map(|index| format!("synthetic-command-{index}"))
            .collect(),
        filesystem: SandboxFilesystemLocalSettings {
            allow_write: (0..48)
                .map(|index| format!("/tmp/allow-write-{index}"))
                .collect(),
            ..Default::default()
        },
        network: SandboxNetworkLocalSettings {
            allowed_domains: (0..48)
                .map(|index| format!("example-{index}.test"))
                .collect(),
            ..Default::default()
        },
        ..SandboxLocalSettings::default()
    }
}

pub fn large_memory_overview(cwd: &Path) -> MemoryOverview {
    MemoryOverview {
        user_memory: MemoryFileOverview {
            label: "User memory".to_string(),
            path: PathBuf::from("/tmp/user/CLAUDE.md"),
            exists: true,
            content: None,
            status: orbcode_protocol::MemorySourceStatus::Empty,
            writable: true,
            trust_boundary: None,
            scope: None,
            skipped_reason: None,
        },
        project_memories: (0..90)
            .map(|index| MemoryFileOverview {
                label: format!("Project memory {index:03}"),
                path: cwd.join(format!("nested/{index}/CLAUDE.md")),
                exists: true,
                content: None,
                status: orbcode_protocol::MemorySourceStatus::Empty,
                writable: true,
                trust_boundary: None,
                scope: None,
                skipped_reason: None,
            })
            .collect(),
        auto_memory_enabled: true,
        auto_memory_dir: PathBuf::from("/tmp/auto-memory"),
    }
}
