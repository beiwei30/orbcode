use std::sync::{
    Arc,
    atomic::{AtomicBool, Ordering},
};
use std::time::Instant;

use orbcode_config::AppConfig;
use orbcode_model_provider::{
    ProviderCancellationToken, debug_request_summary, debug_response_summary,
};
use orbcode_protocol::{
    BudgetOutcome, MessageRole, StreamErrorCategory, StreamEvent, TranscriptMessage,
    TurnCancellationKind, over_budget,
};
use orbcode_session_store::SessionWriteHints;
use tokio::sync::mpsc;
use uuid::Uuid;

use super::{SessionManager, session_stream::SessionProviderStreamSink};
use crate::{
    CoreError,
    agent_loop::no_tool::auto_continue_nudge,
    compaction::compact_result_summary,
    context::build_turn_context_with_memory_home,
    hooks::{non_empty_trimmed, user_prompt_submit_hook_blocking_message},
    retry::execute_stream_with_retry_and_fallback,
    turn_loop::{TurnLoopOutcome, TurnLoopState, wait_for_turn_cancellation},
};

/// The result of a pre-request budget check. `None` from `budget_precheck`
/// means "no cap configured, or under budget with known pricing": proceed
/// normally.
#[derive(Debug, PartialEq)]
pub(super) enum BudgetDecision {
    /// Stop the turn before issuing the request.
    Block {
        outcome: BudgetOutcome,
        total_usd: f64,
        max_budget_usd: f64,
        pricing_known: bool,
    },
    /// Proceed, but surface an advisory warning (unknown pricing, non-strict).
    Warn { total_usd: f64, max_budget_usd: f64 },
}

impl SessionManager {
    /// Decide whether the configured `maxBudgetUsd` cap should stop or warn
    /// before the next provider request. Returns `None` when no cap is set or
    /// the session is under budget with fully known pricing.
    ///
    /// The hard cap is checked first so an already-over total blocks even when
    /// some accumulated usage is unpriced: the real spend is at least the
    /// under-counted total. Only when under the cap does the unknown-pricing
    /// policy apply — `strict` blocks, otherwise it warns and proceeds (never a
    /// silent `$0`).
    pub(super) async fn budget_precheck(
        &self,
        session_id: &str,
        config: &AppConfig,
    ) -> Option<BudgetDecision> {
        let max_budget_usd = config.max_budget_usd()?;
        let (total_usd, pricing_known) = self.live_cost_total(session_id).await;
        if over_budget(total_usd, max_budget_usd) {
            return Some(BudgetDecision::Block {
                outcome: BudgetOutcome::Exceeded,
                total_usd,
                max_budget_usd,
                pricing_known,
            });
        }
        if !pricing_known {
            if config.max_budget_strict_unknown_pricing() {
                return Some(BudgetDecision::Block {
                    outcome: BudgetOutcome::UnknownPricing,
                    total_usd,
                    max_budget_usd,
                    pricing_known,
                });
            }
            return Some(BudgetDecision::Warn {
                total_usd,
                max_budget_usd,
            });
        }
        None
    }

    pub(super) async fn run_turn_loop(
        &self,
        session_id: &str,
        turn_id: Uuid,
        prompt: &str,
        config: &AppConfig,
        cancel_flag: Arc<AtomicBool>,
        tx: &mpsc::UnboundedSender<StreamEvent>,
    ) {
        let permissions = self.permission_context();
        let additional_directories = self.additional_directories();

        // Build an initial context snapshot for session hints so the session
        // file carries branch/provider metadata even when hooks abort the
        // turn before the first provider request.
        let initial_context = build_turn_context_with_memory_home(
            &config.cwd,
            &additional_directories,
            &config.home_dir,
        )
        .await;
        self.transcript_store
            .record_session_hints(
                session_id,
                SessionWriteHints {
                    git_branch: initial_context.git_branch.clone(),
                    provider: Some(config.default_provider),
                },
            )
            .await;
        let mut turn_state = TurnLoopState::default();

        if let Err(error) = permissions.ensure_provider_call_allowed(config.default_provider) {
            let _ = tx.send(StreamEvent::Error {
                session_id: Some(session_id.to_string()),
                provider: Some(config.default_provider),
                message: error.to_string(),
                category: None,
                suggestion: None,
            });
            return;
        }

        let user_prompt_outcome = self
            .run_user_prompt_submit_hooks(session_id, prompt, tx)
            .await;
        if !user_prompt_outcome.blocking_errors.is_empty() {
            if let Err(error) = self
                .transcript_store
                .remove_last_user_prompt_if_matches(session_id, prompt)
                .await
                .map_err(CoreError::from)
            {
                let _ = tx.send(StreamEvent::Error {
                    session_id: Some(session_id.to_string()),
                    provider: Some(config.default_provider),
                    message: error.to_string(),
                    category: None,
                    suggestion: None,
                });
                return;
            }
            let _ = tx.send(StreamEvent::Error {
                session_id: Some(session_id.to_string()),
                provider: None,
                category: None,
                message: user_prompt_submit_hook_blocking_message(
                    &user_prompt_outcome.blocking_errors,
                ),
                suggestion: None,
            });
            return;
        }
        if let Err(error) = self
            .append_hook_additional_contexts(
                session_id,
                "UserPromptSubmit",
                user_prompt_outcome.additional_contexts,
                tx,
            )
            .await
        {
            let _ = tx.send(StreamEvent::Error {
                session_id: Some(session_id.to_string()),
                provider: Some(config.default_provider),
                message: error.to_string(),
                category: None,
                suggestion: None,
            });
            return;
        }

        // Advisory budget warnings (unknown pricing, non-strict policy) are
        // emitted at most once per turn loop so a multi-round turn does not spam
        // the same warning before every provider request.
        let mut budget_warned = false;
        loop {
            if cancel_flag.load(Ordering::SeqCst) {
                if self.active_turns.is_active(session_id, turn_id).await
                    && let Err(error) = self
                        .append_interruption_message(session_id, false, tx)
                        .await
                {
                    let _ = tx.send(StreamEvent::Error {
                        session_id: Some(session_id.to_string()),
                        provider: Some(config.default_provider),
                        message: error.to_string(),
                        category: None,
                        suggestion: None,
                    });
                    return;
                }
                let _ = tx.send(StreamEvent::TurnCancelled {
                    session_id: session_id.to_string(),
                    kind: TurnCancellationKind::BeforeResponse,
                    partial: None,
                    usage: None,
                });
                return;
            }

            if turn_state.provider_request_count > 0
                && let Err(error) = self.flush_queued_user_commands(session_id, tx).await
            {
                let _ = tx.send(StreamEvent::Error {
                    session_id: Some(session_id.to_string()),
                    provider: Some(config.default_provider),
                    message: error.to_string(),
                    category: None,
                    suggestion: None,
                });
                return;
            }

            // Reuse the cached context when the fingerprint (git HEAD,
            // memory-file mtimes, date) is unchanged since the last round.
            // Always rebuild on the first round so the turn starts fresh.
            let context = {
                use crate::context::fingerprint::{
                    compute_fingerprint, is_cache_valid, store_cache,
                };
                let fp = compute_fingerprint(
                    &config.cwd,
                    &additional_directories,
                    Some(&config.home_dir),
                )
                .await;
                if turn_state.provider_request_count > 0 {
                    if let Some(ref cache) = turn_state.context_cache {
                        if is_cache_valid(cache, &fp) {
                            cache.context.clone()
                        } else {
                            let ctx = build_turn_context_with_memory_home(
                                &config.cwd,
                                &additional_directories,
                                &config.home_dir,
                            )
                            .await;
                            turn_state.context_cache = Some(store_cache(fp, ctx.clone()));
                            ctx
                        }
                    } else {
                        let ctx = build_turn_context_with_memory_home(
                            &config.cwd,
                            &additional_directories,
                            &config.home_dir,
                        )
                        .await;
                        turn_state.context_cache = Some(store_cache(fp, ctx.clone()));
                        ctx
                    }
                } else {
                    let ctx = build_turn_context_with_memory_home(
                        &config.cwd,
                        &additional_directories,
                        &config.home_dir,
                    )
                    .await;
                    turn_state.context_cache = Some(store_cache(fp, ctx.clone()));
                    ctx
                }
            };

            // Enforce the optional `maxBudgetUsd` cap before issuing the request.
            // A `Block` decision is terminal: no provider request is made.
            match self.budget_precheck(session_id, config).await {
                Some(BudgetDecision::Block {
                    outcome,
                    total_usd,
                    max_budget_usd,
                    pricing_known,
                }) => {
                    let _ = tx.send(StreamEvent::Budget {
                        session_id: session_id.to_string(),
                        outcome,
                        blocked: true,
                        total_usd,
                        max_budget_usd,
                        pricing_known,
                    });
                    return;
                }
                Some(BudgetDecision::Warn {
                    total_usd,
                    max_budget_usd,
                }) if !budget_warned => {
                    budget_warned = true;
                    let _ = tx.send(StreamEvent::Budget {
                        session_id: session_id.to_string(),
                        outcome: BudgetOutcome::UnknownPricing,
                        blocked: false,
                        total_usd,
                        max_budget_usd,
                        pricing_known: false,
                    });
                }
                _ => {}
            }

            let request = match self
                .provider_request_for_session(
                    session_id,
                    prompt,
                    context.clone(),
                    &turn_state.synthetic_messages,
                    true,
                    true,
                )
                .await
            {
                Ok(request) => request,
                Err(error) => {
                    let _ = tx.send(StreamEvent::Error {
                        session_id: Some(session_id.to_string()),
                        provider: Some(config.default_provider),
                        message: error.to_string(),
                        category: None,
                        suggestion: None,
                    });
                    return;
                }
            };

            // Run the one-shot lightweight compaction BEFORE announcing the
            // request: a compaction round loops back via `continue` without
            // issuing a provider request, so emitting `RequestStarted` /
            // bumping `provider_request_count` here would leave a request event
            // with no response and flush queued commands one round early.
            if !turn_state.lightweight_compacted_for_prompt {
                let compact_started = Instant::now();
                match self
                    .lightweight_compact_before_request(session_id, &request.model, config)
                    .await
                {
                    Ok(Some(lightweight)) => {
                        let duration_ms =
                            compact_started.elapsed().as_millis().min(u64::MAX as u128) as u64;
                        let _ = tx.send(StreamEvent::ContextCompacted {
                            session_id: session_id.to_string(),
                            duration_ms,
                            summary: Some(lightweight.summary),
                            original_message_count: lightweight.result.original_message_count,
                            compacted_message_count: lightweight.result.compacted_message_count,
                            provider_generated: false,
                            fallback_reason: None,
                        });
                        turn_state.lightweight_compacted_for_prompt = true;
                        continue;
                    }
                    Ok(None) => {}
                    Err(error) => {
                        let _ = tx.send(StreamEvent::Error {
                            session_id: Some(session_id.to_string()),
                            provider: Some(config.default_provider),
                            message: error.to_string(),
                            category: None,
                            suggestion: None,
                        });
                        return;
                    }
                }
            }

            let _ = tx.send(StreamEvent::RequestStarted {
                session_id: session_id.to_string(),
                provider: config.default_provider,
                fallback_provider: config.fallback_provider,
                context: context.clone(),
            });
            turn_state.provider_request_count += 1;

            let preflight_error = tokio::select! {
                message = self.prompt_too_long_preflight_error(&request, config) => message,
                _ = wait_for_turn_cancellation(cancel_flag.clone()) => {
                    if self.active_turns.is_active(session_id, turn_id).await
                        && let Err(error) = self
                            .append_interruption_message(session_id, false, tx)
                            .await
                        {
                            let _ = tx.send(StreamEvent::Error {
                                session_id: Some(session_id.to_string()),
                                provider: Some(config.default_provider),
                                message: error.to_string(),
                                category: None,
                                suggestion: None,
                            });
                            return;
                        }
                    let _ = tx.send(StreamEvent::TurnCancelled {
                        session_id: session_id.to_string(),
                        kind: TurnCancellationKind::BeforeResponse,
                        partial: None,
                        usage: None,
                    });
                    return;
                }
            };

            if let Some(message) = preflight_error {
                if !turn_state.auto_compacted_for_prompt {
                    let compact_started = Instant::now();
                    match self
                        .compact_session_before_current_prompt(session_id, prompt)
                        .await
                    {
                        Ok(Some(result)) => {
                            let duration_ms =
                                compact_started.elapsed().as_millis().min(u64::MAX as u128) as u64;
                            let _ = tx.send(StreamEvent::ContextCompacted {
                                session_id: session_id.to_string(),
                                duration_ms,
                                summary: compact_result_summary(&result.session),
                                original_message_count: result.original_message_count,
                                compacted_message_count: result.compacted_message_count,
                                provider_generated: result.provider_generated,
                                fallback_reason: result.fallback_reason.clone(),
                            });
                            turn_state.auto_compacted_for_prompt = true;
                            continue;
                        }
                        Ok(None) => {}
                        Err(error) => {
                            let message = format!(
                                "Prompt is too long, and automatic compaction failed: {error}"
                            );
                            self.run_stop_failure_hooks(
                                session_id,
                                "prompt_too_long",
                                &message,
                                None,
                                tx,
                            )
                            .await;
                            let _ = tx.send(StreamEvent::Error {
                                session_id: Some(session_id.to_string()),
                                provider: Some(config.default_provider),
                                category: Some(StreamErrorCategory::PromptTooLong),
                                message,
                                suggestion: None,
                            });
                            return;
                        }
                    }
                }
                self.run_stop_failure_hooks(session_id, "prompt_too_long", &message, None, tx)
                    .await;
                let _ = tx.send(StreamEvent::Error {
                    session_id: Some(session_id.to_string()),
                    provider: Some(config.default_provider),
                    category: Some(StreamErrorCategory::PromptTooLong),
                    message,
                    suggestion: None,
                });
                return;
            }

            if let Err(error) = self
                .maybe_append_provider_round_diagnostic(session_id, debug_request_summary(&request))
                .await
            {
                let _ = tx.send(StreamEvent::Error {
                    session_id: Some(session_id.to_string()),
                    provider: Some(config.default_provider),
                    message: error.to_string(),
                    category: None,
                    suggestion: None,
                });
                return;
            }

            self.provider_debug_trace
                .record(config.default_provider, "turn", &request)
                .await;
            let mut stream_sink = SessionProviderStreamSink::new(
                self.clone(),
                session_id,
                config.default_provider,
                tx,
                cancel_flag.clone(),
            );
            let stream_result = execute_stream_with_retry_and_fallback(
                config,
                request,
                &mut stream_sink,
                ProviderCancellationToken::from_flag(cancel_flag.clone()),
            )
            .await;
            let session_stream_result = stream_sink.into_session_provider_stream_result();
            let streamed_tool_executions = session_stream_result.streamed_tool_executions;
            let tool_round_response = session_stream_result
                .tool_round_stream
                .into_tool_round_response();
            let response = &tool_round_response.response;

            if cancel_flag.load(Ordering::SeqCst) {
                self.interrupt_streamed_tool_executions(session_id, streamed_tool_executions, tx)
                    .await;
                // Preserve the streamed blocks (thinking + tool_use), not just
                // `response.content`: building the partial from `content` alone
                // drops thinking/tool_use blocks the model already streamed.
                let partial = if !response.blocks.is_empty() {
                    Some(
                        TranscriptMessage::from_blocks(
                            MessageRole::Assistant,
                            response.blocks.clone(),
                        )
                        .with_usage(response.usage.clone()),
                    )
                } else if response.content.trim().is_empty() {
                    None
                } else {
                    Some(
                        TranscriptMessage::new(MessageRole::Assistant, response.content.clone())
                            .with_usage(response.usage.clone()),
                    )
                };
                if let Some(message) = partial.clone() {
                    let _ = self.append_message(session_id, message).await;
                } else if response.usage.total_tokens > 0 {
                    // Even with no content, the provider consumed tokens.
                    // Record the cost so budget tracking stays accurate.
                    let cost_message = TranscriptMessage::new(MessageRole::Assistant, "")
                        .with_usage(response.usage.clone())
                        .with_synthetic(true);
                    self.accumulate_live_cost(session_id, &cost_message).await;
                }
                if self.active_turns.is_active(session_id, turn_id).await
                    && let Err(error) = self
                        .append_interruption_message(session_id, false, tx)
                        .await
                {
                    let _ = tx.send(StreamEvent::Error {
                        session_id: Some(session_id.to_string()),
                        provider: Some(config.default_provider),
                        message: error.to_string(),
                        category: None,
                        suggestion: None,
                    });
                    return;
                }
                let _ = tx.send(StreamEvent::TurnCancelled {
                    session_id: session_id.to_string(),
                    kind: if partial.is_some() {
                        TurnCancellationKind::AssistantStreaming
                    } else {
                        TurnCancellationKind::BeforeResponse
                    },
                    partial,
                    usage: Some(response.usage.clone()),
                });
                return;
            }

            if let Err(error) = stream_result {
                let (error_message, error_category, error_suggestion) =
                    extract_provider_error_fields(&error);
                let is_context_size_error = is_provider_context_size_error(&error_message);
                if is_context_size_error
                    && !turn_state.auto_compacted_for_prompt
                    && response.content.trim().is_empty()
                    && streamed_tool_executions.is_empty()
                {
                    let compact_started = Instant::now();
                    match self
                        .compact_session_before_current_prompt(session_id, prompt)
                        .await
                    {
                        Ok(Some(result)) => {
                            let duration_ms =
                                compact_started.elapsed().as_millis().min(u64::MAX as u128) as u64;
                            let _ = tx.send(StreamEvent::ContextCompacted {
                                session_id: session_id.to_string(),
                                duration_ms,
                                summary: compact_result_summary(&result.session),
                                original_message_count: result.original_message_count,
                                compacted_message_count: result.compacted_message_count,
                                provider_generated: result.provider_generated,
                                fallback_reason: result.fallback_reason.clone(),
                            });
                            turn_state.auto_compacted_for_prompt = true;
                            continue;
                        }
                        Ok(None) => {}
                        Err(error) => {
                            let message = format!(
                                "Provider reported a context-size error, and automatic compaction failed: {error}"
                            );
                            self.run_stop_failure_hooks(
                                session_id,
                                "prompt_too_long",
                                &message,
                                None,
                                tx,
                            )
                            .await;
                            let _ = tx.send(StreamEvent::Error {
                                session_id: Some(session_id.to_string()),
                                provider: Some(config.default_provider),
                                category: Some(StreamErrorCategory::PromptTooLong),
                                message,
                                suggestion: None,
                            });
                            return;
                        }
                    }
                }
                self.interrupt_streamed_tool_executions(session_id, streamed_tool_executions, tx)
                    .await;
                let stop_failure_name = if is_context_size_error {
                    "prompt_too_long"
                } else {
                    "unknown"
                };
                self.run_stop_failure_hooks(
                    session_id,
                    stop_failure_name,
                    &error_message,
                    non_empty_trimmed(&response.content),
                    tx,
                )
                .await;
                let _ = tx.send(StreamEvent::Error {
                    session_id: Some(session_id.to_string()),
                    provider: Some(config.default_provider),
                    category: error_category,
                    message: error_message,
                    suggestion: error_suggestion,
                });
                return;
            }

            if let Err(error) = self
                .maybe_append_provider_round_diagnostic(
                    session_id,
                    debug_response_summary(response),
                )
                .await
            {
                let _ = tx.send(StreamEvent::Error {
                    session_id: Some(session_id.to_string()),
                    provider: Some(config.default_provider),
                    message: error.to_string(),
                    category: None,
                    suggestion: None,
                });
                return;
            }

            let assembled = tool_round_response.response.content.clone();
            match self
                .finish_provider_response_with_streamed_tools(
                    session_id,
                    turn_id,
                    prompt,
                    tool_round_response,
                    assembled,
                    turn_state.auto_continue_attempts,
                    turn_state.stop_hook_active,
                    tx,
                    cancel_flag.clone(),
                    streamed_tool_executions,
                )
                .await
            {
                Ok(TurnLoopOutcome::Continue) => {}
                Ok(TurnLoopOutcome::StopHookContinue) => {
                    turn_state.stop_hook_active = true;
                }
                Ok(TurnLoopOutcome::AutoContinue(reason)) => {
                    turn_state.push_synthetic_context_message(auto_continue_nudge(
                        prompt,
                        turn_state.auto_continue_attempts + 1,
                        reason,
                    ));
                    turn_state.auto_continue_attempts += 1;
                    turn_state.stop_hook_active = false;
                }
                Ok(TurnLoopOutcome::Finished | TurnLoopOutcome::Cancelled) => return,
                Err(error) => {
                    let error_message = error.to_string();
                    self.run_stop_failure_hooks(session_id, "unknown", &error_message, None, tx)
                        .await;
                    let _ = tx.send(StreamEvent::Error {
                        session_id: Some(session_id.to_string()),
                        provider: Some(config.default_provider),
                        message: error_message,
                        category: None,
                        suggestion: None,
                    });
                    return;
                }
            }
        }
    }
}

fn is_provider_context_size_error(message: &str) -> bool {
    let normalized = message.to_ascii_lowercase();
    normalized.contains("prompt is too long")
        || normalized.contains("prompt too long")
        || normalized.contains("prompt-too-long")
        || normalized.contains("context_length_exceeded")
        || normalized.contains("context length")
        || normalized.contains("context window")
        || normalized.contains("context-size")
        || normalized.contains("context size")
        || normalized.contains("maximum context")
        || normalized.contains("too many tokens")
        || normalized.contains("input is too long")
        || normalized.contains("request too large")
        || normalized.contains("http_status/413")
        || normalized.contains(" 413 ")
        || normalized.contains(": 413")
}

fn extract_provider_error_fields(
    error: &CoreError,
) -> (String, Option<StreamErrorCategory>, Option<String>) {
    match error {
        CoreError::ProviderFailed(failure) | CoreError::RetryExhausted(failure) => (
            failure.message.clone(),
            Some(failure.category),
            failure.suggestion.clone(),
        ),
        _ => (error.to_string(), None, None),
    }
}
