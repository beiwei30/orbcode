use std::collections::HashMap;
use std::sync::{Mutex, OnceLock};

use orbcode_protocol::StreamEvent;
use tokio::sync::broadcast;

fn registry() -> &'static Mutex<HashMap<String, broadcast::Sender<StreamEvent>>> {
    static REGISTRY: OnceLock<Mutex<HashMap<String, broadcast::Sender<StreamEvent>>>> =
        OnceLock::new();
    REGISTRY.get_or_init(|| Mutex::new(HashMap::new()))
}

pub fn register_progress_stream(task_id: &str, capacity: usize) -> broadcast::Sender<StreamEvent> {
    let (tx, _) = broadcast::channel(capacity);
    registry()
        .lock()
        .expect("background progress registry mutex")
        .insert(task_id.to_string(), tx.clone());
    tx
}

pub fn subscribe_progress_stream(task_id: &str) -> Option<broadcast::Receiver<StreamEvent>> {
    registry()
        .lock()
        .expect("background progress registry mutex")
        .get(task_id)
        .map(tokio::sync::broadcast::Sender::subscribe)
}

pub fn unregister_progress_stream(task_id: &str) {
    registry()
        .lock()
        .expect("background progress registry mutex")
        .remove(task_id);
}

#[cfg(test)]
mod tests {
    use super::*;
    use orbcode_protocol::SessionSummary;

    fn unique(label: &str) -> String {
        use std::time::{SystemTime, UNIX_EPOCH};
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map_or(0, |d| d.as_nanos());
        format!("{label}-{nanos}-{}", std::process::id())
    }

    #[test]
    fn subscribe_returns_none_when_not_registered() {
        let id = unique("missing");
        assert!(subscribe_progress_stream(&id).is_none());
    }

    #[tokio::test]
    async fn register_subscribe_receive() {
        let id = unique("basic");
        let tx = register_progress_stream(&id, 16);
        let mut rx = subscribe_progress_stream(&id).expect("should subscribe");

        let event = StreamEvent::SessionStarted {
            summary: SessionSummary::default(),
        };
        tx.send(event.clone()).expect("send");

        let received = rx.recv().await.expect("recv");
        assert_eq!(received, event);

        unregister_progress_stream(&id);
        assert!(subscribe_progress_stream(&id).is_none());
    }

    #[tokio::test]
    async fn concurrent_streams_are_isolated() {
        let id_a = unique("iso-a");
        let id_b = unique("iso-b");
        let tx_a = register_progress_stream(&id_a, 16);
        let tx_b = register_progress_stream(&id_b, 16);
        let mut rx_a = subscribe_progress_stream(&id_a).expect("subscribe a");
        let mut rx_b = subscribe_progress_stream(&id_b).expect("subscribe b");

        let event_a = StreamEvent::AssistantDelta {
            session_id: "a".to_string(),
            delta: "hello from a".to_string(),
        };
        let event_b = StreamEvent::AssistantDelta {
            session_id: "b".to_string(),
            delta: "hello from b".to_string(),
        };
        tx_a.send(event_a.clone()).expect("send a");
        tx_b.send(event_b.clone()).expect("send b");

        let got_a = rx_a.recv().await.expect("recv a");
        let got_b = rx_b.recv().await.expect("recv b");
        assert_eq!(got_a, event_a);
        assert_eq!(got_b, event_b);

        unregister_progress_stream(&id_a);
        unregister_progress_stream(&id_b);
    }
}
