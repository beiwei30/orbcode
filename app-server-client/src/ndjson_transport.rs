use std::path::Path;
use std::sync::{
    Arc,
    atomic::{AtomicBool, Ordering},
};

use orbcode_app_server_protocol::{
    ClientMessage, ClientRequestEnvelope, ResponseResult, ServerMessage,
    ServerNotificationEnvelope, ServerRequestEnvelope, ServerRequestResponse,
    ServerResponseEnvelope,
};
use serde_json::Value;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::net::UnixStream;
use tokio::sync::{Mutex, mpsc, oneshot};

use crate::error::ClientError;
use crate::transport::ClientTransport;

use std::collections::HashMap;

pub struct NdjsonTransport {
    writer: Arc<Mutex<tokio::io::WriteHalf<UnixStream>>>,
    pending: Arc<Mutex<HashMap<String, oneshot::Sender<ServerResponseEnvelope>>>>,
    closed: Arc<AtomicBool>,
    notification_rx: Mutex<Option<mpsc::Receiver<ServerNotificationEnvelope>>>,
    server_request_rx: Mutex<Option<mpsc::Receiver<ServerRequestEnvelope>>>,
    _reader_handle: tokio::task::JoinHandle<()>,
}

impl NdjsonTransport {
    pub async fn connect(path: &Path, auth_token: &str) -> Result<Self, ClientError> {
        let stream = UnixStream::connect(path)
            .await
            .map_err(|e| ClientError::Transport(format!("connect: {e}")))?;

        let (reader, writer) = tokio::io::split(stream);
        let writer = Arc::new(Mutex::new(writer));

        // Send auth token as first line
        {
            let mut w = writer.lock().await;
            w.write_all(format!("{auth_token}\n").as_bytes())
                .await
                .map_err(|e| ClientError::Transport(format!("auth write: {e}")))?;
        }

        let pending: Arc<Mutex<HashMap<String, oneshot::Sender<ServerResponseEnvelope>>>> =
            Arc::new(Mutex::new(HashMap::new()));
        let closed = Arc::new(AtomicBool::new(false));
        let (notification_tx, notification_rx) = mpsc::channel(256);
        let (server_request_tx, server_request_rx) = mpsc::channel(64);

        let pending_clone = Arc::clone(&pending);
        let closed_clone = Arc::clone(&closed);
        let reader_handle = tokio::spawn(async move {
            let mut buf_reader = BufReader::new(reader);
            let mut line = String::new();
            loop {
                line.clear();
                match buf_reader.read_line(&mut line).await {
                    Ok(0) | Err(_) => break,
                    Ok(_) => {}
                }
                let trimmed = line.trim();
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
            // Reader exited — drain all pending requests so callers get
            // RecvError instead of hanging indefinitely.
            closed_clone.store(true, Ordering::SeqCst);
            let mut pending = pending_clone.lock().await;
            pending.drain();
        });

        Ok(Self {
            writer,
            pending,
            closed,
            notification_rx: Mutex::new(Some(notification_rx)),
            server_request_rx: Mutex::new(Some(server_request_rx)),
            _reader_handle: reader_handle,
        })
    }

    async fn send_message(&self, msg: &ClientMessage) -> Result<(), ClientError> {
        if self.closed.load(Ordering::SeqCst) {
            return Err(ClientError::Transport("connection closed".into()));
        }
        let json = serde_json::to_string(msg).map_err(ClientError::Serialization)?;
        let mut w = self.writer.lock().await;
        w.write_all(format!("{json}\n").as_bytes())
            .await
            .map_err(|e| ClientError::Transport(format!("write: {e}")))?;
        Ok(())
    }
}

#[async_trait::async_trait]
impl ClientTransport for NdjsonTransport {
    async fn request(&self, method: &str, params: Option<Value>) -> Result<Value, ClientError> {
        let id = uuid::Uuid::new_v4().to_string();
        let (tx, rx) = oneshot::channel();

        {
            let mut pending = self.pending.lock().await;
            if self.closed.load(Ordering::SeqCst) {
                return Err(ClientError::Transport("connection closed".into()));
            }
            pending.insert(id.clone(), tx);
        }

        let msg = ClientMessage::Request(ClientRequestEnvelope {
            id: id.clone(),
            method: method.to_string(),
            params,
        });
        if let Err(e) = self.send_message(&msg).await {
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
        let msg = ClientMessage::Response(ServerRequestResponse { id, result });
        self.send_message(&msg).await
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

#[cfg(test)]
mod tests {
    use super::*;

    use tokio::net::UnixListener;
    use tokio::time::{Duration, timeout};

    #[tokio::test]
    async fn request_after_socket_disconnect_returns_instead_of_hanging() {
        let path = std::path::PathBuf::from(format!(
            "/tmp/orbcode-{}-{}.sock",
            std::process::id(),
            uuid::Uuid::new_v4()
        ));
        let listener = UnixListener::bind(&path).expect("bind socket");

        let server = tokio::spawn(async move {
            let (stream, _) = listener.accept().await.expect("accept");
            let mut reader = BufReader::new(stream);
            let mut token = String::new();
            reader.read_line(&mut token).await.expect("read auth");
        });

        let transport = NdjsonTransport::connect(&path, "dev-token")
            .await
            .expect("connect");
        server.await.expect("server task");
        let _ = std::fs::remove_file(&path);

        let result = timeout(
            Duration::from_secs(1),
            transport.request("session/list", None),
        )
        .await
        .expect("request should not hang after socket disconnect");

        assert!(
            matches!(
                result,
                Err(ClientError::Transport(_) | ClientError::Cancelled)
            ),
            "unexpected result after disconnect: {result:?}"
        );
    }
}
