use orbcode_app_server_protocol::{
    BackgroundCancelResult, BackgroundCreateParams, BackgroundCreateResult, BackgroundEventsResult,
    BackgroundJobParams, BackgroundLogResult, BackgroundTaskListResult, ResponseResult,
};
use serde_json::Value;

use super::{core_error, success, try_parse};
use crate::AppServer;

impl AppServer {
    pub(super) async fn handle_background_create(&self, params: Option<Value>) -> ResponseResult {
        let p: BackgroundCreateParams = try_parse!(params);
        match self.create_background_job(&p.session_id, p.prompt).await {
            Ok(record) => success(BackgroundCreateResult {
                job_id: record.job_id,
                session_id: record.session_id,
                status: format!("{:?}", record.status),
                log_path: record.log_path.into(),
            }),
            Err(e) => core_error(e),
        }
    }

    pub(super) async fn handle_background_list(&self, _params: Option<Value>) -> ResponseResult {
        match self.list_background_jobs().await {
            Ok(views) => success(BackgroundTaskListResult(views)),
            Err(e) => core_error(e),
        }
    }

    pub(super) async fn handle_background_detail(&self, params: Option<Value>) -> ResponseResult {
        let p: BackgroundJobParams = try_parse!(params);
        match self.background_job_detail(&p.job_id).await {
            Ok(view) => success(view),
            Err(e) => core_error(e),
        }
    }

    pub(super) async fn handle_background_cancel(&self, params: Option<Value>) -> ResponseResult {
        let p: BackgroundJobParams = try_parse!(params);
        match self.cancel_background_job(&p.job_id).await {
            Ok(record) => success(BackgroundCancelResult {
                job_id: record.job_id,
                status: format!("{:?}", record.status),
            }),
            Err(e) => core_error(e),
        }
    }

    pub(super) async fn handle_background_log(&self, params: Option<Value>) -> ResponseResult {
        let p: BackgroundJobParams = try_parse!(params);
        match self.read_background_log(&p.job_id).await {
            Ok(log) => success(BackgroundLogResult { log }),
            Err(e) => core_error(e),
        }
    }

    pub(super) async fn handle_background_events(&self, params: Option<Value>) -> ResponseResult {
        let p: BackgroundJobParams = try_parse!(params);
        match self.read_background_events(&p.job_id).await {
            Ok(events) => success(BackgroundEventsResult { events }),
            Err(e) => core_error(e),
        }
    }

    pub(super) async fn handle_background_list_summary(
        &self,
        _params: Option<Value>,
    ) -> ResponseResult {
        match self.list_background_jobs_summary().await {
            Ok(jobs) => success(BackgroundTaskListResult(jobs)),
            Err(e) => core_error(e),
        }
    }

    pub(super) async fn handle_background_cancel_async(
        &self,
        params: Option<Value>,
    ) -> ResponseResult {
        let p: orbcode_app_server_protocol::CancelAsyncTaskParams = try_parse!(params);
        match self.cancel_async_task(&p.session_id, &p.task_id).await {
            Ok(result) => success(result),
            Err(e) => core_error(e),
        }
    }
}
