use orbcode_app_server_protocol::ResponseResult;
use serde::Deserialize;
use serde_json::json;

use super::{core_error, success, try_parse};
use crate::AppServer;

impl AppServer {
    pub(super) async fn handle_workflow_list(
        &self,
        _params: Option<serde_json::Value>,
    ) -> ResponseResult {
        match self.list_workflows().await {
            Ok(workflows) => success(workflows),
            Err(error) => core_error(error),
        }
    }

    pub(super) async fn handle_workflow_start(
        &self,
        params: Option<serde_json::Value>,
    ) -> ResponseResult {
        #[derive(Deserialize)]
        struct Params {
            session_id: String,
            name: String,
            #[serde(default)]
            arguments: String,
        }
        let p: Params = try_parse!(params);
        match self
            .start_workflow(&p.session_id, &p.name, &p.arguments)
            .await
        {
            Ok(task_id) => success(json!({ "task_id": task_id })),
            Err(error) => core_error(error),
        }
    }

    pub(super) async fn handle_workflow_start_dynamic(
        &self,
        params: Option<serde_json::Value>,
    ) -> ResponseResult {
        #[derive(Deserialize)]
        struct Params {
            session_id: String,
            name: String,
            spec: serde_json::Value,
            #[serde(default)]
            arguments: String,
        }
        let p: Params = try_parse!(params);
        match self
            .start_dynamic_workflow(&p.session_id, &p.name, p.spec, &p.arguments)
            .await
        {
            Ok(task_id) => success(json!({ "task_id": task_id })),
            Err(error) => core_error(error),
        }
    }

    pub(super) async fn handle_workflow_resume(
        &self,
        params: Option<serde_json::Value>,
    ) -> ResponseResult {
        #[derive(Deserialize)]
        struct Params {
            run_id: String,
        }
        let p: Params = try_parse!(params);
        match self.resume_workflow(&p.run_id).await {
            Ok(task_id) => success(json!({ "task_id": task_id })),
            Err(error) => core_error(error),
        }
    }
}
