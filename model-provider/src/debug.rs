use std::collections::HashMap;
use std::sync::Arc;

use chrono::Utc;
use orbcode_protocol::{ProviderId, TranscriptMessage};
use serde_json::{Value, json};
use tokio::sync::Mutex;

use crate::{
    ProviderRequest, ProviderRequestDebugSnapshot, provider_request_debug_snapshot,
    provider_visible_messages_value,
};

#[derive(Default)]
struct ProviderDebugTraceInner {
    last_provider_request: Option<ProviderRequestDebugSnapshot>,
    by_source: HashMap<String, ProviderRequestDebugSnapshot>,
}

#[derive(Clone, Default)]
pub struct ProviderDebugTrace {
    inner: Arc<Mutex<ProviderDebugTraceInner>>,
}

impl ProviderDebugTrace {
    pub fn new() -> Self {
        Self::default()
    }

    pub async fn snapshot(&self) -> Option<ProviderRequestDebugSnapshot> {
        self.inner.lock().await.last_provider_request.clone()
    }

    /// Return the most recently recorded snapshot whose `source` matches.
    /// Useful for tests asserting against intermediate requests (e.g. the
    /// child agent request) that have since been overwritten by a follow-up
    /// parent turn request.
    pub async fn snapshot_for_source(&self, source: &str) -> Option<ProviderRequestDebugSnapshot> {
        self.inner.lock().await.by_source.get(source).cloned()
    }

    pub async fn record(&self, provider: ProviderId, source: &str, request: &ProviderRequest) {
        let captured_at = Utc::now().to_rfc3339();
        let mut snapshot = provider_request_debug_snapshot(provider, source, request, captured_at);
        let mut inner = self.inner.lock().await;
        if let Some(previous) = inner.last_provider_request.as_ref()
            && previous.session_id == snapshot.session_id
        {
            snapshot.recent_activity_json = previous.recent_activity_json.clone();
        }
        inner.by_source.insert(source.to_string(), snapshot.clone());
        inner.last_provider_request = Some(snapshot);
    }

    pub async fn clear(&self) {
        let mut inner = self.inner.lock().await;
        inner.last_provider_request = None;
        inner.by_source.clear();
    }

    pub async fn append_activity(&self, activity: Value) {
        let mut inner = self.inner.lock().await;
        let (serialized, source) = {
            let Some(snapshot) = inner.last_provider_request.as_mut() else {
                return;
            };
            let mut activities = serde_json::from_str::<Vec<Value>>(&snapshot.recent_activity_json)
                .unwrap_or_default();
            activities.push(activity);
            let serialized =
                serde_json::to_string_pretty(&activities).unwrap_or_else(|_| "[]".to_string());
            snapshot.recent_activity_json = serialized.clone();
            (serialized, snapshot.source.clone())
        };
        if let Some(by_source) = inner.by_source.get_mut(&source) {
            by_source.recent_activity_json = serialized;
        }
    }

    pub async fn append_message_activity(
        &self,
        default_provider: ProviderId,
        activity_type: &str,
        label: &str,
        message: &TranscriptMessage,
    ) {
        let provider = self
            .snapshot()
            .await
            .map_or(default_provider, |snapshot| snapshot.provider);
        let messages = provider_visible_messages_value(provider, std::slice::from_ref(message));
        self.append_activity(json!({
            "type": activity_type,
            "label": label,
            "messages": messages,
        }))
        .await;
    }
}

#[cfg(test)]
mod tests {
    use orbcode_protocol::{MessageRole, TurnContext};

    use super::*;

    fn provider_request(session_id: &str) -> ProviderRequest {
        ProviderRequest {
            session_id: session_id.to_string(),
            prompt: "hello".to_string(),
            context: TurnContext {
                cwd: "/tmp/project".to_string(),
                current_date: "2026-05-20".to_string(),
                ..Default::default()
            },
            messages: vec![TranscriptMessage::new(MessageRole::User, "hello")],
            system_prompt: String::new(),
            tools: Vec::new(),
            model: "model".to_string(),
            base_url: "https://example.test".to_string(),
            api_key: None,
            auth_token: None,
            disable_thinking: false,
            effort: None,
            options: crate::ProviderRequestOptions::default(),
        }
    }

    #[tokio::test]
    async fn record_preserves_recent_activity_for_same_session() {
        let trace = ProviderDebugTrace::new();
        let request = provider_request("session-1");

        trace.record(ProviderId::Anthropic, "first", &request).await;
        trace.append_activity(json!({"type": "tool_result"})).await;
        trace
            .record(ProviderId::Anthropic, "second", &request)
            .await;

        let snapshot = trace.snapshot().await.expect("snapshot should exist");
        assert_eq!(snapshot.source, "second");
        assert_eq!(
            serde_json::from_str::<Value>(&snapshot.recent_activity_json)
                .expect("activity should parse"),
            json!([{ "type": "tool_result" }])
        );
    }

    #[tokio::test]
    async fn append_message_activity_uses_recorded_provider() {
        let trace = ProviderDebugTrace::new();
        let request = provider_request("session-1");
        trace.record(ProviderId::OpenAi, "request", &request).await;

        trace
            .append_message_activity(
                ProviderId::Anthropic,
                "tool_result_to_llm",
                "tool result",
                &TranscriptMessage::new(MessageRole::User, "done"),
            )
            .await;

        let snapshot = trace.snapshot().await.expect("snapshot should exist");
        let activities: Value =
            serde_json::from_str(&snapshot.recent_activity_json).expect("activity should parse");
        assert_eq!(activities[0]["type"], json!("tool_result_to_llm"));
        assert_eq!(activities[0]["label"], json!("tool result"));
        assert_eq!(activities[0]["messages"][0]["role"], json!("user"));
    }
}
