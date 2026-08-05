use orbcode_app_server_protocol::{
    ResponseResult, WorkflowListResult, WorkflowResumeParams, WorkflowStartDynamicParams,
    WorkflowStartParams, WorkflowTaskResult,
};

use super::{core_error, success, try_parse};
use crate::AppServer;

impl AppServer {
    pub(super) async fn handle_workflow_list(
        &self,
        _params: Option<serde_json::Value>,
    ) -> ResponseResult {
        match self.list_workflows().await {
            Ok(workflows) => success(WorkflowListResult(workflows)),
            Err(error) => core_error(error),
        }
    }

    pub(super) async fn handle_workflow_start(
        &self,
        params: Option<serde_json::Value>,
    ) -> ResponseResult {
        let p: WorkflowStartParams = try_parse!(params);
        match self
            .start_workflow(&p.session_id, &p.name, &p.arguments)
            .await
        {
            Ok(task_id) => success(WorkflowTaskResult { task_id }),
            Err(error) => core_error(error),
        }
    }

    pub(super) async fn handle_workflow_start_dynamic(
        &self,
        params: Option<serde_json::Value>,
    ) -> ResponseResult {
        let p: WorkflowStartDynamicParams = try_parse!(params);
        match self
            .start_dynamic_workflow(&p.session_id, &p.name, p.spec, &p.arguments)
            .await
        {
            Ok(task_id) => success(WorkflowTaskResult { task_id }),
            Err(error) => core_error(error),
        }
    }

    pub(super) async fn handle_workflow_resume(
        &self,
        params: Option<serde_json::Value>,
    ) -> ResponseResult {
        let p: WorkflowResumeParams = try_parse!(params);
        match self.resume_workflow(&p.run_id).await {
            Ok(task_id) => success(WorkflowTaskResult { task_id }),
            Err(error) => core_error(error),
        }
    }
}
