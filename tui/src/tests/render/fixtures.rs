use crate::tests::support::*;

#[test]
fn transcript_fixture_matches_research_flow_baseline() {
    let state = transcript_research_fixture_state();
    let actual = committed_transcript_fixture_text(&state, 90);
    let expected = include_str!("../../../testdata/transcript-fixtures/research-flow.txt");
    assert_fixture(&actual, expected);
}

#[test]
fn research_fixture_prefers_collapsed_summary_groups() {
    let state = transcript_research_fixture_state();
    let actual = committed_transcript_fixture_text(&state, 90);

    assert!(
        actual.contains("Searched for 1 pattern, listed 1 directory (ctrl+o to expand)"),
        "research flow should collapse read/search/list activity by default:\n{actual}"
    );
    assert!(
        actual.contains("Bash(pwd && ls -la)"),
        "research flow should still expose ordinary bash steps:\n{actual}"
    );
    assert!(
        actual.contains("/Users/user/github/sample-workspace-main/crates/render-fixtures"),
        "research flow should preview bash output:\n{actual}"
    );
    assert!(
        !actual.contains("Glob(\"**/Cargo.toml\")") && !actual.contains("Bash($ ls -la)"),
        "collapsed groups should absorb the underlying read/search/list steps in default mode:\n{actual}"
    );
}

#[test]
fn expanded_research_fixture_reveals_absorbed_tool_steps() {
    let mut state = transcript_research_fixture_state();
    state.expanded_tool_details = true;
    let actual = committed_transcript_fixture_text(&state, 90);

    assert!(
        actual.contains("Searched for 1 pattern, listed 1 directory (ctrl+o to collapse)"),
        "expanded transcript should keep the collapsed summary header:\n{actual}"
    );
    assert!(
        actual.contains("Search(pattern: \"**/Cargo.toml\")"),
        "expanded transcript should render grouped search steps via tool cells:\n{actual}"
    );
    assert!(
        actual.contains("Bash(ls -la)"),
        "expanded transcript should render grouped list steps via tool cells:\n{actual}"
    );
    assert!(
        !actual.contains("Glob(\"**/Cargo.toml\")") && !actual.contains("Bash($ ls -la)"),
        "expanded transcript should not fall back to raw tool-use blocks:\n{actual}"
    );
}

#[test]
fn transcript_fixture_matches_structured_workflow_cards() {
    let state = transcript_workflow_cards_fixture_state();
    let actual = committed_transcript_fixture_text(&state, 90);
    let expected = include_str!("../../../testdata/transcript-fixtures/workflow-cards.txt");
    assert_fixture(&actual, expected);
}

#[test]
fn transcript_fixture_matches_typescript_terminal_excerpt() {
    let state = transcript_typescript_terminal_excerpt_fixture_state();
    let actual = committed_transcript_fixture_text(&state, 90);
    let expected =
        include_str!("../../../testdata/transcript-fixtures/typescript-terminal-excerpt.txt");
    assert_fixture(&actual, expected);
}

#[test]
fn expanded_agent_card_renders_prompt_and_response_sections() {
    let cwd = PathBuf::from("/Users/user/github/sample-repo-tui-render");
    let messages = vec![
            TranscriptMessage::from_blocks(
                MessageRole::Assistant,
                vec![TranscriptBlock::ToolUse {
                    id: "agent-tool".to_string(),
                    name: "Agent".to_string(),
                    input: "{\n  \"description\": \"Explore repo\",\n  \"prompt\": \"Check the CLI flow\\nThen summarize the gaps\",\n  \"subagent_type\": \"Explore\"\n}".to_string(),
                }],
            ),
            TranscriptMessage::from_blocks(
                MessageRole::User,
                vec![TranscriptBlock::ToolResult {
                    tool_use_id: "agent-tool".to_string(),
                    content: "## Summary\nThe CLI flow has two entry points.".to_string(),
                    is_error: false,
                    metadata: Some("{\"status\":\"completed\",\"totalToolUseCount\":2,\"totalTokens\":18,\"totalDurationMs\":9200,\"content\":[{\"type\":\"text\",\"text\":\"## Summary\\nThe CLI flow has two entry points.\"}]}".to_string()),
                }],
            ),
        ];

    let (card, _) = build_tool_cell(&messages, 0, &cwd).unwrap();
    let rendered = render_tool_cell_lines(&card, true, None, 80, &cwd)
        .into_iter()
        .map(|line| {
            line.spans
                .iter()
                .map(|span| span.content.as_ref())
                .collect::<String>()
        })
        .collect::<Vec<_>>();

    assert!(rendered.iter().any(|line| line.contains("Prompt:")));
    assert!(
        rendered
            .iter()
            .any(|line| line.contains("Check the CLI flow"))
    );
    assert!(
        rendered
            .iter()
            .any(|line| line.contains("Then summarize the gaps"))
    );
    assert!(!rendered.iter().any(|line| line.contains("Description:")));
    assert!(rendered.iter().any(|line| line.contains("Response:")));
    assert!(rendered.iter().any(|line| line.contains("Summary")));
    assert!(
        rendered
            .iter()
            .any(|line| line.contains("The CLI flow has two entry points."))
    );
}

#[test]
fn final_answer_keeps_prior_tool_steps_scrollable_in_virtual_timeline() {
    let mut state = normal_state("", 0);
    state.request_in_flight = true;

    let tool_use = TranscriptMessage::from_blocks(
        MessageRole::Assistant,
        vec![TranscriptBlock::ToolUse {
            id: "bash-1".to_string(),
            name: "Bash".to_string(),
            input: "{\"command\":\"ls -la\"}".to_string(),
        }],
    );
    let tool_result = TranscriptMessage::from_blocks(
        MessageRole::User,
        vec![TranscriptBlock::ToolResult {
            tool_use_id: "bash-1".to_string(),
            content: "total 60\n-rw-r--r-- Cargo.toml".to_string(),
            is_error: false,
            metadata: Some(
                "{\"status\":\"completed\",\"summary\":\"Listed workspace files.\"}".to_string(),
            ),
        }],
    );

    state.apply_stream_event(StreamEvent::AssistantMessageCompleted {
        message: tool_use,
        provider: ProviderId::Anthropic,
        fallback_from: None,
        usage: TokenUsage::default(),
    });
    state.apply_stream_event(StreamEvent::UserMessage {
        message: tool_result,
    });

    state.pending_assistant =
        "结论\n\nRust 版本已经覆盖核心交互路径，但仍不是完整移植。".to_string();
    state.apply_stream_event(StreamEvent::AssistantMessageCompleted {
        message: TranscriptMessage::new(
            MessageRole::Assistant,
            "结论\n\nRust 版本已经覆盖核心交互路径，但仍不是完整移植。".to_string(),
        ),
        provider: ProviderId::Anthropic,
        fallback_from: None,
        usage: TokenUsage::default(),
    });
    assert!(state.pending_assistant.is_empty());

    let transcript_lines = state.transcript_lines(80);
    let transcript_text = plain_text_lines(&transcript_lines).join("\n");
    assert!(
        transcript_text.contains("Listed 1 directory (ctrl+o to expand)"),
        "committed timeline should keep the tool step before the final answer:\n{transcript_text}"
    );
    assert!(transcript_text.contains("结论"), "{transcript_text}");

    let bottom_view = visible_transcript_lines(&transcript_lines, 80, 4, 0);
    let bottom_text = plain_text_lines(&bottom_view.visible_lines).join("\n");
    assert!(bottom_text.contains("结论"), "{bottom_text}");

    let visual_lines = wrap_styled_lines(&transcript_lines, 80);
    let visual_text = plain_text_lines(&visual_lines);
    let group_line_index = visual_text
        .iter()
        .position(|line| line.contains("Listed 1 directory (ctrl+o to expand)"))
        .expect("tool step should exist in the virtual timeline");
    let requested_scroll = visual_text
        .len()
        .saturating_sub(group_line_index.saturating_add(1));
    let top_view = visible_transcript_lines(&transcript_lines, 80, 4, requested_scroll);
    let top_text = plain_text_lines(&top_view.visible_lines).join("\n");
    assert!(
        top_text.contains("Listed 1 directory (ctrl+o to expand)"),
        "scrolling upward should expose earlier tool steps:\n{top_text}"
    );
}
