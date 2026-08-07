use crate::tests::support::*;

#[test]
fn goal_slash_suggestion_is_registered() {
    assert_eq!(
        slash_command_suggestions("/go")
            .first()
            .map(|command| command.name),
        Some("goal")
    );
    assert_eq!(
        exact_slash_command("/goal").map(|command| command.name),
        Some("goal")
    );
}

#[test]
fn slash_command_suggestions_filter_and_prioritize_fuzzy_matches() {
    let suggestions = slash_command_suggestions("/al");
    assert_eq!(
        suggestions.first().map(|command| command.name),
        Some("allow-all")
    );
    assert!(
        suggestions
            .iter()
            .any(|command| command.description.contains("YOLO permissions"))
    );
    assert!(slash_command_suggestions("/allow-all on").is_empty());
    assert!(slash_command_suggestions("/permissions ").is_empty());
    assert!(
        slash_command_suggestions("/cl")
            .iter()
            .any(|command| command.name == "clear")
    );
    assert!(
        slash_command_suggestions("/in")
            .iter()
            .any(|command| command.name == "instructions")
    );
    assert!(
        slash_command_suggestions("/do")
            .iter()
            .any(|command| command.name == "doctor")
    );
    assert!(
        slash_command_suggestions("/di")
            .iter()
            .any(|command| command.name == "diff")
    );
    assert_eq!(
        slash_command_suggestions("/go")
            .first()
            .map(|command| command.name),
        Some("goal")
    );
    assert!(
        slash_command_suggestions("/ctx")
            .iter()
            .any(|command| command.name == "context")
    );
    assert!(
        slash_command_suggestions("/pe")
            .iter()
            .any(|command| command.name == "permissions")
    );
    assert!(
        slash_command_suggestions("/usg")
            .iter()
            .any(|command| command.name == "usage")
    );
    assert!(
        slash_command_suggestions("/sts")
            .iter()
            .any(|command| command.name == "stats")
    );
    assert!(
        slash_command_suggestions("/st")
            .iter()
            .any(|command| command.name == "status")
    );
    assert_eq!(
        slash_command_suggestions("/prm")
            .first()
            .map(|command| command.name),
        Some("permissions")
    );
    assert_eq!(
        slash_command_suggestions("/instr")
            .first()
            .map(|command| command.name),
        Some("instructions")
    );
}

#[test]
fn slash_command_aliases_match_and_canonicalize() {
    let allowed_tools = slash_command_suggestions("/allowed");
    assert_eq!(
        allowed_tools.first().map(|command| command.name),
        Some("permissions")
    );
    assert_eq!(
        exact_slash_command("/allowed-tools").map(|command| command.name),
        Some("permissions")
    );
    assert_eq!(
        canonicalize_slash_command_line("/allowed-tools"),
        "/permissions"
    );
    assert_eq!(canonicalize_slash_command_line("/yolo on"), "/allow-all on");
    assert_eq!(canonicalize_slash_command_line("/new"), "/clear");
    assert_eq!(canonicalize_slash_command_line("/reset"), "/clear");
}

#[test]
fn slash_command_registry_marks_async_local_commands() {
    assert_eq!(
        async_local_slash_command("/status"),
        Some(AsyncLocalSlashCommand::Status)
    );
    assert_eq!(
        async_local_slash_command("/context"),
        Some(AsyncLocalSlashCommand::Context)
    );
    assert_eq!(
        async_local_slash_command("/ctx"),
        Some(AsyncLocalSlashCommand::Context)
    );
    assert_eq!(
        async_local_slash_command("/usage"),
        Some(AsyncLocalSlashCommand::Usage)
    );
    assert_eq!(
        async_local_slash_command("/stats"),
        Some(AsyncLocalSlashCommand::Stats)
    );
    assert_eq!(
        async_local_slash_command("/hooks"),
        Some(AsyncLocalSlashCommand::Hooks)
    );
    assert_eq!(
        async_local_slash_command("/skills"),
        Some(AsyncLocalSlashCommand::Skills)
    );
    assert_eq!(
        async_local_slash_command("/agents"),
        Some(AsyncLocalSlashCommand::Agents)
    );
    assert_eq!(async_local_slash_command("/allowed-tools"), None);
    assert_eq!(async_local_slash_command("/clear"), None);
    assert_eq!(async_local_slash_command("/status now"), None);
}

#[test]
fn slash_command_registry_marks_tui_local_commands() {
    assert_eq!(
        slash_command_invocation("/add-directory ../docs").map(|invocation| {
            (
                invocation.spec.execution,
                invocation.args.to_string(),
                invocation.spec.name,
            )
        }),
        Some((
            SlashCommandExecution::TuiLocal(TuiLocalSlashCommand::AddDir),
            "../docs".to_string(),
            "add-dir"
        ))
    );
    assert_eq!(
        slash_command_invocation("/?").map(|invocation| invocation.spec.execution),
        Some(SlashCommandExecution::TuiLocal(TuiLocalSlashCommand::Help))
    );
    assert_eq!(
        slash_command_invocation("/keybindings").map(|invocation| invocation.spec.execution),
        Some(SlashCommandExecution::TuiLocal(
            TuiLocalSlashCommand::Keybindings
        ))
    );
    assert_eq!(
        slash_command_invocation("/permissions add deny Bash(rm:*)").map(|invocation| {
            (
                invocation.spec.execution,
                invocation.args.to_string(),
                invocation.spec.name,
            )
        }),
        Some((
            SlashCommandExecution::TuiLocal(TuiLocalSlashCommand::Permissions),
            "add deny Bash(rm:*)".to_string(),
            "permissions"
        ))
    );
    assert_eq!(
        slash_command_invocation("/login anthropic --env-var ANTHROPIC_API_KEY").map(
            |invocation| {
                (
                    invocation.spec.execution,
                    invocation.args.to_string(),
                    invocation.spec.name,
                )
            }
        ),
        Some((
            SlashCommandExecution::TuiLocal(TuiLocalSlashCommand::Login),
            "anthropic --env-var ANTHROPIC_API_KEY".to_string(),
            "login"
        ))
    );
    assert_eq!(
        slash_command_invocation("/logout openai").map(|invocation| {
            (
                invocation.spec.execution,
                invocation.args.to_string(),
                invocation.spec.name,
            )
        }),
        Some((
            SlashCommandExecution::TuiLocal(TuiLocalSlashCommand::Logout),
            "openai".to_string(),
            "logout"
        ))
    );
    assert_eq!(
        slash_command_invocation("/new").map(|invocation| invocation.spec.execution),
        Some(SlashCommandExecution::TuiLocal(TuiLocalSlashCommand::Clear))
    );
    assert_eq!(
        slash_command_invocation("/yolo on").map(|invocation| {
            (
                invocation.spec.execution,
                invocation.args.to_string(),
                invocation.spec.name,
            )
        }),
        Some((
            SlashCommandExecution::TuiLocal(TuiLocalSlashCommand::AllowAll),
            "on".to_string(),
            "allow-all"
        ))
    );
    assert_eq!(
        slash_command_invocation("/compact").map(|invocation| invocation.spec.execution),
        Some(SlashCommandExecution::TuiLocal(
            TuiLocalSlashCommand::Compact
        ))
    );
    assert_eq!(
        slash_command_invocation("/effort high").map(|invocation| {
            (
                invocation.spec.execution,
                invocation.args.to_string(),
                invocation.spec.name,
            )
        }),
        Some((
            SlashCommandExecution::TuiLocal(TuiLocalSlashCommand::Effort),
            "high".to_string(),
            "effort"
        ))
    );
    assert_eq!(
        slash_command_invocation("/config effort high").map(|invocation| {
            (
                invocation.spec.execution,
                invocation.args.to_string(),
                invocation.spec.name,
            )
        }),
        Some((
            SlashCommandExecution::TuiLocal(TuiLocalSlashCommand::Config),
            "effort high".to_string(),
            "config"
        ))
    );
    assert_eq!(
        slash_command_invocation("/config editor-mode").map(|invocation| {
            (
                invocation.spec.execution,
                invocation.args.to_string(),
                invocation.spec.name,
            )
        }),
        Some((
            SlashCommandExecution::TuiLocal(TuiLocalSlashCommand::Config),
            "editor-mode".to_string(),
            "config"
        ))
    );
    assert_eq!(
        slash_command_invocation("/fork branch title").map(|invocation| {
            (
                invocation.spec.execution,
                invocation.args.to_string(),
                invocation.spec.name,
            )
        }),
        Some((
            SlashCommandExecution::TuiLocal(TuiLocalSlashCommand::Fork),
            "branch title".to_string(),
            "fork"
        ))
    );
    assert_eq!(
        slash_command_invocation("/plan update docs").map(|invocation| {
            (
                invocation.spec.execution,
                invocation.args.to_string(),
                invocation.spec.name,
            )
        }),
        Some((
            SlashCommandExecution::TuiLocal(TuiLocalSlashCommand::Plan),
            "update docs".to_string(),
            "plan"
        ))
    );
    assert_eq!(
        slash_command_invocation("/release-notes").map(|invocation| invocation.spec.execution),
        Some(SlashCommandExecution::TuiLocal(
            TuiLocalSlashCommand::ReleaseNotes
        ))
    );
    assert_eq!(
        slash_command_invocation("/model claude-opus-4-6").map(|invocation| {
            (
                invocation.spec.execution,
                invocation.args.to_string(),
                invocation.spec.name,
            )
        }),
        Some((
            SlashCommandExecution::TuiLocal(TuiLocalSlashCommand::Model),
            "claude-opus-4-6".to_string(),
            "model"
        ))
    );
    assert_eq!(
        slash_command_invocation("/session abc123").map(|invocation| {
            (
                invocation.spec.execution,
                invocation.args.to_string(),
                invocation.spec.name,
            )
        }),
        Some((
            SlashCommandExecution::TuiLocal(TuiLocalSlashCommand::Resume),
            "abc123".to_string(),
            "resume"
        ))
    );
    assert_eq!(
        slash_command_invocation("/sandbox-toggle exclude npm run test:*").map(|invocation| {
            (
                invocation.spec.execution,
                invocation.args.to_string(),
                invocation.spec.name,
            )
        }),
        Some((
            SlashCommandExecution::TuiLocal(TuiLocalSlashCommand::Sandbox),
            "exclude npm run test:*".to_string(),
            "sandbox"
        ))
    );
    assert_eq!(
        slash_command_invocation("/sessions").map(|invocation| invocation.spec.execution),
        Some(SlashCommandExecution::TuiLocal(
            TuiLocalSlashCommand::Sessions
        ))
    );
    assert_eq!(
        slash_command_invocation("/theme").map(|invocation| invocation.spec.execution),
        Some(SlashCommandExecution::TuiLocal(TuiLocalSlashCommand::Theme))
    );
    assert_eq!(
        slash_command_invocation("/output-style")
            .map(|invocation| (invocation.spec.execution, invocation.args.to_string())),
        Some((
            SlashCommandExecution::TuiLocal(TuiLocalSlashCommand::OutputStyle),
            String::new()
        ))
    );
    assert_eq!(
        slash_command_invocation("/output-style Verbose")
            .map(|invocation| (invocation.spec.execution, invocation.args.to_string())),
        Some((
            SlashCommandExecution::TuiLocal(TuiLocalSlashCommand::OutputStyle),
            "Verbose".to_string()
        ))
    );
    assert_eq!(
        slash_command_invocation("/vim").map(|invocation| invocation.spec.execution),
        Some(SlashCommandExecution::TuiLocal(TuiLocalSlashCommand::Vim))
    );
    assert_eq!(
        slash_command_invocation("/rewind").map(|invocation| invocation.spec.execution),
        Some(SlashCommandExecution::TuiLocal(
            TuiLocalSlashCommand::Rewind
        ))
    );
    // `/checkpoint` is the documented alias for `/rewind`.
    assert_eq!(
        slash_command_invocation("/checkpoint")
            .map(|invocation| { (invocation.spec.execution, invocation.spec.name) }),
        Some((
            SlashCommandExecution::TuiLocal(TuiLocalSlashCommand::Rewind),
            "rewind"
        ))
    );
}

#[test]
fn slash_command_registry_marks_local_output_commands() {
    assert_eq!(
        local_output_slash_command("/tools"),
        Some((LocalOutputSlashCommand::Tools, String::new()))
    );
    assert_eq!(
        local_output_slash_command("/llm-request"),
        Some((LocalOutputSlashCommand::LastRequest, String::new()))
    );
    assert_eq!(
        local_output_slash_command("/last-request"),
        Some((LocalOutputSlashCommand::LastRequest, String::new()))
    );
    assert_eq!(
        local_output_slash_command("/trace"),
        Some((LocalOutputSlashCommand::LastRequest, String::new()))
    );
    assert_eq!(
        local_output_slash_command("/mcp capabilities"),
        Some((
            LocalOutputSlashCommand::McpInspection,
            "capabilities".to_string()
        ))
    );
    assert_eq!(
        local_output_slash_command("/mcp read docs readme"),
        Some((
            LocalOutputSlashCommand::McpInspection,
            "read docs readme".to_string()
        ))
    );
    let (command, args) =
        local_output_slash_command("/mcp read docs readme").expect("mcp invocation");
    assert!(!command.handles_args(&args));
    assert_eq!(local_output_slash_command("/tool Bash {}"), None);
}

#[test]
fn slash_command_registry_declares_feedback_contract() {
    for command in slash_commands() {
        assert!(!command.feedback.deferred.as_str().is_empty());
    }
    assert_eq!(
        slash_commands::SlashCommandFeedback::DEFAULT.deferred,
        slash_commands::SlashCommandDeferredFeedback::Direct
    );
    assert_eq!(
        exact_slash_command("/model").map(|command| command.feedback),
        Some(slash_commands::SlashCommandFeedback::DEFAULT)
    );
    assert_eq!(
        exact_slash_command("/config").map(|command| command.feedback),
        Some(slash_commands::SlashCommandFeedback::DEFAULT)
    );
    assert_eq!(
        exact_slash_command("/instructions").map(|command| command.feedback),
        Some(slash_commands::SlashCommandFeedback::DIRECT_DEFERRED)
    );
    assert_eq!(
        exact_slash_command("/stats").map(|command| command.feedback),
        Some(slash_commands::SlashCommandFeedback::SUMMARY_DIRECT_DEFERRED)
    );
    assert_eq!(
        slash_commands::SlashCommandFeedback {
            deferred: slash_commands::SlashCommandDeferredFeedback::Hidden,
            show_summary: false,
        }
        .deferred,
        slash_commands::SlashCommandDeferredFeedback::Hidden
    );
}

#[test]
fn slash_command_suggestion_lines_render_command_and_description() {
    let state = normal_state("/a", 2);
    let rendered = plain_text_lines(&state.slash_command_suggestion_lines(100));

    assert!(
        rendered
            .iter()
            .any(|line| line.contains("/allow-all") && line.contains("YOLO permissions"))
    );
    assert!(
        rendered
            .first()
            .is_some_and(|line| line.starts_with("│  › "))
    );
    let first = state.slash_command_suggestion_lines(100).remove(0);
    assert_eq!(first.spans[2].style, Style::default());
    assert_eq!(first.spans[3].style, Style::default());
    assert_eq!(first.spans[5].style, Style::default());

    let all_suggestions = normal_state("/", 1).slash_command_suggestion_lines(100);
    let second = &all_suggestions[1];
    assert_eq!(second.spans[3].style, empty_transcript_placeholder_style());
    assert_eq!(second.spans[5].style, empty_transcript_placeholder_style());
}

#[test]
fn slash_command_suggestion_lines_cache_reuses_content_until_input_changes() {
    let mut state = normal_state("/a", 2);

    let first = plain_text_lines(state.cached_slash_command_suggestion_lines(100)).join("\n");
    let second = plain_text_lines(state.cached_slash_command_suggestion_lines(100)).join("\n");

    assert_eq!(first, second);
    assert_eq!(state.slash_suggestion_lines_cache.misses, 1);
    assert_eq!(state.slash_suggestion_lines_cache.hits, 1);

    state.input = "/al".to_string();
    state.input_cursor = state.input.len();
    state.slash_command_selected = 0;
    let changed = plain_text_lines(state.cached_slash_command_suggestion_lines(100)).join("\n");
    assert_ne!(first, changed);
    assert_eq!(state.slash_suggestion_lines_cache.misses, 2);
}

#[test]
#[ignore = "manual stress test for slash suggestion line caching"]
fn slash_command_suggestion_lines_cache_stress_reuses_large_suggestion_view() {
    const FRAME_COUNT: usize = 1_000;

    let mut state = normal_state("/", 1);
    let started = Instant::now();
    let mut last_visible_len = 0;
    for _ in 0..FRAME_COUNT {
        let lines = state.cached_slash_command_suggestion_lines(120);
        last_visible_len = lines.len();
    }
    let duration = started.elapsed();

    assert_eq!(state.slash_suggestion_lines_cache.misses, 1);
    assert_eq!(
        state.slash_suggestion_lines_cache.hits,
        (FRAME_COUNT - 1) as u64
    );
    eprintln!(
        "frames={FRAME_COUNT} visible_lines={last_visible_len} cache_hits={} cache_misses={} loop_us={}",
        state.slash_suggestion_lines_cache.hits,
        state.slash_suggestion_lines_cache.misses,
        duration.as_micros()
    );
}

#[test]
fn slash_argument_completion_suggests_and_completes_arguments() {
    let mut state = normal_state("/permissions ", "/permissions ".len());
    assert!(state.slash_command_suggestion_lines(100).is_empty());
    assert!(!state.complete_selected_slash_argument_completion());

    let mut state = normal_state("/config e", "/config e".len());
    assert!(state.complete_selected_slash_argument_completion());
    assert_eq!(state.input, "/config effort ");

    let mut state = normal_state("/mcp s", "/mcp s".len());
    assert!(state.complete_selected_slash_argument_completion());
    assert_eq!(state.input, "/mcp servers ");
}

#[test]
fn slash_argument_completion_suggests_mcp_tools_and_resources() {
    let catalog = McpSlashSuggestionCatalog {
        servers: vec![McpServerSlashSuggestion {
            id: "docs".to_string(),
            summary: "Documentation server".to_string(),
        }],
        tools: vec![McpToolSlashSuggestion {
            server_id: "docs".to_string(),
            name: "search".to_string(),
            provider_name: "mcp__docs__search".to_string(),
            description: "Search docs".to_string(),
        }],
        resources: vec![McpResourceSlashSuggestion {
            server_id: "docs".to_string(),
            uri: "mcp://docs/readme".to_string(),
            name: "README".to_string(),
            description: "Project README".to_string(),
        }],
    };

    let mut state = normal_state("/tool mcp__", "/tool mcp__".len());
    state.update_mcp_slash_suggestions(catalog.clone());
    assert!(state.complete_selected_slash_argument_completion());
    assert_eq!(state.input, "/tool mcp__docs__search ");

    let mut state = normal_state("/mcp call docs s", "/mcp call docs s".len());
    state.update_mcp_slash_suggestions(catalog.clone());
    assert!(state.complete_selected_slash_argument_completion());
    assert_eq!(state.input, "/mcp call docs search ");

    let mut state = normal_state("/mcp read docs mcp://d", "/mcp read docs mcp://d".len());
    state.update_mcp_slash_suggestions(catalog);
    assert!(state.complete_selected_slash_argument_completion());
    assert_eq!(state.input, "/mcp read docs mcp://docs/readme ");
}

#[test]
fn slash_argument_completion_shows_hint_without_completing_placeholders() {
    let mut state = normal_state("/model ", "/model ".len());
    let rendered = plain_text_lines(&state.slash_command_suggestion_lines(100));

    assert!(
        rendered
            .iter()
            .any(|line| line.contains("[model]") && line.contains("arguments for /model"))
    );
    assert!(!state.complete_selected_slash_argument_completion());
    assert_eq!(state.input, "/model ");
}

#[test]
fn slash_command_suggestion_view_scrolls_with_selection() {
    let view = slash_command_suggestion_view("/", 7).expect("slash suggestions");

    assert_eq!(view.visible_count, SLASH_COMMAND_VISIBLE_ROWS);
    assert!(view.start > 0);
    assert_eq!(view.selected, 7);

    let visible = normal_state("/", 1).slash_command_suggestion_lines(100);
    assert_eq!(visible.len(), SLASH_COMMAND_VISIBLE_ROWS);
    assert!(visible[0].spans[0].style != empty_transcript_placeholder_style());
}

#[test]
fn slash_command_scrollbar_length_tracks_filtered_result_count() {
    let unfiltered = slash_command_suggestion_view("/", 0).expect("all commands");
    let filtered = slash_command_suggestion_view("/al", 0).expect("filtered commands");

    let unfiltered_thumb = (0..unfiltered.visible_count)
        .filter(|row| slash_command_scrollbar_active(*row, &unfiltered))
        .count();
    let filtered_thumb = (0..filtered.visible_count)
        .filter(|row| slash_command_scrollbar_active(*row, &filtered))
        .count();

    assert!(filtered.command_count() < unfiltered.command_count());
    assert!(unfiltered_thumb < unfiltered.visible_count);
    assert_eq!(filtered_thumb, filtered.visible_count);
}

#[test]
fn add_dir_completion_lists_matching_directories() {
    let cwd = test_temp_path("add-dir-completion");
    std::fs::create_dir_all(cwd.join("annotated-transformer")).expect("annotated");
    std::fs::create_dir_all(cwd.join("anthropic-cookbook")).expect("cookbook");
    std::fs::create_dir_all(cwd.join("other")).expect("other");
    std::fs::write(cwd.join("anthropic-notes.txt"), "not a directory").expect("file");
    let mut state = normal_state("/add-dir acb", "/add-dir acb".len());
    state.cwd = cwd;

    let rendered = plain_text_lines(&state.slash_command_suggestion_lines(100));

    assert!(
        rendered
            .iter()
            .any(|line| line.contains("anthropic-cookbook/"))
    );
    assert!(
        !rendered
            .iter()
            .any(|line| line.contains("annotated-transformer/"))
    );
    assert!(!rendered.iter().any(|line| line.contains("other/")));
    assert!(!rendered.iter().any(|line| line.contains("anthropic-notes")));
    let first = state.slash_command_suggestion_lines(100).remove(0);
    assert_eq!(first.spans[2].style, Style::default());
}

#[test]
fn add_dir_completion_accepts_selected_directory() {
    let cwd = test_temp_path("add-dir-complete");
    std::fs::create_dir_all(cwd.join("alpha")).expect("alpha");
    std::fs::create_dir_all(cwd.join("apple")).expect("apple");
    let mut state = normal_state("/add-dir a", "/add-dir a".len());
    state.cwd = cwd;

    assert!(state.complete_selected_add_dir_completion());
    assert_eq!(state.input, "/add-dir alpha/");
    assert_eq!(state.input_cursor, state.input.len());
}

#[test]
fn add_dir_completion_scrollbar_tracks_directory_viewport() {
    let cwd = test_temp_path("add-dir-scroll");
    for index in 0..15 {
        std::fs::create_dir_all(cwd.join(format!("a{index:02}"))).expect("dir");
    }
    let mut state = normal_state("/add-dir a", "/add-dir a".len());
    state.cwd = cwd;
    state.slash_command_selected = 9;
    let view = state
        .add_dir_completion_view()
        .expect("directory completions");

    assert_eq!(view.visible_count, ADD_DIR_COMPLETION_VISIBLE_ROWS);
    assert!(view.start > 0);
    let thumb = (0..view.visible_count)
        .filter(|row| add_dir_completion_scrollbar_active(*row, &view))
        .count();
    assert!(thumb < view.visible_count);
}

#[test]
fn render_slash_command_help_includes_allow_all_and_goal() {
    let help = render_slash_command_help();
    assert!(help.contains("/allow-all on|off"));
    assert!(help.contains("/clear"));
    assert!(help.contains("/context"));
    assert!(help.contains("/doctor"));
    assert!(help.contains("/instructions"));
    assert!(help.contains("/goal [create|edit|pause|resume|clear|budget]"));
    assert!(help.contains("/memory"));
    assert!(help.contains("/permissions"));
    assert!(help.contains("aliases: allowed-tools"));
    assert!(help.contains("/plan [open|<description>]"));
    assert!(help.contains("/status"));
    assert!(help.contains("/stats"));
    assert!(help.contains("/usage"));
    assert!(help.contains("/mcp call <server> <tool> [input]"));
}

#[test]
fn rendered_suggestion_group_headers_and_recency() {
    use crate::dynamic_slash_commands::{DynamicSlashCommandSource, DynamicSlashCommandSpec};
    use crate::slash_commands::register_dynamic_slash_commands;
    use std::sync::{Mutex, OnceLock};

    fn guard() -> &'static Mutex<()> {
        static G: OnceLock<Mutex<()>> = OnceLock::new();
        G.get_or_init(|| Mutex::new(()))
    }
    let _lock = guard().lock().expect("test guard poisoned");
    crate::slash_commands::clear_recency();

    // --- Group headers appear between built-in and user commands ---
    // `/vi` matches built-in `vim` + user `vim-ext` → fits in 6-row viewport
    register_dynamic_slash_commands(vec![DynamicSlashCommandSpec {
        name: "vim-ext".into(),
        aliases: Vec::new(),
        description: "Extended vim mode".into(),
        argument_hint: None,
        source: DynamicSlashCommandSource::User,
        hidden: false,
        prompt_body: "vim-ext body".into(),
        mcp_prompt: None,
        workflow_name: None,
    }]);

    let state = normal_state("/vi", 3);
    let rendered = plain_text_lines(&state.slash_command_suggestion_lines(120));

    assert!(
        rendered.iter().any(|line| line.contains("── Built-in ──")),
        "expected Built-in header: {rendered:#?}"
    );
    assert!(
        rendered.iter().any(|line| line.contains("── User ──")),
        "expected User header: {rendered:#?}"
    );
    assert!(
        rendered.iter().any(|line| line.contains("/vim ")),
        "expected /vim command: {rendered:#?}"
    );
    assert!(
        rendered.iter().any(|line| line.contains("/vim-ext")),
        "expected /vim-ext command: {rendered:#?}"
    );

    // --- Plugin group header shows plugin name ---
    register_dynamic_slash_commands(vec![DynamicSlashCommandSpec {
        name: "acme:vim-mode".into(),
        aliases: Vec::new(),
        description: "Acme vim integration".into(),
        argument_hint: None,
        source: DynamicSlashCommandSource::Plugin {
            plugin_id: "acme@market".into(),
            plugin_name: "acme".into(),
        },
        hidden: false,
        prompt_body: "acme body".into(),
        mcp_prompt: None,
        workflow_name: None,
    }]);

    let state = normal_state("/vim", 4);
    let rendered = plain_text_lines(&state.slash_command_suggestion_lines(120));
    assert!(
        rendered
            .iter()
            .any(|line| line.contains("── Plugin: acme ──")),
        "expected 'Plugin: acme' header: {rendered:#?}"
    );

    // --- Group header uses muted style ---
    register_dynamic_slash_commands(vec![DynamicSlashCommandSpec {
        name: "vim-ext".into(),
        aliases: Vec::new(),
        description: "Extended vim mode".into(),
        argument_hint: None,
        source: DynamicSlashCommandSource::User,
        hidden: false,
        prompt_body: "vim-ext body".into(),
        mcp_prompt: None,
        workflow_name: None,
    }]);

    let state = normal_state("/vi", 3);
    let lines = state.slash_command_suggestion_lines(120);
    let muted = empty_transcript_placeholder_style();
    let header_line = lines
        .iter()
        .find(|line| plain_text_line(line).contains("── Built-in ──"))
        .expect("Built-in header line");
    assert_eq!(
        header_line.spans[3].style, muted,
        "group header label should use muted style"
    );

    // --- Group headers don't have selection marker ---
    let rendered = plain_text_lines(&lines);
    for line in &rendered {
        if line.contains("──") {
            assert!(
                !line.contains("› "),
                "group header should not have selection marker: {line}"
            );
        }
    }
    let first_command_line = rendered
        .iter()
        .find(|line| !line.contains("──") && line.contains('/'))
        .expect("at least one command line");
    assert!(
        first_command_line.contains("› "),
        "first command should be selected: {first_command_line}"
    );

    // --- Recency boost moves command earlier in rendered lines ---
    register_dynamic_slash_commands(Vec::new());
    crate::slash_commands::clear_recency();

    let baseline = plain_text_lines(&normal_state("/sta", 4).slash_command_suggestion_lines(120));
    let stats_before = baseline
        .iter()
        .position(|line| line.contains("/stats"))
        .expect("/stats in baseline");
    let status_before = baseline
        .iter()
        .position(|line| line.contains("/status"))
        .expect("/status in baseline");
    assert!(
        stats_before < status_before,
        "before recency: /stats ({stats_before}) should precede /status ({status_before})"
    );

    record_slash_command_use("status");

    let boosted = plain_text_lines(&normal_state("/sta", 4).slash_command_suggestion_lines(120));
    let stats_after = boosted
        .iter()
        .position(|line| line.contains("/stats"))
        .expect("/stats in boosted");
    let status_after = boosted
        .iter()
        .position(|line| line.contains("/status"))
        .expect("/status in boosted");
    assert!(
        status_after < stats_after,
        "after recency: /status ({status_after}) should precede /stats ({stats_after})"
    );

    crate::slash_commands::clear_recency();
    register_dynamic_slash_commands(Vec::new());
}
