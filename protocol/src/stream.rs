use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::background_task_view::BackgroundTaskView;
use crate::permission::{
    McpTrustApprovalRequest, McpTrustResolutionKind, PermissionRequest, PermissionResolutionKind,
};
use crate::provider::ProviderId;
use crate::session::{SessionId, SessionSummary, TranscriptMessage, TurnContext};
use crate::tool::ToolUseCompletionKind;
use crate::usage::TokenUsage;

// ---------------------------------------------------------------------------
// Progress envelope types
// ---------------------------------------------------------------------------

/// Known fields that appear in progress payloads emitted by tools and hooks.
#[derive(Clone, Debug, Default, Deserialize, Serialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct ProgressData {
    #[serde(default, rename = "type", skip_serializing_if = "Option::is_none")]
    pub progress_type: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub status: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub hook_event_name: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub result: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub duration_ms: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub exit_code: Option<i64>,
    /// Embedded transcript message (a nested JSON object; kept opaque).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub message: Option<Value>,
}

impl ProgressData {
    /// Returns `true` when at least one known field is populated.
    pub fn has_known_field(&self) -> bool {
        self.progress_type.is_some()
            || self.status.is_some()
            || self.error.is_some()
            || self.hook_event_name.is_some()
            || self.result.is_some()
            || self.duration_ms.is_some()
            || self.exit_code.is_some()
            || self.message.is_some()
    }
}

/// Progress records arrive in two shapes:
/// - Wrapped: `{ "data": { "type": ..., "status": ... } }`
/// - Flat: `{ "type": ..., "status": ... }`
///
/// This enum deserializes both forms via `#[serde(untagged)]`.
#[derive(Clone, Debug, Deserialize, Serialize, PartialEq)]
#[serde(untagged)]
#[non_exhaustive]
pub enum ProgressEnvelope {
    Wrapped { data: ProgressData },
    Flat(ProgressData),
}

impl ProgressEnvelope {
    /// Access the inner [`ProgressData`] regardless of shape.
    pub fn data(&self) -> &ProgressData {
        match self {
            Self::Wrapped { data } | Self::Flat(data) => data,
        }
    }

    /// Parse a `serde_json::Value` into a [`ProgressData`], rejecting empty
    /// objects (objects without any known progress field).
    pub fn parse(value: &Value) -> Option<ProgressData> {
        let envelope: ProgressEnvelope = serde_json::from_value(value.clone()).ok()?;
        let data = match envelope {
            Self::Wrapped { data } | Self::Flat(data) => data,
        };
        if data.has_known_field() {
            Some(data)
        } else {
            None
        }
    }
}

#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum StreamErrorCategory {
    Auth,
    RateLimit,
    Overload,
    Network,
    InvalidRequest,
    PromptTooLong,
    MaxOutput,
    ServerError,
    AccountSuspended,
    UnsupportedProvider,
    Interrupted,
    RetryExhausted,
    Other,
}

impl StreamErrorCategory {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Auth => "auth",
            Self::RateLimit => "rate_limit",
            Self::Overload => "overload",
            Self::Network => "network",
            Self::InvalidRequest => "invalid_request",
            Self::PromptTooLong => "prompt_too_long",
            Self::MaxOutput => "max_output",
            Self::ServerError => "server_error",
            Self::AccountSuspended => "account_suspended",
            Self::UnsupportedProvider => "unsupported_provider",
            Self::Interrupted => "interrupted",
            Self::RetryExhausted => "retry_exhausted",
            Self::Other => "other",
        }
    }

    /// Convenience alias for [`Self::as_str`], matching the method name used by
    /// the former `ProviderErrorCategory` in model-provider.
    pub fn label(self) -> &'static str {
        self.as_str()
    }
}

#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
#[non_exhaustive]
pub enum TurnCancellationKind {
    BeforeResponse,
    AssistantStreaming,
    ToolStage,
}

impl TurnCancellationKind {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::BeforeResponse => "before_response",
            Self::AssistantStreaming => "assistant_streaming",
            Self::ToolStage => "tool_stage",
        }
    }
}

/// Outcome of a pre-request `maxBudgetUsd` check. `Exceeded` means the
/// accumulated cost has reached or passed the cap; `UnknownPricing` means some
/// accumulated usage came from an unpriced model so the running total
/// understates real spend.
#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
#[non_exhaustive]
pub enum BudgetOutcome {
    Exceeded,
    UnknownPricing,
}

impl BudgetOutcome {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Exceeded => "exceeded",
            Self::UnknownPricing => "unknown_pricing",
        }
    }
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct NormalizedEvent {
    pub kind: String,
    pub content: Option<String>,
}

// `Eq` is intentionally omitted: the `Budget` variant carries `f64` cost
// fields, which are `PartialEq` but not `Eq`.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
#[serde(tag = "event", rename_all = "snake_case")]
#[non_exhaustive]
pub enum StreamEvent {
    SessionStarted {
        summary: SessionSummary,
    },
    SessionLoaded {
        summary: SessionSummary,
    },
    RequestStarted {
        session_id: SessionId,
        provider: ProviderId,
        fallback_provider: Option<ProviderId>,
        context: TurnContext,
    },
    UserMessage {
        message: TranscriptMessage,
    },
    AssistantMessageStarted {
        session_id: SessionId,
        provider: ProviderId,
        fallback_from: Option<ProviderId>,
    },
    ThinkingStarted {
        session_id: SessionId,
        provider: ProviderId,
    },
    ThinkingDelta {
        session_id: SessionId,
        delta: String,
    },
    ThinkingCompleted {
        session_id: SessionId,
        provider: ProviderId,
    },
    AssistantDelta {
        session_id: SessionId,
        delta: String,
    },
    PermissionRequested {
        request: PermissionRequest,
    },
    PermissionResolved {
        session_id: SessionId,
        request_id: String,
        kind: PermissionResolutionKind,
    },
    McpTrustApprovalRequested {
        request: McpTrustApprovalRequest,
    },
    McpTrustApprovalResolved {
        session_id: SessionId,
        request_id: String,
        kind: McpTrustResolutionKind,
    },
    ToolUseStarted {
        session_id: SessionId,
        tool_use_id: String,
        tool_name: String,
        #[serde(default, skip_serializing_if = "String::is_empty")]
        tool_input: String,
    },
    ToolProgress {
        session_id: SessionId,
        tool_use_id: String,
        tool_name: String,
        progress: Value,
    },
    HookProgress {
        session_id: SessionId,
        hook_event_name: String,
        progress: Value,
    },
    HookNotice {
        session_id: SessionId,
        hook_event_name: String,
        message: String,
        is_error: bool,
    },
    ToolUseCompleted {
        session_id: SessionId,
        tool_use_id: String,
        tool_name: String,
        kind: ToolUseCompletionKind,
    },
    AssistantMessageCompleted {
        message: TranscriptMessage,
        provider: ProviderId,
        fallback_from: Option<ProviderId>,
        usage: TokenUsage,
    },
    AssistantMessageDiscarded {
        session_id: SessionId,
        provider: ProviderId,
        fallback_provider: ProviderId,
        reason: String,
    },
    ContextCompacted {
        session_id: SessionId,
        duration_ms: u64,
        summary: Option<String>,
        original_message_count: usize,
        compacted_message_count: usize,
        provider_generated: bool,
        fallback_reason: Option<String>,
    },
    TurnCancelled {
        session_id: SessionId,
        kind: TurnCancellationKind,
        partial: Option<TranscriptMessage>,
        usage: Option<TokenUsage>,
    },
    TurnFinished {
        session_id: SessionId,
        provider: ProviderId,
        fallback_from: Option<ProviderId>,
        usage: TokenUsage,
    },
    /// Pre-request `maxBudgetUsd` check signal. When `blocked` is `true` this is
    /// terminal for the turn: no provider request is issued after it fires. When
    /// `false` it is an advisory warning (unknown pricing under the non-strict
    /// policy) and the turn proceeds. `total_usd` is the accumulated session cost
    /// and `max_budget_usd` the configured cap. `pricing_known` is `false` when
    /// some accumulated usage came from an unpriced model, so `total_usd`
    /// understates real spend.
    Budget {
        session_id: SessionId,
        outcome: BudgetOutcome,
        blocked: bool,
        total_usd: f64,
        max_budget_usd: f64,
        pricing_known: bool,
    },
    /// A tool (typically `AskUserQuestion`) needs the client to prompt the user
    /// for input. The client should display `question` (and optional `options`)
    /// and resolve via `AskUserQuestionResolved` when the user answers or the
    /// request times out.
    AskUserQuestionRequested {
        #[serde(default, skip_serializing_if = "String::is_empty")]
        session_id: SessionId,
        request_id: String,
        question: String,
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        options: Vec<String>,
    },
    /// Resolution of a prior `AskUserQuestionRequested`. `answer` is `None`
    /// when the request was cancelled or timed out.
    AskUserQuestionResolved {
        request_id: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        answer: Option<String>,
    },
    /// A local shell task (long-running Bash subprocess) transitioned to a new
    /// lifecycle state. Emitted by the app-server layer so the TUI can render
    /// status cards without polling the filesystem.
    LocalTaskProgress {
        session_id: SessionId,
        task_id: String,
        status: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        command: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        exit_code: Option<i32>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        signal: Option<i32>,
    },
    /// A complete background task view changed. This lets clients update
    /// workflow/background task cards directly without waiting for a poll.
    BackgroundTaskUpdated {
        session_id: SessionId,
        task: BackgroundTaskView,
    },
    Error {
        session_id: Option<SessionId>,
        provider: Option<ProviderId>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        category: Option<StreamErrorCategory>,
        message: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        suggestion: Option<String>,
    },
}

impl StreamEvent {
    /// Returns `true` if this event signals the end of a turn's event stream.
    ///
    /// After a terminal event, no more events for the current turn will be
    /// emitted on the channel. Consumers can use this to clean up
    /// subscriptions, flush buffers, or transition UI state.
    ///
    /// Terminal events:
    /// - [`StreamEvent::TurnFinished`] -- normal completion.
    /// - [`StreamEvent::TurnCancelled`] -- the turn was cancelled (before
    ///   response, during streaming, or at the tool stage).
    /// - [`StreamEvent::Budget`] with `blocked: true` -- budget exceeded, no
    ///   provider request issued.
    /// - [`StreamEvent::Error`] -- an error caused the turn loop to exit.
    pub fn is_terminal(&self) -> bool {
        matches!(
            self,
            StreamEvent::TurnFinished { .. }
                | StreamEvent::TurnCancelled { .. }
                | StreamEvent::Error { .. }
        ) || matches!(self, StreamEvent::Budget { blocked: true, .. })
    }

    pub fn normalize(&self) -> NormalizedEvent {
        match self {
            Self::SessionStarted { summary } => NormalizedEvent {
                kind: "session_started".to_string(),
                content: Some(summary.session_id.clone()),
            },
            Self::SessionLoaded { summary } => NormalizedEvent {
                kind: "session_loaded".to_string(),
                content: Some(summary.session_id.clone()),
            },
            Self::RequestStarted {
                provider, context, ..
            } => NormalizedEvent {
                kind: "request_started".to_string(),
                content: Some(format!("{provider}:{}", context.compact_summary())),
            },
            Self::UserMessage { message } => NormalizedEvent {
                kind: "user_message".to_string(),
                content: Some(message.content.clone()),
            },
            Self::AssistantMessageStarted {
                provider,
                fallback_from,
                ..
            } => NormalizedEvent {
                kind: "assistant_started".to_string(),
                content: Some(match fallback_from {
                    Some(from) => format!("{provider}<-{from}"),
                    None => provider.to_string(),
                }),
            },
            Self::ThinkingStarted { provider, .. } => NormalizedEvent {
                kind: "thinking_started".to_string(),
                content: Some(provider.to_string()),
            },
            Self::ThinkingDelta { delta, .. } => NormalizedEvent {
                kind: "thinking_delta".to_string(),
                content: Some(delta.clone()),
            },
            Self::ThinkingCompleted { provider, .. } => NormalizedEvent {
                kind: "thinking_completed".to_string(),
                content: Some(provider.to_string()),
            },
            Self::AssistantDelta { delta, .. } => NormalizedEvent {
                kind: "assistant_delta".to_string(),
                content: Some(delta.clone()),
            },
            Self::PermissionRequested { request } => NormalizedEvent {
                kind: "permission_requested".to_string(),
                content: Some(request.summary()),
            },
            Self::PermissionResolved {
                request_id, kind, ..
            } => NormalizedEvent {
                kind: "permission_resolved".to_string(),
                content: Some(format!("{request_id}:{}", kind.as_str())),
            },
            Self::McpTrustApprovalRequested { request } => NormalizedEvent {
                kind: "mcp_trust_approval_requested".to_string(),
                content: Some(format!("{}:{}", request.server_id, request.tool_name)),
            },
            Self::McpTrustApprovalResolved {
                request_id, kind, ..
            } => NormalizedEvent {
                kind: "mcp_trust_approval_resolved".to_string(),
                content: Some(format!("{request_id}:{}", kind.as_str())),
            },
            Self::ToolUseStarted {
                tool_use_id,
                tool_name,
                ..
            } => NormalizedEvent {
                kind: "tool_use_started".to_string(),
                content: Some(format!("{tool_name}:{tool_use_id}")),
            },
            Self::ToolProgress {
                tool_use_id,
                tool_name,
                progress,
                ..
            } => NormalizedEvent {
                kind: "tool_progress".to_string(),
                content: Some(format!(
                    "{tool_name}:{tool_use_id}:{}",
                    ProgressEnvelope::parse(progress)
                        .and_then(|d| d.progress_type)
                        .as_deref()
                        .unwrap_or("unknown")
                )),
            },
            Self::HookProgress {
                hook_event_name,
                progress,
                ..
            } => NormalizedEvent {
                kind: "hook_progress".to_string(),
                content: Some(format!(
                    "{hook_event_name}:{}",
                    ProgressEnvelope::parse(progress)
                        .and_then(|d| d.result)
                        .as_deref()
                        .unwrap_or("unknown")
                )),
            },
            Self::HookNotice {
                hook_event_name,
                message,
                is_error,
                ..
            } => NormalizedEvent {
                kind: "hook_notice".to_string(),
                content: Some(format!(
                    "{}:{}:{}",
                    hook_event_name,
                    if *is_error { "error" } else { "info" },
                    message
                )),
            },
            Self::ToolUseCompleted {
                tool_use_id,
                tool_name,
                kind,
                ..
            } => NormalizedEvent {
                kind: "tool_use_completed".to_string(),
                content: Some(format!("{tool_name}:{tool_use_id}:{}", kind.as_str())),
            },
            Self::AssistantMessageCompleted {
                message,
                provider,
                fallback_from,
                ..
            } => NormalizedEvent {
                kind: "assistant_completed".to_string(),
                content: Some(match fallback_from {
                    Some(from) => format!("{provider}<-{from}:{}", message.content),
                    None => format!("{provider}:{}", message.content),
                }),
            },
            Self::AssistantMessageDiscarded {
                provider,
                fallback_provider,
                reason,
                ..
            } => NormalizedEvent {
                kind: "assistant_discarded".to_string(),
                content: Some(format!("{provider}->{fallback_provider}:{reason}")),
            },
            Self::ContextCompacted {
                original_message_count,
                compacted_message_count,
                provider_generated,
                fallback_reason,
                ..
            } => NormalizedEvent {
                kind: "context_compacted".to_string(),
                content: Some(format!(
                    "{original_message_count}->{compacted_message_count}:{}:{}",
                    if *provider_generated {
                        "provider"
                    } else {
                        "fallback"
                    },
                    fallback_reason.as_deref().unwrap_or("none")
                )),
            },
            Self::TurnCancelled { kind, partial, .. } => NormalizedEvent {
                kind: "turn_cancelled".to_string(),
                content: Some(match partial {
                    Some(message) => format!("{}:{}", kind.as_str(), message.content),
                    None => kind.as_str().to_string(),
                }),
            },
            Self::TurnFinished {
                provider,
                fallback_from,
                usage,
                ..
            } => NormalizedEvent {
                kind: "turn_finished".to_string(),
                content: Some(match fallback_from {
                    Some(from) => format!("{provider}<-{from}:{}", usage.total_tokens),
                    None => format!("{provider}:{}", usage.total_tokens),
                }),
            },
            Self::Budget {
                outcome,
                blocked,
                total_usd,
                max_budget_usd,
                pricing_known,
                ..
            } => NormalizedEvent {
                kind: "budget".to_string(),
                content: Some(format!(
                    "{}:{}:{total_usd:.6}/{max_budget_usd:.6}:{}",
                    outcome.as_str(),
                    if *blocked { "blocked" } else { "warn" },
                    if *pricing_known { "known" } else { "unknown" },
                )),
            },
            Self::AskUserQuestionRequested {
                request_id,
                question,
                ..
            } => NormalizedEvent {
                kind: "ask_user_question_requested".to_string(),
                content: Some(format!("{request_id}:{question}")),
            },
            Self::AskUserQuestionResolved { request_id, answer } => NormalizedEvent {
                kind: "ask_user_question_resolved".to_string(),
                content: Some(format!(
                    "{request_id}:{}",
                    answer.as_deref().unwrap_or("<cancelled>")
                )),
            },
            Self::LocalTaskProgress {
                task_id, status, ..
            } => NormalizedEvent {
                kind: "local_task_progress".to_string(),
                content: Some(format!("{task_id}:{status}")),
            },
            Self::BackgroundTaskUpdated { task, .. } => NormalizedEvent {
                kind: "background_task_updated".to_string(),
                content: Some(format!("{}:{}:{}", task.kind, task.task_id, task.status)),
            },
            Self::Error {
                provider,
                category,
                message,
                ..
            } => NormalizedEvent {
                kind: "error".to_string(),
                content: Some(match (provider, category) {
                    (Some(provider), Some(category)) => {
                        format!("{provider}:{}:{message}", category.as_str())
                    }
                    (Some(provider), None) => format!("{provider}:{message}"),
                    _ => message.clone(),
                }),
            },
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::TimeZone;
    use serde_json::json;

    #[test]
    fn progress_envelope_wrapped_shape() {
        let value = json!({
            "data": {
                "type": "agent_progress",
                "status": "Running tool...",
                "hookEventName": "PreToolUse",
                "result": "completed",
                "durationMs": 42,
                "exitCode": 0
            }
        });
        let data = ProgressEnvelope::parse(&value).expect("should parse wrapped");
        assert_eq!(data.progress_type.as_deref(), Some("agent_progress"));
        assert_eq!(data.status.as_deref(), Some("Running tool..."));
        assert_eq!(data.hook_event_name.as_deref(), Some("PreToolUse"));
        assert_eq!(data.result.as_deref(), Some("completed"));
        assert_eq!(data.duration_ms, Some(42));
        assert_eq!(data.exit_code, Some(0));
    }

    #[test]
    fn progress_envelope_flat_shape() {
        let value = json!({
            "type": "tool_status",
            "status": "Searching files...",
            "error": "some error detail"
        });
        let data = ProgressEnvelope::parse(&value).expect("should parse flat");
        assert_eq!(data.progress_type.as_deref(), Some("tool_status"));
        assert_eq!(data.status.as_deref(), Some("Searching files..."));
        assert_eq!(data.error.as_deref(), Some("some error detail"));
        assert_eq!(data.hook_event_name, None);
        assert_eq!(data.result, None);
        assert_eq!(data.duration_ms, None);
        assert_eq!(data.exit_code, None);
    }

    #[test]
    fn progress_envelope_rejects_irrelevant_object() {
        // An object with no known progress fields should return None
        let value = json!({ "foo": "bar", "baz": 123 });
        assert!(ProgressEnvelope::parse(&value).is_none());
    }

    #[test]
    fn progress_envelope_rejects_empty_object() {
        let value = json!({});
        assert!(ProgressEnvelope::parse(&value).is_none());
    }

    #[test]
    fn progress_envelope_rejects_wrapped_empty_data() {
        let value = json!({ "data": {} });
        assert!(ProgressEnvelope::parse(&value).is_none());
    }

    #[test]
    fn progress_envelope_wrapped_with_message_field() {
        let value = json!({
            "data": {
                "type": "agent_progress",
                "message": { "type": "assistant", "message": { "content": "hello" } }
            }
        });
        let data = ProgressEnvelope::parse(&value).expect("should parse");
        assert_eq!(data.progress_type.as_deref(), Some("agent_progress"));
        assert!(data.message.is_some());
    }

    #[test]
    fn progress_envelope_flat_minimal() {
        // Only a `status` field present
        let value = json!({ "status": "Working..." });
        let data = ProgressEnvelope::parse(&value).expect("should parse");
        assert_eq!(data.status.as_deref(), Some("Working..."));
        assert_eq!(data.progress_type, None);
    }

    // -----------------------------------------------------------------------
    // is_terminal() tests
    // -----------------------------------------------------------------------

    #[test]
    fn turn_finished_is_terminal() {
        use crate::provider::ProviderId;
        let event = StreamEvent::TurnFinished {
            session_id: "s1".into(),
            provider: ProviderId::Anthropic,
            fallback_from: None,
            usage: TokenUsage::default(),
        };
        assert!(event.is_terminal());
    }

    #[test]
    fn turn_cancelled_is_terminal() {
        for kind in [
            TurnCancellationKind::BeforeResponse,
            TurnCancellationKind::AssistantStreaming,
            TurnCancellationKind::ToolStage,
        ] {
            let event = StreamEvent::TurnCancelled {
                session_id: "s1".into(),
                kind,
                partial: None,
                usage: None,
            };
            assert!(
                event.is_terminal(),
                "TurnCancelled({kind:?}) should be terminal"
            );
        }
    }

    #[test]
    fn budget_blocked_is_terminal() {
        let event = StreamEvent::Budget {
            session_id: "s1".into(),
            outcome: BudgetOutcome::Exceeded,
            blocked: true,
            total_usd: 5.0,
            max_budget_usd: 5.0,
            pricing_known: true,
        };
        assert!(event.is_terminal());
    }

    #[test]
    fn budget_not_blocked_is_not_terminal() {
        let event = StreamEvent::Budget {
            session_id: "s1".into(),
            outcome: BudgetOutcome::UnknownPricing,
            blocked: false,
            total_usd: 3.0,
            max_budget_usd: 5.0,
            pricing_known: false,
        };
        assert!(!event.is_terminal());
    }

    #[test]
    fn error_is_terminal() {
        let event = StreamEvent::Error {
            session_id: Some("s1".into()),
            provider: None,
            category: None,
            message: "something went wrong".into(),
            suggestion: None,
        };
        assert!(event.is_terminal());
    }

    #[test]
    fn assistant_delta_is_not_terminal() {
        let event = StreamEvent::AssistantDelta {
            session_id: "s1".into(),
            delta: "hello".into(),
        };
        assert!(!event.is_terminal());
    }

    #[test]
    fn tool_use_started_is_not_terminal() {
        let event = StreamEvent::ToolUseStarted {
            session_id: "s1".into(),
            tool_use_id: "tu-1".into(),
            tool_name: "bash".into(),
            tool_input: String::new(),
        };
        assert!(!event.is_terminal());
    }

    #[test]
    fn ask_user_question_requested_is_not_terminal() {
        let event = StreamEvent::AskUserQuestionRequested {
            session_id: "s1".into(),
            request_id: "auq-1".into(),
            question: "Pick a colour".into(),
            options: vec!["red".into(), "blue".into()],
        };
        assert!(!event.is_terminal());
    }

    #[test]
    fn ask_user_question_resolved_is_not_terminal() {
        let event = StreamEvent::AskUserQuestionResolved {
            request_id: "auq-1".into(),
            answer: Some("red".into()),
        };
        assert!(!event.is_terminal());
    }

    #[test]
    fn ask_user_question_resolved_cancelled_is_not_terminal() {
        let event = StreamEvent::AskUserQuestionResolved {
            request_id: "auq-2".into(),
            answer: None,
        };
        assert!(!event.is_terminal());
    }

    // -----------------------------------------------------------------------
    // serde round-trip tests for AskUserQuestion variants
    // -----------------------------------------------------------------------

    #[test]
    fn ask_user_question_requested_serde_round_trip() {
        let event = StreamEvent::AskUserQuestionRequested {
            session_id: "s1".into(),
            request_id: "auq-42".into(),
            question: "Which environment?".into(),
            options: vec!["staging".into(), "production".into()],
        };
        let json = serde_json::to_string(&event).unwrap();
        let deserialized: StreamEvent = serde_json::from_str(&json).unwrap();
        assert_eq!(event, deserialized);
        // Verify the tag is present
        let value: serde_json::Value = serde_json::from_str(&json).unwrap();
        assert_eq!(value["event"], "ask_user_question_requested");
        assert_eq!(value["session_id"], "s1");
        assert_eq!(value["request_id"], "auq-42");
        assert_eq!(value["question"], "Which environment?");
        assert_eq!(
            value["options"],
            serde_json::json!(["staging", "production"])
        );
    }

    #[test]
    fn ask_user_question_requested_empty_options_serde_round_trip() {
        let event = StreamEvent::AskUserQuestionRequested {
            session_id: "s1".into(),
            request_id: "auq-43".into(),
            question: "Free-form answer?".into(),
            options: Vec::new(),
        };
        let json = serde_json::to_string(&event).unwrap();
        let deserialized: StreamEvent = serde_json::from_str(&json).unwrap();
        assert_eq!(event, deserialized);
        // Empty vec should be omitted via skip_serializing_if
        let value: serde_json::Value = serde_json::from_str(&json).unwrap();
        assert!(value.get("options").is_none());
    }

    #[test]
    fn ask_user_question_resolved_serde_round_trip() {
        let event = StreamEvent::AskUserQuestionResolved {
            request_id: "auq-42".into(),
            answer: Some("staging".into()),
        };
        let json = serde_json::to_string(&event).unwrap();
        let deserialized: StreamEvent = serde_json::from_str(&json).unwrap();
        assert_eq!(event, deserialized);
        let value: serde_json::Value = serde_json::from_str(&json).unwrap();
        assert_eq!(value["event"], "ask_user_question_resolved");
        assert_eq!(value["request_id"], "auq-42");
        assert_eq!(value["answer"], "staging");
    }

    #[test]
    fn ask_user_question_resolved_cancelled_serde_round_trip() {
        let event = StreamEvent::AskUserQuestionResolved {
            request_id: "auq-44".into(),
            answer: None,
        };
        let json = serde_json::to_string(&event).unwrap();
        let deserialized: StreamEvent = serde_json::from_str(&json).unwrap();
        assert_eq!(event, deserialized);
        // None answer should be omitted via skip_serializing_if
        let value: serde_json::Value = serde_json::from_str(&json).unwrap();
        assert!(value.get("answer").is_none());
    }

    #[test]
    fn background_task_updated_normalizes_and_round_trips() {
        let timestamp = chrono::Utc.with_ymd_and_hms(2026, 5, 27, 0, 0, 0).unwrap();
        let task = BackgroundTaskView {
            task_id: "workflow-1".to_string(),
            session_id: "s1".to_string(),
            kind: crate::BackgroundTaskViewKind::Workflow,
            status: crate::BackgroundTaskViewStatus::Running,
            description: "generated workflow".to_string(),
            cwd: "/tmp".to_string(),
            created_at: timestamp,
            updated_at: timestamp,
            started_at: Some(timestamp),
            finished_at: None,
            pid: None,
            exit_code: None,
            signal: None,
            error: None,
            model: None,
            provider: None,
            permission_mode: None,
            agent_type: None,
            child_session_id: None,
            cancellation_reason: None,
            label: Some("Generated workflow".to_string()),
            log_tail: None,
            progress_events: None,
            workflow_steps: None,
        };
        let event = StreamEvent::BackgroundTaskUpdated {
            session_id: "s1".to_string(),
            task,
        };

        assert_eq!(
            event.normalize(),
            NormalizedEvent {
                kind: "background_task_updated".to_string(),
                content: Some("workflow:workflow-1:running".to_string()),
            }
        );
        assert!(!event.is_terminal());
        let json = serde_json::to_string(&event).unwrap();
        let deserialized: StreamEvent = serde_json::from_str(&json).unwrap();
        assert_eq!(event, deserialized);
    }

    #[test]
    fn request_started_is_not_terminal() {
        use crate::provider::ProviderId;
        let event = StreamEvent::RequestStarted {
            session_id: "s1".into(),
            provider: ProviderId::Anthropic,
            fallback_provider: None,
            context: TurnContext::default(),
        };
        assert!(!event.is_terminal());
    }
}
