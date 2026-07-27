use crate::tests::support::*;

#[test]
fn permission_panel_renders_file_write_with_rust_highlighting() {
    let permission = PermissionOverlayState::new(PermissionRequest {
        request_id: "req-1".to_string(),
        session_id: "session".to_string(),
        tool_use_id: "tool-1".to_string(),
        tool_name: "Write".to_string(),
        tool_input: serde_json::json!({
            "file_path": "/tmp/a/hello.rs",
            "content": "fn main() {\n    println!(\"Hello, world!\");\n}\n"
        })
        .to_string(),
        requires_tools_permission: true,
        requires_network_permission: false,
    });

    let panel = permission_panel_content(&permission, 100);
    let rendered = plain_text_lines(&panel.body);

    assert_eq!(rendered.first().map(String::as_str), Some("Create file"));
    assert!(
        rendered
            .get(1)
            .is_some_and(|line| line.chars().all(|ch| ch == '─'))
    );
    assert!(rendered.iter().any(|line| line.contains("hello.rs +3 -0")));
    assert!(
        rendered
            .iter()
            .any(|line| line.contains("Do you want to edit /tmp/a/hello.rs?"))
    );
    assert!(
        rendered
            .iter()
            .any(|line| line.contains("Yes, and don't ask again for: file operations in /tmp/a"))
    );
    let code_line = panel
        .body
        .iter()
        .find(|line| {
            line.spans
                .iter()
                .map(|span| span.content.as_ref())
                .collect::<String>()
                .contains("fn main")
        })
        .expect("expected rendered Rust code line");
    assert!(
        code_line
            .spans
            .iter()
            .any(|span| span.content.as_ref() == "fn" && span.style.fg.is_some())
    );
}

#[test]
fn permission_panel_renders_read_as_file_path_not_json() {
    let permission = PermissionOverlayState::new(PermissionRequest {
        request_id: "req-1".to_string(),
        session_id: "session".to_string(),
        tool_use_id: "tool-1".to_string(),
        tool_name: "Read".to_string(),
        tool_input: serde_json::json!({
            "file_path": "/tmp/a/Cargo.toml"
        })
        .to_string(),
        requires_tools_permission: true,
        requires_network_permission: false,
    });

    let panel = permission_panel_content(&permission, 100);
    let rendered = plain_text_lines(&panel.body);

    assert_eq!(rendered.first().map(String::as_str), Some("Read file"));
    assert!(
        rendered
            .get(1)
            .is_some_and(|line| line.chars().all(|ch| ch == '─'))
    );
    assert!(
        rendered
            .iter()
            .any(|line| line.contains("File /tmp/a/Cargo.toml"))
    );
    assert!(
        rendered
            .iter()
            .any(|line| line.contains("Do you want to read this file?"))
    );
    assert!(!rendered.iter().any(|line| line.contains("Path ")));
    assert!(
        !rendered
            .iter()
            .any(|line| line.contains("Do you want to read /tmp/a/Cargo.toml?"))
    );
    assert!(!rendered.iter().any(|line| line == "Input"));
    assert!(!rendered.iter().any(|line| line.contains("file_path")));
}

#[test]
fn permission_panel_renders_glob_as_human_readable_search() {
    let permission = PermissionOverlayState::new(PermissionRequest {
        request_id: "req-1".to_string(),
        session_id: "session".to_string(),
        tool_use_id: "tool-1".to_string(),
        tool_name: "Glob".to_string(),
        tool_input: serde_json::json!({
            "pattern": "**/Cargo.toml"
        })
        .to_string(),
        requires_tools_permission: true,
        requires_network_permission: false,
    });

    let panel = permission_panel_content(&permission, 100);
    let rendered = plain_text_lines(&panel.body);

    assert_eq!(rendered.first().map(String::as_str), Some("Search files"));
    assert!(rendered.iter().any(|line| line.contains("Tool Glob")));
    assert!(
        rendered
            .iter()
            .any(|line| line.contains("Pattern **/Cargo.toml"))
    );
    assert!(
        rendered
            .iter()
            .any(|line| line.contains("Do you want to search files?"))
    );
    assert!(!rendered.iter().any(|line| line == "Input"));
    assert!(!rendered.iter().any(|line| line == "{"));
    assert!(!rendered.iter().any(|line| line == "}"));
    assert!(!rendered.iter().any(|line| line.contains("\"pattern\"")));
}

#[test]
fn permission_panel_renders_grep_as_human_readable_search() {
    let permission = PermissionOverlayState::new(PermissionRequest {
        request_id: "req-1".to_string(),
        session_id: "session".to_string(),
        tool_use_id: "tool-1".to_string(),
        tool_name: "Grep".to_string(),
        tool_input: serde_json::json!({
            "pattern": r"ToolSpec\s*\{",
            "path": "orbcode/tools/src",
            "output_mode": "content"
        })
        .to_string(),
        requires_tools_permission: true,
        requires_network_permission: false,
    });

    let panel = permission_panel_content(&permission, 100);
    let rendered = plain_text_lines(&panel.body);

    assert_eq!(rendered.first().map(String::as_str), Some("Search content"));
    assert!(
        rendered
            .iter()
            .any(|line| line.contains(r"Regex ToolSpec\s*\{"))
    );
    assert!(
        rendered
            .iter()
            .any(|line| line.contains("Search in orbcode/tools/src"))
    );
    assert!(
        rendered
            .iter()
            .any(|line| line.contains("Show matching lines"))
    );
    assert!(
        rendered
            .iter()
            .any(|line| line.contains("Do you want to search file contents in orbcode/tools/src?"))
    );
    assert!(rendered.iter().any(|line| {
        line.contains("Yes, and don't ask again for: searches under orbcode/tools/src")
    }));
    assert!(!rendered.iter().any(|line| line.contains("Tool Grep")));
    assert!(
        !rendered
            .iter()
            .any(|line| line.contains("Output Mode content"))
    );
}

#[test]
fn permission_panel_renders_agent_as_delegation_preview() {
    let permission = PermissionOverlayState::new(PermissionRequest {
            request_id: "req-1".to_string(),
            session_id: "session".to_string(),
            tool_use_id: "tool-1".to_string(),
            tool_name: "Agent".to_string(),
            tool_input: serde_json::json!({
                "description": "检查 permission panel 实现",
                "prompt": "请详细检查 orbcode/tui/src/lib.rs 文件中的 permission panel 实现，包括：\n1. Permission panel 的数据结构定义\n2. 渲染逻辑和 UI 组件",
                "subagent_type": "Explore"
            })
            .to_string(),
            requires_tools_permission: true,
            requires_network_permission: false,
        });

    let panel = permission_panel_content(&permission, 100);
    let rendered = plain_text_lines(&panel.body);

    assert_eq!(rendered.first().map(String::as_str), Some("Run agent"));
    assert!(rendered.iter().any(|line| line.contains("Agent Explore")));
    assert!(
        rendered
            .iter()
            .any(|line| line.contains("Task 检查 permission panel 实现"))
    );
    assert!(rendered.iter().any(|line| line.contains("Prompt:")));
    assert!(
        rendered
            .iter()
            .any(|line| line.contains("  请详细检查 orbcode/tui/src/lib.rs"))
    );
    assert!(
        rendered
            .iter()
            .any(|line| line.contains("  1. Permission panel 的数据结构定义"))
    );
    assert!(
        !rendered
            .iter()
            .any(|line| line.contains("  - 1. Permission panel")),
        "{rendered:#?}"
    );
    assert!(
        rendered
            .iter()
            .any(|line| line.contains("Do you want to run this subagent?"))
    );
    assert!(!rendered.iter().any(|line| line.contains("subagent_type")));
}

#[test]
fn permission_panel_renders_workflow_as_generated_plan_preview() {
    let permission = PermissionOverlayState::new(PermissionRequest {
        request_id: "req-1".to_string(),
        session_id: "session".to_string(),
        tool_use_id: "tool-1".to_string(),
        tool_name: "Workflow".to_string(),
        tool_input: serde_json::json!({
            "name": "dynamic:provider",
            "arguments": "ok",
            "spec": {
                "schema_version": 1,
                "description": "Provider multi-step workflow check",
                "steps": [
                    {
                        "phase": {
                            "name": "Phase ok",
                            "steps": [
                                { "parallel": { "steps": [
                                    { "log": { "message": "parallel ok" } },
                                    { "pipeline": { "steps": [
                                        { "log": { "message": "pipe-one ok" } },
                                        { "log": { "message": "pipe-two ok" } }
                                    ] } }
                                ] } }
                            ]
                        }
                    },
                    { "log": { "message": "done ok" } }
                ]
            }
        })
        .to_string(),
        requires_tools_permission: true,
        requires_network_permission: false,
    });

    let panel = permission_panel_content(&permission, 100);
    let rendered = plain_text_lines(&panel.body);

    assert_eq!(rendered.first().map(String::as_str), Some("Start workflow"));
    assert!(
        rendered
            .iter()
            .any(|line| line.contains("Workflow dynamic:provider"))
    );
    assert!(rendered.iter().any(|line| line.contains("Arguments ok")));
    assert!(
        rendered
            .iter()
            .any(|line| line.contains("Provider multi-step workflow check"))
    );
    assert!(
        rendered
            .iter()
            .any(|line| line.contains("Top-level steps 2 step(s)"))
    );
    assert!(
        rendered
            .iter()
            .any(|line| line.contains("1. phase Phase ok"))
    );
    assert!(rendered.iter().any(|line| line.contains("2. log done ok")));
    assert!(
        rendered
            .iter()
            .any(|line| line.contains("Do you want to start this workflow?"))
    );
    assert!(
        rendered
            .iter()
            .any(|line| line.contains("Yes, and don't ask again for: Workflow"))
    );
}

#[test]
fn permission_panel_indents_wrapped_prompt_lines() {
    let permission = PermissionOverlayState::new(PermissionRequest {
            request_id: "req-1".to_string(),
            session_id: "session".to_string(),
            tool_use_id: "tool-1".to_string(),
            tool_name: "Agent".to_string(),
            tool_input: serde_json::json!({
                "description": "Explore permission panel implementation",
                "prompt": "Please examine the permission panel implementation in orbcode/tui/src/lib.rs.\nFocus on:\n1. How the permission panel is structured and rendered\n2. What permission types are supported\n3. How user input/approval is handled\n4. The overall architecture of the permission system in the TUI\nProvide a detailed analysis of the permission panel code, including key functions, data structures, and the flow of permission requests.",
                "subagent_type": "Explore"
            })
            .to_string(),
            requires_tools_permission: true,
            requires_network_permission: false,
        });

    let panel = permission_panel_content(&permission, 80);
    let rendered = plain_text_lines(&panel.body);
    let prompt_start = rendered
        .iter()
        .position(|line| line == "Prompt:")
        .expect("prompt heading")
        + 1;
    let question_start = rendered
        .iter()
        .position(|line| line.contains("Do you want to run this subagent?"))
        .expect("permission question");

    for line in rendered[prompt_start..question_start]
        .iter()
        .filter(|line| !line.is_empty())
    {
        assert!(line.starts_with("  "), "{rendered:#?}");
    }
}

#[test]
fn permission_panel_renders_todo_as_item_preview() {
    let permission = PermissionOverlayState::new(PermissionRequest {
        request_id: "req-1".to_string(),
        session_id: "session".to_string(),
        tool_use_id: "tool-1".to_string(),
        tool_name: "TodoWrite".to_string(),
        tool_input: serde_json::json!({
            "list": "release",
            "mode": "replace",
            "items": [
                {"title": "Update plans", "done": true},
                {"title": "Run tests", "done": false}
            ]
        })
        .to_string(),
        requires_tools_permission: true,
        requires_network_permission: false,
    });

    let panel = permission_panel_content(&permission, 100);
    let rendered = plain_text_lines(&panel.body);

    assert_eq!(
        rendered.first().map(String::as_str),
        Some("Update todo list")
    );
    assert!(rendered.iter().any(|line| line.contains("List release")));
    assert!(
        rendered
            .iter()
            .any(|line| line.contains("Mode replace list"))
    );
    assert!(rendered.iter().any(|line| line.contains("Items 2 item(s)")));
    assert!(
        rendered
            .iter()
            .any(|line| line.contains("Update plans (done)"))
    );
    assert!(
        rendered
            .iter()
            .any(|line| line.contains("Run tests (todo)"))
    );
    assert!(
        rendered
            .iter()
            .any(|line| line.contains("Do you want to update the todo list?"))
    );
}

#[test]
fn permission_panel_renders_task_update_as_action_preview() {
    let permission = PermissionOverlayState::new(PermissionRequest {
        request_id: "req-1".to_string(),
        session_id: "session".to_string(),
        tool_use_id: "tool-1".to_string(),
        tool_name: "TaskUpdate".to_string(),
        tool_input: serde_json::json!({
            "taskId": "task-7",
            "status": "in_progress",
            "subject": "Permission preview polish",
            "addBlockedBy": ["task-2", "task-3"]
        })
        .to_string(),
        requires_tools_permission: true,
        requires_network_permission: false,
    });

    let panel = permission_panel_content(&permission, 100);
    let rendered = plain_text_lines(&panel.body);

    assert_eq!(rendered.first().map(String::as_str), Some("Update task"));
    assert!(
        rendered
            .iter()
            .any(|line| line.contains("Action Task update"))
    );
    assert!(rendered.iter().any(|line| line.contains("Task task-7")));
    assert!(
        rendered
            .iter()
            .any(|line| line.contains("Status in_progress"))
    );
    assert!(
        rendered
            .iter()
            .any(|line| line.contains("Blocked by 2 task(s)"))
    );
    assert!(
        rendered
            .iter()
            .any(|line| line.contains("Do you want to update this task?"))
    );
    assert!(!rendered.iter().any(|line| line.contains("addBlockedBy")));
}

#[test]
fn permission_panel_renders_exit_plan_as_plan_preview() {
    let permission = PermissionOverlayState::new(PermissionRequest {
        request_id: "req-1".to_string(),
        session_id: "session".to_string(),
        tool_use_id: "tool-1".to_string(),
        tool_name: "ExitPlanMode".to_string(),
        tool_input: serde_json::json!({
            "plan": "1. Update permission previews\n2. Run focused tests",
            "allowedPrompts": [
                {"tool": "Edit", "prompt": "patch orbcode/tui/src/lib.rs"},
                {"tool": "Bash", "prompt": "cargo test -p orbcode-tui permission_panel"}
            ]
        })
        .to_string(),
        requires_tools_permission: true,
        requires_network_permission: false,
    });

    let panel = permission_panel_content(&permission, 100);
    let rendered = plain_text_lines(&panel.body);

    assert_eq!(rendered.first().map(String::as_str), Some("Exit plan mode"));
    assert!(rendered.iter().any(|line| line.contains("Plan:")));
    assert!(
        rendered
            .iter()
            .any(|line| line.contains("Update permission previews"))
    );
    assert!(
        rendered
            .iter()
            .any(|line| line.contains("Allowed prompts 2 item(s)"))
    );
    assert!(
        rendered
            .iter()
            .any(|line| line.contains("Bash cargo test -p orbcode-tui permission_panel"))
    );
    assert!(
        rendered
            .iter()
            .any(|line| line.contains("Do you want to accept this plan?"))
    );
    assert!(!rendered.iter().any(|line| line.contains("allowedPrompts")));
}

#[test]
fn permission_panel_fallbacks_render_all_known_tools_without_raw_json() {
    let samples = vec![
        (
            "Glob",
            serde_json::json!({"pattern": "**/*.rs", "path": "src"}),
        ),
        (
            "Grep",
            serde_json::json!({"pattern": "PermissionRequest", "path": "orbcode", "glob": "**/*.rs", "output_mode": "content", "-n": true}),
        ),
        (
            "NotebookEdit",
            serde_json::json!({"path": "notes.ipynb", "cell_type": "code", "source": "print('hi')"}),
        ),
        (
            "WebFetch",
            serde_json::json!({"url": "https://example.com"}),
        ),
        (
            "WebSearch",
            serde_json::json!({"query": "rust ratatui permission ui"}),
        ),
        (
            "AskUserQuestion",
            serde_json::json!({"question": "Pick one", "options": ["A", "B"]}),
        ),
        (
            "TodoWrite",
            serde_json::json!({"list": "default", "mode": "replace", "items": [{"title": "Ship permission UI", "done": false}]}),
        ),
        (
            "TaskCreate",
            serde_json::json!({"subject": "Investigate", "description": "Check the Rust implementation", "owner": "agent"}),
        ),
        ("TaskGet", serde_json::json!({"taskId": "task-1"})),
        ("TaskList", serde_json::json!({})),
        (
            "TaskUpdate",
            serde_json::json!({"taskId": "task-1", "status": "in_progress", "addBlocks": ["Look at tui"]}),
        ),
        (
            "TaskOutput",
            serde_json::json!({"task_id": "task-1", "block": false, "timeout": 1000}),
        ),
        ("TaskStop", serde_json::json!({"task_id": "task-1"})),
        ("EnterPlanMode", serde_json::json!({})),
        (
            "ExitPlanMode",
            serde_json::json!({"plan": "Do the work", "allowedPrompts": [{"tool": "Bash", "prompt": "cargo test"}]}),
        ),
        ("VerifyPlanExecution", serde_json::json!({})),
        (
            "Skill",
            serde_json::json!({"skill": "teach-me", "args": "rust"}),
        ),
        (
            "ToolSearch",
            serde_json::json!({"query": "browser", "max_results": 3}),
        ),
        (
            "LSP",
            serde_json::json!({"operation": "goToDefinition", "filePath": "src/lib.rs", "line": 10, "character": 4}),
        ),
        (
            "Agent",
            serde_json::json!({"description": "Explore", "prompt": "Inspect the repo", "subagent_type": "explorer"}),
        ),
    ];

    for (tool_name, input) in samples {
        let permission = PermissionOverlayState::new(PermissionRequest {
            request_id: "req-1".to_string(),
            session_id: "session".to_string(),
            tool_use_id: "tool-1".to_string(),
            tool_name: tool_name.to_string(),
            tool_input: input.to_string(),
            requires_tools_permission: true,
            requires_network_permission: false,
        });
        let panel = permission_panel_content(&permission, 100);
        let rendered = plain_text_lines(&panel.body);

        assert!(
            !rendered.iter().any(|line| line.trim() == "{"
                || line.trim() == "}"
                || line.trim() == "["
                || line.trim() == "]"),
            "{tool_name} rendered raw JSON delimiters: {rendered:#?}"
        );
        assert!(
            !rendered.iter().any(|line| line.contains("\":")),
            "{tool_name} rendered raw JSON key syntax: {rendered:#?}"
        );
        assert!(
            !rendered.iter().any(|line| line == "Input"),
            "{tool_name} rendered generic JSON input heading: {rendered:#?}"
        );
    }
}

#[test]
fn permission_panel_bottom_pins_options_and_scrolls_to_details() {
    let content = (1..=20)
        .map(|line| format!("println!(\"line {line}\");"))
        .collect::<Vec<_>>()
        .join("\n");
    let permission = PermissionOverlayState::new(PermissionRequest {
        request_id: "req-1".to_string(),
        session_id: "session".to_string(),
        tool_use_id: "tool-1".to_string(),
        tool_name: "Write".to_string(),
        tool_input: serde_json::json!({
            "file_path": "/tmp/a/test.rs",
            "content": content
        })
        .to_string(),
        requires_tools_permission: true,
        requires_network_permission: false,
    });

    let panel = permission_panel_content(&permission, 80);
    let all_lines = plain_text_lines(&panel.body);

    assert!(all_lines.iter().any(|line| line.contains("20 +")));
    assert!(!all_lines.iter().any(|line| line.contains("truncated")));
    assert!(!all_lines.iter().any(|line| line.contains("more lines")));

    let bottom = permission_panel_viewport(&panel.body, 80, 8, 0);
    let bottom_lines = plain_text_lines(&bottom.body);
    assert!(bottom_lines.iter().any(|line| line.contains("1. Yes")));
    assert!(bottom_lines.iter().any(|line| line.contains("No")));
    assert!(bottom_lines.iter().any(|line| line.contains("PgUp/PgDn")));
    assert!(!bottom_lines.iter().any(|line| line.contains("Create file")));

    let top = permission_panel_viewport(&panel.body, 80, 8, usize::MAX / 2);
    let top_lines = plain_text_lines(&top.body);
    assert!(top_lines.iter().any(|line| line.contains("Create file")));
}

#[test]
fn permission_panel_content_cache_reuses_body_across_scroll_and_selection() {
    let mut permission = PermissionOverlayState::new(long_agent_permission_request());
    let first_body_len = {
        let cached = permission.cached_panel_content(80);
        cached.content.body.len()
    };

    permission.panel_scroll = 10;
    permission.viewport.selection = Some(TranscriptSelectionState {
        area: Rect::new(0, 0, 80, 20),
        anchor: TranscriptSelectionPoint { row: 2, column: 0 },
        focus: TranscriptSelectionPoint { row: 5, column: 8 },
    });
    let second_body_len = {
        let cached = permission.cached_panel_content(80);
        cached.content.body.len()
    };

    assert_eq!(first_body_len, second_body_len);
    assert_eq!(permission.content_cache.misses, 1);
    assert_eq!(permission.content_cache.hits, 1);

    permission.selected_option = 1;
    let _ = permission.cached_panel_content(80);

    assert_eq!(permission.content_cache.misses, 2);
    assert_eq!(permission.content_cache.hits, 1);
}

#[test]
#[ignore = "manual stress test for permission panel content caching"]
fn permission_panel_content_cache_stress_reuses_long_prompt_body() {
    const FRAME_COUNT: usize = 1_000;
    const INNER_WIDTH: usize = 98;
    const INNER_HEIGHT: u16 = 20;

    let mut permission = PermissionOverlayState::new(long_agent_permission_request());
    let started = Instant::now();
    let mut last_body_len = 0;
    let mut last_wrapped_len = 0;
    let mut last_visible_len = 0;
    for frame in 0..FRAME_COUNT {
        permission.panel_scroll = frame % 40;
        if frame % 2 == 0 {
            permission.viewport.selection = Some(TranscriptSelectionState {
                area: Rect::new(0, 0, INNER_WIDTH as u16, INNER_HEIGHT),
                anchor: TranscriptSelectionPoint { row: 2, column: 0 },
                focus: TranscriptSelectionPoint { row: 10, column: 8 },
            });
        } else {
            permission.viewport.clear_selection();
        }

        let panel_scroll = permission.panel_scroll;
        let (body_len, wrapped_len, viewport) = {
            let cached = permission.cached_panel_content(INNER_WIDTH);
            (
                cached.content.body.len(),
                cached.wrapped_body.len(),
                permission_panel_viewport_from_wrapped(
                    cached.wrapped_body,
                    INNER_HEIGHT,
                    panel_scroll,
                ),
            )
        };
        assert!(viewport.body.len() <= INNER_HEIGHT as usize);
        last_body_len = body_len;
        last_wrapped_len = wrapped_len;
        last_visible_len = viewport.body.len();
    }
    let duration = started.elapsed();

    assert_eq!(permission.content_cache.misses, 1);
    assert_eq!(permission.content_cache.hits, (FRAME_COUNT - 1) as u64);
    eprintln!(
        "frames={FRAME_COUNT} body_lines={last_body_len} wrapped_lines={last_wrapped_len} visible_lines={last_visible_len} cache_hits={} cache_misses={} loop_us={}",
        permission.content_cache.hits,
        permission.content_cache.misses,
        duration.as_micros()
    );
}

#[test]
fn permission_panel_height_shows_full_content_when_space_allows() {
    let permission = PermissionOverlayState::new(PermissionRequest {
            request_id: "req-1".to_string(),
            session_id: "session".to_string(),
            tool_use_id: "tool-1".to_string(),
            tool_name: "Agent".to_string(),
            tool_input: serde_json::json!({
                "description": "检查 permission panel 实现",
                "prompt": "请详细检查 orbcode/tui/src/lib.rs 文件中的 permission panel 实现，包括：\n1. Permission panel 的数据结构定义\n2. 渲染逻辑和 UI 组件",
                "subagent_type": "Explore"
            })
            .to_string(),
            requires_tools_permission: true,
            requires_network_permission: false,
        });

    let body = permission_panel_content(&permission, 138);
    let area = Rect::new(0, 0, 140, 30);
    let expected = wrap_styled_lines(&body.body, area.width.saturating_sub(2) as usize)
        .len()
        .saturating_add(2) as u16;

    assert_eq!(permission_panel_height(&body.body, area), expected);
}

#[test]
fn permission_panel_height_reserves_context_when_space_is_constrained() {
    let permission = PermissionOverlayState::new(PermissionRequest {
            request_id: "req-1".to_string(),
            session_id: "session".to_string(),
            tool_use_id: "tool-1".to_string(),
            tool_name: "Agent".to_string(),
            tool_input: serde_json::json!({
                "description": "Explore permission panel implementation",
                "prompt": "Please examine the permission panel implementation in orbcode/tui/src/lib.rs.\nFocus on:\n1. How the permission panel is structured and rendered\n2. What permission types are supported\n3. How user input/approval is handled\n4. The overall architecture of the permission system in the TUI\nProvide a detailed analysis of the permission panel code, including key functions, data structures, and the flow of permission requests.",
                "subagent_type": "Explore"
            })
            .to_string(),
            requires_tools_permission: true,
            requires_network_permission: false,
        });

    let body = permission_panel_content(&permission, 78);
    let area = Rect::new(0, 0, 80, 20);
    let height = permission_panel_height_with_context(&body.body, area);

    assert_eq!(area.height.saturating_sub(height), 8);
}

#[test]
fn permissions_slash_output_renders_detail_without_quote_bar() {
    let message = local_slash_command_output_message(
        "/permissions".to_string(),
        "Permissions loaded.".to_string(),
        Some("Permissions:\n  allow-all: disabled".to_string()),
        slash_commands::SlashCommandDeferredFeedback::Direct,
    );
    let note = parse_local_transcript_note(&message).expect("permissions slash command note");

    let rendered = plain_text_lines(&render_local_transcript_note_lines(note, 80, false));
    let rendered_text = rendered.join("\n");

    assert_eq!(rendered.first().map(String::as_str), Some("❯ /permissions"));
    assert!(
        rendered.iter().any(|line| line.starts_with("  └  Tip: ")),
        "{rendered_text}"
    );
    assert!(
        rendered.iter().any(|line| line == "   Permissions:"),
        "{rendered_text}"
    );
    assert!(
        rendered
            .iter()
            .any(|line| line == "     allow-all: disabled"),
        "{rendered_text}"
    );
    assert!(
        !rendered_text.contains("Permissions loaded."),
        "{rendered_text}"
    );
    assert!(!rendered_text.contains("▎"), "{rendered_text}");
}

#[test]
fn render_permission_overview_includes_gates_and_rules() {
    let overview = PermissionOverview {
        permissions: orbcode_app_server::PermissionContext {
            cwd: PathBuf::from("/tmp/project"),
            allow_network: true,
            provider_allow_network: false,
            allow_tools: false,
            allowed_rules: vec![PermissionRule::parse("Bash(git status:*)")],
            denied_rules: vec![PermissionRule::parse("Bash(rm:*)")],
            ask_rules: Vec::new(),
            additional_directories: vec![PathBuf::from("/tmp/extra")],
        },
        allow_all: true,
        settings_allowed_rules: vec!["Bash(git status:*)".to_string()],
        settings_denied_rules: vec!["Bash(rm:*)".to_string()],
        startup_allowed_rules: vec!["Read(src/**)".to_string()],
        startup_denied_rules: Vec::new(),
        edited_allowed_rules: vec!["Write(src/generated/**)".to_string()],
        edited_denied_rules: Vec::new(),
        runtime_allowed_rules: vec!["Grep(orbcode/**)".to_string()],
        runtime_denied_rules: vec!["Bash(git clean:*)".to_string()],
        configured_additional_directories: vec![PathBuf::from("/tmp/configured")],
        session_additional_directories: vec![PathBuf::from("/tmp/extra")],
    };

    let rendered = render_permission_overview(&overview);

    assert!(rendered.contains("Permissions:"));
    assert!(rendered.contains("  allow-all: enabled"));
    assert!(rendered.contains("  tools: disabled"));
    assert!(rendered.contains("  tool network: enabled"));
    assert!(rendered.contains("  provider network: disabled"));
    assert!(rendered.contains("  allow rules: 4"));
    assert!(rendered.contains("  deny rules: 2"));
    assert!(rendered.contains("Allow rules:"));
    assert!(rendered.contains("Deny rules:"));
    assert!(rendered.contains("Bash(git status:*) (configured · settings)"));
    assert!(rendered.contains("Bash(rm:*) (configured · settings)"));
    assert!(rendered.contains("Grep(orbcode/**) (session)"));
    assert!(rendered.contains("Bash(git clean:*) (session)"));
    assert!(!rendered.contains("Source layers:"));
    assert!(!rendered.contains("Configured allow rules:"));
    assert!(!rendered.contains("Session allow rules:"));
    assert!(rendered.contains("Read(src/**) (configured · env/CLI)"));
    assert!(rendered.contains("Write(src/generated/**) (configured · settings edit)"));
    assert!(rendered.contains("Configured directories:"));
    assert!(rendered.contains("/tmp/configured"));
    assert!(rendered.contains("Session-only directories:"));
    assert!(rendered.contains("/tmp/extra"));
    assert!(rendered.contains("Editable sources:"));
    assert!(rendered.contains("settings: writes home settings.json"));
    assert!(rendered.contains("env/CLI rules and project-local or managed settings"));
}
