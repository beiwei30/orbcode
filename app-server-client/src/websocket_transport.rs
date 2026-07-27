use std::collections::HashMap;
use std::sync::{
    Arc,
    atomic::{AtomicBool, Ordering},
};

use futures::{SinkExt, StreamExt};
use orbcode_app_server_protocol::{
    ClientMessage, ClientRequestEnvelope, ResponseResult, ServerMessage,
    ServerNotificationEnvelope, ServerRequestEnvelope, ServerRequestResponse,
    ServerResponseEnvelope,
};
use serde_json::Value;
use tokio::sync::{Mutex, mpsc, oneshot};
use tokio_tungstenite::tungstenite::Message;

use crate::error::ClientError;
use crate::transport::ClientTransport;

pub struct WebSocketTransport {
    writer_tx: mpsc::UnboundedSender<ClientMessage>,
    pending: Arc<Mutex<HashMap<String, oneshot::Sender<ServerResponseEnvelope>>>>,
    closed: Arc<AtomicBool>,
    notification_rx: Mutex<Option<mpsc::Receiver<ServerNotificationEnvelope>>>,
    server_request_rx: Mutex<Option<mpsc::Receiver<ServerRequestEnvelope>>>,
    _reader_handle: tokio::task::JoinHandle<()>,
    _writer_handle: tokio::task::JoinHandle<()>,
}

impl WebSocketTransport {
    pub async fn connect(endpoint: &str, auth_token: &str) -> Result<Self, ClientError> {
        let url = websocket_url(endpoint);
        let (ws_stream, _) = tokio_tungstenite::connect_async(&url)
            .await
            .map_err(|e| ClientError::Transport(format!("websocket connect: {e}")))?;
        let (mut writer, mut reader) = ws_stream.split();

        writer
            .send(Message::Text(auth_token.to_string().into()))
            .await
            .map_err(|e| ClientError::Transport(format!("websocket auth write: {e}")))?;

        let pending: Arc<Mutex<HashMap<String, oneshot::Sender<ServerResponseEnvelope>>>> =
            Arc::new(Mutex::new(HashMap::new()));
        let closed = Arc::new(AtomicBool::new(false));
        let (notification_tx, notification_rx) = mpsc::channel(256);
        let (server_request_tx, server_request_rx) = mpsc::channel(64);
        let (writer_tx, mut writer_rx) = mpsc::unbounded_channel::<ClientMessage>();

        let writer_closed = Arc::clone(&closed);
        let writer_handle = tokio::spawn(async move {
            while let Some(msg) = writer_rx.recv().await {
                let Ok(json) = serde_json::to_string(&msg) else {
                    continue;
                };
                if writer.send(Message::Text(json.into())).await.is_err() {
                    break;
                }
            }
            writer_closed.store(true, Ordering::SeqCst);
            let _ = writer.send(Message::Close(None)).await;
        });

        let pending_clone = Arc::clone(&pending);
        let reader_closed = Arc::clone(&closed);
        let reader_handle = tokio::spawn(async move {
            while let Some(msg_result) = reader.next().await {
                let msg = match msg_result {
                    Ok(msg) => msg,
                    Err(_) => break,
                };
                let text = match msg {
                    Message::Text(text) => text,
                    Message::Close(_) => break,
                    Message::Ping(_)
                    | Message::Pong(_)
                    | Message::Binary(_)
                    | Message::Frame(_) => {
                        continue;
                    }
                };
                let trimmed = text.trim();
                if trimmed.is_empty() {
                    continue;
                }
                let Ok(msg) = serde_json::from_str::<ServerMessage>(trimmed) else {
                    continue;
                };
                match msg {
                    ServerMessage::Response(env) => {
                        if let Some(tx) = pending_clone.lock().await.remove(&env.id) {
                            let _ = tx.send(env);
                        }
                    }
                    ServerMessage::Notification(n) => {
                        let _ = notification_tx.try_send(n);
                    }
                    ServerMessage::Request(r) => {
                        let _ = server_request_tx.try_send(r);
                    }
                    _ => {}
                }
            }

            reader_closed.store(true, Ordering::SeqCst);
            let mut pending = pending_clone.lock().await;
            pending.drain();
        });

        Ok(Self {
            writer_tx,
            pending,
            closed,
            notification_rx: Mutex::new(Some(notification_rx)),
            server_request_rx: Mutex::new(Some(server_request_rx)),
            _reader_handle: reader_handle,
            _writer_handle: writer_handle,
        })
    }

    fn send_message(&self, msg: ClientMessage) -> Result<(), ClientError> {
        if self.closed.load(Ordering::SeqCst) {
            return Err(ClientError::Transport("websocket connection closed".into()));
        }
        self.writer_tx
            .send(msg)
            .map_err(|_| ClientError::Transport("websocket writer closed".into()))
    }
}

#[async_trait::async_trait]
impl ClientTransport for WebSocketTransport {
    async fn request(&self, method: &str, params: Option<Value>) -> Result<Value, ClientError> {
        let id = uuid::Uuid::new_v4().to_string();
        let (tx, rx) = oneshot::channel();

        {
            let mut pending = self.pending.lock().await;
            if self.closed.load(Ordering::SeqCst) {
                return Err(ClientError::Transport("websocket connection closed".into()));
            }
            pending.insert(id.clone(), tx);
        }

        let msg = ClientMessage::Request(ClientRequestEnvelope {
            id: id.clone(),
            method: method.to_string(),
            params,
        });
        if let Err(e) = self.send_message(msg) {
            self.pending.lock().await.remove(&id);
            return Err(e);
        }

        let response = rx.await.map_err(|_| ClientError::Cancelled)?;
        match response.result {
            ResponseResult::Success { data } => Ok(data.unwrap_or(Value::Null)),
            ResponseResult::Error(e) => Err(ClientError::Protocol(e)),
            _ => Err(ClientError::Transport(
                "unsupported response result".to_string(),
            )),
        }
    }

    async fn respond_to_server_request(
        &self,
        id: String,
        result: ResponseResult,
    ) -> Result<(), ClientError> {
        self.send_message(ClientMessage::Response(ServerRequestResponse {
            id,
            result,
        }))
    }

    async fn take_notification_receiver(
        &self,
    ) -> Option<mpsc::Receiver<ServerNotificationEnvelope>> {
        self.notification_rx.lock().await.take()
    }

    async fn take_server_request_receiver(&self) -> Option<mpsc::Receiver<ServerRequestEnvelope>> {
        self.server_request_rx.lock().await.take()
    }
}

fn websocket_url(endpoint: &str) -> String {
    if endpoint.starts_with("ws://") || endpoint.starts_with("wss://") {
        endpoint.to_string()
    } else {
        format!("ws://{endpoint}")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    use tokio::net::TcpListener;
    use tokio::time::{Duration, timeout};

    #[tokio::test]
    async fn request_after_websocket_disconnect_returns_instead_of_hanging() {
        let listener = TcpListener::bind("127.0.0.1:0").await.expect("bind");
        let addr = listener.local_addr().expect("local addr");

        let server = tokio::spawn(async move {
            let (stream, _) = listener.accept().await.expect("accept");
            let mut ws = tokio_tungstenite::accept_async(stream)
                .await
                .expect("accept websocket");
            let _auth = ws.next().await.expect("auth frame").expect("auth ok");
        });

        let transport = WebSocketTransport::connect(&format!("ws://{addr}"), "dev-token")
            .await
            .expect("connect");
        server.await.expect("server task");

        let result = timeout(
            Duration::from_secs(1),
            transport.request("session/list", None),
        )
        .await
        .expect("request should not hang after websocket disconnect");

        assert!(
            matches!(
                result,
                Err(ClientError::Transport(_) | ClientError::Cancelled)
            ),
            "unexpected result after disconnect: {result:?}"
        );
    }
}
