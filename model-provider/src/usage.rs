use orbcode_protocol::TokenUsage;
use serde_json::Value;

pub fn merge_usage(target: &mut TokenUsage, update: &TokenUsage) {
    let update_has_input = update.input_tokens > 0
        || update.cache_creation_input_tokens > 0
        || update.cache_read_input_tokens > 0;
    let update_has_output = update.output_tokens > 0;
    if update.input_tokens > 0 {
        target.input_tokens = update.input_tokens;
    }
    if update.cache_creation_input_tokens > 0 {
        target.cache_creation_input_tokens = update.cache_creation_input_tokens;
    }
    if update.cache_read_input_tokens > 0 {
        target.cache_read_input_tokens = update.cache_read_input_tokens;
    }
    if update.output_tokens > 0 {
        target.output_tokens = update.output_tokens;
    }
    if update.server_tool_use.web_search_requests > 0 {
        target.server_tool_use.web_search_requests = update.server_tool_use.web_search_requests;
    }
    if update.server_tool_use.web_fetch_requests > 0 {
        target.server_tool_use.web_fetch_requests = update.server_tool_use.web_fetch_requests;
    }
    if let Some(service_tier) = update.service_tier.as_deref() {
        target.service_tier = Some(service_tier.to_string());
    }
    if update.cache_creation.ephemeral_1h_input_tokens > 0 {
        target.cache_creation.ephemeral_1h_input_tokens =
            update.cache_creation.ephemeral_1h_input_tokens;
    }
    if update.cache_creation.ephemeral_5m_input_tokens > 0 {
        target.cache_creation.ephemeral_5m_input_tokens =
            update.cache_creation.ephemeral_5m_input_tokens;
    }
    if !update.iterations.is_empty() {
        target.iterations = update.iterations.clone();
    }
    if let Some(speed) = update.speed.as_deref() {
        target.speed = Some(speed.to_string());
    }
    if update.total_tokens > 0 && (update_has_input || !update_has_output) {
        target.total_tokens = update.total_tokens;
    } else {
        target.refresh_total_from_components();
    }
}

pub fn usage_from_value(value: Option<&Value>) -> TokenUsage {
    let Some(value) = value else {
        return TokenUsage::default();
    };
    let mut usage = serde_json::from_value::<TokenUsage>(value.clone()).unwrap_or_default();
    usage.refresh_total_from_components();
    usage
}
