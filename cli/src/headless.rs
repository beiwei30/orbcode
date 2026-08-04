use std::io::{self, Write};

use anyhow::Result;
use orbcode_app_server::{
    AppServer, PermissionDecision, PermissionMode, StreamErrorCategory, StreamEvent, TokenUsage,
    ToolUseCompletionKind,
};
use orbcode_app_server_client::AppClient;
use serde::Serialize;

use crate::args::{CliInputFormat, CliOutputFormat};
use crate::control::{ControlFrame, parse_control_line};
use crate::exit_code::{HeadlessOutcome, classify_outcome};
use crate::stream_json::{
    CostFields, InitMetadata, McpServerInfo, StreamJsonEmitter, control_response_error,
    control_response_success, control_response_success_with_data,
};

/// Typed representation of the session-state control response payload.
#[derive(Serialize)]
struct SessionStateResponse {
    session_id: String,
    cwd: String,
    model_display_name: String,
    model_name: String,
    model_capabilities: Vec<String>,
    effort_level: Option<&'static str>,
    default_provider: String,
    fallback_provider: Option<String>,
    sandbox_mode: String,
    persisted_session_count: usize,
    background_job_count: usize,
    available_tool_count: usize,
    configured_mcp_server_count: usize,
}

/// Typed representation of the context-usage control response payload.
#[derive(Serialize)]
struct ContextUsageResponse {
    model: String,
    estimated_tokens: u32,
    categories: ContextCategoriesResponse,
    context_window: u32,
    effective_context_window: u32,
    free_space_tokens: u32,
    percent_left: u32,
    is_above_auto_compact_threshold: bool,
    is_above_warning_threshold: bool,
    is_above_error_threshold: bool,
    is_at_blocking_limit: bool,
}

#[derive(Serialize)]
struct ContextCategoriesResponse {
    system_prompt: u32,
    system_tools: u32,
    mcp_tools: u32,
    memory: u32,
    skills: u32,
    conversation: u32,
    attachments: u32,
    uncategorized: u32,
}

/// Error-case response for control queries.
#[derive(Serialize)]
struct ErrorResponse {
    error: String,
}

pub(crate) async fn run_headless_prompt(
    app_server: AppServer,
    client: &AppClient,
    session: Option<String>,
    prompt: String,
) -> Result<String> {
    let bootstrap = app_server.bootstrap(session.as_deref()).await?;
    println!("session {}", bootstrap.session.session_id);
    println!(
        "provider {} fallback {:?} retries {} permissions {} | tools {} | mcp servers {}",
        bootstrap.default_provider,
        bootstrap
            .fallback_provider
            .map(|provider| provider.to_string()),
        bootstrap.max_retries,
        bootstrap.permissions.describe(),
        bootstrap.available_tool_count,
        bootstrap.configured_mcp_server_count
    );

    let mut stream = app_server
        .submit_turn(&bootstrap.session.session_id, prompt)
        .await?;

    loop {
        tokio::select! {
            maybe_event = stream.recv() => {
                match maybe_event {
                    Some(StreamEvent::RequestStarted { context, .. }) => {
                        eprintln!("context {}", context.compact_summary());
                    }
                    Some(StreamEvent::AssistantDelta { delta, .. }) => {
                        print!("{delta}");
                        io::stdout().flush()?;
                    }
                    Some(StreamEvent::PermissionRequested { request }) => {
                        let approved =
                            headless_permission_decision(&request, bootstrap.permissions.allow_tools, bootstrap.permissions.allow_network);
                        let _ = app_server
                            .respond_to_permission_request(
                                &request.request_id,
                                if approved {
                                    PermissionDecision::Approve
                                } else {
                                    PermissionDecision::Deny
                                },
                            )
                            .await;
                        eprintln!("{} tool request {}", if approved { "approved" } else { "denied" }, request.summary());
                    }
                    Some(StreamEvent::PermissionResolved { kind, request_id, .. }) => {
                        if matches!(
                            kind,
                            orbcode_protocol::PermissionResolutionKind::Interrupted
                        ) {
                            eprintln!("tool request {request_id} interrupted");
                        }
                    }
                    Some(StreamEvent::ToolUseStarted { tool_name, .. }) => {
                        eprintln!("running tool `{tool_name}`");
                    }
                    Some(StreamEvent::ToolUseCompleted {
                        tool_name, kind, ..
                    }) => {
                        eprintln!(
                            "tool `{}` {}",
                            tool_name,
                            match kind {
                                orbcode_protocol::ToolUseCompletionKind::Success => "completed",
                                orbcode_protocol::ToolUseCompletionKind::ExecutionFailed => {
                                    "failed during execution"
                                }
                                orbcode_protocol::ToolUseCompletionKind::PermissionDenied => {
                                    "was denied"
                                }
                                orbcode_protocol::ToolUseCompletionKind::Interrupted => {
                                    "was interrupted"
                                }
                                orbcode_protocol::ToolUseCompletionKind::UnknownTool => {
                                    "was rejected as unknown"
                                }
                                _ => "finished",
                            }
                        );
                    }
                    Some(StreamEvent::AssistantMessageCompleted { .. }) => {
                        println!();
                    }
                    Some(StreamEvent::AssistantMessageDiscarded {
                        provider,
                        fallback_provider,
                        ..
                    }) => {
                        eprintln!(
                            "\n[discarded partial response from {provider}; switching to {fallback_provider}]"
                        );
                    }
                    Some(StreamEvent::ContextCompacted {
                        duration_ms,
                        original_message_count,
                        compacted_message_count,
                        provider_generated,
                        fallback_reason,
                        ..
                    }) => {
                        eprintln!(
                            "{}",
                            context_compacted_log_line(
                                duration_ms,
                                original_message_count,
                                compacted_message_count,
                                provider_generated,
                                fallback_reason.as_deref(),
                            )
                        );
                    }
                    Some(StreamEvent::TurnCancelled { kind, usage, .. }) => {
                        println!(
                            "\n[cancelled: {}]",
                            match kind {
                                orbcode_protocol::TurnCancellationKind::BeforeResponse => {
                                    "before response"
                                }
                                orbcode_protocol::TurnCancellationKind::AssistantStreaming => {
                                    "assistant streaming"
                                }
                                orbcode_protocol::TurnCancellationKind::ToolStage => "tool stage",
                                _ => "cancelled",
                            }
                        );
                        if let Some(usage) = usage {
                            eprintln!("partial usage: {} tokens", usage.total_tokens);
                        }
                        break;
                    }
                    Some(StreamEvent::TurnFinished {
                        provider,
                        fallback_from,
                        usage,
                        ..
                    }) => {
                        match fallback_from {
                            Some(from) => eprintln!(
                                "completed with {} (fallback from {}) | input={} output={} total={}",
                                provider,
                                from,
                                usage.input_tokens,
                                usage.output_tokens,
                                usage.total_tokens
                            ),
                            None => eprintln!(
                                "completed with {} | input={} output={} total={}",
                                provider, usage.input_tokens, usage.output_tokens, usage.total_tokens
                            ),
                        }
                        break;
                    }
                    Some(StreamEvent::Error { provider, message, .. }) => {
                        match provider {
                            Some(provider) => eprintln!("{provider}: {message}"),
                            None => eprintln!("{message}"),
                        }
                        break;
                    }
                    Some(StreamEvent::Budget {
                        blocked,
                        outcome,
                        total_usd,
                        max_budget_usd,
                        ..
                    }) => {
                        if blocked {
                            eprintln!(
                                "[budget] max budget reached ({}): counted ${total_usd:.4} of ${max_budget_usd:.4} cap; turn stopped",
                                outcome.as_str()
                            );
                            break;
                        }
                        eprintln!(
                            "[budget] warning: spend cannot be fully priced; counted ${total_usd:.4} of ${max_budget_usd:.4} cap"
                        );
                    }
                    Some(_) => {}
                    None => break,
                }
            }
            signal = tokio::signal::ctrl_c() => {
                signal?;
                if client.cancel_turn(&bootstrap.session.session_id).await.is_ok() {
                    eprintln!("\n[cancelling]");
                }
            }
        }
    }

    Ok(bootstrap.session.session_id)
}

pub(crate) async fn run_print_mode(
    app_server: AppServer,
    client: &AppClient,
    session: Option<String>,
    positional_prompt: Option<String>,
    output_format: CliOutputFormat,
    input_format: CliInputFormat,
    verbose: bool,
) -> Result<()> {
    let bootstrap = app_server.bootstrap(session.as_deref()).await?;
    let session_id = bootstrap.session.session_id.clone();
    let model_name = client
        .model_name()
        .await
        .ok()
        .and_then(|v| v["model_name"].as_str().map(String::from))
        .unwrap_or_else(|| "unknown".to_string());
    let mut run = HeadlessRun::new(
        output_format,
        verbose,
        StreamJsonEmitter::new(session_id.clone(), model_name),
        bootstrap.permissions.allow_tools,
        bootstrap.permissions.allow_network,
    );

    if matches!(output_format, CliOutputFormat::StreamJson) {
        let meta = build_init_metadata(client, &bootstrap).await;
        let init = run.emitter.build_system_init(&meta);
        write_json_line(&init)?;
    }

    match input_format {
        CliInputFormat::Text => {
            let prompt = match positional_prompt {
                Some(prompt) => prompt,
                None => super::invalid_cli_input(
                    "--print requires a prompt or --input-format stream-json",
                ),
            };
            run.run_text_turn(&app_server, &session_id, prompt).await?;
        }
        CliInputFormat::StreamJson => {
            run.run_control_input_loop(&app_server, client, &session_id)
                .await?;
        }
    }

    let cost = build_cost_fields(client, &session_id).await;
    let (outcome, result_payload) = run.finalize(&cost);

    match output_format {
        CliOutputFormat::Text => {
            if let Some(message) = &run.last_error {
                eprintln!("error: {message}");
            }
            if run.was_cancelled {
                eprintln!("[cancelled]");
            }
            if matches!(outcome, HeadlessOutcome::PermissionDenied) {
                eprintln!("[permission denied]");
            }
        }
        CliOutputFormat::Json | CliOutputFormat::StreamJson => {
            write_json_line(&result_payload)?;
        }
    }

    if verbose && cost.total_cost_usd > 0.0 {
        eprintln!("Session cost: ${:.4}", cost.total_cost_usd);
    }

    io::stdout().flush().ok();
    io::stderr().flush().ok();
    let code = outcome.code();
    if code != 0 {
        std::process::exit(code);
    }
    Ok(())
}

pub(crate) async fn run_background_worker(
    app_server: AppServer,
    client: &AppClient,
    job_id: String,
    session_id: String,
    prompt: String,
) -> Result<()> {
    #[cfg(unix)]
    let mut sigterm = tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate())?;
    let mut received_signal: Option<i32> = None;

    let permissions = app_server.permissions();
    let _ = app_server
        .mark_background_running(&job_id, Some(std::process::id()))
        .await;
    app_server
        .append_background_log(
            &job_id,
            &format!("session {session_id}\nprompt {}\n", prompt.trim()),
        )
        .await?;

    let model_name = client
        .model_name()
        .await
        .ok()
        .and_then(|v| v["model_name"].as_str().map(String::from))
        .unwrap_or_else(|| "unknown".to_string());
    let mut emitter = StreamJsonEmitter::new(session_id.clone(), model_name.clone());
    let started = std::time::Instant::now();
    {
        let tool_names: Vec<String> = client
            .list_tools()
            .await
            .ok()
            .and_then(|v| v.as_array().cloned())
            .unwrap_or_default()
            .into_iter()
            .filter_map(|t| t["name"].as_str().map(String::from))
            .collect();
        let mcp_servers: Vec<McpServerInfo> = client
            .list_mcp_servers()
            .await
            .ok()
            .and_then(|v| v.as_array().cloned())
            .unwrap_or_default()
            .iter()
            .map(|s| McpServerInfo {
                name: s["id"].as_str().unwrap_or("").to_string(),
                status: s["status"].as_str().unwrap_or("").to_string(),
            })
            .collect();
        let permission_mode_value = client.permission_mode().await.ok();
        let permission_mode_str = permission_mode_value
            .as_ref()
            .and_then(|v| v["mode"].as_str())
            .unwrap_or("default");
        let permission_mode = match permission_mode_str {
            "acceptEdits" => PermissionMode::AcceptEdits,
            "bypassPermissions" => PermissionMode::BypassPermissions,
            "plan" => PermissionMode::Plan,
            _ => PermissionMode::Default,
        };
        let meta = InitMetadata {
            session_id: session_id.clone(),
            cwd: std::env::current_dir()
                .unwrap_or_default()
                .display()
                .to_string(),
            model: model_name,
            tool_names,
            mcp_servers,
            permission_mode,
        };
        let init = emitter.build_system_init(&meta);
        let _ = app_server
            .append_background_event_line(&job_id, &init)
            .await;
    }

    let mut stream = match app_server.submit_turn(&session_id, prompt.clone()).await {
        Ok(stream) => stream,
        Err(error) => {
            let message = error.to_string();
            app_server
                .append_background_log(&job_id, &format!("error {message}\n"))
                .await?;
            app_server
                .fail_background_job_with_exit(&job_id, message.clone(), Some(1), None)
                .await?;
            let cost = build_cost_fields(client, &session_id).await;
            let result = emitter.build_result(
                "error_during_execution",
                true,
                started.elapsed().as_millis() as u64,
                0,
                0,
                &orbcode_protocol::TokenUsage::default(),
                &cost,
                None,
                None,
                &[message],
            );
            let _ = app_server
                .append_background_event_line(&job_id, &result)
                .await;
            return Ok(());
        }
    };

    loop {
        #[cfg(unix)]
        let event = tokio::select! {
            event = stream.recv() => event,
            _ = sigterm.recv() => {
                if received_signal.is_none() {
                    received_signal = Some(15);
                    let _ = client.cancel_turn(&session_id).await;
                }
                continue;
            }
        };
        #[cfg(not(unix))]
        let event = stream.recv().await;

        let Some(event) = event else {
            break;
        };
        for record in emitter.process(&event) {
            let _ = app_server
                .append_background_event_line(&job_id, &record)
                .await;
        }
        match event {
            StreamEvent::RequestStarted { context, .. } => {
                app_server
                    .append_background_log(
                        &job_id,
                        &format!("context {}\n", context.compact_summary()),
                    )
                    .await?;
            }
            StreamEvent::AssistantDelta { delta, .. } => {
                app_server.append_background_log(&job_id, &delta).await?;
            }
            StreamEvent::PermissionRequested { request } => {
                let approved = headless_permission_decision(
                    &request,
                    permissions.allow_tools,
                    permissions.allow_network,
                );
                let _ = app_server
                    .respond_to_permission_request(
                        &request.request_id,
                        if approved {
                            PermissionDecision::Approve
                        } else {
                            PermissionDecision::Deny
                        },
                    )
                    .await;
                app_server
                    .append_background_log(
                        &job_id,
                        &format!(
                            "{} tool request {}\n",
                            if approved { "approved" } else { "denied" },
                            request.summary()
                        ),
                    )
                    .await?;
            }
            StreamEvent::PermissionResolved {
                kind, request_id, ..
            } => {
                if matches!(
                    kind,
                    orbcode_protocol::PermissionResolutionKind::Interrupted
                ) {
                    app_server
                        .append_background_log(
                            &job_id,
                            &format!("tool request {request_id} interrupted\n"),
                        )
                        .await?;
                }
            }
            StreamEvent::ToolUseStarted { tool_name, .. } => {
                app_server
                    .append_background_log(&job_id, &format!("running tool `{tool_name}`\n"))
                    .await?;
            }
            StreamEvent::ToolUseCompleted {
                tool_name, kind, ..
            } => {
                app_server
                    .append_background_log(
                        &job_id,
                        &format!(
                            "tool `{}` {}\n",
                            tool_name,
                            match kind {
                                orbcode_protocol::ToolUseCompletionKind::Success => "completed",
                                orbcode_protocol::ToolUseCompletionKind::ExecutionFailed => {
                                    "failed during execution"
                                }
                                orbcode_protocol::ToolUseCompletionKind::PermissionDenied => {
                                    "was denied"
                                }
                                orbcode_protocol::ToolUseCompletionKind::Interrupted => {
                                    "was interrupted"
                                }
                                orbcode_protocol::ToolUseCompletionKind::UnknownTool => {
                                    "was rejected as unknown"
                                }
                                _ => "finished",
                            }
                        ),
                    )
                    .await?;
            }
            StreamEvent::AssistantMessageCompleted { .. } => {
                app_server.append_background_log(&job_id, "\n").await?;
            }
            StreamEvent::AssistantMessageDiscarded {
                provider,
                fallback_provider,
                ..
            } => {
                app_server
                    .append_background_log(
                        &job_id,
                        &format!(
                            "\n[discarded partial response from {provider}; switching to {fallback_provider}]\n"
                        ),
                    )
                    .await?;
            }
            StreamEvent::ContextCompacted {
                duration_ms,
                original_message_count,
                compacted_message_count,
                provider_generated,
                fallback_reason,
                ..
            } => {
                app_server
                    .append_background_log(
                        &job_id,
                        &format!(
                            "{}\n",
                            context_compacted_log_line(
                                duration_ms,
                                original_message_count,
                                compacted_message_count,
                                provider_generated,
                                fallback_reason.as_deref(),
                            )
                        ),
                    )
                    .await?;
            }
            StreamEvent::TurnCancelled {
                kind, ref usage, ..
            } => {
                if let Some(usage) = usage {
                    app_server
                        .append_background_log(
                            &job_id,
                            &format!(
                                "\n[cancelled: {}] partial total={}\n",
                                match kind {
                                    orbcode_protocol::TurnCancellationKind::BeforeResponse => {
                                        "before response"
                                    }
                                    orbcode_protocol::TurnCancellationKind::AssistantStreaming => {
                                        "assistant streaming"
                                    }
                                    orbcode_protocol::TurnCancellationKind::ToolStage => {
                                        "tool stage"
                                    }
                                    _ => "cancelled",
                                },
                                usage.total_tokens
                            ),
                        )
                        .await?;
                } else {
                    app_server
                        .append_background_log(
                            &job_id,
                            &format!(
                                "\n[cancelled: {}]\n",
                                match kind {
                                    orbcode_protocol::TurnCancellationKind::BeforeResponse => {
                                        "before response"
                                    }
                                    orbcode_protocol::TurnCancellationKind::AssistantStreaming => {
                                        "assistant streaming"
                                    }
                                    orbcode_protocol::TurnCancellationKind::ToolStage => {
                                        "tool stage"
                                    }
                                    _ => "cancelled",
                                }
                            ),
                        )
                        .await?;
                }
                if let Some(sig) = received_signal {
                    app_server
                        .mark_background_cancelled_with_signal(
                            &job_id,
                            Some(format!("turn cancelled ({})", kind.as_str())),
                            Some(sig),
                        )
                        .await?;
                } else {
                    app_server
                        .mark_background_cancelled(
                            &job_id,
                            Some(format!("turn cancelled ({})", kind.as_str())),
                        )
                        .await?;
                }
                let cancel_usage = usage.clone().unwrap_or_default();
                let cost = build_cost_fields(client, &session_id).await;
                let result = emitter.build_result(
                    "error_during_execution",
                    true,
                    started.elapsed().as_millis() as u64,
                    0,
                    1,
                    &cancel_usage,
                    &cost,
                    Some("cancelled"),
                    None,
                    &["turn cancelled".to_string()],
                );
                let _ = app_server
                    .append_background_event_line(&job_id, &result)
                    .await;
                break;
            }
            StreamEvent::TurnFinished {
                provider,
                fallback_from,
                usage,
                ..
            } => {
                app_server
                    .append_background_log(
                        &job_id,
                        &match fallback_from {
                            Some(from) => format!(
                                "\ncompleted with {} (fallback from {}) | input={} output={} total={}\n",
                                provider,
                                from,
                                usage.input_tokens,
                                usage.output_tokens,
                                usage.total_tokens
                            ),
                            None => format!(
                                "\ncompleted with {} | input={} output={} total={}\n",
                                provider,
                                usage.input_tokens,
                                usage.output_tokens,
                                usage.total_tokens
                            ),
                        },
                    )
                    .await?;
                app_server
                    .complete_background_job_with_exit(&job_id, Some(0), None)
                    .await?;
                let cost = build_cost_fields(client, &session_id).await;
                let result = emitter.build_result(
                    "success",
                    false,
                    started.elapsed().as_millis() as u64,
                    0,
                    1,
                    &usage,
                    &cost,
                    Some("end_turn"),
                    None,
                    &[],
                );
                let _ = app_server
                    .append_background_event_line(&job_id, &result)
                    .await;
                break;
            }
            StreamEvent::Error {
                provider, message, ..
            } => {
                let rendered = match provider {
                    Some(provider) => format!("{provider}: {message}"),
                    None => message,
                };
                app_server
                    .append_background_log(&job_id, &format!("\nerror {rendered}\n"))
                    .await?;
                app_server
                    .fail_background_job_with_exit(&job_id, rendered.clone(), Some(1), None)
                    .await?;
                let cost = build_cost_fields(client, &session_id).await;
                let result = emitter.build_result(
                    "error_during_execution",
                    true,
                    started.elapsed().as_millis() as u64,
                    0,
                    1,
                    &orbcode_protocol::TokenUsage::default(),
                    &cost,
                    None,
                    None,
                    &[rendered],
                );
                let _ = app_server
                    .append_background_event_line(&job_id, &result)
                    .await;
                break;
            }
            StreamEvent::Budget {
                blocked,
                outcome,
                total_usd,
                max_budget_usd,
                ..
            } => {
                if blocked {
                    let rendered = format!(
                        "max budget reached ({}): counted ${total_usd:.4} of ${max_budget_usd:.4} cap",
                        outcome.as_str()
                    );
                    app_server
                        .append_background_log(&job_id, &format!("\nbudget {rendered}\n"))
                        .await?;
                    app_server
                        .fail_background_job_with_exit(&job_id, rendered.clone(), Some(1), None)
                        .await?;
                    let cost = build_cost_fields(client, &session_id).await;
                    let result = emitter.build_result(
                        "error_max_budget_usd",
                        true,
                        started.elapsed().as_millis() as u64,
                        0,
                        1,
                        &orbcode_protocol::TokenUsage::default(),
                        &cost,
                        None,
                        None,
                        &[rendered],
                    );
                    let _ = app_server
                        .append_background_event_line(&job_id, &result)
                        .await;
                    break;
                }
                app_server
                    .append_background_log(
                        &job_id,
                        &format!(
                            "\n[budget] warning: spend cannot be fully priced; counted ${total_usd:.4} of ${max_budget_usd:.4} cap\n"
                        ),
                    )
                    .await?;
            }
            _ => {}
        }
    }

    Ok(())
}

enum EventOutcome {
    Continue,
    Terminal,
    Permission(orbcode_protocol::PermissionRequest),
}

struct HeadlessRun {
    output_format: CliOutputFormat,
    verbose: bool,
    emitter: StreamJsonEmitter,
    allow_tools: bool,
    allow_network: bool,
    total_turn_count: u32,
    total_usage: TokenUsage,
    combined_assistant_text: String,
    last_error: Option<String>,
    last_error_category: Option<StreamErrorCategory>,
    stop_reason: Option<String>,
    was_cancelled: bool,
    budget_blocked: bool,
    permission_denied: bool,
    started: std::time::Instant,
    api_duration_ms: u64,
}

impl HeadlessRun {
    fn new(
        output_format: CliOutputFormat,
        verbose: bool,
        emitter: StreamJsonEmitter,
        allow_tools: bool,
        allow_network: bool,
    ) -> Self {
        Self {
            output_format,
            verbose,
            emitter,
            allow_tools,
            allow_network,
            total_turn_count: 0,
            total_usage: TokenUsage::default(),
            combined_assistant_text: String::new(),
            last_error: None,
            last_error_category: None,
            stop_reason: None,
            was_cancelled: false,
            budget_blocked: false,
            permission_denied: false,
            started: std::time::Instant::now(),
            api_duration_ms: 0,
        }
    }

    fn record_event(
        &mut self,
        event: &StreamEvent,
        assistant_text: &mut String,
        turn_started: std::time::Instant,
    ) -> Result<EventOutcome> {
        // Feed events to the emitter for both `stream-json` and `json`: the
        // emitter accumulates `modelUsage` and `permission_denials`, which the
        // final `json` result reads via `build_result`. Only `stream-json`
        // writes the per-event records; `json` emits a single final object.
        if matches!(
            self.output_format,
            CliOutputFormat::StreamJson | CliOutputFormat::Json
        ) {
            let records = self.emitter.process(event);
            if matches!(self.output_format, CliOutputFormat::StreamJson) {
                for record in records {
                    write_json_line(&record)?;
                }
            }
        }

        match event {
            StreamEvent::AssistantDelta { delta, .. } => {
                if matches!(self.output_format, CliOutputFormat::Text) {
                    print!("{delta}");
                    io::stdout().flush().ok();
                }
                assistant_text.push_str(delta);
                Ok(EventOutcome::Continue)
            }
            StreamEvent::AssistantMessageCompleted { .. } => {
                if matches!(self.output_format, CliOutputFormat::Text) {
                    println!();
                }
                Ok(EventOutcome::Continue)
            }
            StreamEvent::PermissionRequested { request } => {
                Ok(EventOutcome::Permission(request.clone()))
            }
            StreamEvent::TurnFinished { usage, .. } => {
                accumulate_usage(&mut self.total_usage, usage);
                self.stop_reason = Some("end_turn".to_string());
                self.api_duration_ms += turn_started.elapsed().as_millis() as u64;
                Ok(EventOutcome::Terminal)
            }
            StreamEvent::TurnCancelled { usage, .. } => {
                if let Some(usage) = usage {
                    accumulate_usage(&mut self.total_usage, usage);
                }
                self.was_cancelled = true;
                self.stop_reason = Some("cancelled".to_string());
                self.api_duration_ms += turn_started.elapsed().as_millis() as u64;
                Ok(EventOutcome::Terminal)
            }
            StreamEvent::ToolUseCompleted { kind, .. } => {
                if matches!(kind, ToolUseCompletionKind::PermissionDenied) {
                    self.permission_denied = true;
                }
                Ok(EventOutcome::Continue)
            }
            StreamEvent::Error {
                message, category, ..
            } => {
                self.last_error = Some(message.clone());
                self.last_error_category = *category;
                self.stop_reason = Some("error".to_string());
                self.api_duration_ms += turn_started.elapsed().as_millis() as u64;
                Ok(EventOutcome::Terminal)
            }
            StreamEvent::Budget {
                blocked,
                outcome,
                total_usd,
                max_budget_usd,
                ..
            } if *blocked => {
                self.budget_blocked = true;
                self.last_error = Some(format!(
                    "max budget reached ({}): counted ${total_usd:.4} of ${max_budget_usd:.4} cap",
                    outcome.as_str()
                ));
                self.stop_reason = Some("budget_exceeded".to_string());
                self.api_duration_ms += turn_started.elapsed().as_millis() as u64;
                Ok(EventOutcome::Terminal)
            }
            _ => Ok(EventOutcome::Continue),
        }
    }

    fn append_assistant_text(&mut self, text: &str) {
        if !self.combined_assistant_text.is_empty() {
            self.combined_assistant_text.push('\n');
        }
        self.combined_assistant_text.push_str(text);
    }

    async fn run_text_turn(
        &mut self,
        app_server: &AppServer,
        session_id: &str,
        prompt: String,
    ) -> Result<()> {
        self.total_turn_count += 1;
        let mut stream = app_server.submit_turn(session_id, prompt).await?;
        let turn_started = std::time::Instant::now();
        let mut assistant_text = String::new();

        while let Some(event) = stream.recv().await {
            match self.record_event(&event, &mut assistant_text, turn_started)? {
                EventOutcome::Continue => {}
                EventOutcome::Terminal => break,
                EventOutcome::Permission(request) => {
                    let approved = headless_permission_decision(
                        &request,
                        self.allow_tools,
                        self.allow_network,
                    );
                    let _ = app_server
                        .respond_to_permission_request(
                            &request.request_id,
                            if approved {
                                PermissionDecision::Approve
                            } else {
                                PermissionDecision::Deny
                            },
                        )
                        .await;
                    if self.verbose && matches!(self.output_format, CliOutputFormat::Text) {
                        eprintln!(
                            "{} tool request {}",
                            if approved { "approved" } else { "denied" },
                            request.summary()
                        );
                    }
                }
            }
        }

        self.append_assistant_text(&assistant_text);
        Ok(())
    }

    async fn run_control_input_loop(
        &mut self,
        app_server: &AppServer,
        client: &AppClient,
        session_id: &str,
    ) -> Result<()> {
        let (tx, mut control_rx) = tokio::sync::mpsc::unbounded_channel::<ControlFrame>();
        // Detached stdin reader; this loop owns shutdown by observing channel
        // closure and EOF frames.
        let _stdin_control_reader_handle = tokio::spawn(stdin_control_reader(tx));

        let mut pending_prompts: std::collections::VecDeque<String> =
            std::collections::VecDeque::new();
        let mut stdin_open = true;

        loop {
            let prompt = if let Some(prompt) = pending_prompts.pop_front() {
                prompt
            } else if !stdin_open {
                break;
            } else {
                let mut next_prompt = None;
                while stdin_open {
                    match control_rx.recv().await {
                        Some(frame) => {
                            if let Some(prompt) = self
                                .handle_idle_frame(frame, app_server, client, session_id)
                                .await?
                            {
                                next_prompt = Some(prompt);
                                break;
                            }
                        }
                        None => stdin_open = false,
                    }
                }
                match next_prompt {
                    Some(prompt) => prompt,
                    None => break,
                }
            };

            self.run_one_control_turn(
                app_server,
                client,
                session_id,
                prompt,
                &mut control_rx,
                &mut pending_prompts,
                &mut stdin_open,
            )
            .await?;

            if self.last_error.is_some() || self.was_cancelled {
                break;
            }
        }

        Ok(())
    }

    async fn handle_idle_frame(
        &mut self,
        frame: ControlFrame,
        app_server: &AppServer,
        client: &AppClient,
        session_id: &str,
    ) -> Result<Option<String>> {
        match frame {
            ControlFrame::UserPrompt(text) => Ok(Some(text)),
            ControlFrame::Interrupt { request_id } => {
                self.emit_control_response(control_response_success(&request_id))?;
                Ok(None)
            }
            ControlFrame::SetPermissionMode { request_id, mode } => {
                app_server.set_permission_mode(mode);
                self.apply_permission_mode(mode);
                self.emit_control_response(control_response_success(&request_id))?;
                Ok(None)
            }
            ControlFrame::GetSessionState { request_id } => {
                let data = self.build_session_state_response(client, session_id).await;
                self.emit_control_response(control_response_success_with_data(&request_id, data))?;
                Ok(None)
            }
            ControlFrame::GetContextUsage { request_id } => {
                let data = self.build_context_usage_response(client, session_id).await;
                self.emit_control_response(control_response_success_with_data(&request_id, data))?;
                Ok(None)
            }
            ControlFrame::SetMaxThinkingTokens { request_id, .. } => {
                self.emit_control_response(max_thinking_tokens_unsupported(&request_id))?;
                Ok(None)
            }
            ControlFrame::Unsupported {
                request_id,
                subtype,
            } => {
                self.emit_control_response(control_response_error(
                    &request_id,
                    &format!("unsupported control request subtype `{subtype}`"),
                ))?;
                Ok(None)
            }
            ControlFrame::Ignore => Ok(None),
            ControlFrame::ParseError {
                request_id,
                message,
            } => {
                self.report_parse_error(request_id, message)?;
                Ok(None)
            }
        }
    }

    async fn run_one_control_turn(
        &mut self,
        app_server: &AppServer,
        client: &AppClient,
        session_id: &str,
        prompt: String,
        control_rx: &mut tokio::sync::mpsc::UnboundedReceiver<ControlFrame>,
        pending_prompts: &mut std::collections::VecDeque<String>,
        stdin_open: &mut bool,
    ) -> Result<()> {
        self.total_turn_count += 1;
        let mut stream = app_server.submit_turn(session_id, prompt).await?;
        let turn_started = std::time::Instant::now();
        let mut assistant_text = String::new();
        let mut held_denials: Vec<String> = Vec::new();

        loop {
            if *stdin_open {
                tokio::select! {
                    maybe_event = stream.recv() => {
                        match maybe_event {
                            Some(event) => {
                                if self
                                    .handle_control_turn_event(
                                        &event,
                                        &mut assistant_text,
                                        turn_started,
                                        app_server,
                                        &mut held_denials,
                                        true,
                                    )
                                    .await?
                                {
                                    break;
                                }
                            }
                            None => break,
                        }
                    }
                    maybe_frame = control_rx.recv() => {
                        match maybe_frame {
                            Some(frame) => {
                                self.handle_mid_turn_frame(
                                    frame,
                                    app_server,
                                    client,
                                    session_id,
                                    pending_prompts,
                                )
                                .await?;
                            }
                            None => {
                                *stdin_open = false;
                                for request_id in held_denials.drain(..) {
                                    let _ = app_server
                                        .respond_to_permission_request(
                                            &request_id,
                                            PermissionDecision::Deny,
                                        )
                                        .await;
                                }
                            }
                        }
                    }
                }
            } else {
                match stream.recv().await {
                    Some(event) => {
                        if self
                            .handle_control_turn_event(
                                &event,
                                &mut assistant_text,
                                turn_started,
                                app_server,
                                &mut held_denials,
                                false,
                            )
                            .await?
                        {
                            break;
                        }
                    }
                    None => break,
                }
            }
        }

        self.append_assistant_text(&assistant_text);
        Ok(())
    }

    async fn handle_control_turn_event(
        &mut self,
        event: &StreamEvent,
        assistant_text: &mut String,
        turn_started: std::time::Instant,
        app_server: &AppServer,
        held_denials: &mut Vec<String>,
        hold_denials: bool,
    ) -> Result<bool> {
        match self.record_event(event, assistant_text, turn_started)? {
            EventOutcome::Continue => Ok(false),
            EventOutcome::Terminal => Ok(true),
            EventOutcome::Permission(request) => {
                let approved =
                    headless_permission_decision(&request, self.allow_tools, self.allow_network);
                if approved {
                    let _ = app_server
                        .respond_to_permission_request(
                            &request.request_id,
                            PermissionDecision::Approve,
                        )
                        .await;
                } else if hold_denials {
                    held_denials.push(request.request_id);
                } else {
                    let _ = app_server
                        .respond_to_permission_request(
                            &request.request_id,
                            PermissionDecision::Deny,
                        )
                        .await;
                }
                Ok(false)
            }
        }
    }

    async fn handle_mid_turn_frame(
        &mut self,
        frame: ControlFrame,
        app_server: &AppServer,
        client: &AppClient,
        session_id: &str,
        pending_prompts: &mut std::collections::VecDeque<String>,
    ) -> Result<()> {
        match frame {
            ControlFrame::Interrupt { request_id } => {
                let _ = client.interrupt_turn(session_id).await;
                self.emit_control_response(control_response_success(&request_id))?;
            }
            ControlFrame::SetPermissionMode { request_id, mode } => {
                app_server.set_permission_mode(mode);
                self.apply_permission_mode(mode);
                self.emit_control_response(control_response_success(&request_id))?;
            }
            ControlFrame::GetSessionState { request_id } => {
                let data = self.build_session_state_response(client, session_id).await;
                self.emit_control_response(control_response_success_with_data(&request_id, data))?;
            }
            ControlFrame::GetContextUsage { request_id } => {
                let data = self.build_context_usage_response(client, session_id).await;
                self.emit_control_response(control_response_success_with_data(&request_id, data))?;
            }
            ControlFrame::SetMaxThinkingTokens { request_id, .. } => {
                self.emit_control_response(max_thinking_tokens_unsupported(&request_id))?;
            }
            ControlFrame::UserPrompt(text) => pending_prompts.push_back(text),
            ControlFrame::Unsupported {
                request_id,
                subtype,
            } => {
                self.emit_control_response(control_response_error(
                    &request_id,
                    &format!("unsupported control request subtype `{subtype}`"),
                ))?;
            }
            ControlFrame::Ignore => {}
            ControlFrame::ParseError {
                request_id,
                message,
            } => self.report_parse_error(request_id, message)?,
        }
        Ok(())
    }

    async fn build_session_state_response(
        &self,
        client: &AppClient,
        session_id: &str,
    ) -> serde_json::Value {
        match client.status_overview(session_id).await {
            Ok(overview) => {
                // Extract fields from the protocol Value response.
                serde_json::to_value(SessionStateResponse {
                    session_id: overview["session_id"].as_str().unwrap_or("").to_string(),
                    cwd: overview["cwd"].as_str().unwrap_or("").to_string(),
                    model_display_name: overview["model_display_name"]
                        .as_str()
                        .unwrap_or("")
                        .to_string(),
                    model_name: overview["model_name"].as_str().unwrap_or("").to_string(),
                    model_capabilities: overview["model_capabilities"]
                        .as_array()
                        .map(|arr| {
                            arr.iter()
                                .filter_map(|v| v.as_str().map(String::from))
                                .collect()
                        })
                        .unwrap_or_default(),
                    effort_level: overview["effort_level"].as_str().map(|s| match s {
                        "low" => "low",
                        "medium" => "medium",
                        _ => "high",
                    }),
                    default_provider: overview["default_provider"]
                        .as_str()
                        .unwrap_or("")
                        .to_string(),
                    fallback_provider: overview["fallback_provider"].as_str().map(String::from),
                    sandbox_mode: overview["sandbox_mode"].as_str().unwrap_or("").to_string(),
                    persisted_session_count: overview["persisted_session_count"]
                        .as_u64()
                        .unwrap_or(0) as usize,
                    background_job_count: overview["background_job_count"].as_u64().unwrap_or(0)
                        as usize,
                    available_tool_count: overview["available_tool_count"].as_u64().unwrap_or(0)
                        as usize,
                    configured_mcp_server_count: overview["configured_mcp_server_count"]
                        .as_u64()
                        .unwrap_or(0) as usize,
                })
                .expect("SessionStateResponse serialization is infallible")
            }
            Err(e) => serde_json::to_value(ErrorResponse {
                error: e.to_string(),
            })
            .expect("ErrorResponse serialization is infallible"),
        }
    }

    async fn build_context_usage_response(
        &self,
        client: &AppClient,
        session_id: &str,
    ) -> serde_json::Value {
        match client.context_overview(session_id).await {
            Ok(overview) => {
                // The protocol response wraps usage under "context".
                let ctx = &overview["context"];
                let usage = if ctx.is_null() { &overview } else { ctx };
                let cats = &usage["categories"];
                serde_json::to_value(ContextUsageResponse {
                    model: usage["model"].as_str().unwrap_or("").to_string(),
                    estimated_tokens: usage["estimated_tokens"].as_u64().unwrap_or(0) as u32,
                    categories: ContextCategoriesResponse {
                        system_prompt: cats["system_prompt"].as_u64().unwrap_or(0) as u32,
                        system_tools: cats["system_tools"].as_u64().unwrap_or(0) as u32,
                        mcp_tools: cats["mcp_tools"].as_u64().unwrap_or(0) as u32,
                        memory: cats["memory"].as_u64().unwrap_or(0) as u32,
                        skills: cats["skills"].as_u64().unwrap_or(0) as u32,
                        conversation: cats["conversation"].as_u64().unwrap_or(0) as u32,
                        attachments: cats["attachments"].as_u64().unwrap_or(0) as u32,
                        uncategorized: cats["uncategorized"].as_u64().unwrap_or(0) as u32,
                    },
                    context_window: usage["context_window"].as_u64().unwrap_or(0) as u32,
                    effective_context_window: usage["effective_context_window"]
                        .as_u64()
                        .unwrap_or(0) as u32,
                    free_space_tokens: usage["free_space_tokens"].as_u64().unwrap_or(0) as u32,
                    percent_left: usage["percent_left"].as_u64().unwrap_or(0) as u32,
                    is_above_auto_compact_threshold: usage["is_above_auto_compact_threshold"]
                        .as_bool()
                        .unwrap_or(false),
                    is_above_warning_threshold: usage["is_above_warning_threshold"]
                        .as_bool()
                        .unwrap_or(false),
                    is_above_error_threshold: usage["is_above_error_threshold"]
                        .as_bool()
                        .unwrap_or(false),
                    is_at_blocking_limit: usage["is_at_blocking_limit"].as_bool().unwrap_or(false),
                })
                .expect("ContextUsageResponse serialization is infallible")
            }
            Err(e) => serde_json::to_value(ErrorResponse {
                error: e.to_string(),
            })
            .expect("ErrorResponse serialization is infallible"),
        }
    }

    fn apply_permission_mode(&mut self, mode: PermissionMode) {
        self.allow_tools = matches!(
            mode,
            PermissionMode::AcceptEdits
                | PermissionMode::BypassPermissions
                | PermissionMode::DontAsk
                | PermissionMode::Auto
        );
        self.allow_network = matches!(
            mode,
            PermissionMode::BypassPermissions | PermissionMode::DontAsk
        );
    }

    fn emit_control_response(&self, response: serde_json::Value) -> Result<()> {
        write_json_line(&response)
    }

    fn report_parse_error(&self, request_id: Option<String>, message: String) -> Result<()> {
        match request_id {
            Some(request_id) => {
                self.emit_control_response(control_response_error(&request_id, &message))
            }
            None => {
                eprintln!("warning: {message}");
                Ok(())
            }
        }
    }

    fn finalize(&mut self, cost: &CostFields) -> (HeadlessOutcome, serde_json::Value) {
        let duration_ms = self.started.elapsed().as_millis() as u64;
        let permission_denied_terminal =
            self.permission_denied && self.combined_assistant_text.trim().is_empty();
        let outcome = if self.budget_blocked {
            HeadlessOutcome::MaxBudget
        } else {
            classify_outcome(
                self.last_error_category,
                self.last_error.is_some(),
                self.was_cancelled,
                permission_denied_terminal,
            )
        };
        let is_error = outcome.is_error();
        let subtype = outcome.result_subtype();

        let mut errors_vec: Vec<String> = self.last_error.iter().cloned().collect();
        if errors_vec.is_empty() {
            match outcome {
                HeadlessOutcome::PermissionDenied => {
                    errors_vec.push("tool call denied by permission policy".to_string());
                }
                HeadlessOutcome::Cancelled => {
                    errors_vec.push("turn cancelled".to_string());
                }
                _ => {}
            }
        }
        let result_text = (!is_error).then_some(self.combined_assistant_text.as_str());
        let result_payload = self.emitter.build_result(
            subtype,
            is_error,
            duration_ms,
            self.api_duration_ms,
            self.total_turn_count,
            &self.total_usage,
            cost,
            self.stop_reason.as_deref(),
            result_text,
            &errors_vec,
        );
        (outcome, result_payload)
    }
}

fn max_thinking_tokens_unsupported(request_id: &str) -> serde_json::Value {
    control_response_error(
        request_id,
        "unsupported control request subtype `set_max_thinking_tokens`: no runtime thinking-token override is currently wired",
    )
}

pub(crate) fn write_json_line(value: &serde_json::Value) -> Result<()> {
    let mut stdout = io::stdout().lock();
    serde_json::to_writer(&mut stdout, value)?;
    stdout.write_all(b"\n")?;
    stdout.flush().ok();
    Ok(())
}

fn accumulate_usage(total: &mut TokenUsage, delta: &TokenUsage) {
    total.input_tokens = total.input_tokens.saturating_add(delta.input_tokens);
    total.output_tokens = total.output_tokens.saturating_add(delta.output_tokens);
    total.cache_creation_input_tokens = total
        .cache_creation_input_tokens
        .saturating_add(delta.cache_creation_input_tokens);
    total.cache_read_input_tokens = total
        .cache_read_input_tokens
        .saturating_add(delta.cache_read_input_tokens);
    total.total_tokens = total.total_tokens.saturating_add(delta.total_tokens);
    total.server_tool_use.web_search_requests = total
        .server_tool_use
        .web_search_requests
        .saturating_add(delta.server_tool_use.web_search_requests);
    total.server_tool_use.web_fetch_requests = total
        .server_tool_use
        .web_fetch_requests
        .saturating_add(delta.server_tool_use.web_fetch_requests);
}

pub(crate) async fn build_cost_fields(client: &AppClient, session_id: &str) -> CostFields {
    match client.cost_overview(session_id).await {
        Ok(overview) => {
            let total_cost_usd = overview["cost"]["total_cost_usd"].as_f64().unwrap_or(0.0);
            let has_unknown = overview["cost"]["has_unknown_model_cost"]
                .as_bool()
                .unwrap_or(true);
            let is_api_priced = overview["cost"]["billing_basis"]
                .as_str()
                .is_none_or(|basis| basis == "api");
            CostFields {
                total_cost_usd,
                pricing_known: !has_unknown && is_api_priced,
                model_costs: None,
            }
        }
        Err(_) => CostFields::default(),
    }
}

pub(crate) async fn build_init_metadata(
    client: &AppClient,
    bootstrap: &orbcode_app_server::BootstrapState,
) -> InitMetadata {
    let tool_names: Vec<String> = client
        .list_tools()
        .await
        .ok()
        .and_then(|v| v.as_array().cloned())
        .unwrap_or_default()
        .into_iter()
        .filter_map(|t| t["name"].as_str().map(String::from))
        .collect();
    let mcp_servers: Vec<McpServerInfo> = client
        .list_mcp_servers()
        .await
        .ok()
        .and_then(|v| v.as_array().cloned())
        .unwrap_or_default()
        .iter()
        .map(|s| McpServerInfo {
            name: s["id"].as_str().unwrap_or("").to_string(),
            status: s["status"].as_str().unwrap_or("").to_string(),
        })
        .collect();
    let permission_mode_value = client.permission_mode().await.ok();
    let permission_mode_str = permission_mode_value
        .as_ref()
        .and_then(|v| v["mode"].as_str())
        .unwrap_or("default");
    let permission_mode = match permission_mode_str {
        "acceptEdits" => PermissionMode::AcceptEdits,
        "bypassPermissions" => PermissionMode::BypassPermissions,
        "plan" => PermissionMode::Plan,
        _ => PermissionMode::Default,
    };
    InitMetadata {
        session_id: bootstrap.session.session_id.clone(),
        cwd: bootstrap.cwd.display().to_string(),
        model: bootstrap.model_display_name.clone(),
        tool_names,
        mcp_servers,
        permission_mode,
    }
}

async fn stdin_control_reader(tx: tokio::sync::mpsc::UnboundedSender<ControlFrame>) {
    use tokio::io::{AsyncBufReadExt, BufReader};

    let stdin = tokio::io::stdin();
    let mut lines = BufReader::new(stdin).lines();
    loop {
        match lines.next_line().await {
            Ok(Some(line)) => {
                let frame = parse_control_line(&line);
                if matches!(frame, ControlFrame::Ignore) {
                    continue;
                }
                if tx.send(frame).is_err() {
                    break;
                }
            }
            Ok(None) => break,
            Err(error) => {
                let _ = tx.send(ControlFrame::ParseError {
                    request_id: None,
                    message: format!("error reading stdin: {error}"),
                });
                break;
            }
        }
    }
}

pub(crate) fn headless_permission_decision(
    request: &orbcode_protocol::PermissionRequest,
    allow_tools: bool,
    allow_network: bool,
) -> bool {
    (!request.requires_tools_permission || allow_tools)
        && (!request.requires_network_permission || allow_network)
}

pub(crate) fn context_compacted_log_line(
    duration_ms: u64,
    original_message_count: usize,
    compacted_message_count: usize,
    provider_generated: bool,
    fallback_reason: Option<&str>,
) -> String {
    let source = if provider_generated {
        "provider summary"
    } else {
        "local fallback"
    };
    let mut line = format!(
        "context compacted in {duration_ms} ms ({original_message_count} -> {compacted_message_count} messages, {source})"
    );
    if let Some(reason) = fallback_reason
        .map(str::trim)
        .filter(|reason| !reason.is_empty())
    {
        line.push_str(": ");
        line.push_str(reason);
    }
    line
}
