use serde_json::Value;
use uuid::Uuid;

use crate::{
    ToolContext, ToolError, ToolOutcome, ToolRegistry,
    payload::{field_or_raw, parse_payload},
    types::AskUserRequest,
};

impl ToolRegistry {
    pub(crate) async fn ask_user_question(
        &self,
        input: &str,
        context: &ToolContext,
    ) -> Result<ToolOutcome, ToolError> {
        let payload = parse_payload(input)?;
        let question = field_or_raw(&payload, "question", input)?;
        let options: Vec<String> = payload
            .get("options")
            .and_then(Value::as_array)
            .map(|items| {
                items
                    .iter()
                    .filter_map(Value::as_str)
                    .map(String::from)
                    .collect()
            })
            .unwrap_or_default();

        let ask_tx = context.ask_user_tx.as_ref().ok_or_else(|| {
            ToolError::ExecutionFailed(
                "AskUserQuestion is not available in this context".to_string(),
            )
        })?;

        let request_id = Uuid::new_v4().to_string();
        let (response_tx, response_rx) = tokio::sync::oneshot::channel();

        let req = AskUserRequest {
            request_id: request_id.clone(),
            question: question.clone(),
            options: options.clone(),
            response_tx,
        };

        ask_tx
            .send(req)
            .map_err(|_| ToolError::ExecutionFailed("ask-user channel closed".to_string()))?;

        let answer = tokio::select! {
            result = response_rx => result.unwrap_or(None),
            _ = context.cancellation.cancelled() => None,
        };

        let options_str = if options.is_empty() {
            "free-form".to_string()
        } else {
            options.join(", ")
        };

        let status = match &answer {
            Some(a) => format!("answered: {a}"),
            None => "cancelled".to_string(),
        };

        Ok(ToolOutcome {
            name: "ask-user-question".to_string(),
            summary: format!("Asked user: {question}"),
            output: format!("question: {question}\noptions: {options_str}\n{status}"),
            metadata: answer
                .as_ref()
                .map(|a| serde_json::json!({"answer": a, "request_id": request_id})),
            changed_paths: Vec::new(),
        })
    }
}
