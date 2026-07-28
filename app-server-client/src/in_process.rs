use std::collections::HashMap;
use std::sync::Arc;

use tokio::sync::{Mutex, mpsc, oneshot};
use tokio::task::JoinHandle;

use orbcode_app_server::AppServer;
use orbcode_app_server::message_processor::{CHANNEL_SINK_CAPACITY, ChannelSink, MessageProcessor};
use orbcode_app_server_protocol::{
    ClientMessage, ClientRequestEnvelope, ResponseResult, ServerMessage,
    ServerNotificationEnvelope, ServerRequestEnvelope, ServerRequestResponse,
    ServerResponseEnvelope,
};

use crate::error::ClientError;
use crate::transport::ClientTransport;

/// In-process transport that routes all messages through a
/// [`MessageProcessor`], ensuring that even in-process callers go through the
/// full protocol path (initialize, request/response, notifications, server
/// requests).
///
/// The transport spawns two background tasks:
/// 1. A **processor task** that reads from the client channel and feeds each
///    message into `MessageProcessor::handle_message`.
/// 2. A **router task** that reads server messages from the processor's sink
///    channel and dispatches them:
///    - `ServerMessage::Response` -> resolved via pending oneshot
///    - `ServerMessage::Notification` -> forwarded to `notification_tx`
///    - `ServerMessage::Request` -> forwarded to `server_request_tx`
pub struct InProcessTransport {
    /// Send client messages (requests + responses to server-requests) into the
    /// processor task.
    client_tx: mpsc::UnboundedSender<ClientMessage>,

    /// Pending request responses keyed by request ID.
    pending: Arc<Mutex<HashMap<String, oneshot::Sender<ServerResponseEnvelope>>>>,

    /// Outbound channel for stream-event notifications.
    _notification_tx: mpsc::Sender<ServerNotificationEnvelope>,
    /// Receiver half -- taken once by the consumer via
    /// [`take_notification_receiver`].
    notification_rx: Mutex<Option<mpsc::Receiver<ServerNotificationEnvelope>>>,

    /// Outbound channel for server-initiated requests (permissions, MCP trust).
    _server_request_tx: mpsc::Sender<ServerRequestEnvelope>,
    /// Receiver half -- taken once by the consumer via
    /// [`take_server_request_receiver`].
    server_request_rx: Mutex<Option<mpsc::Receiver<ServerRequestEnvelope>>>,

    /// Handle to the processor task (aborted on drop).
    _processor_handle: JoinHandle<()>,
    /// Handle to the router task (aborted on drop).
    _router_handle: JoinHandle<()>,
}

impl InProcessTransport {
    /// Wrap an `AppServer` and start the background processor + router tasks.
    pub fn new(app_server: AppServer) -> Self {
        // Client -> Processor channel.
        let (client_tx, client_rx) = mpsc::unbounded_channel::<ClientMessage>();

        // Two-tier Processor -> Router channels:
        // - Lossless (unbounded): responses, server-requests, terminal notifications
        // - Best-effort (bounded): deltas, progress, non-terminal notifications
        let (lossless_tx, lossless_rx) = mpsc::unbounded_channel::<ServerMessage>();
        let (best_effort_tx, best_effort_rx) =
            mpsc::channel::<ServerMessage>(CHANNEL_SINK_CAPACITY);

        // Pending request map.
        let pending: Arc<Mutex<HashMap<String, oneshot::Sender<ServerResponseEnvelope>>>> =
            Arc::new(Mutex::new(HashMap::new()));

        // Notification fan-out (bounded to limit memory under backpressure).
        let (notification_tx, notification_rx) = mpsc::channel::<ServerNotificationEnvelope>(256);

        // Server-request fan-out (bounded; server-requests are infrequent).
        let (server_request_tx, server_request_rx) = mpsc::channel::<ServerRequestEnvelope>(64);

        // --- Processor task ---
        let processor_handle = {
            let sink = Arc::new(ChannelSink::new(lossless_tx, best_effort_tx));
            let mut processor = MessageProcessor::new(app_server.clone(), sink);
            let mut client_rx = client_rx;
            tokio::spawn(async move {
                while let Some(msg) = client_rx.recv().await {
                    processor.handle_message(msg).await;
                }
            })
        };

        // --- Router task ---
        // Drains both lossless and best-effort channels. Messages from the
        // lossless channel (responses, server-requests, terminal notifications)
        // are forwarded with guaranteed delivery (.send().await). Messages
        // from the best-effort channel (deltas, progress) use try_send and
        // may be dropped under backpressure.
        let router_handle = {
            let pending = Arc::clone(&pending);
            let notif_tx = notification_tx.clone();
            let srv_req_tx = server_request_tx.clone();
            tokio::spawn(route_messages(
                lossless_rx,
                best_effort_rx,
                pending,
                notif_tx,
                srv_req_tx,
            ))
        };

        Self {
            client_tx,
            pending,
            _notification_tx: notification_tx,
            notification_rx: Mutex::new(Some(notification_rx)),
            _server_request_tx: server_request_tx,
            server_request_rx: Mutex::new(Some(server_request_rx)),
            _processor_handle: processor_handle,
            _router_handle: router_handle,
        }
    }

    /// Send a client request through the protocol path and wait for its
    /// response.
    pub async fn request(
        &self,
        method: &str,
        params: Option<serde_json::Value>,
    ) -> Result<serde_json::Value, ClientError> {
        let id = uuid::Uuid::new_v4().to_string();
        let (tx, rx) = oneshot::channel();

        self.pending.lock().await.insert(id.clone(), tx);

        self.client_tx
            .send(ClientMessage::Request(ClientRequestEnvelope {
                id: id.clone(),
                method: method.to_string(),
                params,
            }))
            .map_err(|_| ClientError::Transport("client channel closed".into()))?;

        let response = rx.await.map_err(|_| ClientError::Cancelled)?;

        match response.result {
            ResponseResult::Success { data } => Ok(data.unwrap_or(serde_json::Value::Null)),
            ResponseResult::Error(e) => Err(ClientError::Protocol(e)),
            _ => Err(ClientError::Transport(
                "unsupported response result".to_string(),
            )),
        }
    }

    /// Respond to a server-initiated request (e.g. a permission prompt).
    #[allow(clippy::unused_async)] // Public transport API is async; in-process send completes synchronously.
    pub async fn respond_to_server_request(
        &self,
        id: String,
        result: ResponseResult,
    ) -> Result<(), ClientError> {
        self.client_tx
            .send(ClientMessage::Response(ServerRequestResponse {
                id,
                result,
            }))
            .map_err(|_| ClientError::Transport("client channel closed".into()))?;
        Ok(())
    }

    /// Take the notification receiver. This can only be called once; subsequent
    /// calls return `None`.
    pub async fn take_notification_receiver(
        &self,
    ) -> Option<mpsc::Receiver<ServerNotificationEnvelope>> {
        self.notification_rx.lock().await.take()
    }

    /// Take the server-request receiver. This can only be called once;
    /// subsequent calls return `None`.
    pub async fn take_server_request_receiver(
        &self,
    ) -> Option<mpsc::Receiver<ServerRequestEnvelope>> {
        self.server_request_rx.lock().await.take()
    }
}

#[async_trait::async_trait]
impl ClientTransport for InProcessTransport {
    async fn request(
        &self,
        method: &str,
        params: Option<serde_json::Value>,
    ) -> Result<serde_json::Value, ClientError> {
        self.request(method, params).await
    }

    async fn respond_to_server_request(
        &self,
        id: String,
        result: ResponseResult,
    ) -> Result<(), ClientError> {
        self.respond_to_server_request(id, result).await
    }

    async fn take_notification_receiver(
        &self,
    ) -> Option<mpsc::Receiver<ServerNotificationEnvelope>> {
        self.take_notification_receiver().await
    }

    async fn take_server_request_receiver(&self) -> Option<mpsc::Receiver<ServerRequestEnvelope>> {
        self.take_server_request_receiver().await
    }
}

async fn route_messages(
    mut lossless_rx: mpsc::UnboundedReceiver<ServerMessage>,
    mut best_effort_rx: mpsc::Receiver<ServerMessage>,
    pending: Arc<Mutex<HashMap<String, oneshot::Sender<ServerResponseEnvelope>>>>,
    notif_tx: mpsc::Sender<ServerNotificationEnvelope>,
    srv_req_tx: mpsc::Sender<ServerRequestEnvelope>,
) {
    let mut lossless_open = true;
    let mut best_effort_open = true;
    while lossless_open || best_effort_open {
        tokio::select! {
            biased;
            msg = best_effort_rx.recv(), if best_effort_open => {
                if let Some(msg) = msg {
                    route_message(msg, &pending, &notif_tx, &srv_req_tx, false).await;
                } else {
                    best_effort_open = false;
                }
            }
            msg = lossless_rx.recv(), if lossless_open => {
                if let Some(msg) = msg {
                    route_message(msg, &pending, &notif_tx, &srv_req_tx, true).await;
                } else {
                    lossless_open = false;
                }
            }
        }
    }
}

/// Route a single server message to the appropriate consumer.
/// When `lossless` is true, notifications use `.send().await` for guaranteed
/// delivery; otherwise they use `try_send` and may be dropped.
async fn route_message(
    msg: ServerMessage,
    pending: &Arc<Mutex<HashMap<String, oneshot::Sender<ServerResponseEnvelope>>>>,
    notif_tx: &mpsc::Sender<ServerNotificationEnvelope>,
    srv_req_tx: &mpsc::Sender<ServerRequestEnvelope>,
    lossless: bool,
) {
    match msg {
        ServerMessage::Response(resp) => {
            let mut map = pending.lock().await;
            if let Some(tx) = map.remove(&resp.id) {
                let _ = tx.send(resp);
            }
        }
        ServerMessage::Notification(notif) => {
            if lossless {
                let _ = notif_tx.send(notif).await;
            } else {
                let _ = notif_tx.try_send(notif);
            }
        }
        ServerMessage::Request(req) => {
            let _ = srv_req_tx.send(req).await;
        }
        _ => {}
    }
}

impl Drop for InProcessTransport {
    fn drop(&mut self) {
        self._processor_handle.abort();
        self._router_handle.abort();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make_terminal_notification() -> ServerMessage {
        ServerMessage::Notification(ServerNotificationEnvelope {
            method: "stream/event".to_string(),
            params: serde_json::json!({
                "event": {
                    "event": "turn_finished",
                    "session_id": "s1",
                    "provider": "anthropic",
                    "usage": { "input_tokens": 0, "output_tokens": 0 }
                }
            }),
        })
    }

    fn make_delta_notification(i: usize) -> ServerMessage {
        ServerMessage::Notification(ServerNotificationEnvelope {
            method: "stream/event".to_string(),
            params: serde_json::json!({
                "event": {
                    "event": "assistant_delta",
                    "session_id": "s1",
                    "delta": format!("chunk-{i}")
                }
            }),
        })
    }

    #[tokio::test]
    async fn queued_delta_is_routed_before_terminal_notification() {
        for _ in 0..128 {
            let pending = Arc::new(Mutex::new(HashMap::new()));
            let (lossless_tx, lossless_rx) = mpsc::unbounded_channel();
            let (best_effort_tx, best_effort_rx) = mpsc::channel(1);
            let (notif_tx, mut notif_rx) = mpsc::channel(2);
            let (srv_req_tx, _srv_req_rx) = mpsc::channel(1);

            best_effort_tx
                .try_send(make_delta_notification(0))
                .expect("queue delta");
            lossless_tx
                .send(make_terminal_notification())
                .expect("queue terminal");
            drop(best_effort_tx);
            drop(lossless_tx);

            route_messages(lossless_rx, best_effort_rx, pending, notif_tx, srv_req_tx).await;

            let first = notif_rx.recv().await.expect("delta notification");
            let second = notif_rx.recv().await.expect("terminal notification");
            assert_eq!(first.params["event"]["event"], "assistant_delta");
            assert_eq!(second.params["event"]["event"], "turn_finished");
        }
    }

    /// Regression test: when the notification fan-out queue is full,
    /// terminal notifications (lossless path) must still be delivered,
    /// while best-effort notifications (deltas) may be dropped.
    #[tokio::test]
    async fn terminal_notification_survives_full_fanout_queue() {
        let pending: Arc<Mutex<HashMap<String, oneshot::Sender<ServerResponseEnvelope>>>> =
            Arc::new(Mutex::new(HashMap::new()));
        // Very small notification channel — fills up quickly.
        let (notif_tx, mut notif_rx) = mpsc::channel::<ServerNotificationEnvelope>(2);
        let (srv_req_tx, _srv_req_rx) = mpsc::channel::<ServerRequestEnvelope>(1);

        // Fill the notification channel with best-effort deltas.
        for i in 0..2 {
            route_message(
                make_delta_notification(i),
                &pending,
                &notif_tx,
                &srv_req_tx,
                false,
            )
            .await;
        }

        // Channel is now full (capacity 2). A best-effort delta should be dropped.
        route_message(
            make_delta_notification(99),
            &pending,
            &notif_tx,
            &srv_req_tx,
            false,
        )
        .await;

        // Drain the two buffered deltas to make room.
        let _ = notif_rx.recv().await;
        let _ = notif_rx.recv().await;

        // Now fill again.
        for i in 0..2 {
            route_message(
                make_delta_notification(i),
                &pending,
                &notif_tx,
                &srv_req_tx,
                false,
            )
            .await;
        }

        // Channel full again. Send a terminal notification via lossless path.
        // This must NOT be dropped — .send().await will wait until space is
        // available. We drain from a separate task to unblock.
        let notif_tx_clone = notif_tx.clone();
        let pending_clone = Arc::clone(&pending);
        let srv_req_tx_clone = srv_req_tx.clone();
        let send_handle = tokio::spawn(async move {
            route_message(
                make_terminal_notification(),
                &pending_clone,
                &notif_tx_clone,
                &srv_req_tx_clone,
                true, // lossless!
            )
            .await;
        });

        // Drain one to make room for the terminal notification.
        let _ = notif_rx.recv().await;

        // Wait for the lossless send to complete.
        tokio::time::timeout(std::time::Duration::from_secs(2), send_handle)
            .await
            .expect("lossless send should complete")
            .expect("task should not panic");

        // Drain remaining. The terminal notification must be among them.
        let mut found_terminal = false;
        while let Ok(notif) = notif_rx.try_recv() {
            if notif.params["event"]["event"] == "turn_finished" {
                found_terminal = true;
            }
        }
        assert!(
            found_terminal,
            "terminal notification must survive full fan-out queue"
        );
    }

    #[tokio::test]
    async fn concurrent_requests_resolve_to_correct_ids() {
        use std::time::{SystemTime, UNIX_EPOCH};

        use orbcode_app_server::AppServer;
        use orbcode_config::AppConfigOverrides;

        let unique = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("time")
            .as_nanos();
        let home = std::env::temp_dir().join(format!("orbcode-concurrent-home-{unique}"));
        let cwd = std::env::temp_dir().join(format!("orbcode-concurrent-cwd-{unique}"));
        tokio::fs::create_dir_all(&home).await.unwrap();
        tokio::fs::create_dir_all(&cwd).await.unwrap();

        let app = AppServer::new(
            cwd,
            AppConfigOverrides {
                home_dir: Some(home),
                ..AppConfigOverrides::default()
            },
        )
        .await
        .unwrap();

        let transport = InProcessTransport::new(app);
        transport
            .request(
                "initialize",
                Some(serde_json::json!({
                    "protocol_version": "1.0",
                    "client_info": { "name": "test", "version": "0.1" }
                })),
            )
            .await
            .unwrap();

        // Fire 5 concurrent requests and verify each gets the correct response
        let (r1, r2, r3, r4, r5) = tokio::join!(
            transport.request("session/list", None),
            transport.request("permission/mode", None),
            transport.request("settings/effort", None),
            transport.request("tools/list", None),
            transport.request("context/preview", None),
        );

        r1.expect("session/list");
        let mode = r2.expect("permission/mode");
        assert!(mode["mode"].is_string(), "permission/mode should have mode");
        r3.expect("settings/effort");
        let tools = r4.expect("tools/list");
        assert!(tools.is_array(), "tools/list should return array");
        r5.expect("context/preview");
    }

    // -----------------------------------------------------------------------
    // Backpressure: best-effort dropped but lossless preserved under load
    // -----------------------------------------------------------------------

    /// Exercises the router's two-tier routing: when the notification fan-out
    /// channel is full, best-effort notifications are dropped while lossless
    /// notifications (terminal events) and responses are always delivered.
    #[tokio::test]
    async fn backpressure_best_effort_dropped_lossless_preserved_in_router() {
        let pending: Arc<Mutex<HashMap<String, oneshot::Sender<ServerResponseEnvelope>>>> =
            Arc::new(Mutex::new(HashMap::new()));
        // Very small notification channel -- fills up quickly.
        let (notif_tx, mut notif_rx) = mpsc::channel::<ServerNotificationEnvelope>(2);
        let (srv_req_tx, _srv_req_rx) = mpsc::channel::<ServerRequestEnvelope>(64);

        // Flood with best-effort deltas (lossless=false).
        let flood_count = 20;
        for i in 0..flood_count {
            route_message(
                make_delta_notification(i),
                &pending,
                &notif_tx,
                &srv_req_tx,
                false,
            )
            .await;
        }

        // Count how many arrived.
        let mut best_effort_count = 0;
        while notif_rx.try_recv().is_ok() {
            best_effort_count += 1;
        }
        // Some must have been dropped (capacity=2, sent 20).
        assert!(
            best_effort_count < flood_count,
            "best-effort should be dropped under backpressure \
             (got {best_effort_count}, sent {flood_count})"
        );
        assert!(
            best_effort_count >= 2,
            "at least the channel capacity should be buffered"
        );

        // Now send a lossless response -- must always arrive.
        let (tx, rx) = oneshot::channel();
        pending.lock().await.insert("bp-resp-1".to_string(), tx);
        route_message(
            ServerMessage::Response(ServerResponseEnvelope {
                id: "bp-resp-1".to_string(),
                result: ResponseResult::Success { data: None },
            }),
            &pending,
            &notif_tx,
            &srv_req_tx,
            true,
        )
        .await;

        let resp = tokio::time::timeout(std::time::Duration::from_secs(1), rx)
            .await
            .expect("response should arrive")
            .expect("channel should not be dropped");
        assert_eq!(resp.id, "bp-resp-1");
    }

    // -----------------------------------------------------------------------
    // Backpressure: server-requests never dropped
    // -----------------------------------------------------------------------

    /// Server-request messages are routed through the server_request channel
    /// with guaranteed delivery (.send().await). This test verifies that
    /// server-requests are never dropped even under notification backpressure.
    #[tokio::test]
    async fn backpressure_server_requests_never_dropped_in_router() {
        let pending: Arc<Mutex<HashMap<String, oneshot::Sender<ServerResponseEnvelope>>>> =
            Arc::new(Mutex::new(HashMap::new()));
        // Tiny notification channel to create backpressure.
        let (notif_tx, _notif_rx) = mpsc::channel::<ServerNotificationEnvelope>(1);
        // Server-request channel with modest capacity.
        let (srv_req_tx, mut srv_req_rx) = mpsc::channel::<ServerRequestEnvelope>(64);

        // Saturate the notification channel with deltas.
        for i in 0..10 {
            route_message(
                make_delta_notification(i),
                &pending,
                &notif_tx,
                &srv_req_tx,
                false,
            )
            .await;
        }

        // Send server-requests. They use .send().await and must all arrive.
        let request_count = 10;
        for i in 0..request_count {
            route_message(
                ServerMessage::Request(ServerRequestEnvelope {
                    id: format!("srv-bp-{i}"),
                    method: "permission/request".to_string(),
                    params: serde_json::json!({"i": i}),
                }),
                &pending,
                &notif_tx,
                &srv_req_tx,
                true,
            )
            .await;
        }

        let mut received = 0;
        while srv_req_rx.try_recv().is_ok() {
            received += 1;
        }
        assert_eq!(
            received, request_count,
            "all server-requests must be delivered (got {received}, expected {request_count})"
        );
    }

    // -----------------------------------------------------------------------
    // Backpressure: responses still arrive when notification channel is full
    // -----------------------------------------------------------------------

    /// Fills the notification channel to capacity with best-effort messages
    /// then verifies that response messages are still routed to their pending
    /// oneshots without being blocked.
    #[tokio::test]
    async fn backpressure_responses_arrive_when_notifications_full_in_router() {
        let pending: Arc<Mutex<HashMap<String, oneshot::Sender<ServerResponseEnvelope>>>> =
            Arc::new(Mutex::new(HashMap::new()));
        // Notification channel with capacity 1 -- immediately full.
        let (notif_tx, _notif_rx) = mpsc::channel::<ServerNotificationEnvelope>(1);
        let (srv_req_tx, _srv_req_rx) = mpsc::channel::<ServerRequestEnvelope>(64);

        // Fill the notification channel.
        route_message(
            make_delta_notification(0),
            &pending,
            &notif_tx,
            &srv_req_tx,
            false,
        )
        .await;
        // Additional deltas will be dropped, but that's fine.
        route_message(
            make_delta_notification(1),
            &pending,
            &notif_tx,
            &srv_req_tx,
            false,
        )
        .await;

        // Register pending oneshots and send responses.
        let resp_count = 5;
        let mut receivers = Vec::new();
        for i in 0..resp_count {
            let (tx, rx) = oneshot::channel();
            pending.lock().await.insert(format!("resp-full-{i}"), tx);
            receivers.push(rx);

            route_message(
                ServerMessage::Response(ServerResponseEnvelope {
                    id: format!("resp-full-{i}"),
                    result: ResponseResult::Success {
                        data: Some(serde_json::json!({"i": i})),
                    },
                }),
                &pending,
                &notif_tx,
                &srv_req_tx,
                true,
            )
            .await;
        }

        // All receivers must resolve.
        for (i, rx) in receivers.into_iter().enumerate() {
            let resp = tokio::time::timeout(std::time::Duration::from_secs(1), rx)
                .await
                .unwrap_or_else(|_| panic!("response {i} should arrive"))
                .expect("channel should not be dropped");
            assert_eq!(resp.id, format!("resp-full-{i}"));
        }
    }

    #[tokio::test]
    async fn drop_transport_cancels_inflight_request() {
        use std::time::{SystemTime, UNIX_EPOCH};

        use orbcode_app_server::AppServer;
        use orbcode_config::AppConfigOverrides;

        let unique = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("time")
            .as_nanos();
        let home = std::env::temp_dir().join(format!("orbcode-drop-home-{unique}"));
        let cwd = std::env::temp_dir().join(format!("orbcode-drop-cwd-{unique}"));
        tokio::fs::create_dir_all(&home).await.unwrap();
        tokio::fs::create_dir_all(&cwd).await.unwrap();

        let mut env = orbcode_app_server::sealed_provider_env_overrides();
        env.insert(
            "ANTHROPIC_BASE_URL".to_string(),
            "mock://anthropic?scenario=hang".to_string(),
        );
        env.insert("ANTHROPIC_API_KEY".to_string(), "test-key".to_string());

        let app = AppServer::new(
            cwd,
            AppConfigOverrides {
                home_dir: Some(home),
                env_overrides: env,
                ..AppConfigOverrides::default()
            },
        )
        .await
        .unwrap();

        let transport = InProcessTransport::new(app);
        transport
            .request(
                "initialize",
                Some(serde_json::json!({
                    "protocol_version": "1.0",
                    "client_info": { "name": "test", "version": "0.1" }
                })),
            )
            .await
            .unwrap();

        // Bootstrap to get a session
        let state = transport.request("session/bootstrap", None).await.unwrap();
        let sid = state["session"]["session_id"].as_str().unwrap().to_string();

        // Take notification receiver to avoid backpressure
        let _rx = transport.take_notification_receiver().await;

        // Submit a hanging turn so there is real in-flight work
        transport
            .request(
                "turn/submit",
                Some(serde_json::json!({"session_id": sid, "prompt": "hang"})),
            )
            .await
            .unwrap();

        // Insert a pending oneshot into the transport's real pending map,
        // simulating an in-flight request that hasn't received its response.
        let (tx, rx) = oneshot::channel::<ServerResponseEnvelope>();
        transport
            .pending
            .lock()
            .await
            .insert("inflight-req".to_string(), tx);

        // Drop the transport — this aborts processor and router tasks
        drop(transport);

        // The pending oneshot receiver should get RecvError (sender dropped)
        let result = rx.await;
        assert!(
            result.is_err(),
            "in-flight request should be cancelled after InProcessTransport drop"
        );
    }
}
