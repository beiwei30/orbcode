use async_trait::async_trait;
use orbcode_app_server_protocol::{
    ResponseResult, ServerNotificationEnvelope, ServerRequestEnvelope,
};
use serde_json::Value;
use tokio::sync::mpsc;

use crate::ClientError;

#[async_trait]
pub trait ClientTransport: Send + Sync + 'static {
    async fn request(&self, method: &str, params: Option<Value>) -> Result<Value, ClientError>;

    async fn respond_to_server_request(
        &self,
        id: String,
        result: ResponseResult,
    ) -> Result<(), ClientError>;

    async fn take_notification_receiver(
        &self,
    ) -> Option<mpsc::Receiver<ServerNotificationEnvelope>>;

    async fn take_server_request_receiver(&self) -> Option<mpsc::Receiver<ServerRequestEnvelope>>;
}
