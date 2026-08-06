use std::{
    collections::HashMap,
    io::{self, Write},
    sync::{Arc, Mutex, mpsc as std_mpsc},
};

use anyhow::Result;
use orbcode_app_server::AppServer;
use orbcode_app_server_client::PermissionMode;
use orbcode_app_server_client::{AppClient, PermissionDecision, PermissionDecisionWire};
use orbcode_protocol::{
    AskUserCancellationReason, AskUserQuestionSpec, AskUserResponseOutcome,
    AsyncCancellationOutcome, ControlRequest, ControlRequestEnvelope, SUPPORTED_CONTROL_SUBTYPES,
    SdkAsyncCancellationResponse, SdkContextCategoriesResponse, SdkContextUsageResponse,
    SdkInitializeResponse, SdkMcpServerStatus, SdkMcpStatusResponse, SdkModelChangeResponse,
    SdkSeedReadStateResponse, SdkSessionStateResponse, SdkThinkingBudgetResponse,
    StreamErrorCategory, StreamEvent, TokenUsage, ToolPermissionResult, ToolUseCompletionKind,
    validate_ask_user_outcome,
};
use serde::Serialize;

use crate::args::{CliInputFormat, CliOutputFormat};
use crate::control::{ControlFrame, parse_control_line};
use crate::exit_code::{HeadlessOutcome, classify_outcome};
use crate::stream_json::{
    CostFields, InitMetadata, McpServerInfo, StreamJsonEmitter, control_response_error,
    control_response_success,
};

struct JsonWriteCommand {
    sequence: u64,
    value: serde_json::Value,
    completion: std_mpsc::SyncSender<std::result::Result<(), String>>,
}

/// All duplex stdout records pass through one sequence-aware writer worker.
#[derive(Clone)]
struct OrderedJsonWriter {
    gate: Arc<Mutex<u64>>,
    sender: std_mpsc::Sender<JsonWriteCommand>,
}

impl Default for OrderedJsonWriter {
    fn default() -> Self {
        let (sender, receiver) = std_mpsc::channel::<JsonWriteCommand>();
        std::thread::Builder::new()
            .name("orbcode-stream-json-writer".to_string())
            .spawn(move || {
                let mut expected_sequence = 0_u64;
                let mut terminal_error: Option<String> = None;
                while let Ok(command) = receiver.recv() {
                    let result = if command.sequence != expected_sequence {
                        Err(format!(
                            "stdout sequence mismatch: expected {expected_sequence}, got {}",
                            command.sequence
                        ))
                    } else if let Some(error) = terminal_error.as_ref() {
                        Err(error.clone())
                    } else {
                        write_json_line(&command.value).map_err(|error| error.to_string())
                    };
                    expected_sequence = command.sequence.saturating_add(1);
                    if let Err(error) = &result {
                        terminal_error.get_or_insert_with(|| error.clone());
                    }
                    let _ = command.completion.send(result);
                }
            })
            .expect("spawn stream-json writer worker");
        Self {
            gate: Arc::new(Mutex::new(0)),
            sender,
        }
    }
}

impl OrderedJsonWriter {
    fn write(&self, value: &serde_json::Value) -> Result<()> {
        let mut next_sequence = self
            .gate
            .lock()
            .map_err(|_| anyhow::anyhow!("stdout writer lock poisoned"))?;
        let sequence = *next_sequence;
        *next_sequence = next_sequence.saturating_add(1);
        let (completion, completed) = std_mpsc::sync_channel(1);
        self.sender
            .send(JsonWriteCommand {
                sequence,
                value: value.clone(),
                completion,
            })
            .map_err(|_| anyhow::anyhow!("stdout writer worker stopped"))?;
        completed
            .recv()
            .map_err(|_| anyhow::anyhow!("stdout writer worker stopped"))?
            .map_err(anyhow::Error::msg)
    }
}

pub(crate) async fn run_headless_prompt(
    client: &AppClient,
    session: Option<String>,
    prompt: String,
) -> Result<String> {
    let bootstrap = client.bootstrap(session.as_deref()).await?;
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

    let mut stream = client
        .submit_turn_stream(&bootstrap.session.session_id, prompt)
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
                        let _ = client
                            .respond_to_permission_request(
                                &request.request_id,
                                if approved {
                                    PermissionDecisionWire::Approve
                                } else {
                                    PermissionDecisionWire::Deny
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
                                orbcode_protocol::ToolUseCompletionKind::Cancelled => {
                                    "was cancelled"
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
    client: &AppClient,
    session: Option<String>,
    positional_prompt: Option<String>,
    output_format: CliOutputFormat,
    input_format: CliInputFormat,
    verbose: bool,
) -> Result<()> {
    let bootstrap = client.bootstrap(session.as_deref()).await?;
    let session_id = bootstrap.session.session_id.clone();
    let model_name = client
        .status_overview_typed(&session_id)
        .await
        .ok()
        .map(|result| result.model_name)
        .unwrap_or_else(|| "unknown".to_string());
    let mut run = HeadlessRun::new(
        output_format,
        verbose,
        StreamJsonEmitter::new(session_id.clone(), model_name),
        bootstrap.permissions.allow_tools,
        bootstrap.permissions.allow_network,
    );
    if let Ok(result) = client.permission_mode().await {
        run.apply_permission_mode(result.mode);
    }

    if matches!(output_format, CliOutputFormat::StreamJson) {
        let meta = build_init_metadata(client, &bootstrap).await;
        let init = run.emitter.build_system_init(&meta);
        run.write_json(&init)?;
    }

    match input_format {
        CliInputFormat::Text => {
            let prompt = match positional_prompt {
                Some(prompt) => prompt,
                None => super::invalid_cli_input(
                    "--print requires a prompt or --input-format stream-json",
                ),
            };
            run.run_text_turn(client, &session_id, prompt).await?;
        }
        CliInputFormat::StreamJson => {
            run.run_control_input_loop(client, &session_id).await?;
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
            run.write_json(&result_payload)?;
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
    // Explicit boundary exception: this is the private worker subprocess, not
    // the duplex SDK runner. Durable background-job lifecycle, event-file, and
    // log mutations below stay on AppServer until those worker-only operations
    // gain protocol DTOs. Turn, permissions, settings, and introspection cross
    // AppClient.
    #[cfg(unix)]
    let mut sigterm = tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate())?;
    let mut received_signal: Option<i32> = None;

    let permissions = client.permission_overview().await?.permissions;
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
        .status_overview_typed(&session_id)
        .await
        .ok()
        .map(|result| result.model_name)
        .unwrap_or_else(|| "unknown".to_string());
    let mut emitter = StreamJsonEmitter::new(session_id.clone(), model_name.clone());
    let started = std::time::Instant::now();
    {
        let tool_names = client.list_tool_names().await.unwrap_or_default();
        let mcp_servers = client
            .mcp_status()
            .await
            .unwrap_or_default()
            .into_iter()
            .map(|server| McpServerInfo {
                name: server.id,
                status: server.status,
            })
            .collect();
        let permission_mode = client
            .permission_mode()
            .await
            .map_or(PermissionMode::Default, |result| result.mode);
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

    let mut stream = match client.submit_turn_stream(&session_id, prompt.clone()).await {
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
                let _ = client
                    .respond_to_permission_request(
                        &request.request_id,
                        if approved {
                            PermissionDecisionWire::Approve
                        } else {
                            PermissionDecisionWire::Deny
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
                                orbcode_protocol::ToolUseCompletionKind::Cancelled => {
                                    "was cancelled"
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

enum ControlAction {
    None,
    Prompt(String),
}

// This hot-path value lives only across one `select!`; boxing every large
// StreamEvent would add an allocation to every streamed delta.
#[allow(clippy::large_enum_variant)]
enum TurnInput {
    Control(Option<ControlFrame>),
    Stream(Option<StreamEvent>),
}

/// Deterministically alternate priority when both a stdin control and turn
/// event are ready. A control flood therefore cannot starve provider events,
/// while controls (including interrupt and queued prompts) still make progress.
async fn next_turn_input(
    control_rx: &mut tokio::sync::mpsc::UnboundedReceiver<ControlFrame>,
    stream: &mut tokio::sync::mpsc::UnboundedReceiver<StreamEvent>,
    stdin_open: bool,
    prefer_control: bool,
) -> TurnInput {
    if !stdin_open {
        return TurnInput::Stream(stream.recv().await);
    }
    if prefer_control {
        tokio::select! {
            biased;
            frame = control_rx.recv() => TurnInput::Control(frame),
            event = stream.recv() => TurnInput::Stream(event),
        }
    } else {
        tokio::select! {
            biased;
            event = stream.recv() => TurnInput::Stream(event),
            frame = control_rx.recv() => TurnInput::Control(frame),
        }
    }
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
    writer: OrderedJsonWriter,
    permission_mode: PermissionMode,
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
            writer: OrderedJsonWriter::default(),
            permission_mode: PermissionMode::Default,
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
                    self.writer.write(&record)?;
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
        client: &AppClient,
        session_id: &str,
        prompt: String,
    ) -> Result<()> {
        self.total_turn_count += 1;
        let mut stream = client.submit_turn_stream(session_id, prompt).await?;
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
                    let _ = client
                        .respond_to_permission_request(
                            &request.request_id,
                            if approved {
                                PermissionDecisionWire::Approve
                            } else {
                                PermissionDecisionWire::Deny
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

    async fn run_control_input_loop(&mut self, client: &AppClient, session_id: &str) -> Result<()> {
        let (tx, mut control_rx) = tokio::sync::mpsc::unbounded_channel::<ControlFrame>();
        // Detached stdin reader; this loop owns shutdown by observing channel
        // closure and EOF frames.
        let _stdin_control_reader_handle = tokio::spawn(stdin_control_reader(tx));

        let mut pending_prompts: std::collections::VecDeque<String> =
            std::collections::VecDeque::new();
        let mut pending_permissions = std::collections::HashMap::<String, String>::new();
        let mut pending_asks = HashMap::<String, Vec<AskUserQuestionSpec>>::new();
        let mut server_request_rx =
            client.take_server_request_receiver().await.ok_or_else(|| {
                anyhow::anyhow!("headless control server-request receiver unavailable")
            })?;
        let mut server_requests_open = true;
        let mut stdin_open = true;

        let result = async {
            loop {
            let prompt = if let Some(prompt) = pending_prompts.pop_front() {
                prompt
            } else if !stdin_open {
                break;
            } else {
                let mut next_prompt = None;
                while stdin_open {
                    tokio::select! {
                        biased;
                        maybe_request = server_request_rx.recv(), if server_requests_open => {
                            match maybe_request {
                                Some(request) => self
                                    .handle_server_request(
                                        request,
                                        client,
                                        session_id,
                                        &mut pending_permissions,
                                        &mut pending_asks,
                                        true,
                                    )
                                    .await?,
                                None => server_requests_open = false,
                            }
                        }
                        maybe_frame = control_rx.recv() => {
                            match maybe_frame {
                                Some(frame) => match self
                                    .dispatch_control_frame(
                                        frame,
                                        client,
                                        session_id,
                                        &mut pending_permissions,
                                        &mut pending_asks,
                                    )
                                    .await?
                                {
                                    ControlAction::Prompt(prompt) => {
                                        next_prompt = Some(prompt);
                                        break;
                                    }
                                    ControlAction::None => {}
                                },
                                None => {
                                    stdin_open = false;
                                    self.deny_pending_permissions(client, &mut pending_permissions)
                                        .await;
                                }
                            }
                        }
                    }
                }
                match next_prompt {
                    Some(prompt) => prompt,
                    None => break,
                }
            };

            self.run_one_control_turn(
                client,
                session_id,
                prompt,
                &mut control_rx,
                &mut pending_prompts,
                &mut pending_permissions,
                &mut pending_asks,
                &mut stdin_open,
                &mut server_request_rx,
                &mut server_requests_open,
            )
            .await?;

            if self.last_error.is_some() || self.was_cancelled {
                break;
            }
            }

            Ok(())
        }
        .await;

        self.deny_pending_permissions(client, &mut pending_permissions)
            .await;
        cancel_pending_asks(client, &mut pending_asks).await;
        result
    }

    async fn dispatch_control_frame(
        &mut self,
        frame: ControlFrame,
        client: &AppClient,
        session_id: &str,
        pending_permissions: &mut std::collections::HashMap<String, String>,
        pending_asks: &mut HashMap<String, Vec<AskUserQuestionSpec>>,
    ) -> Result<ControlAction> {
        match frame {
            ControlFrame::UserPrompt(text) => Ok(ControlAction::Prompt(text)),
            ControlFrame::Initialize { request_id } => {
                match self.build_initialize_response(client, session_id).await {
                    Ok(data) => self.emit_typed_control_success(&request_id, &data)?,
                    Err(error) => {
                        self.emit_control_response(control_response_error(&request_id, &error))?
                    }
                }
                Ok(ControlAction::None)
            }
            ControlFrame::Interrupt { request_id } => {
                let _ = client.interrupt_turn(session_id).await;
                self.emit_control_response(control_response_success(&request_id))?;
                Ok(ControlAction::None)
            }
            ControlFrame::SetPermissionMode { request_id, mode } => {
                match client.set_permission_mode(mode).await {
                    Ok(_) => {
                        self.apply_permission_mode(mode);
                        self.emit_control_response(control_response_success(&request_id))?;
                    }
                    Err(error) => self.emit_control_response(control_response_error(
                        &request_id,
                        &error.to_string(),
                    ))?,
                }
                Ok(ControlAction::None)
            }
            ControlFrame::GetSessionState { request_id } => {
                match self.build_session_state_response(client, session_id).await {
                    Ok(data) => self.emit_typed_control_success(&request_id, &data)?,
                    Err(error) => {
                        self.emit_control_response(control_response_error(&request_id, &error))?
                    }
                }
                Ok(ControlAction::None)
            }
            ControlFrame::GetContextUsage { request_id } => {
                match self.build_context_usage_response(client, session_id).await {
                    Ok(data) => self.emit_typed_control_success(&request_id, &data)?,
                    Err(error) => {
                        self.emit_control_response(control_response_error(&request_id, &error))?
                    }
                }
                Ok(ControlAction::None)
            }
            ControlFrame::McpStatus { request_id } => {
                match self.build_mcp_status_response(client).await {
                    Ok(data) => self.emit_typed_control_success(&request_id, &data)?,
                    Err(error) => {
                        self.emit_control_response(control_response_error(&request_id, &error))?
                    }
                }
                Ok(ControlAction::None)
            }
            ControlFrame::SetModel { request_id, model } => {
                match client.set_session_model(session_id, model).await {
                    Ok(result) => self.emit_typed_control_success(
                        &request_id,
                        &SdkModelChangeResponse {
                            provider: result.model_selection.provider.as_str().to_string(),
                            model: result.model_selection.resolution.request_model,
                            display_name: result.model_selection.resolution.display_name,
                        },
                    )?,
                    Err(error) => self.emit_control_response(control_response_error(
                        &request_id,
                        &error.to_string(),
                    ))?,
                }
                Ok(ControlAction::None)
            }
            ControlFrame::SetMaxThinkingTokens {
                request_id,
                max_thinking_tokens,
            } => {
                match client
                    .set_max_thinking_tokens(session_id, max_thinking_tokens)
                    .await
                {
                    Ok(result) => self.emit_typed_control_success(
                        &request_id,
                        &SdkThinkingBudgetResponse {
                            max_thinking_tokens: result.max_thinking_tokens,
                        },
                    )?,
                    Err(error) => self.emit_control_response(control_response_error(
                        &request_id,
                        &error.to_string(),
                    ))?,
                }
                Ok(ControlAction::None)
            }
            ControlFrame::SeedReadState {
                request_id,
                path,
                mtime,
            } => {
                match client.seed_read_state(session_id, &path, mtime).await {
                    Ok(result) => self.emit_typed_control_success(
                        &request_id,
                        &SdkSeedReadStateResponse {
                            path: result.path.to_string_lossy().into_owned(),
                            mtime: result.mtime,
                            seeded: result.seeded,
                        },
                    )?,
                    Err(error) => self.emit_control_response(control_response_error(
                        &request_id,
                        &error.to_string(),
                    ))?,
                }
                Ok(ControlAction::None)
            }
            ControlFrame::RewindFiles { request_id } => {
                self.emit_control_response(control_response_error(
                    &request_id,
                    "unsupported control request subtype `rewind_files`: Orbcode has no file checkpoint contract; transcript rewind is intentionally not substituted",
                ))?;
                Ok(ControlAction::None)
            }
            ControlFrame::CancelAsyncMessage {
                request_id,
                message_uuid,
            } => {
                match client.cancel_async_task(session_id, &message_uuid).await {
                    Ok(result) => {
                        let outcome = match result.outcome {
                            orbcode_app_server_client::AsyncCancellationResultKind::Signalled => {
                                AsyncCancellationOutcome::Signalled
                            }
                            orbcode_app_server_client::AsyncCancellationResultKind::AlreadyTerminal => {
                                AsyncCancellationOutcome::AlreadyTerminal
                            }
                            orbcode_app_server_client::AsyncCancellationResultKind::NotFound => {
                                AsyncCancellationOutcome::NotFound
                            }
                        };
                        self.emit_typed_control_success(
                            &request_id,
                            &SdkAsyncCancellationResponse {
                                task_id: result.task_id,
                                outcome,
                                cancelled: matches!(outcome, AsyncCancellationOutcome::Signalled),
                            },
                        )?;
                    }
                    Err(error) => self.emit_control_response(control_response_error(
                        &request_id,
                        &error.to_string(),
                    ))?,
                }
                Ok(ControlAction::None)
            }
            ControlFrame::AskUserResponse {
                request_id,
                outcome,
            } => {
                let Some(questions) = pending_asks.get(&request_id) else {
                    self.emit_control_response(control_response_error(
                        &request_id,
                        "unknown, stale, or duplicate server request id",
                    ))?;
                    return Ok(ControlAction::None);
                };
                if let Err(error) = validate_ask_user_outcome(questions, &outcome) {
                    self.emit_control_response(control_response_error(
                        &request_id,
                        &format!("invalid AskUserQuestion response: {error}"),
                    ))?;
                    return Ok(ControlAction::None);
                }
                if client
                    .respond_to_ask_user_question_outcome(&request_id, outcome)
                    .await
                {
                    pending_asks.remove(&request_id);
                    self.emit_control_response(control_response_success(&request_id))?;
                } else {
                    self.emit_control_response(control_response_error(
                        &request_id,
                        "unknown, stale, or duplicate server request id",
                    ))?;
                }
                Ok(ControlAction::None)
            }
            ControlFrame::ServerResponse { request_id, result } => {
                let Some(expected_tool_use_id) = pending_permissions.get(&request_id).cloned()
                else {
                    self.emit_control_response(control_response_error(
                        &request_id,
                        "duplicate or late can_use_tool response",
                    ))?;
                    return Ok(ControlAction::None);
                };
                let permission_result = match result {
                    Ok(value) => match serde_json::from_value::<ToolPermissionResult>(value) {
                        Ok(result) => Ok(result),
                        Err(error) => {
                            self.emit_control_response(control_response_error(
                                &request_id,
                                &format!("invalid can_use_tool response: {error}"),
                            ))?;
                            return Ok(ControlAction::None);
                        }
                    },
                    Err(error) => Err(error),
                };
                let (decision, interrupt, returned_tool_use_id) = match permission_result {
                    Ok(ToolPermissionResult::Allow { tool_use_id, .. }) => {
                        (PermissionDecision::Approve, false, tool_use_id)
                    }
                    Ok(ToolPermissionResult::Deny {
                        interrupt,
                        tool_use_id,
                        ..
                    }) => (PermissionDecision::Deny, interrupt, tool_use_id),
                    Err(_) => (PermissionDecision::Deny, false, None),
                };
                if returned_tool_use_id
                    .as_deref()
                    .is_some_and(|id| id != expected_tool_use_id)
                {
                    self.emit_control_response(control_response_error(
                        &request_id,
                        "can_use_tool response toolUseID does not match the pending request",
                    ))?;
                    return Ok(ControlAction::None);
                }
                if client
                    .respond_to_pending_permission_request(&request_id, decision)
                    .await
                {
                    pending_permissions.remove(&request_id);
                    if interrupt {
                        let _ = client.interrupt_turn(session_id).await;
                    }
                } else {
                    self.emit_control_response(control_response_error(
                        &request_id,
                        "duplicate or late can_use_tool response",
                    ))?;
                }
                Ok(ControlAction::None)
            }
            ControlFrame::Unsupported {
                request_id,
                subtype,
            } => {
                self.emit_control_response(control_response_error(
                    &request_id,
                    &format!("unsupported control request subtype `{subtype}`"),
                ))?;
                Ok(ControlAction::None)
            }
            ControlFrame::Ignore => Ok(ControlAction::None),
            ControlFrame::ParseError {
                request_id,
                message,
            } => {
                self.report_parse_error(request_id, message)?;
                Ok(ControlAction::None)
            }
        }
    }

    async fn run_one_control_turn(
        &mut self,
        client: &AppClient,
        session_id: &str,
        prompt: String,
        control_rx: &mut tokio::sync::mpsc::UnboundedReceiver<ControlFrame>,
        pending_prompts: &mut std::collections::VecDeque<String>,
        pending_permissions: &mut std::collections::HashMap<String, String>,
        pending_asks: &mut HashMap<String, Vec<AskUserQuestionSpec>>,
        stdin_open: &mut bool,
        server_request_rx: &mut tokio::sync::mpsc::Receiver<
            orbcode_app_server_client::ServerRequestEnvelope,
        >,
        server_requests_open: &mut bool,
    ) -> Result<()> {
        self.total_turn_count += 1;
        let mut stream = client.submit_turn_stream(session_id, prompt).await?;
        let turn_started = std::time::Instant::now();
        let mut assistant_text = String::new();
        let mut prefer_control = true;

        loop {
            // Server requests retain first priority so a permission callback is
            // exposed promptly. Stdin controls and stream events alternate
            // same-tick priority through `next_turn_input`, preventing either
            // side from starving the other under sustained load.
            tokio::select! {
                biased;
                maybe_request = server_request_rx.recv(), if *server_requests_open => {
                    match maybe_request {
                        Some(request) => self
                            .handle_server_request(
                                request,
                                client,
                                session_id,
                                pending_permissions,
                                pending_asks,
                                *stdin_open,
                            )
                            .await?,
                        None => *server_requests_open = false,
                    }
                }
                input = next_turn_input(control_rx, &mut stream, *stdin_open, prefer_control) => {
                    match input {
                        TurnInput::Control(Some(frame)) => {
                            prefer_control = false;
                            if let ControlAction::Prompt(prompt) = self
                                .dispatch_control_frame(
                                    frame,
                                    client,
                                    session_id,
                                    pending_permissions,
                                    pending_asks,
                                )
                                .await?
                            {
                                pending_prompts.push_back(prompt);
                            }
                        }
                        TurnInput::Control(None) => {
                            *stdin_open = false;
                            self.deny_pending_permissions(client, pending_permissions).await;
                            cancel_pending_asks(client, pending_asks).await;
                        }
                        TurnInput::Stream(Some(event)) => {
                            prefer_control = true;
                            match self.record_event(&event, &mut assistant_text, turn_started)? {
                                EventOutcome::Terminal => {
                                    self.deny_pending_permissions(client, pending_permissions).await;
                                    cancel_pending_asks(client, pending_asks).await;
                                    break;
                                }
                                EventOutcome::Continue | EventOutcome::Permission(_) => {}
                            }
                        }
                        TurnInput::Stream(None) => {
                            self.deny_pending_permissions(client, pending_permissions).await;
                            cancel_pending_asks(client, pending_asks).await;
                            break;
                        }
                    }
                }
            }
        }

        self.append_assistant_text(&assistant_text);
        Ok(())
    }

    async fn handle_server_request(
        &mut self,
        envelope: orbcode_app_server_client::ServerRequestEnvelope,
        client: &AppClient,
        session_id: &str,
        pending_permissions: &mut std::collections::HashMap<String, String>,
        pending_asks: &mut HashMap<String, Vec<AskUserQuestionSpec>>,
        stdin_open: bool,
    ) -> Result<()> {
        if envelope.method == orbcode_app_server_client::method::SERVER_REQUEST_ASK_USER {
            let request: orbcode_app_server_client::AskUserQuestionRequest =
                match serde_json::from_value(envelope.params) {
                    Ok(request) => request,
                    Err(error) => {
                        client
                            .reject_server_request(
                                envelope.id,
                                format!("invalid AskUserQuestion server request: {error}"),
                            )
                            .await?;
                        return Ok(());
                    }
                };
            let questions = match request.canonical_questions() {
                Ok(questions) => questions,
                Err(error) => {
                    client
                        .reject_server_request(
                            envelope.id,
                            format!("invalid AskUserQuestion server request: {error}"),
                        )
                        .await?;
                    return Ok(());
                }
            };
            if request.session_id != session_id {
                client
                    .reject_server_request(
                        envelope.id,
                        "AskUserQuestion session does not match the active headless session",
                    )
                    .await?;
                return Ok(());
            }
            if !stdin_open {
                let _ = client
                    .respond_to_ask_user_question_outcome(
                        &request.request_id,
                        AskUserResponseOutcome::Cancelled {
                            reason: AskUserCancellationReason::Disconnect,
                        },
                    )
                    .await;
                return Ok(());
            }
            pending_asks.insert(request.request_id, questions);
            return Ok(());
        }

        if envelope.method != orbcode_app_server_client::method::SERVER_REQUEST_PERMISSION {
            client
                .reject_server_request(
                    envelope.id,
                    format!(
                        "headless SDK control channel cannot handle server request `{}`",
                        envelope.method
                    ),
                )
                .await?;
            return Ok(());
        }
        let request: orbcode_protocol::PermissionRequest =
            match serde_json::from_value(envelope.params) {
                Ok(request) => request,
                Err(error) => {
                    client
                        .reject_server_request(
                            envelope.id,
                            format!("invalid permission server request: {error}"),
                        )
                        .await?;
                    return Ok(());
                }
            };
        if request.session_id != session_id {
            let _ = client
                .respond_to_pending_permission_request(
                    &request.request_id,
                    PermissionDecision::Deny,
                )
                .await;
            return Ok(());
        }
        if !stdin_open {
            let _ = client
                .respond_to_pending_permission_request(
                    &request.request_id,
                    PermissionDecision::Deny,
                )
                .await;
            return Ok(());
        }

        let input = serde_json::from_str(&request.tool_input)
            .unwrap_or_else(|_| serde_json::Value::String(request.tool_input.clone()));
        let permission_request_id = request.request_id.clone();
        pending_permissions.insert(permission_request_id.clone(), request.tool_use_id.clone());
        let frame = ControlRequestEnvelope::new(
            permission_request_id.clone(),
            ControlRequest::CanUseTool {
                tool_name: request.tool_name,
                input,
                tool_use_id: request.tool_use_id,
                blocked_path: None,
                decision_reason: None,
            },
        );
        if let Err(error) = self.writer.write(&serde_json::to_value(frame)?) {
            pending_permissions.remove(&permission_request_id);
            let _ = client
                .respond_to_pending_permission_request(
                    &permission_request_id,
                    PermissionDecision::Deny,
                )
                .await;
            return Err(error);
        }
        Ok(())
    }

    async fn deny_pending_permissions(
        &self,
        client: &AppClient,
        pending_permissions: &mut std::collections::HashMap<String, String>,
    ) {
        let mut ids = client.pending_permission_request_ids().await;
        for id in pending_permissions.keys() {
            if !ids.contains(id) {
                ids.push(id.clone());
            }
        }
        for request_id in ids {
            let _ = client
                .respond_to_pending_permission_request(&request_id, PermissionDecision::Deny)
                .await;
        }
        pending_permissions.clear();
    }

    async fn build_initialize_response(
        &self,
        client: &AppClient,
        session_id: &str,
    ) -> Result<SdkInitializeResponse, String> {
        let status = client
            .status_overview_typed(session_id)
            .await
            .map_err(|error| error.to_string())?;
        let tools = client
            .list_tool_names()
            .await
            .map_err(|error| error.to_string())?;
        let mcp_servers = client
            .mcp_status()
            .await
            .map_err(|error| error.to_string())?
            .into_iter()
            .map(sdk_mcp_status)
            .collect();
        let permission_mode = client
            .permission_mode()
            .await
            .map_err(|error| error.to_string())?
            .mode
            .as_str()
            .to_string();
        Ok(SdkInitializeResponse {
            protocol_version: "1.0".to_string(),
            session_id: status.session_id,
            provider: status.default_provider.as_str().to_string(),
            model: status.model_name,
            permission_mode,
            tools,
            mcp_servers,
            supported_controls: SUPPORTED_CONTROL_SUBTYPES
                .iter()
                .map(|value| (*value).to_string())
                .collect(),
        })
    }

    async fn build_mcp_status_response(
        &self,
        client: &AppClient,
    ) -> Result<SdkMcpStatusResponse, String> {
        Ok(SdkMcpStatusResponse {
            mcp_servers: client
                .mcp_status()
                .await
                .map_err(|error| error.to_string())?
                .into_iter()
                .map(sdk_mcp_status)
                .collect(),
        })
    }

    async fn build_session_state_response(
        &self,
        client: &AppClient,
        session_id: &str,
    ) -> Result<SdkSessionStateResponse, String> {
        let overview = client
            .status_overview_typed(session_id)
            .await
            .map_err(|error| error.to_string())?;
        let controls = client
            .session_control_state(session_id)
            .await
            .map_err(|error| error.to_string())?;
        Ok(SdkSessionStateResponse {
            session_id: overview.session_id,
            cwd: overview.cwd.to_string_lossy().into_owned(),
            model_display_name: controls.model_selection.resolution.display_name,
            model_name: controls.model_selection.resolution.request_model,
            model_capabilities: controls.model_selection.resolution.capabilities,
            effort_level: controls
                .effort_level
                .map(|effort| effort.as_str().to_string()),
            max_thinking_tokens: overview.max_thinking_tokens,
            default_provider: overview.default_provider.as_str().to_string(),
            fallback_provider: overview
                .fallback_provider
                .map(|provider| provider.as_str().to_string()),
            sandbox_mode: overview.sandbox_mode,
            persisted_session_count: overview.persisted_session_count,
            background_job_count: overview.background_job_count,
            available_tool_count: overview.available_tool_count,
            configured_mcp_server_count: overview.configured_mcp_server_count,
        })
    }

    async fn build_context_usage_response(
        &self,
        client: &AppClient,
        session_id: &str,
    ) -> Result<SdkContextUsageResponse, String> {
        let overview = client
            .context_overview_typed(session_id)
            .await
            .map_err(|error| error.to_string())?;
        let max_thinking_tokens = overview.max_thinking_tokens;
        let usage = overview.usage;
        let categories = usage.categories;
        Ok(SdkContextUsageResponse {
            model: usage.model,
            max_thinking_tokens,
            estimated_tokens: usage.estimated_tokens,
            categories: SdkContextCategoriesResponse {
                system_prompt: categories.system_prompt,
                system_tools: categories.system_tools,
                mcp_tools: categories.mcp_tools,
                memory: categories.memory,
                skills: categories.skills,
                conversation: categories.conversation,
                attachments: categories.attachments,
                uncategorized: categories.uncategorized,
            },
            context_window: usage.context_window,
            effective_context_window: usage.effective_context_window,
            free_space_tokens: usage.free_space_tokens,
            percent_left: usage.percent_left,
            is_above_auto_compact_threshold: usage.is_above_auto_compact_threshold,
            is_above_warning_threshold: usage.is_above_warning_threshold,
            is_above_error_threshold: usage.is_above_error_threshold,
            is_at_blocking_limit: usage.is_at_blocking_limit,
        })
    }

    fn emit_typed_control_success<T: Serialize>(&self, request_id: &str, data: &T) -> Result<()> {
        let response = orbcode_protocol::ControlResponseEnvelope::success_with(request_id, data)?;
        self.writer.write(&serde_json::to_value(response)?)
    }

    fn apply_permission_mode(&mut self, mode: PermissionMode) {
        self.permission_mode = mode;
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
        self.writer.write(&response)
    }

    fn write_json(&self, value: &serde_json::Value) -> Result<()> {
        self.writer.write(value)
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

async fn cancel_pending_asks(
    client: &AppClient,
    pending_asks: &mut HashMap<String, Vec<AskUserQuestionSpec>>,
) {
    let request_ids: Vec<String> = pending_asks.keys().cloned().collect();
    pending_asks.clear();
    for request_id in request_ids {
        let _ = client
            .respond_to_ask_user_question_outcome(
                &request_id,
                AskUserResponseOutcome::Cancelled {
                    reason: AskUserCancellationReason::Disconnect,
                },
            )
            .await;
    }
}

fn sdk_mcp_status(
    server: orbcode_app_server_client::McpServerStatusOverview,
) -> SdkMcpServerStatus {
    let status = match server.status.as_str() {
        "ready" => "connected",
        "unauthorized" => "needs-auth",
        "starting" | "restarting" => "pending",
        "disabled" | "stopped" => "disabled",
        "failed" => "failed",
        other => other,
    };
    SdkMcpServerStatus {
        name: server.id,
        status: status.to_string(),
        error: server.error,
    }
}

pub(crate) fn write_json_line(value: &serde_json::Value) -> Result<()> {
    let mut stdout = io::stdout().lock();
    serde_json::to_writer(&mut stdout, value)?;
    stdout.write_all(b"\n")?;
    stdout.flush()?;
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
            let total_cost_usd = overview.cost.total_cost_usd;
            let has_unknown = overview.cost.has_unknown_model_cost;
            let is_api_priced = matches!(
                overview.cost.billing_basis,
                orbcode_app_server_client::BillingBasis::Api
            );
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
    bootstrap: &orbcode_app_server_client::BootstrapState,
) -> InitMetadata {
    let tool_names = client.list_tool_names().await.unwrap_or_default();
    let mcp_servers = client
        .mcp_status()
        .await
        .unwrap_or_default()
        .into_iter()
        .map(|server| McpServerInfo {
            name: server.id,
            status: server.status,
        })
        .collect();
    let permission_mode = client
        .permission_mode()
        .await
        .map_or(PermissionMode::Default, |result| result.mode);
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
