use serde::{Deserialize, Serialize};

// Re-export for convenience so consumers do not need to pull in
// `orbcode_protocol` directly for the most common notification payload.
pub use orbcode_protocol::StreamEvent;

/// Payload for [`method::NOTIFICATION_STREAM_EVENT`](crate::method::NOTIFICATION_STREAM_EVENT)
/// notifications. Wraps a single [`StreamEvent`] from the protocol crate
/// together with the `subscription_id` that identifies the turn subscription
/// that produced it.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct StreamEventNotification {
    /// Opaque identifier for the turn subscription that produced this event.
    /// Clients use this to correlate events to the `submit_turn` call that
    /// started the stream.
    pub subscription_id: String,
    pub event: StreamEvent,
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn stream_event_notification_roundtrip() {
        // Build a minimal StreamEvent variant to embed.
        let event = StreamEvent::Error {
            session_id: None,
            provider: None,
            category: None,
            message: "test error".into(),
            suggestion: None,
        };
        let notif = StreamEventNotification {
            subscription_id: "sub-1".to_string(),
            event: event.clone(),
        };
        let json = serde_json::to_string(&notif).unwrap();
        let back: StreamEventNotification = serde_json::from_str(&json).unwrap();
        // StreamEvent implements PartialEq, so we can compare directly.
        assert_eq!(back.event, event);
        assert_eq!(back.subscription_id, "sub-1");
    }

    #[test]
    fn stream_event_notification_json_structure() {
        let event = StreamEvent::Error {
            session_id: None,
            provider: None,
            category: None,
            message: "oops".into(),
            suggestion: Some("retry".into()),
        };
        let notif = StreamEventNotification {
            subscription_id: "sub-2".to_string(),
            event,
        };
        let value = serde_json::to_value(&notif).unwrap();
        // The notification should have an "event" key whose value is the
        // internally-tagged StreamEvent.
        assert!(value.get("event").is_some());
        assert_eq!(value["subscription_id"], json!("sub-2"));
        assert_eq!(value["event"]["event"], json!("error"));
        assert_eq!(value["event"]["message"], json!("oops"));
        assert_eq!(value["event"]["suggestion"], json!("retry"));
    }
}
