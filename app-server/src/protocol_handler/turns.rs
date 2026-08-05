use std::collections::HashMap;
use std::sync::Arc;

use orbcode_app_server_protocol::{
    ResponseResult, SessionIdParams, TurnCancelResult, TurnInterruptResult, TurnSubmitParams,
    TurnSubmitResult,
};
use orbcode_protocol::StreamEvent;
use serde_json::Value;
use tokio::sync::{Mutex, mpsc};

use super::{core_error, success, try_parse};
use crate::AppServer;

/// Registry of active stream subscriptions produced by `turn/submit`.
///
/// Phase 3 will wire up event delivery via notifications; for now the
/// receiver is stashed here so it is not dropped prematurely.
pub(crate) type ActiveStreams = Arc<Mutex<HashMap<String, mpsc::UnboundedReceiver<StreamEvent>>>>;

impl AppServer {
    pub(super) async fn handle_turn_submit(&self, params: Option<Value>) -> ResponseResult {
        let p: TurnSubmitParams = try_parse!(params);
        match self.submit_turn(&p.session_id, p.prompt).await {
            Ok(rx) => {
                let subscription_id = uuid::Uuid::new_v4().to_string();
                let streams = self.active_streams();
                streams.lock().await.insert(subscription_id.clone(), rx);
                success(TurnSubmitResult { subscription_id })
            }
            Err(e) => core_error(e),
        }
    }

    pub(super) async fn handle_turn_steer(&self, params: Option<Value>) -> ResponseResult {
        let p: TurnSubmitParams = try_parse!(params);
        match self.steer_turn(&p.session_id, p.prompt).await {
            Ok(()) => super::success_empty(),
            Err(e) => core_error(e),
        }
    }

    pub(super) async fn handle_turn_cancel(&self, params: Option<Value>) -> ResponseResult {
        let p: SessionIdParams = try_parse!(params);
        let cancelled = self.cancel_turn(&p.session_id).await;
        success(TurnCancelResult { cancelled })
    }

    pub(super) async fn handle_turn_interrupt(&self, params: Option<Value>) -> ResponseResult {
        let p: SessionIdParams = try_parse!(params);
        let interrupted = self.interrupt_turn(&p.session_id).await;
        success(TurnInterruptResult { interrupted })
    }
}
